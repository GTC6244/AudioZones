//! Real PipeWire backend (Linux, `--features pipewire-backend`).
//!
//! Owns a PipeWire main loop on a dedicated thread — the single writer of the model,
//! exactly the shape the spike proved. The registry listener builds an in-memory model
//! purely from registry global props (no per-object proxies needed for the read path).
//! Commands (link create/destroy) are marshaled onto the loop thread via `pw::channel`.
//!
//! Verified on real hardware (box 192.168.1.25): WirePlumber leaves device links alone,
//! and `create_object("link-factory")` from another thread works.
//!
//! Read path (nodes/ports/links) + link create/destroy + volume/mute via the node's
//! `Props.channelVolumes` SPA POD, including per-channel zone volume (#6). Device globals
//! are tracked so a node's stable key can fold in its hardware path, disambiguating two
//! identical cards (#9).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use pipewire as pw;
use pw::types::ObjectType;
use tokio::sync::broadcast;

use crate::backend::{BackendError, PwBackend};
use crate::identity::{node_key, port_key};
use crate::model::{Action, VolumeTarget};
use crate::wire::{Direction, GraphState, LinkView, NodeView, PortView};

// ---- in-memory model, written only by the loop thread ----

#[derive(Default)]
struct Model {
    nodes: HashMap<u32, NodeRec>,
    ports: HashMap<u32, PortRec>,
    links: HashMap<u32, (u32, u32)>, // link id -> (output_port_id, input_port_id)
    devices: HashMap<u32, DeviceRec>, // device id -> hardware path (for identical-card disambiguation)
    /// Current output route per (device id, profile-device index). This is the *audible*
    /// hardware-mixer volume — the layer `wpctl`/GNOME move and the one we must WRITE to
    /// actually change output (the node's Props volume is decoupled on hardware sinks).
    routes: HashMap<(u32, i32), RouteRec>,
}

#[derive(Clone)]
struct RouteRec {
    /// The route's own index on the device (needed to address it on a Route set_param).
    index: i32,
    /// The route's per-channel volumes (audio.position order) — what we overlay + write.
    channel_volumes: Vec<f32>,
}

struct NodeRec {
    name: String,
    media_class: String,
    /// Owning device's global id (from the node's `device.id` prop). Resolves to a
    /// hardware path that disambiguates two identical cards (#9).
    device_id: Option<u32>,
    /// The node's device index within its card profile (`card.profile.device` prop). A
    /// device `Route` param carries the same index, so this is how a route's volume —
    /// the layer `wpctl`/GNOME move — maps back to this node on rescan (#3).
    card_profile_device: Option<i32>,
    /// Live raw-linear per-channel volumes, filled by the Props param listener. Empty =
    /// not yet reported / no volume control.
    channel_volumes: Vec<f32>,
    muted: bool,
}

struct DeviceRec {
    /// Port-position-stable hardware path (`device.bus-path`, falling back to
    /// `api.alsa.path` / `object.path`). `None` if the device exposed no such prop.
    /// Two identical USB cards differ here even when their `node.name` collides.
    path: Option<String>,
}

struct PortRec {
    node_id: u32,
    name: String,
    dir: Direction,
    /// PipeWire `port.id` — the channel index into the node's `channel_volumes`
    /// (matches `audio.position` order). `None` if the port didn't report one.
    channel: Option<usize>,
}

impl Model {
    /// The disambiguator folded into a node's stable key: its owning device's path.
    fn node_disambiguator(&self, n: &NodeRec) -> Option<String> {
        n.device_id
            .and_then(|did| self.devices.get(&did))
            .and_then(|d| d.path.clone())
    }

    /// A node's stable key, including the identical-card disambiguator when available.
    fn node_stable_key(&self, n: &NodeRec) -> String {
        node_key(&n.name, &n.media_class, self.node_disambiguator(n).as_deref())
    }

    /// Compute a port's stable key from its owning node (or None if node unknown).
    fn port_stable_key(&self, port_id: u32) -> Option<String> {
        let p = self.ports.get(&port_id)?;
        let n = self.nodes.get(&p.node_id)?;
        let nk = self.node_stable_key(n);
        Some(port_key(&nk, &p.name, p.dir))
    }

    fn build_graph(&self) -> GraphState {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (id, n) in &self.nodes {
            let nk = self.node_stable_key(n);
            let ports = self
                .ports
                .iter()
                .filter(|(_, p)| p.node_id == *id)
                .map(|(_pid, p)| PortView {
                    key: port_key(&nk, &p.name, p.dir),
                    name: p.name.clone(),
                    direction: p.dir,
                    channel: p.channel,
                })
                .collect();
            // Representative volume = the loudest channel (matches the one-knob UI).
            let volume = n
                .channel_volumes
                .iter()
                .cloned()
                .fold(None::<f32>, |a, x| Some(a.map_or(x, |m| m.max(x))));
            nodes.push(NodeView {
                key: nk,
                name: n.name.clone(),
                media_class: n.media_class.clone(),
                ports,
                volume,
                channel_volumes: n.channel_volumes.clone(),
                muted: n.muted,
                present: true,
            });
        }

        let links = self
            .links
            .values()
            .filter_map(|(out_id, in_id)| {
                Some(LinkView {
                    output_port: self.port_stable_key(*out_id)?,
                    input_port: self.port_stable_key(*in_id)?,
                })
            })
            .collect();

        GraphState { connected: true, nodes, links, zones: Vec::new() }
    }

    fn find_port_id(&self, key: &str) -> Option<(u32, u32)> {
        self.ports
            .keys()
            .find_map(|pid| (self.port_stable_key(*pid)? == key).then_some(*pid))
            .map(|pid| (pid, self.ports[&pid].node_id))
    }

    /// Resolve a node stable key to (node id, channel count). Channels = the node's
    /// ports in its primary direction (In for a sink, Out for a source), excluding
    /// monitor ports. Used to size `channelVolumes`.
    fn find_node(&self, key: &str) -> Option<(u32, usize)> {
        for (id, n) in &self.nodes {
            if self.node_stable_key(n) == key {
                let primary = if n.media_class.contains("Source") {
                    Direction::Out
                } else {
                    Direction::In
                };
                let channels = self
                    .ports
                    .values()
                    .filter(|p| p.node_id == *id && p.dir == primary && !p.name.starts_with("monitor"))
                    .count();
                return Some((*id, channels));
            }
        }
        None
    }

    /// Resolve a node stable key to the device route that carries its *audible* volume:
    /// `(device id, profile-device index, route record)`. `None` for stream nodes (no
    /// device) or before the route has been seen — callers then fall back to node Props.
    fn route_for_node(&self, key: &str) -> Option<(u32, i32, RouteRec)> {
        for n in self.nodes.values() {
            if self.node_stable_key(n) == key {
                let dev_id = n.device_id?;
                let dev_index = n.card_profile_device?;
                return self.routes.get(&(dev_id, dev_index)).map(|r| (dev_id, dev_index, r.clone()));
            }
        }
        None
    }
}

// ---- commands marshaled onto the loop thread ----

enum Cmd {
    CreateLink { output_key: String, input_key: String },
    DestroyLink { output_key: String, input_key: String },
    SetVolume { node_key: String, target: VolumeTarget },
    SetMute { node_key: String, muted: bool },
    /// Pull current volume/mute from PipeWire: enum each node's `Props` and each device's
    /// `Route` params. Results arrive on the existing `.param` listeners, which update the
    /// model and publish. The route read is what picks up `wpctl`/GNOME changes (#3).
    Rescan,
}

/// Build the full `channelVolumes` array to push for a [`VolumeTarget`]. `channelVolumes`
/// must cover every channel, so per-channel targets preserve `current` on unlisted channels
/// (defaulting unknown channels to full to avoid surprise-muting — `current` is normally
/// seeded by the Props listener before any zone reconcile runs).
fn build_channel_volumes(target: &VolumeTarget, channel_count: usize, current: &[f32]) -> Vec<f32> {
    match target {
        VolumeTarget::Uniform(v) => {
            let v = v.clamp(0.0, 1.0);
            vec![v; channel_count.max(current.len()).max(1)]
        }
        VolumeTarget::Channels(pairs) => {
            let max_idx = pairs.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
            let n = channel_count.max(current.len()).max(max_idx);
            let mut out: Vec<f32> = (0..n).map(|i| current.get(i).copied().unwrap_or(1.0)).collect();
            for (idx, v) in pairs {
                if *idx < out.len() {
                    out[*idx] = v.clamp(0.0, 1.0);
                }
            }
            out
        }
    }
}

pub struct PipewireBackend {
    model: Arc<Mutex<Model>>,
    tx: broadcast::Sender<GraphState>,
    cmd_tx: pw::channel::Sender<Cmd>,
}

impl PipewireBackend {
    pub fn new() -> Result<Self, BackendError> {
        let model = Arc::new(Mutex::new(Model::default()));
        let (tx, _) = broadcast::channel::<GraphState>(16);
        let (cmd_tx, cmd_rx) = pw::channel::channel::<Cmd>();

        let model_thread = model.clone();
        let tx_thread = tx.clone();

        // Hand back any setup error from the loop thread before returning.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        thread::spawn(move || {
            run_loop(model_thread, tx_thread, cmd_rx, ready_tx);
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { model, tx, cmd_tx }),
            Ok(Err(e)) => Err(BackendError::Rejected(format!("pipewire init failed: {e}"))),
            Err(_) => Err(BackendError::Rejected("pipewire loop thread died".into())),
        }
    }
}

impl PwBackend for PipewireBackend {
    fn snapshot(&self) -> GraphState {
        self.model.lock().unwrap().build_graph()
    }

    fn subscribe(&self) -> broadcast::Receiver<GraphState> {
        self.tx.subscribe()
    }

    fn rescan(&self) -> Result<(), BackendError> {
        // Marshal onto the loop thread; the enum_params responses (and resulting snapshot
        // broadcast) happen there, asynchronously — exactly like every other change.
        self.cmd_tx
            .send(Cmd::Rescan)
            .map_err(|_| BackendError::Rejected("loop thread gone".into()))
    }

    fn apply(&self, action: Action) -> Result<(), BackendError> {
        match action {
            Action::CreateLink { output_port, input_port } => self
                .cmd_tx
                .send(Cmd::CreateLink { output_key: output_port, input_key: input_port })
                .map_err(|_| BackendError::Rejected("loop thread gone".into())),
            Action::DestroyLink { output_port, input_port } => self
                .cmd_tx
                .send(Cmd::DestroyLink { output_key: output_port, input_key: input_port })
                .map_err(|_| BackendError::Rejected("loop thread gone".into())),
            // Optimistic model update: reflect our own change immediately. PipeWire's
            // Props param-change events for ALSA nodes are unreliable (and wpctl changes
            // the device-route volume, a separate layer — see Q4), so we don't wait for a
            // confirmation event. The listener still seeds correct INITIAL volumes. We
            // compute the exact array we're about to send so the model and the device agree.
            Action::SetVolume { node_key, target } => {
                if let Ok(mut m) = self.model.lock() {
                    if let Some((nid, channels)) = m.find_node(&node_key) {
                        let current = m.nodes.get(&nid).map(|n| n.channel_volumes.clone()).unwrap_or_default();
                        let vols = build_channel_volumes(&target, channels, &current);
                        if let Some(n) = m.nodes.get_mut(&nid) {
                            n.channel_volumes = vols;
                        }
                    }
                }
                self.cmd_tx
                    .send(Cmd::SetVolume { node_key, target })
                    .map_err(|_| BackendError::Rejected("loop thread gone".into()))
            }
            Action::SetMute { node_key, muted } => {
                if let Ok(mut m) = self.model.lock() {
                    if let Some((nid, _)) = m.find_node(&node_key) {
                        if let Some(n) = m.nodes.get_mut(&nid) {
                            n.muted = muted;
                        }
                    }
                }
                self.cmd_tx
                    .send(Cmd::SetMute { node_key, muted })
                    .map_err(|_| BackendError::Rejected("loop thread gone".into()))
            }
        }
    }
}

fn run_loop(
    model: Arc<Mutex<Model>>,
    tx: broadcast::Sender<GraphState>,
    cmd_rx: pw::channel::Receiver<Cmd>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    pw::init();
    let mainloop = match pw::main_loop::MainLoop::new(None) {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let context = match pw::context::Context::new(&mainloop) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let core = match context.connect(None) {
        Ok(c) => std::rc::Rc::new(c),
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let registry = match core.get_registry() {
        Ok(r) => std::rc::Rc::new(r),
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };

    // Registry -> model, then broadcast a fresh snapshot.
    let publish = {
        let model = model.clone();
        let tx = tx.clone();
        move || {
            let g = model.lock().unwrap().build_graph();
            let _ = tx.send(g);
        }
    };

    // Node proxies, bound as nodes appear — needed to set params (volume/mute) and to
    // receive Props events (live volume read-back). Live only on the loop thread (!Send).
    // Param listeners are held alive in a parallel map (Box<dyn Any>: kept, never downcast).
    let node_proxies: std::rc::Rc<RefCell<HashMap<u32, pw::node::Node>>> =
        std::rc::Rc::new(RefCell::new(HashMap::new()));
    let node_listeners: std::rc::Rc<RefCell<HashMap<u32, Box<dyn std::any::Any>>>> =
        std::rc::Rc::new(RefCell::new(HashMap::new()));

    // Device proxies, bound as devices appear — needed to read `device.bus-path` from the
    // device's INFO props (the registry global only carries name/class, not the bus path).
    // That path disambiguates two identical cards (#9). Same lifetime pattern as nodes.
    let device_proxies: std::rc::Rc<RefCell<HashMap<u32, pw::device::Device>>> =
        std::rc::Rc::new(RefCell::new(HashMap::new()));
    let device_listeners: std::rc::Rc<RefCell<HashMap<u32, Box<dyn std::any::Any>>>> =
        std::rc::Rc::new(RefCell::new(HashMap::new()));

    let _listener = {
        let model_g = model.clone();
        let publish_g = publish.clone();
        let registry_g = registry.clone();
        let proxies_g = node_proxies.clone();
        let listeners_g = node_listeners.clone();
        let dev_proxies_g = device_proxies.clone();
        let dev_listeners_g = device_listeners.clone();
        let model_r = model.clone();
        let publish_r = publish.clone();
        let proxies_r = node_proxies.clone();
        let listeners_r = node_listeners.clone();
        let dev_proxies_r = device_proxies.clone();
        let dev_listeners_r = device_listeners.clone();
        registry
            .add_listener_local()
            .global(move |g| {
                let changed = ingest_global(&model_g, g);
                if g.type_ == ObjectType::Node {
                    if let Ok(node) = registry_g.bind::<pw::node::Node, _>(g) {
                        let id = g.id;
                        // Per-node Props listener -> live volume/mute into the model.
                        let model_p = model_g.clone();
                        let publish_p = publish_g.clone();
                        // Per-node INFO listener -> learn `card.profile.device`, the index
                        // that maps this node to its device's output Route. The registry
                        // global doesn't carry it (only device.id), so we read it from INFO,
                        // same as the device bus-path (#9). Needed to write the audible volume.
                        let model_i = model_g.clone();
                        let listener = node
                            .add_listener_local()
                            .info(move |info| {
                                let Some(props) = info.props() else { return };
                                let Some(cpd) = props.get("card.profile.device").and_then(|s| s.parse::<i32>().ok()) else { return };
                                let mut m = model_i.lock().unwrap();
                                if let Some(n) = m.nodes.get_mut(&id) {
                                    n.card_profile_device = Some(cpd);
                                }
                            })
                            .param(move |_seq, _id, _idx, _next, param| {
                                let Some(pod) = param else { return };
                                let Some((chans, mute)) = decode_props(pod) else { return };
                                let mut m = model_p.lock().unwrap();
                                if let Some(n) = m.nodes.get_mut(&id) {
                                    if let Some(c) = chans {
                                        if !c.is_empty() {
                                            n.channel_volumes = c;
                                        }
                                    }
                                    if let Some(mu) = mute {
                                        n.muted = mu;
                                    }
                                }
                                drop(m);
                                publish_p();
                            })
                            .register();
                        node.subscribe_params(&[pw::spa::param::ParamType::Props]);
                        proxies_g.borrow_mut().insert(id, node);
                        listeners_g.borrow_mut().insert(id, Box::new(listener));
                    }
                }
                if g.type_ == ObjectType::Device {
                    if let Ok(device) = registry_g.bind::<pw::device::Device, _>(g) {
                        let id = g.id;
                        // Device INFO listener -> learn the port-position-stable bus path.
                        let model_d = model_g.clone();
                        let publish_d = publish_g.clone();
                        // Device Route param listener -> the *audible* hardware-mixer volume
                        // (the layer wpctl/GNOME move). We both READ it (reflect it onto the
                        // device's nodes) and cache the route index + channel volumes so a
                        // volume change can WRITE the route back. We subscribe to Route below,
                        // so external (wpctl/GNOME) changes now reflect live too.
                        let model_dp = model_g.clone();
                        let publish_dp = publish_g.clone();
                        let listener = device
                            .add_listener_local()
                            .param(move |_seq, _id, _idx, _next, param| {
                                let Some(pod) = param else { return };
                                let Some((route_index, route_device, chans, mute)) = decode_route(pod) else { return };
                                let mut m = model_dp.lock().unwrap();
                                // Cache the route so SetVolume can address + overlay it.
                                if let Some(c) = &chans {
                                    if !c.is_empty() {
                                        m.routes.insert((id, route_device), RouteRec { index: route_index, channel_volumes: c.clone() });
                                    }
                                }
                                // Reflect it onto the node(s) on this device whose profile-device
                                // index matches the route's. Keyed on (device_id, route dev).
                                let mut changed = false;
                                for n in m.nodes.values_mut() {
                                    if n.device_id == Some(id) && n.card_profile_device == Some(route_device) {
                                        if let Some(c) = &chans {
                                            if !c.is_empty() {
                                                n.channel_volumes = c.clone();
                                                changed = true;
                                            }
                                        }
                                        if let Some(mu) = mute {
                                            n.muted = mu;
                                            changed = true;
                                        }
                                    }
                                }
                                drop(m);
                                if changed {
                                    publish_dp();
                                }
                            })
                            .info(move |info| {
                                let Some(props) = info.props() else { return };
                                // bus-path is the physical USB/PCI port (stable across reboot);
                                // api.alsa.path is a card-index fallback. NOT object.path /
                                // object.serial — those track enumeration order, not position.
                                let path = props
                                    .get("device.bus-path")
                                    .or_else(|| props.get("api.alsa.path"))
                                    .map(|s| s.to_string());
                                if path.is_none() {
                                    return;
                                }
                                let mut m = model_d.lock().unwrap();
                                let changed = m
                                    .devices
                                    .get_mut(&id)
                                    .map(|d| {
                                        let diff = d.path != path;
                                        d.path = path;
                                        diff
                                    })
                                    .unwrap_or(false);
                                drop(m);
                                // Node keys fold in this path, so re-publish to settle them.
                                if changed {
                                    publish_d();
                                }
                            })
                            .register();
                        // Keep route state fresh: subscribe for ongoing changes AND enum once
                        // now to seed the cache (subscribe only emits on change, so without
                        // the enum we'd have no route index/volumes to write until something
                        // else moves it). Needed to write volume; also reflects external changes.
                        device.subscribe_params(&[pw::spa::param::ParamType::Route]);
                        device.enum_params(0, Some(pw::spa::param::ParamType::Route), 0, u32::MAX);
                        dev_proxies_g.borrow_mut().insert(id, device);
                        dev_listeners_g.borrow_mut().insert(id, Box::new(listener));
                    }
                }
                if changed {
                    publish_g();
                }
            })
            .global_remove(move |id| {
                // Drop the listener before the proxy.
                listeners_r.borrow_mut().remove(&id);
                proxies_r.borrow_mut().remove(&id);
                dev_listeners_r.borrow_mut().remove(&id);
                dev_proxies_r.borrow_mut().remove(&id);
                let mut m = model_r.lock().unwrap();
                let changed = m.nodes.remove(&id).is_some()
                    | m.ports.remove(&id).is_some()
                    | m.links.remove(&id).is_some()
                    | m.devices.remove(&id).is_some();
                drop(m);
                if changed {
                    publish_r();
                }
            })
            .register()
    };

    // Commands from the axum threads run here, on the loop thread.
    let created: RefCell<Vec<pw::link::Link>> = RefCell::new(Vec::new());
    let core_cmd = core.clone();
    let registry_cmd = registry.clone();
    let model_cmd = model.clone();
    let proxies_cmd = node_proxies.clone();
    let dev_proxies_cmd = device_proxies.clone();
    let _recv = cmd_rx.attach(mainloop.loop_(), move |cmd| match cmd {
        Cmd::CreateLink { output_key, input_key } => {
            let ids = {
                let m = model_cmd.lock().unwrap();
                m.find_port_id(&output_key).zip(m.find_port_id(&input_key))
            };
            let Some(((out_port, out_node), (in_port, in_node))) = ids else {
                tracing::warn!("create_link: unknown port key(s)");
                return;
            };
            let props = pw::properties::properties! {
                *pw::keys::LINK_OUTPUT_NODE => out_node.to_string(),
                *pw::keys::LINK_OUTPUT_PORT => out_port.to_string(),
                *pw::keys::LINK_INPUT_NODE  => in_node.to_string(),
                *pw::keys::LINK_INPUT_PORT  => in_port.to_string(),
                *pw::keys::OBJECT_LINGER => "true",
            };
            match core_cmd.create_object::<pw::link::Link>("link-factory", &props) {
                Ok(link) => created.borrow_mut().push(link),
                Err(e) => tracing::warn!("create_object link failed: {e:?}"),
            }
        }
        Cmd::DestroyLink { output_key, input_key } => {
            let link_id = {
                let m = model_cmd.lock().unwrap();
                let out = m.find_port_id(&output_key).map(|(p, _)| p);
                let inp = m.find_port_id(&input_key).map(|(p, _)| p);
                match (out, inp) {
                    (Some(o), Some(i)) => m
                        .links
                        .iter()
                        .find(|(_, (lo, li))| *lo == o && *li == i)
                        .map(|(id, _)| *id),
                    _ => None,
                }
            };
            if let Some(id) = link_id {
                registry_cmd.destroy_global(id);
            } else {
                tracing::warn!("destroy_link: no matching link");
            }
        }
        Cmd::SetVolume { node_key, target } => {
            // Prefer the device Route (the audible hardware mixer). The node's Props volume
            // is decoupled from output on hardware sinks (box-confirmed: node 0.1 vs route
            // 1.0), so writing it changes nothing you can hear. Overlay the zone's channels
            // onto the route's current channelVolumes so other channels are preserved.
            let route = model_cmd.lock().unwrap().route_for_node(&node_key);
            if let Some((dev_id, dev_index, rec)) = route {
                let vols = build_channel_volumes(&target, rec.channel_volumes.len(), &rec.channel_volumes);
                if let Some(device) = dev_proxies_cmd.borrow().get(&dev_id) {
                    set_route_props(device, rec.index, dev_index, Some(vols.clone()), None);
                }
                // The device does NOT emit a Route event for our own writes, so the cached
                // route would stay at its pre-write value — and the next zone's write (which
                // overlays its channels onto this cache) would clobber the channels we just
                // set, making co-resident zones appear "tied". Update the cache ourselves so
                // each write preserves every other zone's latest level.
                if let Ok(mut m) = model_cmd.lock() {
                    if let Some(r) = m.routes.get_mut(&(dev_id, dev_index)) {
                        r.channel_volumes = vols;
                    }
                }
                return;
            }
            tracing::warn!("set_volume: no route for node {node_key}, falling back to node Props");
            // Fallback: no device route (stream node) -> node Props volume is audible there.
            let resolved = {
                let m = model_cmd.lock().unwrap();
                m.find_node(&node_key).map(|(nid, channels)| {
                    let current = m.nodes.get(&nid).map(|n| n.channel_volumes.clone()).unwrap_or_default();
                    (nid, channels, current)
                })
            };
            let Some((nid, channels, current)) = resolved else {
                tracing::warn!("set_volume: unknown node key");
                return;
            };
            if let Some(node) = proxies_cmd.borrow().get(&nid) {
                let vols = build_channel_volumes(&target, channels, &current);
                set_node_props(node, Some(vols), None);
            }
        }
        Cmd::SetMute { node_key, muted } => {
            // Mute on the route too (node-wide; PipeWire has no per-channel mute).
            let route = model_cmd.lock().unwrap().route_for_node(&node_key);
            if let Some((dev_id, dev_index, rec)) = route {
                if let Some(device) = dev_proxies_cmd.borrow().get(&dev_id) {
                    set_route_props(device, rec.index, dev_index, None, Some(muted));
                }
                return;
            }
            let resolved = model_cmd.lock().unwrap().find_node(&node_key);
            let Some((nid, _)) = resolved else {
                tracing::warn!("set_mute: unknown node key");
                return;
            };
            if let Some(node) = proxies_cmd.borrow().get(&nid) {
                set_node_props(node, None, Some(muted));
            }
        }
        Cmd::Rescan => {
            // Pull, don't watch (#3). enum_params delivers current values on the existing
            // node `.param` (Props) and device `.param` (Route) listeners, which update the
            // model and publish a fresh snapshot. num=u32::MAX asks for every instance; SPA
            // bounds it. The Route read is what surfaces wpctl/GNOME volume changes.
            for node in proxies_cmd.borrow().values() {
                node.enum_params(0, Some(pw::spa::param::ParamType::Props), 0, u32::MAX);
            }
            for device in dev_proxies_cmd.borrow().values() {
                device.enum_params(0, Some(pw::spa::param::ParamType::Route), 0, u32::MAX);
            }
        }
    });

    let _ = ready.send(Ok(()));
    mainloop.run(); // blocks; this thread is the single pw writer.
}

/// Set `Props` on a node: per-channel volumes and/or mute, via a serialized SPA POD.
/// This is the Q4 target — zone volume lives on the sink node's `channelVolumes`.
fn set_node_props(node: &pw::node::Node, channel_volumes: Option<Vec<f32>>, mute: Option<bool>) {
    use pw::spa::pod::{serialize::PodSerializer, Object, Property, PropertyFlags, Value, ValueArray};

    let mut properties = Vec::new();
    if let Some(vols) = channel_volumes {
        properties.push(Property {
            key: pw::spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(vols)),
        });
    }
    if let Some(m) = mute {
        properties.push(Property {
            key: pw::spa::sys::SPA_PROP_mute,
            flags: PropertyFlags::empty(),
            value: Value::Bool(m),
        });
    }
    if properties.is_empty() {
        return;
    }

    let object = Value::Object(Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pw::spa::sys::SPA_PARAM_Props,
        properties,
    });

    let mut bytes = Vec::new();
    if PodSerializer::serialize(std::io::Cursor::new(&mut bytes), &object).is_ok() {
        if let Some(pod) = pw::spa::pod::Pod::from_bytes(&bytes) {
            node.set_param(pw::spa::param::ParamType::Props, 0, pod);
        }
    }
}

/// Set a device `Route`'s volume/mute — the *audible* hardware-mixer layer (what wpctl
/// writes). Builds `ParamRoute{ index, device, props: Props{ channelVolumes?, mute? },
/// save }` and applies it to the device. `route_index`/`device_index` address the route;
/// `channel_volumes` must cover all of the route's channels (overlay the zone's onto the
/// current ones before calling). Mirrors `set_node_props`' POD-build pattern.
fn set_route_props(
    device: &pw::device::Device,
    route_index: i32,
    device_index: i32,
    channel_volumes: Option<Vec<f32>>,
    mute: Option<bool>,
) {
    use pw::spa::pod::{serialize::PodSerializer, Object, Property, PropertyFlags, Value, ValueArray};

    // Inner Props object carrying the actual volume/mute.
    let mut props = Vec::new();
    if let Some(vols) = channel_volumes {
        props.push(Property {
            key: pw::spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(vols)),
        });
    }
    if let Some(m) = mute {
        props.push(Property { key: pw::spa::sys::SPA_PROP_mute, flags: PropertyFlags::empty(), value: Value::Bool(m) });
    }
    if props.is_empty() {
        return;
    }
    let props_obj = Value::Object(Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pw::spa::sys::SPA_PARAM_Props,
        properties: props,
    });

    // Outer ParamRoute object: address the route + carry the props, and persist it.
    let route_obj = Value::Object(Object {
        type_: pw::spa::sys::SPA_TYPE_OBJECT_ParamRoute,
        id: pw::spa::sys::SPA_PARAM_Route,
        properties: vec![
            Property { key: pw::spa::sys::SPA_PARAM_ROUTE_index, flags: PropertyFlags::empty(), value: Value::Int(route_index) },
            Property { key: pw::spa::sys::SPA_PARAM_ROUTE_device, flags: PropertyFlags::empty(), value: Value::Int(device_index) },
            Property { key: pw::spa::sys::SPA_PARAM_ROUTE_props, flags: PropertyFlags::empty(), value: props_obj },
            Property { key: pw::spa::sys::SPA_PARAM_ROUTE_save, flags: PropertyFlags::empty(), value: Value::Bool(true) },
        ],
    });

    let mut bytes = Vec::new();
    if PodSerializer::serialize(std::io::Cursor::new(&mut bytes), &route_obj).is_ok() {
        if let Some(pod) = pw::spa::pod::Pod::from_bytes(&bytes) {
            device.set_param(pw::spa::param::ParamType::Route, 0, pod);
        }
    }
}

/// Decode a node `Props` POD into (per-channel volumes, mute). The channel array is `None`
/// when no `channelVolumes` is present (a scalar-only Props emission), so callers leave the
/// known per-channel value untouched. Returns `None` for the whole POD when it carries
/// neither channelVolumes nor mute, so unrelated Props emissions don't trigger redundant
/// snapshot broadcasts.
fn decode_props(pod: &pw::spa::pod::Pod) -> Option<(Option<Vec<f32>>, Option<bool>)> {
    use pw::spa::pod::{deserialize::PodDeserializer, Value};

    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(obj) = value else { return None };

    let (channel_volumes, mute) = scan_props_object(obj);
    // Nothing relevant in this Props emission — signal "no change" so the caller skips
    // the model update and the broadcast it would otherwise trigger.
    if channel_volumes.is_none() && mute.is_none() {
        return None;
    }
    Some((channel_volumes, mute))
}

/// Pull `channelVolumes` + `mute` out of a SPA Props object. Shared by `decode_props`
/// (a node's top-level Props) and `decode_route` (the Props nested inside a device Route).
fn scan_props_object(obj: pw::spa::pod::Object) -> (Option<Vec<f32>>, Option<bool>) {
    use pw::spa::pod::{Value, ValueArray};

    let mut channel_volumes: Option<Vec<f32>> = None;
    let mut mute: Option<bool> = None;
    for p in obj.properties {
        match p.key {
            // Volume is read ONLY from channelVolumes — the per-channel layer we control.
            // The scalar SPA_PROP_volume is a separate master that stays 1.0 here; using
            // it would clobber the real per-channel value on scalar-only Props emissions.
            pw::spa::sys::SPA_PROP_channelVolumes => {
                if let Value::ValueArray(ValueArray::Float(v)) = p.value {
                    channel_volumes = Some(v);
                }
            }
            pw::spa::sys::SPA_PROP_mute => {
                if let Value::Bool(b) = p.value {
                    mute = Some(b);
                }
            }
            _ => {}
        }
    }
    (channel_volumes, mute)
}

/// Decode a device `Route` param into (route index, profile-device index, per-channel
/// volumes, mute). The route `index` + `device` together address the route for a write;
/// the nested `props` Props object holds the channelVolumes/mute that `wpctl`/GNOME move
/// (the audible hardware-mixer layer, separate from node Props — Q4/#3). Returns `None`
/// if it carries no usable volume/mute, so callers skip redundant updates.
fn decode_route(pod: &pw::spa::pod::Pod) -> Option<(i32, i32, Option<Vec<f32>>, Option<bool>)> {
    use pw::spa::pod::{deserialize::PodDeserializer, Value};

    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(obj) = value else { return None };

    let mut index: Option<i32> = None;
    let mut device_index: Option<i32> = None;
    let mut props: Option<(Option<Vec<f32>>, Option<bool>)> = None;
    for p in obj.properties {
        match p.key {
            pw::spa::sys::SPA_PARAM_ROUTE_index => {
                if let Value::Int(i) = p.value {
                    index = Some(i);
                }
            }
            pw::spa::sys::SPA_PARAM_ROUTE_device => {
                if let Value::Int(i) = p.value {
                    device_index = Some(i);
                }
            }
            // The route's volume/mute live in a nested Props object — same shape as a
            // node's Props, so reuse the scanner.
            pw::spa::sys::SPA_PARAM_ROUTE_props => {
                if let Value::Object(inner) = p.value {
                    props = Some(scan_props_object(inner));
                }
            }
            _ => {}
        }
    }

    let index = index?;
    let device_index = device_index?;
    let (channel_volumes, mute) = props?;
    if channel_volumes.is_none() && mute.is_none() {
        return None;
    }
    Some((index, device_index, channel_volumes, mute))
}

/// Fold one registry global into the model. Returns true if the model changed.
fn ingest_global(model: &Arc<Mutex<Model>>, g: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>) -> bool {
    let props = match g.props {
        Some(p) => p,
        None => return false,
    };
    let mut m = model.lock().unwrap();
    match g.type_ {
        ObjectType::Device => {
            // The registry global only carries device.name/class — NOT the bus path. That
            // comes from the device's INFO event (see the device listener in run_loop),
            // which fills `path`. Preserve any path already learned across re-announces.
            let path = m.devices.get(&g.id).and_then(|d| d.path.clone());
            m.devices.insert(g.id, DeviceRec { path });
            true
        }
        ObjectType::Node => {
            // Preserve volume + the INFO-learned profile-device index across a re-announce
            // (the registry global re-parse below would otherwise wipe card.profile.device,
            // which only arrives via the node INFO event).
            let (channel_volumes, muted, prev_cpd) = m
                .nodes
                .get(&g.id)
                .map(|n| (n.channel_volumes.clone(), n.muted, n.card_profile_device))
                .unwrap_or((Vec::new(), false, None));
            let device_id = props.get("device.id").and_then(|s| s.parse().ok());
            let card_profile_device = props.get("card.profile.device").and_then(|s| s.parse().ok()).or(prev_cpd);
            m.nodes.insert(
                g.id,
                NodeRec {
                    name: props.get("node.name").unwrap_or("").to_string(),
                    media_class: props.get("media.class").unwrap_or("").to_string(),
                    device_id,
                    card_profile_device,
                    channel_volumes,
                    muted,
                },
            );
            true
        }
        ObjectType::Port => {
            let node_id = props.get("node.id").and_then(|s| s.parse().ok());
            let Some(node_id) = node_id else { return false };
            let dir = match props.get("port.direction") {
                Some("out") => Direction::Out,
                _ => Direction::In,
            };
            // `port.id` is the channel index into the node's channelVolumes (box-verified:
            // it lines up with `audio.position`). Used to drive only a zone's own channels.
            let channel = props.get("port.id").and_then(|s| s.parse().ok());
            m.ports.insert(
                g.id,
                PortRec { node_id, name: props.get("port.name").unwrap_or("").to_string(), dir, channel },
            );
            true
        }
        ObjectType::Link => {
            let out = props.get("link.output.port").and_then(|s| s.parse().ok());
            let inp = props.get("link.input.port").and_then(|s| s.parse().ok());
            if let (Some(o), Some(i)) = (out, inp) {
                m.links.insert(g.id, (o, i));
                true
            } else {
                false
            }
        }
        _ => false,
    }
}
