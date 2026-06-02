//! Wire types — the single source of truth for what crosses the network.
//!
//! Protocol is SNAPSHOT-ONLY (eng-review decision): the server sends the entire
//! `GraphState` whenever anything changes. No deltas, no seq numbers. The graph is
//! tens of objects, so a few KB resent on change is free, and a whole class of
//! sync bugs disappears.
//!
//! These structs derive `JsonSchema` so the Dart client models can be generated
//! from them (zero cross-language drift). Keep them flat and obvious.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Everything a client needs to render, in one message.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct GraphState {
    /// True when the server is talking to a live PipeWire (false = degraded/mock).
    pub connected: bool,
    pub nodes: Vec<NodeView>,
    pub links: Vec<LinkView>,
    pub zones: Vec<ZoneView>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NodeView {
    /// Stable key: `(node.name, media.class)[+serial]`. Survives reconnect; numeric
    /// PipeWire ids do not. See `identity`.
    pub key: String,
    pub name: String,
    pub media_class: String,
    pub ports: Vec<PortView>,
    /// 0.0..=1.0. `None` if this node has no volume control.
    pub volume: Option<f32>,
    pub muted: bool,
    /// False when the device is not currently present (unplugged); zones depending
    /// on it show "degraded".
    pub present: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct PortView {
    /// Stable key: `(node_key, port.name, direction)`.
    pub key: String,
    pub name: String,
    pub direction: Direction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkView {
    pub output_port: String, // port key (Direction::Out)
    pub input_port: String,  // port key (Direction::In)
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ZoneView {
    pub name: String,
    pub active: bool,
    /// True when the zone is active but some of its devices/ports are missing.
    pub degraded: bool,
    /// Stable keys of devices the zone wants but can't currently reach.
    pub missing: Vec<String>,
}
