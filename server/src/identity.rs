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
//! KNOWN LIMITATION: two *identical* cards can share a `node.name`. Durably disambiguating
//! them needs a port-position-stable property (e.g. `api.alsa.path` / `device.bus-path`),
//! not `object.serial`. Deferred until a two-card setup exists to test against.

use crate::wire::Direction;

/// Stable key for a device/node: `media.class|node.name`.
pub fn node_key(node_name: &str, media_class: &str) -> String {
    format!("{media_class}|{node_name}")
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
        let a = node_key("alsa_output.usb-0d8c_..surround-71.2", "Audio/Sink");
        assert_eq!(a, node_key("alsa_output.usb-0d8c_..surround-71.2", "Audio/Sink"));
        assert_eq!(a, "Audio/Sink|alsa_output.usb-0d8c_..surround-71.2");
    }

    #[test]
    fn distinct_devices_get_distinct_keys() {
        assert_ne!(
            node_key("alsa_output.pci-..analog-stereo", "Audio/Sink"),
            node_key("alsa_output.usb-..surround-71.2", "Audio/Sink"),
        );
    }

    #[test]
    fn same_name_different_class_distinct() {
        assert_ne!(
            node_key("dev", "Audio/Sink"),
            node_key("dev", "Audio/Source"),
        );
    }

    #[test]
    fn port_keys_distinguish_direction() {
        let nk = node_key("Card", "Audio/Sink");
        assert_ne!(
            port_key(&nk, "playback_FL", Direction::In),
            port_key(&nk, "playback_FL", Direction::Out)
        );
    }
}
