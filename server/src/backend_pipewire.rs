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
//! Implemented: read path (nodes/ports/links), link create/destroy, and volume/mute
//! via the node's `Props.channelVolumes`/`mute` SPA POD (read through a per-node Props
//! listener, written via `set_node_props`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use pipewire as pw;
use pw::types::ObjectType;
use tokio::sync::broadcast;

use crate::backend::{BackendError, PwBackend};
use crate::identity::{node_key, port_key};
use crate::model::Action;
use crate::wire::{Direction, GraphState, LinkView, NodeView, PortView};

// ---- in-memory model, written only by the loop thread ----

#[derive(Default)]
struct Model {
    nodes: HashMap<u32, NodeRec>,
    ports: HashMap<u32, PortRec>,
    links: HashMap<u32, (u32, u32)>, // link id -> (output_port_id, input_port_id)
}

struct NodeRec {
    name: String,
    media_class: String,
    /// Live volume (representative across channels) + mute, filled by the Props
    /// param listener. `None` volume = not yet reported / no volume control.
    volume: Option<f32>,
    muted: bool,
}

struct PortRec {
    node_id: u32,
    name: String,
    dir: Direction,
}

impl Model {
    /// Compute a port's stable key from its owning node (or None if node unknown).
    fn port_stable_key(&self, port_id: u32) -> Option<String> {
        let p = self.ports.get(&port_id)?;
        let n = self.nodes.get(&p.node_id)?;
        let nk = node_key(&n.name, &n.media_class);
        Some(port_key(&nk, &p.name, p.dir))
    }

    fn build_graph(&self) -> GraphState {
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (id, n) in &self.nodes {
            let nk = node_key(&n.name, &n.media_class);
            let ports = self
                .ports
                .iter()
                .filter(|(_, p)| p.node_id == *id)
                .map(|(_pid, p)| PortView {
                    key: port_key(&nk, &p.name, p.dir),
                    name: p.name.clone(),
                    direction: p.dir,
                })
                .collect();
            nodes.push(NodeView {
                key: nk,
                name: n.name.clone(),
                media_class: n.media_class.clone(),
                ports,
                volume: n.volume,
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
            if node_key(&n.name, &n.media_class) == key {
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
}

// ---- commands marshaled onto the loop thread ----

enum Cmd {
    CreateLink { output_key: String, input_key: String },
    DestroyLink { output_key: String, input_key: String },
    SetVolume { node_key: String, volume: f32 },
    SetMute { node_key: String, muted: bool },
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
            // confirmation event. The listener still seeds correct INITIAL volumes.
            Action::SetVolume { node_key, volume } => {
                if let Ok(mut m) = self.model.lock() {
                    if let Some((nid, _)) = m.find_node(&node_key) {
                        if let Some(n) = m.nodes.get_mut(&nid) {
                            n.volume = Some(volume.clamp(0.0, 1.0));
                        }
                    }
                }
                self.cmd_tx
                    .send(Cmd::SetVolume { node_key, volume })
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

    let _listener = {
        let model_g = model.clone();
        let publish_g = publish.clone();
        let registry_g = registry.clone();
        let proxies_g = node_proxies.clone();
        let listeners_g = node_listeners.clone();
        let model_r = model.clone();
        let publish_r = publish.clone();
        let proxies_r = node_proxies.clone();
        let listeners_r = node_listeners.clone();
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
                        let listener = node
                            .add_listener_local()
                            .param(move |_seq, _id, _idx, _next, param| {
                                let Some(pod) = param else { return };
                                let Some((vol, mute)) = decode_props(pod) else { return };
                                let mut m = model_p.lock().unwrap();
                                if let Some(n) = m.nodes.get_mut(&id) {
                                    if vol.is_some() {
                                        n.volume = vol;
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
                if changed {
                    publish_g();
                }
            })
            .global_remove(move |id| {
                // Drop the listener before the proxy.
                listeners_r.borrow_mut().remove(&id);
                proxies_r.borrow_mut().remove(&id);
                let mut m = model_r.lock().unwrap();
                let changed = m.nodes.remove(&id).is_some()
                    | m.ports.remove(&id).is_some()
                    | m.links.remove(&id).is_some();
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
        Cmd::SetVolume { node_key, volume } => {
            let resolved = model_cmd.lock().unwrap().find_node(&node_key);
            let Some((nid, channels)) = resolved else {
                tracing::warn!("set_volume: unknown node key");
                return;
            };
            if let Some(node) = proxies_cmd.borrow().get(&nid) {
                let n = channels.max(1);
                set_node_props(node, Some(vec![volume.clamp(0.0, 1.0); n]), None);
            }
        }
        Cmd::SetMute { node_key, muted } => {
            let resolved = model_cmd.lock().unwrap().find_node(&node_key);
            let Some((nid, _)) = resolved else {
                tracing::warn!("set_mute: unknown node key");
                return;
            };
            if let Some(node) = proxies_cmd.borrow().get(&nid) {
                set_node_props(node, None, Some(muted));
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

/// Decode a node `Props` POD into (representative volume, mute). Volume is derived
/// ONLY from `channelVolumes` (the max channel); the scalar `SPA_PROP_volume` is
/// intentionally ignored. Returns `None` when the POD carries neither channelVolumes
/// nor mute, so unrelated Props emissions don't trigger redundant snapshot broadcasts.
fn decode_props(pod: &pw::spa::pod::Pod) -> Option<(Option<f32>, Option<bool>)> {
    use pw::spa::pod::{deserialize::PodDeserializer, Value, ValueArray};

    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(obj) = value else { return None };

    let mut volume: Option<f32> = None;
    let mut mute: Option<bool> = None;
    for p in obj.properties {
        match p.key {
            // Volume is read ONLY from channelVolumes — the per-channel layer we control.
            // The scalar SPA_PROP_volume is a separate master that stays 1.0 here; using
            // it would clobber the real per-channel value on scalar-only Props emissions.
            pw::spa::sys::SPA_PROP_channelVolumes => {
                if let Value::ValueArray(ValueArray::Float(v)) = p.value {
                    volume = v.into_iter().fold(volume, |acc, x| Some(acc.map_or(x, |m| m.max(x))));
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
    // Nothing relevant in this Props emission — signal "no change" so the caller skips
    // the model update and the broadcast it would otherwise trigger.
    if volume.is_none() && mute.is_none() {
        return None;
    }
    Some((volume, mute))
}

/// Fold one registry global into the model. Returns true if the model changed.
fn ingest_global(model: &Arc<Mutex<Model>>, g: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>) -> bool {
    let props = match g.props {
        Some(p) => p,
        None => return false,
    };
    let mut m = model.lock().unwrap();
    match g.type_ {
        ObjectType::Node => {
            // Preserve any volume already learned for this id (re-announce shouldn't wipe it).
            let (volume, muted) = m
                .nodes
                .get(&g.id)
                .map(|n| (n.volume, n.muted))
                .unwrap_or((None, false));
            m.nodes.insert(
                g.id,
                NodeRec {
                    name: props.get("node.name").unwrap_or("").to_string(),
                    media_class: props.get("media.class").unwrap_or("").to_string(),
                    volume,
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
            m.ports.insert(
                g.id,
                PortRec { node_id, name: props.get("port.name").unwrap_or("").to_string(), dir },
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
