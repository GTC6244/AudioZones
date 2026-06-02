//! Zone definitions + persistence.
//!
//! A zone is a saved-state LENS over the graph: a named set of links + per-node
//! volumes. Persisted as hand-editable TOML with an atomic temp+rename write and a
//! `version` field for forward migration. The store ALSO persists which zones are
//! active, so they reassert after a server restart (eng-review decision).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZoneError {
    #[error("io error on zones file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse zones TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialize zones TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("no such zone: {0}")]
    NoSuchZone(String),
    #[error("zone already exists: {0}")]
    ZoneExists(String),
    #[error("invalid zone: {0}")]
    InvalidZone(String),
}

/// One link inside a zone, addressed by stable port keys.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkSpec {
    pub output_port: String,
    pub input_port: String,
}

/// One channel's desired level, addressed by 0-based channel index in the node's
/// channel order. Lets a zone drive a subset of a multi-channel card (e.g. an 8-ch
/// USB card's channels 6-7 -> the patio amp) without touching the others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelVolume {
    pub channel: usize,
    pub volume: f32,
}

/// Desired volume for a node inside a zone. Two modes:
///  - uniform: `volume = 0.6` applies one level to every channel (the common case);
///  - per-channel: `channels = [{channel=6, volume=0.6}, ...]` drives specific channels
///    and leaves the rest untouched. `channels` takes precedence when non-empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub node_key: String,
    /// Uniform level for all channels. Optional so a spec can be per-channel only.
    #[serde(default)]
    pub volume: Option<f32>,
    /// Per-channel overrides; when non-empty these win over `volume`.
    #[serde(default)]
    pub channels: Vec<ChannelVolume>,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneDef {
    pub name: String,
    #[serde(default)]
    pub links: Vec<LinkSpec>,
    #[serde(default)]
    pub volumes: Vec<VolumeSpec>,
}

impl ZoneDef {
    /// The node whose volume the zone tile controls: the first volume-spec's node, else
    /// the sink behind the zone's first link (the input-port side). `None` if neither
    /// exists. The sink node key is the prefix of the input port key before `#`.
    pub fn primary_node(&self) -> Option<String> {
        if let Some(v) = self.volumes.first() {
            return Some(v.node_key.clone());
        }
        let port = &self.links.first()?.input_port;
        let nk = port.split('#').next().unwrap_or(port);
        Some(nk.to_string())
    }
}

const CURRENT_VERSION: u32 = 1;

fn default_version() -> u32 {
    CURRENT_VERSION
}

/// The on-disk document: zone definitions + the set of currently-active zone names.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneStore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub zones: Vec<ZoneDef>,
    /// Names of zones that are "on". Reasserted on boot.
    #[serde(default)]
    pub active: Vec<String>,
    #[serde(skip)]
    path: PathBuf,
}

impl ZoneStore {
    /// Load from disk. A missing file yields an empty store (first run). A corrupt
    /// file is an error the caller can surface without crashing the server.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ZoneError> {
        let path = path.as_ref().to_path_buf();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut store: ZoneStore = toml::from_str(&text)?;
                store.path = path;
                Ok(store)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ZoneStore {
                version: CURRENT_VERSION,
                zones: Vec::new(),
                active: Vec::new(),
                path,
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomic write: serialize to `<file>.tmp`, fsync, rename over the real file,
    /// then fsync the parent dir so the rename itself is durable. A crash mid-write
    /// never corrupts the existing zones; a power loss after rename keeps the new file.
    pub fn save(&self) -> Result<(), ZoneError> {
        let text = toml::to_string_pretty(self)?;
        let tmp = self.path.with_extension("toml.tmp");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        // Fsync the directory entry: the file contents were synced above, but the
        // rename (the directory update) must also be flushed to survive power loss.
        if let Some(dir) = self.path.parent() {
            // An empty parent means the CWD; fsync "." in that case.
            let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ZoneDef> {
        self.zones.iter().find(|z| z.name == name)
    }

    /// Add a new zone definition. The name is the zone's identity (`get`/`set_active`
    /// key off it), so reject an empty name and a duplicate. The stored name is trimmed.
    /// Caller is responsible for `save()`.
    pub fn add_zone(&mut self, mut zone: ZoneDef) -> Result<(), ZoneError> {
        let name = zone.name.trim().to_string();
        if name.is_empty() {
            return Err(ZoneError::InvalidZone("zone name must not be empty".into()));
        }
        if self.get(&name).is_some() {
            return Err(ZoneError::ZoneExists(name));
        }
        zone.name = name;
        self.zones.push(zone);
        Ok(())
    }

    /// Remove a zone by name, also dropping it from the active set. Errors if unknown.
    /// Note: this does not tear down any links the zone created — reconcile only *creates*
    /// links (conservative v1, see `model::reconcile`), so a deleted zone's live links
    /// linger until removed in the Graph lens, exactly as deactivating one does. Caller saves.
    pub fn remove_zone(&mut self, name: &str) -> Result<(), ZoneError> {
        if self.get(name).is_none() {
            return Err(ZoneError::NoSuchZone(name.to_string()));
        }
        self.zones.retain(|z| z.name != name);
        self.active.retain(|n| n != name);
        Ok(())
    }

    pub fn is_active(&self, name: &str) -> bool {
        self.active.iter().any(|n| n == name)
    }

    pub fn set_active(&mut self, name: &str, active: bool) -> Result<(), ZoneError> {
        if self.get(name).is_none() {
            return Err(ZoneError::NoSuchZone(name.to_string()));
        }
        let present = self.is_active(name);
        if active && !present {
            self.active.push(name.to_string());
        } else if !active && present {
            self.active.retain(|n| n != name);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("audiozones-test-{name}.toml"))
    }

    #[test]
    fn missing_file_yields_empty_store() {
        let p = tmpfile("missing");
        let _ = std::fs::remove_file(&p);
        let store = ZoneStore::load(&p).unwrap();
        assert!(store.zones.is_empty());
        assert_eq!(store.version, CURRENT_VERSION);
    }

    #[test]
    fn round_trip_preserves_zones_and_active_set() {
        let p = tmpfile("roundtrip");
        let mut store = ZoneStore::load(&p).unwrap();
        store.zones.push(ZoneDef {
            name: "patio".into(),
            links: vec![LinkSpec {
                output_port: "Audio/Sink|Card#out:monitor_FL".into(),
                input_port: "Audio/Sink|Amp#in:playback_FL".into(),
            }],
            volumes: vec![VolumeSpec {
                node_key: "Audio/Sink|Amp".into(),
                volume: Some(0.6),
                channels: vec![],
                muted: false,
            }],
        });
        store.set_active("patio", true).unwrap();
        store.save().unwrap();

        let reloaded = ZoneStore::load(&p).unwrap();
        assert_eq!(reloaded.zones.len(), 1);
        assert!(reloaded.is_active("patio"));
        assert_eq!(reloaded.zones[0].links.len(), 1);
        assert_eq!(reloaded.zones[0].volumes[0].volume, Some(0.6));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn primary_node_prefers_volume_spec_then_link_sink() {
        // A volume spec wins outright.
        let z = ZoneDef {
            name: "z".into(),
            links: vec![LinkSpec { output_port: "Audio/Source|Cap#out:cap_FL".into(), input_port: "Audio/Sink|Amp#in:playback_FL".into() }],
            volumes: vec![VolumeSpec { node_key: "Audio/Sink|Amp".into(), volume: Some(0.5), channels: vec![], muted: false }],
        };
        assert_eq!(z.primary_node().as_deref(), Some("Audio/Sink|Amp"));

        // No volume spec -> derive the sink node from the first link's input port.
        let z2 = ZoneDef {
            name: "z2".into(),
            links: vec![LinkSpec { output_port: "Audio/Source|Cap#out:cap_FL".into(), input_port: "Audio/Sink|Patio Amp#in:playback_FL".into() }],
            volumes: vec![],
        };
        assert_eq!(z2.primary_node().as_deref(), Some("Audio/Sink|Patio Amp"));

        // Nothing to control.
        let z3 = ZoneDef { name: "z3".into(), links: vec![], volumes: vec![] };
        assert_eq!(z3.primary_node(), None);
    }

    #[test]
    fn per_channel_volume_round_trips() {
        let p = tmpfile("perchannel");
        let mut store = ZoneStore::load(&p).unwrap();
        store.zones.push(ZoneDef {
            name: "patio".into(),
            links: vec![],
            volumes: vec![VolumeSpec {
                node_key: "Audio/Sink|8ch Card".into(),
                volume: None,
                channels: vec![
                    ChannelVolume { channel: 6, volume: 0.6 },
                    ChannelVolume { channel: 7, volume: 0.6 },
                ],
                muted: false,
            }],
        });
        store.save().unwrap();
        let reloaded = ZoneStore::load(&p).unwrap();
        let v = &reloaded.zones[0].volumes[0];
        assert_eq!(v.volume, None);
        assert_eq!(v.channels, vec![
            ChannelVolume { channel: 6, volume: 0.6 },
            ChannelVolume { channel: 7, volume: 0.6 },
        ]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn add_zone_appends_and_trims_name() {
        let p = tmpfile("addzone");
        let _ = std::fs::remove_file(&p);
        let mut store = ZoneStore::load(&p).unwrap();
        store
            .add_zone(ZoneDef {
                name: "  patio  ".into(),
                links: vec![LinkSpec {
                    output_port: "Audio/Source|Cap#out:capture_FL".into(),
                    input_port: "Audio/Sink|Amp#in:playback_FL".into(),
                }],
                volumes: vec![],
            })
            .unwrap();
        assert_eq!(store.zones.len(), 1);
        // Trimmed name is what's stored and what `get` keys off.
        assert!(store.get("patio").is_some());
    }

    #[test]
    fn add_zone_rejects_duplicate_name() {
        let p = tmpfile("dupzone");
        let _ = std::fs::remove_file(&p);
        let mut store = ZoneStore::load(&p).unwrap();
        let z = || ZoneDef { name: "patio".into(), links: vec![], volumes: vec![] };
        store.add_zone(z()).unwrap();
        assert!(matches!(store.add_zone(z()), Err(ZoneError::ZoneExists(_))));
        assert_eq!(store.zones.len(), 1);
    }

    #[test]
    fn add_zone_rejects_empty_name() {
        let p = tmpfile("emptyname");
        let _ = std::fs::remove_file(&p);
        let mut store = ZoneStore::load(&p).unwrap();
        let r = store.add_zone(ZoneDef { name: "   ".into(), links: vec![], volumes: vec![] });
        assert!(matches!(r, Err(ZoneError::InvalidZone(_))));
        assert!(store.zones.is_empty());
    }

    #[test]
    fn remove_zone_drops_definition_and_active_entry() {
        let p = tmpfile("removezone");
        let _ = std::fs::remove_file(&p);
        let mut store = ZoneStore::load(&p).unwrap();
        store.add_zone(ZoneDef { name: "patio".into(), links: vec![], volumes: vec![] }).unwrap();
        store.set_active("patio", true).unwrap();
        assert!(store.is_active("patio"));

        store.remove_zone("patio").unwrap();
        assert!(store.get("patio").is_none());
        assert!(!store.is_active("patio"));
        assert!(matches!(store.remove_zone("patio"), Err(ZoneError::NoSuchZone(_))));
    }

    #[test]
    fn activating_unknown_zone_errors() {
        let p = tmpfile("unknown");
        let _ = std::fs::remove_file(&p);
        let mut store = ZoneStore::load(&p).unwrap();
        assert!(store.set_active("ghost", true).is_err());
    }

    #[test]
    fn corrupt_file_is_an_error_not_a_panic() {
        let p = tmpfile("corrupt");
        std::fs::write(&p, "this is not valid toml = = =").unwrap();
        assert!(ZoneStore::load(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
