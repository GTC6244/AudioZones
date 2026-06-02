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
}

/// One link inside a zone, addressed by stable port keys.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkSpec {
    pub output_port: String,
    pub input_port: String,
}

/// Desired volume for a node inside a zone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub node_key: String,
    pub volume: f32,
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

    /// Atomic write: serialize to `<file>.tmp`, fsync, rename over the real file.
    /// A crash mid-write never corrupts the existing zones.
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
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ZoneDef> {
        self.zones.iter().find(|z| z.name == name)
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
                volume: 0.6,
                muted: false,
            }],
        });
        store.set_active("patio", true).unwrap();
        store.save().unwrap();

        let reloaded = ZoneStore::load(&p).unwrap();
        assert_eq!(reloaded.zones.len(), 1);
        assert!(reloaded.is_active("patio"));
        assert_eq!(reloaded.zones[0].links.len(), 1);
        let _ = std::fs::remove_file(&p);
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
