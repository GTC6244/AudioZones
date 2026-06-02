//! Server config — one hand-editable TOML file, same spirit as the zone store.
//! Holds the bind address, the bearer token (gates REST + WS), and the zones path.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// e.g. "0.0.0.0:4040" (LAN) — the token still gates every request.
    pub bind: String,
    /// Shared secret. Clients send it as `Authorization: Bearer <token>` (or `?token=`).
    pub token: String,
    /// Path to the zones TOML file.
    pub zones_file: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:4040".into(),
            token: "dev-token".into(),
            zones_file: "./zones.toml".into(),
        }
    }
}

impl Config {
    /// Load from a TOML file. Missing file -> dev defaults (with a warning logged by
    /// the caller). The config file should be mode 0600 since it holds the token.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg = toml::from_str(&text)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
}
