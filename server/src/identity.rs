//! Stable identity — the riskiest assumption in the design (per the codex review),
//! so it lives in one place with its own tests.
//!
//! PipeWire numeric node/port ids are NOT stable across reconnect; zones must bind to
//! logical identity instead. We key a device by `(media.class, node.name)`.
//!
//! REAL-HARDWARE FINDING (box 192.168.1.25, 2026-06-02): `object.serial` is a per-session
//! monotonic counter, NOT a durable hardware id — it changes on every reboot. An earlier
//! version folded it into the key, which would have broken zones across restarts. We do
//! NOT use it. `node.name` (e.g. `alsa_output.usb-0d8c_..analog-surround-71.2`) is stable
//! across reconnect for a given device+profile and is the right key.
//!
//! TWO IDENTICAL CARDS: two cards of the same model share a `node.name`, so the bare
//! `media.class|node.name` key collides. We disambiguate with a port-position-stable
//! hardware path (`device.bus-path`, falling back to `api.alsa.path` / `object.path`),
//! which the real backend reads from the owning Device global. NOT `object.serial`
//! (per-session, changes on reboot). When a path is available the key becomes
//! `media.class|node.name@path`; when it isn't (mock backend, virtual nodes) the key
//! stays `media.class|node.name`, so single-card and mock setups are unaffected.
//!
//! MIGRATION NOTE: turning this on changes real-hardware keys (they gain `@path`), so an
//! existing `zones.toml` bound to bare keys must be re-pointed once. Untested against an
//! actual two-card rig — only one card exists today — but the collision is covered by unit
//! tests below.

use crate::wire::Direction;

/// Stable key for a device/node: `media.class|node.name`, plus an optional
/// port-position-stable hardware path (`@path`) to disambiguate identical cards.
/// `disambiguator` is the owning device's bus path; `None`/empty yields the bare key.
pub fn node_key(node_name: &str, media_class: &str, disambiguator: Option<&str>) -> String {
    match disambiguator {
        Some(d) if !d.is_empty() => format!("{media_class}|{node_name}@{d}"),
        _ => format!("{media_class}|{node_name}"),
    }
}

/// Stable key for a port, derived from its owning device's key.
pub fn port_key(node_key: &str, port_name: &str, dir: Direction) -> String {
    let d = match dir {
        Direction::In => "in",
        Direction::Out => "out",
    };
    format!("{node_key}#{d}:{port_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_key_is_stable_and_serial_free() {
        // Same logical device -> same deterministic key, no volatile session id baked in.
        let a = node_key("alsa_output.usb-0d8c_..surround-71.2", "Audio/Sink", None);
        assert_eq!(a, node_key("alsa_output.usb-0d8c_..surround-71.2", "Audio/Sink", None));
        assert_eq!(a, "Audio/Sink|alsa_output.usb-0d8c_..surround-71.2");
    }

    #[test]
    fn distinct_devices_get_distinct_keys() {
        assert_ne!(
            node_key("alsa_output.pci-..analog-stereo", "Audio/Sink", None),
            node_key("alsa_output.usb-..surround-71.2", "Audio/Sink", None),
        );
    }

    #[test]
    fn same_name_different_class_distinct() {
        assert_ne!(
            node_key("dev", "Audio/Sink", None),
            node_key("dev", "Audio/Source", None),
        );
    }

    #[test]
    fn identical_cards_disambiguated_by_bus_path() {
        // Two same-model cards share node.name + media.class; the bare key collides...
        assert_eq!(
            node_key("alsa_output.usb-Generic_USB_Audio", "Audio/Sink", None),
            node_key("alsa_output.usb-Generic_USB_Audio", "Audio/Sink", None),
        );
        // ...but their port-position-stable bus paths pull them apart.
        let a = node_key("alsa_output.usb-Generic_USB_Audio", "Audio/Sink", Some("usb-0000:00:14.0-1"));
        let b = node_key("alsa_output.usb-Generic_USB_Audio", "Audio/Sink", Some("usb-0000:00:14.0-2"));
        assert_ne!(a, b);
        // Same card, same slot -> same key across reconnect (bus path is position-stable).
        assert_eq!(a, node_key("alsa_output.usb-Generic_USB_Audio", "Audio/Sink", Some("usb-0000:00:14.0-1")));
    }

    #[test]
    fn empty_disambiguator_yields_bare_key() {
        assert_eq!(
            node_key("Card", "Audio/Sink", Some("")),
            node_key("Card", "Audio/Sink", None),
        );
    }

    #[test]
    fn port_keys_distinguish_direction() {
        let nk = node_key("Card", "Audio/Sink", None);
        assert_ne!(
            port_key(&nk, "playback_FL", Direction::In),
            port_key(&nk, "playback_FL", Direction::Out)
        );
    }
}
