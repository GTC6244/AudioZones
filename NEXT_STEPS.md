# AudioZones — Next Steps

Status as of 2026-06-02. The system works end-to-end on real hardware (Ubuntu box
`192.168.1.25`, PipeWire 1.0.5 + WirePlumber, StarTech ICUSBAUDIO7D / CM106 8-channel
USB card). Full design + decisions: `~/.gstack/projects/audiozones/GTC6244-amarillo-design-*.md`
(mirrored in `.context/`).

## Done (verified on hardware)
- Rust server (`server/`): full PipeWire graph model, snapshot-only WebSocket, REST,
  bearer auth, reconcile-driven zones, TOML persistence. 13 unit tests pass.
- Real `PipewireBackend` (`--features pipewire-backend`): read live graph, create/destroy
  links, set volume/mute (`Props.channelVolumes`), read live volume back. All exercised
  against the 8-ch card.
- Flutter app (`app/`): wired to the box, reads the live graph incl. volumes. Zone lens
  (on/off + degraded) and Graph lens (node volume/mute + tap-two-ports linking).
- Dart models generated from the Rust wire schema (`gen-dart.sh`, zero-drift).

## Next — by priority

### P1 — make it usable day-to-day
1. **Zone-tile volume control (app).** Backend supports per-node volume now; the Zone
   lens tile only has on/off. Needs a `volume` (and maybe `muted`) field on `ZoneView`
   in `server/src/wire.rs` (decide "which node represents the zone" — likely the zone's
   primary sink), then a slider on the tile. Regenerate Dart models after the wire change.
2. **Runtime settings screen (app).** `AppConfig.dev` hardcodes `192.168.1.25` + `dev-token`.
   Add a settings screen to set server host/port/token at runtime (persist with
   shared_preferences). Today changing the server means editing `lib/config.dart`.
3. **Real token + run as a service (ops).** Replace `dev-token` in `audiozones.toml` with
   a long random token (chmod 600). Add a `systemd --user` unit so the server starts on
   login/boot instead of the current `setsid nohup` (see design doc Distribution Plan).

### P2 — correctness / fidelity
4. **External device-route volume watcher (backend).** `wpctl`/GNOME volume changes hit a
   separate layer (Q4) and aren't live-reflected. Watch the Device's route params (not just
   node `Props`) to pick these up. Until then the app only reflects its own volume changes.
5. **Perceptual volume display (app or backend).** Volumes are raw-linear `channelVolumes`
   (built-in reads 0.0156 vs GNOME's "25%"). Apply a cube-root mapping for display, or
   expose both raw + perceptual.
6. **Per-channel zone volume (backend + model).** `Action::SetVolume` carries one uniform
   level; `channelVolumes` is an 8-float array. Support per-channel for "card channels 7-8
   → patio at 0.6". Needs a richer Action + zone schema.
7. **Optimistic zone toggle (app).** Zone on/off currently waits for the snapshot round-trip.
   Flip optimistically + revert on failure for snappier feel.

### P3 — scale / polish
8. **Event coalescing (backend).** `PipewireBackend` broadcasts a fresh snapshot per
   registry event (a burst at startup = many sends). Batch per pw-loop iteration
   (eng-review decision) — accumulate changes, flush one snapshot when the queue drains.
9. **Two-identical-cards identity (backend).** Stable keys use `media.class|node.name`;
   two identical USB cards could collide. Disambiguate with `device.bus-path` (port
   position). Only one card present, so untested — revisit when a second exists.
10. **Graph lens polish (app).** Tablet/landscape layout, search/grouping; the full
    drag-to-connect editor (deferred from v1).

### Cross-cutting
- **Commit the code.** Greenfield repo is still uncommitted (server/, app/, spike/).
  First real commit + push.
- **CI.** GitHub Actions: run `gen-dart.sh` + `git diff --exit-code` (protocol drift guard),
  `cargo test`, `flutter analyze`; build server binary + APK on tag → GitHub Releases.
- **Home Assistant integration (optional).** Kept as a future distribution channel — expose
  zones as HA entities from the same server.

## Known environment notes
- The agent dev Mac blocks direct raw-socket egress to the LAN (`curl` works, sockets get
  EHOSTUNREACH); app↔box was verified via `ssh -L 4040:127.0.0.1:4040`. Real phones/Macs on
  the LAN connect directly. `app/bin/probe.dart` is a headless connectivity check.
- Box code lives at `~/audiozones/` (no git on box; synced via tar-over-ssh from the Mac).
  Run: `cd ~/audiozones/server && PATH=$HOME/.cargo/bin:$PATH XDG_RUNTIME_DIR=/run/user/1000 cargo run --features pipewire-backend`.
