//! The PipeWire backend boundary.
//!
//! Everything that touches PipeWire hides behind `PwBackend`. The rest of the server
//! (model, api, ws) only knows this trait. That's the seam the spike's Q3 answer
//! plugs into: `MockBackend` today (runs anywhere), `PipewireBackend` later (Linux),
//! and if WirePlumber fights raw PipeWire, a `WirePlumberBackend` — same trait, no
//! changes above it.

use std::sync::Mutex;

use thiserror::Error;
use tokio::sync::broadcast;

use crate::identity::{node_key, port_key};
use crate::model::{Action, VolumeTarget};
use crate::wire::{Direction, GraphState, LinkView, NodeView, PortView};

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("backend rejected action: {0}")]
    Rejected(String),
}

/// The only interface the rest of the server has to PipeWire.
pub trait PwBackend: Send + Sync + 'static {
    /// Current full graph (nodes/links/volumes). Snapshot-only protocol.
    fn snapshot(&self) -> GraphState;
    /// Apply one mutation. The pw backend marshals this onto its loop thread.
    fn apply(&self, action: Action) -> Result<(), BackendError>;
    /// Subscribe to graph changes; each message is a fresh full snapshot.
    fn subscribe(&self) -> broadcast::Receiver<GraphState>;
    /// Actively re-read volume/mute state from PipeWire and broadcast a fresh snapshot.
    /// This is the pull-based counterpart to the push listeners: external changes
    /// (`wpctl`/GNOME) land on the device's route params and don't reliably emit node
    /// `Props` events, so they only surface on an explicit rescan. See NEXT_STEPS #3.
    fn rescan(&self) -> Result<(), BackendError>;
}

/// In-memory backend with no PipeWire dependency. Lets the whole server compile and
/// run on macOS, drives the Flutter app during development, and backs the API tests.
pub struct MockBackend {
    inner: Mutex<GraphState>,
    tx: broadcast::Sender<GraphState>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self { inner: Mutex::new(seed_graph()), tx }
    }
}

impl PwBackend for MockBackend {
    fn snapshot(&self) -> GraphState {
        self.inner.lock().unwrap().clone()
    }

    fn subscribe(&self) -> broadcast::Receiver<GraphState> {
        self.tx.subscribe()
    }

    fn rescan(&self) -> Result<(), BackendError> {
        // No live PipeWire to re-read; just re-broadcast the current snapshot so the
        // refresh path (app -> REST -> WS) is exercisable on the dev/mock setup.
        let _ = self.tx.send(self.inner.lock().unwrap().clone());
        Ok(())
    }

    fn apply(&self, action: Action) -> Result<(), BackendError> {
        let snap = {
            let mut g = self.inner.lock().unwrap();
            match action {
                Action::CreateLink { output_port, input_port } => {
                    let lv = LinkView { output_port, input_port };
                    if !g.links.contains(&lv) {
                        g.links.push(lv);
                    }
                }
                Action::DestroyLink { output_port, input_port } => {
                    g.links
                        .retain(|l| !(l.output_port == output_port && l.input_port == input_port));
                }
                Action::SetVolume { node_key, target } => {
                    let n = g
                        .nodes
                        .iter_mut()
                        .find(|n| n.key == node_key)
                        .ok_or_else(|| BackendError::Rejected(format!("no node {node_key}")))?;
                    apply_volume_target(n, &target);
                }
                Action::SetMute { node_key, muted } => {
                    let n = g
                        .nodes
                        .iter_mut()
                        .find(|n| n.key == node_key)
                        .ok_or_else(|| BackendError::Rejected(format!("no node {node_key}")))?;
                    n.muted = muted;
                }
            }
            g.clone()
        };
        let _ = self.tx.send(snap); // ignore: no subscribers is fine
        Ok(())
    }
}

/// Apply a [`VolumeTarget`] to a node's `channel_volumes`, then refresh the representative
/// `volume` (the max across channels). Uniform replaces all channels; Channels overlays the
/// listed indices (growing the array if needed), leaving the rest untouched.
fn apply_volume_target(n: &mut NodeView, target: &VolumeTarget) {
    match target {
        VolumeTarget::Uniform(v) => {
            let v = v.clamp(0.0, 1.0);
            let len = n.channel_volumes.len().max(1);
            n.channel_volumes = vec![v; len];
        }
        VolumeTarget::Channels(pairs) => {
            for (idx, v) in pairs {
                if *idx >= n.channel_volumes.len() {
                    n.channel_volumes.resize(idx + 1, 0.0);
                }
                n.channel_volumes[*idx] = v.clamp(0.0, 1.0);
            }
        }
    }
    n.volume = n.channel_volumes.iter().cloned().fold(None::<f32>, |a, x| Some(a.map_or(x, |m| m.max(x))));
}

/// Seed a believable graph: one USB capture source + two sink "amps" to route to.
fn seed_graph() -> GraphState {
    let src = node_key("USB Capture", "Audio/Source", None);
    let kitchen = node_key("Kitchen Amp", "Audio/Sink", None);
    let patio = node_key("Patio Amp", "Audio/Sink", None);

    let source = NodeView {
        ports: vec![
            PortView { key: port_key(&src, "capture_FL", Direction::Out), name: "capture_FL".into(), direction: Direction::Out, channel: Some(0) },
            PortView { key: port_key(&src, "capture_FR", Direction::Out), name: "capture_FR".into(), direction: Direction::Out, channel: Some(1) },
        ],
        key: src,
        name: "USB Capture".into(),
        media_class: "Audio/Source".into(),
        volume: None,
        channel_volumes: vec![],
        muted: false,
        present: true,
    };

    GraphState {
        connected: false, // mock backend is never "really" connected to PipeWire
        nodes: vec![source, sink(&kitchen, "Kitchen Amp"), sink(&patio, "Patio Amp")],
        links: vec![],
        zones: vec![],
    }
}

fn sink(key: &str, name: &str) -> NodeView {
    NodeView {
        ports: vec![
            PortView { key: port_key(key, "playback_FL", Direction::In), name: "playback_FL".into(), direction: Direction::In, channel: Some(0) },
            PortView { key: port_key(key, "playback_FR", Direction::In), name: "playback_FR".into(), direction: Direction::In, channel: Some(1) },
        ],
        key: key.to_string(),
        name: name.to_string(),
        media_class: "Audio/Sink".into(),
        volume: Some(0.8),
        channel_volumes: vec![0.8, 0.8],
        muted: false,
        present: true,
    }
}
