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

use crate::wire::GraphState;
use crate::zones::{ZoneStore, VolumeSpec};

/// A single mutation the backend should apply.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    CreateLink { output_port: String, input_port: String },
    DestroyLink { output_port: String, input_port: String },
    SetVolume { node_key: String, volume: f32 },
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

    // Create desired links that don't exist yet.
    for (out, inp) in &desired.links {
        if !actual_links.contains(&(out.clone(), inp.clone())) {
            actions.push(Action::CreateLink {
                output_port: out.clone(),
                input_port: inp.clone(),
            });
        }
    }

    // Correct volumes/mute where the node exists and differs.
    for v in &desired.volumes {
        if let Some(node) = actual.nodes.iter().find(|n| n.key == v.node_key) {
            if node.volume.map_or(true, |cur| (cur - v.volume).abs() > 0.001) {
                actions.push(Action::SetVolume {
                    node_key: v.node_key.clone(),
                    volume: v.volume,
                });
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
            muted: false,
            present,
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
            volumes: vec![VolumeSpec { node_key: "AMP".into(), volume: 0.5, muted: false }],
        });
        s.set_active("patio", true).unwrap();
        s
    }

    #[test]
    fn reconcile_creates_missing_link() {
        let store = store_with_active_patio();
        let desired = desired_from_active(&store);
        let actual = GraphState::default(); // no links yet
        let actions = reconcile(&desired, &actual);
        assert!(actions.contains(&Action::CreateLink {
            output_port: "OUT".into(),
            input_port: "IN".into()
        }));
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
        assert_eq!(actions, vec![Action::SetVolume { node_key: "AMP".into(), volume: 0.5 }]);
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
