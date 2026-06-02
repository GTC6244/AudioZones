//! AudioZones server library. Exposed so both the `audiozones-server` binary and the
//! `gen_schema` tool (which emits the JSON Schema the Dart client is generated from)
//! share the same module tree.

pub mod api;
pub mod backend;
#[cfg(feature = "pipewire-backend")]
pub mod backend_pipewire;
pub mod config;
pub mod identity;
pub mod model;
pub mod wire;
pub mod zones;
