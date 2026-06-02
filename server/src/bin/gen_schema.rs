//! Emits the JSON Schema for the wire protocol's root type (`GraphState`) to stdout.
//! This schema is the SINGLE SOURCE OF TRUTH for the cross-language protocol — the
//! Dart client models are generated from it (zero drift). Regenerate with:
//!
//!   cargo run --bin gen_schema > ../protocol.schema.json
//!
//! CI guard: regenerate, then `git diff --exit-code protocol.schema.json` — if a Rust
//! wire type changed without regenerating, CI goes red.

use audiozones_server::wire::GraphState;

fn main() {
    let schema = schemars::schema_for!(GraphState);
    println!("{}", serde_json::to_string_pretty(&schema).expect("serialize schema"));
}
