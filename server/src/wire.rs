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
    /// Stable key: `media.class|node.name`. Survives reconnect; numeric PipeWire ids
    /// do not. Deliberately excludes `object.serial` (per-session, not durable). See `identity`.
    pub key: String,
    pub name: String,
    pub media_class: String,
    pub ports: Vec<PortView>,
    /// Representative level, 0.0..=1.0 (the max across channels). `None` if this node has
    /// no volume control. For a one-knob UI; per-channel detail lives in `channel_volumes`.
    pub volume: Option<f32>,
    /// Raw-linear per-channel volumes in the node's channel order (empty if no volume
    /// control). Lets a client show/drive individual channels (e.g. "card ch 7-8 -> patio").
    #[serde(default)]
    pub channel_volumes: Vec<f32>,
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
    /// The zone's defined links (its routing recipe) — independent of what's currently
    /// live in the graph. Lets a client edit the zone (add/remove links) without a
    /// separate fetch. Distinct from top-level `GraphState.links`, which are live links.
    pub links: Vec<LinkView>,
    pub active: bool,
    /// True when the zone is active but some of its devices/ports are missing.
    pub degraded: bool,
    /// Stable keys of devices the zone wants but can't currently reach.
    pub missing: Vec<String>,
    /// The zone's representative node — the sink whose volume the zone tile controls
    /// (the first volume-spec node, else the sink behind the zone's first link). `None`
    /// when the zone has no controllable node. Clients PUT volume changes to this key.
    pub volume_node: Option<String>,
    /// Live representative volume (0.0..=1.0) of `volume_node`, if that node is present.
    /// `None` -> the tile shows no slider (node absent or zone has no volume node).
    pub volume: Option<f32>,
    /// Live mute state of `volume_node` (false when there's no volume node).
    pub muted: bool,
}
