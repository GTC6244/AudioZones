# AudioZones server

Control-plane server: REST commands + a WebSocket that pushes the full graph snapshot.
Builds and runs anywhere via a mock backend; the real PipeWire backend (Linux) plugs in
behind the same `PwBackend` trait once the spike settles raw-PipeWire vs WirePlumber.

## Run

```bash
cargo run                 # dev defaults: 127.0.0.1:4040, token "dev-token", ./zones.toml
cargo test                # unit tests (identity, zones, reconcile)
```

For real use, copy `audiozones.example.toml` → `audiozones.toml`, set a real token, `chmod 600`.

## Layout

```
wire.rs      snapshot types (GraphState/NodeView/...) — also the codegen source for Dart
identity.rs  stable device/port keys (the riskiest assumption; isolated + tested)
zones.rs     ZoneDef + ZoneStore (TOML, atomic save, persists the active set)
model.rs     reconcile(desired, actual) -> [Action]; the controller core
backend.rs   PwBackend trait + MockBackend (no PipeWire dep)
config.rs    server config (bind/token/zones_file)
api.rs       axum REST + WebSocket + bearer-token middleware
main.rs      wiring: load -> reassert active zones -> relay -> serve
```

## API

| Method | Path | Body | Effect |
|--------|------|------|--------|
| GET  | `/graph` | — | full `GraphState` snapshot |
| GET  | `/zones` | — | zone views (active/degraded/missing) |
| POST | `/zones/:name/activate` | — | mark active, reconcile, persist |
| POST | `/zones/:name/deactivate` | — | mark inactive, persist |
| PUT  | `/nodes/:key/volume` | `{"volume":0.5,"muted":false}` | set node volume/mute |
| POST | `/links` | `{"output_port":"…","input_port":"…"}` | create a link |
| DELETE | `/links` | `{"output_port":"…","input_port":"…"}` | destroy a link |
| GET  | `/ws` | — | WebSocket: snapshot on connect, then on every change |

All routes (incl. `/ws`) require `Authorization: Bearer <token>` or `?token=<token>`.

```bash
curl -s -H "Authorization: Bearer dev-token" localhost:4040/graph
curl -s -X POST -H "Authorization: Bearer dev-token" localhost:4040/zones/patio/activate
```

## Status

Mock backend only. Next: generate Dart models from the `wire` schema, then implement
`PipewireBackend` (Linux) — gated behind `--features pipewire-backend`.
