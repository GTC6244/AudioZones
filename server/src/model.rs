//! Reconciliation — the controller core.
//!
//!    DESIRED STATE                         ACTUAL STATE
//!    (active zones' merged                 (live graph: current
//!     links + volumes)                      links + node volumes)
//!         │                                      │
//!         └──────────────► reconcile() ◄─────────┘
//!                              │
//!              emits the minimal set of Actions that drive
//!              actual -> desired. Runs on every graph change
//!              AND every zone activate/deactivate.
//!
//! "Turn a zone on" = add its intent to desired + reconcile.
//! "Device returns"  = graph change -> reconcile -> missing links get created.
//! One mechanism, every case. Auto-reapply falls out for free.

use std::collections::BTreeSet;

use crate::wire::{GraphState, NodeView};
use crate::zones::{ZoneStore, VolumeSpec};

/// Closeness threshold for "this volume already matches" — avoids reconcile churn from
/// float round-trips.
const VOL_EPS: f32 = 0.001;

/// What level to push to a node's volume.
#[derive(Clone, Debug, PartialEq)]
pub enum VolumeTarget {
    /// Same raw-linear level on every channel.
    Uniform(f32),
    /// Specific channels by 0-based index; channels not listed keep their current value.
    Channels(Vec<(usize, f32)>),
}

/// A single mutation the backend should apply.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    CreateLink { output_port: String, input_port: String },
    DestroyLink { output_port: String, input_port: String },
    SetVolume { node_key: String, target: VolumeTarget },
    SetMute { node_key: String, muted: bool },
}

/// The merged target derived from all active zones.
#[derive(Debug, Default)]
pub struct Desired {
    pub links: BTreeSet<(String, String)>, // (output_port, input_port)
    pub volumes: Vec<VolumeSpec>,           // last-writer-wins on conflict (see below)
}

/// Build the desired state from the active zones.
///
/// Links union (a sink mixing two sources is normal). Volume conflicts on the same
/// node resolve last-active-wins (documented eng-review default).
pub fn desired_from_active(store: &ZoneStore) -> Desired {
    let mut d = Desired::default();
    for name in &store.active {
        let Some(zone) = store.get(name) else { continue };
        for l in &zone.links {
            d.links.insert((l.output_port.clone(), l.input_port.clone()));
        }
        for v in &zone.volumes {
            // last-writer-wins: drop any prior entry for this node, push the new one.
            d.volumes.retain(|e| e.node_key != v.node_key);
            d.volumes.push(v.clone());
        }
    }
    d
}

/// Diff desired vs actual, emit the minimal Actions. Only links belonging to the
/// desired set are created; links present in actual but not desired are torn down
/// ONLY if they look zone-managed... for v1 we take the conservative path and do not
/// destroy links we didn't create (avoids fighting WirePlumber's own links). We DO
/// create missing desired links and correct volumes.
pub fn reconcile(desired: &Desired, actual: &GraphState) -> Vec<Action> {
    let mut actions = Vec::new();

    let actual_links: BTreeSet<(String, String)> = actual
        .links
        .iter()
        .map(|l| (l.output_port.clone(), l.input_port.clone()))
        .collect();

    // Ports that actually exist right now. A desired link is only creatable once BOTH
    // its endpoints are present — otherwise the backend can't resolve the keys and would
    // just log "unknown port key(s)". Gating here means reconcile is safe to run on every
    // graph change: during the startup burst (ports arriving one at a time) we stay quiet,
    // then create the link the moment both endpoints appear (auto-reapply, no warnings).
    let present_ports: BTreeSet<&str> = actual
        .nodes
        .iter()
        .filter(|n| n.present)
        .flat_map(|n| n.ports.iter().map(|p| p.key.as_str()))
        .collect();

    // Create desired links whose endpoints both exist and aren't already linked.
    for (out, inp) in &desired.links {
        if !actual_links.contains(&(out.clone(), inp.clone()))
            && present_ports.contains(out.as_str())
            && present_ports.contains(inp.as_str())
        {
            actions.push(Action::CreateLink {
                output_port: out.clone(),
                input_port: inp.clone(),
            });
        }
    }

    // Correct volumes/mute where the node exists and differs.
    for v in &desired.volumes {
        if let Some(node) = actual.nodes.iter().find(|n| n.key == v.node_key) {
            if let Some(target) = volume_correction(v, node) {
                actions.push(Action::SetVolume { node_key: v.node_key.clone(), target });
            }
            if node.muted != v.muted {
                actions.push(Action::SetMute {
                    node_key: v.node_key.clone(),
                    muted: v.muted,
                });
            }
        }
    }

    actions
}

/// The volume Action needed to bring `node` to what `spec` wants, or `None` if it already
/// matches. Per-channel specs win over the uniform `volume`; a spec with neither is a no-op.
fn volume_correction(spec: &VolumeSpec, node: &NodeView) -> Option<VolumeTarget> {
    if !spec.channels.is_empty() {
        let any_diff = spec.channels.iter().any(|c| {
            node.channel_volumes
                .get(c.channel)
                .map_or(true, |cur| (cur - c.volume).abs() > VOL_EPS)
        });
        return any_diff.then(|| {
            VolumeTarget::Channels(spec.channels.iter().map(|c| (c.channel, c.volume)).collect())
        });
    }
    let vol = spec.volume?;
    // Prefer the live per-channel array; fall back to the representative scalar when the
    // backend reported no channels (e.g. a node with only a master volume).
    let matches = if node.channel_volumes.is_empty() {
        node.volume.map_or(false, |cur| (cur - vol).abs() <= VOL_EPS)
    } else {
        node.channel_volumes.iter().all(|cur| (cur - vol).abs() <= VOL_EPS)
    };
    (!matches).then_some(VolumeTarget::Uniform(vol))
}

/// Stable keys of devices an active zone wants but that aren't present -> "degraded".
pub fn missing_for_zone(
    store: &ZoneStore,
    zone_name: &str,
    actual: &GraphState,
) -> Vec<String> {
    let present_ports: BTreeSet<&str> = actual
        .nodes
        .iter()
        .filter(|n| n.present)
        .flat_map(|n| n.ports.iter().map(|p| p.key.as_str()))
        .collect();

    let Some(zone) = store.get(zone_name) else { return Vec::new() };
    let mut missing = BTreeSet::new();
    for l in &zone.links {
        if !present_ports.contains(l.output_port.as_str()) {
            missing.insert(l.output_port.clone());
        }
        if !present_ports.contains(l.input_port.as_str()) {
            missing.insert(l.input_port.clone());
        }
    }
    missing.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Direction, LinkView, NodeView, PortView};
    use crate::zones::{LinkSpec, ZoneDef};

    fn node(key: &str, vol: Option<f32>, present: bool, ports: Vec<PortView>) -> NodeView {
        NodeView {
            key: key.into(),
            name: key.into(),
            media_class: "Audio/Sink".into(),
            ports,
            volume: vol,
            channel_volumes: vol.map(|v| vec![v]).unwrap_or_default(),
            muted: false,
            present,
        }
    }

    fn node_channels(key: &str, channels: Vec<f32>, ports: Vec<PortView>) -> NodeView {
        NodeView {
            key: key.into(),
            name: key.into(),
            media_class: "Audio/Sink".into(),
            ports,
            volume: channels.iter().cloned().fold(None::<f32>, |a, x| Some(a.map_or(x, |m| m.max(x)))),
            channel_volumes: channels,
            muted: false,
            present: true,
        }
    }
    fn port(key: &str, dir: Direction) -> PortView {
        PortView { key: key.into(), name: key.into(), direction: dir }
    }

    fn store_with_active_patio() -> ZoneStore {
        let p = std::env::temp_dir().join("audiozones-model-test.toml");
        let _ = std::fs::remove_file(&p);
        let mut s = ZoneStore::load(&p).unwrap();
        s.zones.push(ZoneDef {
            name: "patio".into(),
            links: vec![LinkSpec { output_port: "OUT".into(), input_port: "IN".into() }],
            volumes: vec![VolumeSpec { node_key: "AMP".into(), volume: Some(0.5), channels: vec![], muted: false }],
        });
        s.set_active("patio", true).unwrap();
        s
    }

    #[test]
    fn reconcile_creates_missing_link() {
        let store = store_with_active_patio();
        let desired = desired_from_active(&store);
        // Both endpoint ports are present, no link yet -> create it.
        let actual = GraphState {
            nodes: vec![
                node("src", None, true, vec![port("OUT", Direction::Out)]),
                node("amp", None, true, vec![port("IN", Direction::In)]),
            ],
            ..Default::default()
        };
        let actions = reconcile(&desired, &actual);
        assert!(actions.contains(&Action::CreateLink {
            output_port: "OUT".into(),
            input_port: "IN".into()
        }));
    }

    #[test]
    fn reconcile_skips_link_when_a_port_is_absent() {
        // The boot-race guard: a desired link whose endpoints haven't appeared yet must
        // NOT be emitted (the backend can't resolve the keys -> "unknown port key" spam).
        let store = store_with_active_patio();
        let desired = desired_from_active(&store);
        // Only OUT present; IN's node hasn't been announced yet.
        let actual = GraphState {
            nodes: vec![node("src", None, true, vec![port("OUT", Direction::Out)])],
            ..Default::default()
        };
        let actions = reconcile(&desired, &actual);
        assert!(
            !actions.iter().any(|a| matches!(a, Action::CreateLink { .. })),
            "should not create a link while an endpoint is absent, got {actions:?}"
        );
    }

    #[test]
    fn reconcile_is_noop_when_already_satisfied() {
        let store = store_with_active_patio();
        let desired = desired_from_active(&store);
        let actual = GraphState {
            links: vec![LinkView { output_port: "OUT".into(), input_port: "IN".into() }],
            nodes: vec![node("AMP", Some(0.5), true, vec![])],
            ..Default::default()
        };
        let actions = reconcile(&desired, &actual);
        assert!(actions.is_empty(), "nothing to do, got {actions:?}");
    }

    #[test]
    fn reconcile_corrects_volume() {
        let store = store_with_active_patio();
        let desired = desired_from_active(&store);
        let actual = GraphState {
            links: vec![LinkView { output_port: "OUT".into(), input_port: "IN".into() }],
            nodes: vec![node("AMP", Some(1.0), true, vec![])], // wrong volume
            ..Default::default()
        };
        let actions = reconcile(&desired, &actual);
        assert_eq!(
            actions,
            vec![Action::SetVolume { node_key: "AMP".into(), target: VolumeTarget::Uniform(0.5) }]
        );
    }

    #[test]
    fn reconcile_corrects_only_specified_channels() {
        use crate::zones::{ChannelVolume, LinkSpec, ZoneDef};
        let p = std::env::temp_dir().join("audiozones-model-perchannel.toml");
        let _ = std::fs::remove_file(&p);
        let mut store = ZoneStore::load(&p).unwrap();
        store.zones.push(ZoneDef {
            name: "patio".into(),
            links: vec![LinkSpec { output_port: "OUT".into(), input_port: "IN".into() }],
            volumes: vec![VolumeSpec {
                node_key: "CARD".into(),
                volume: None,
                channels: vec![ChannelVolume { channel: 6, volume: 0.6 }, ChannelVolume { channel: 7, volume: 0.6 }],
                muted: false,
            }],
        });
        store.set_active("patio", true).unwrap();
        let desired = desired_from_active(&store);

        // 8-ch card: channels 6,7 are wrong (1.0) -> a Channels correction; the rest untouched.
        let actual = GraphState {
            links: vec![LinkView { output_port: "OUT".into(), input_port: "IN".into() }],
            nodes: vec![node_channels("CARD", vec![1.0; 8], vec![])],
            ..Default::default()
        };
        let actions = reconcile(&desired, &actual);
        assert_eq!(
            actions,
            vec![Action::SetVolume { node_key: "CARD".into(), target: VolumeTarget::Channels(vec![(6, 0.6), (7, 0.6)]) }]
        );

        // Already at 0.6 on 6,7 -> no-op (other channels' values are irrelevant).
        let mut chans = vec![1.0; 8];
        chans[6] = 0.6;
        chans[7] = 0.6;
        let satisfied = GraphState {
            links: vec![LinkView { output_port: "OUT".into(), input_port: "IN".into() }],
            nodes: vec![node_channels("CARD", chans, vec![])],
            ..Default::default()
        };
        assert!(reconcile(&desired, &satisfied).is_empty());
    }

    #[test]
    fn missing_device_flags_degraded() {
        let store = store_with_active_patio();
        // OUT present, IN absent -> IN reported missing.
        let actual = GraphState {
            nodes: vec![node("card", None, true, vec![port("OUT", Direction::Out)])],
            ..Default::default()
        };
        let missing = missing_for_zone(&store, "patio", &actual);
        assert_eq!(missing, vec!["IN".to_string()]);
    }

    #[test]
    fn device_returns_then_reconcile_completes_zone() {
        // auto-reapply: once the IN device is present, reconcile creates the link.
        let store = store_with_active_patio();
        let desired = desired_from_active(&store);
        let actual = GraphState {
            nodes: vec![
                node("card", None, true, vec![port("OUT", Direction::Out)]),
                node("amp", Some(0.5), true, vec![port("IN", Direction::In)]),
            ],
            links: vec![],
            ..Default::default()
        };
        let actions = reconcile(&desired, &actual);
        assert!(actions.contains(&Action::CreateLink {
            output_port: "OUT".into(),
            input_port: "IN".into()
        }));
    }
}
