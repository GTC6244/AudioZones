# AudioZones — Next Steps

Status as of 2026-06-02. The system works end-to-end on real hardware (Ubuntu box
`192.168.1.25`, PipeWire 1.0.5 + WirePlumber, StarTech ICUSBAUDIO7D / CM106 8-channel
USB card). Full design + decisions: `~/.gstack/projects/audiozones/GTC6244-amarillo-design-*.md`
(mirrored in `.context/`).

## Done (verified on hardware)
- Rust server (`server/`): full PipeWire graph model, snapshot-only WebSocket, REST,
  bearer auth, reconcile-driven zones, TOML persistence. 18 unit tests pass.
- Real `PipewireBackend` (`--features pipewire-backend`): read live graph, create/destroy
  links, set volume/mute (`Props.channelVolumes`), read live volume back. All exercised
  against the 8-ch card.
- Flutter app (`app/`): wired to the box, reads the live graph incl. volumes. Zone lens
  (on/off + degraded) and Graph lens (node volume/mute + tap-two-ports linking).
- Dart models generated from the Rust wire schema (`gen-dart.sh`, zero-drift).
- **Zone-tile volume control (#1).** `ZoneView` now carries `volume`/`muted`/`volume_node`
  (the zone's representative node — first volume-spec node, else the sink behind its first
  link). The Zone tile shows a slider + mute that PUT to `volume_node`. Verified on the
  mock: activating `patio` reconciles the sink 0.8 → 0.5.
- **Perceptual volume display (#5).** `app/lib/volume.dart` maps raw-linear ⟷ perceptual
  with a cube-root curve (raw 0.0156 ⇒ "25%", matching wpctl/GNOME). Both the Zone and
  Graph sliders ride in perceptual space and send raw.
- **Per-channel zone volume (#6).** `VolumeSpec` gains `channels = [{channel, volume}]`
  (precedence over uniform `volume`); `Action::SetVolume` carries `VolumeTarget::{Uniform,
  Channels}`; the pipewire backend writes the full `channelVolumes` array, preserving
  unlisted channels. NodeView now exposes `channel_volumes`. **Box-verified** on the 8-ch
  card: a zone with `channels=[{6,0.6},{7,0.6}]` moved only channels 6-7 to 0.6 and left
  0-5 at 0.5; a uniform PUT drove all 8 channels.
- **Two-identical-card identity (#9).** `node_key` folds in a port-position-stable hardware
  path so two same-model cards no longer collide. The path is `device.bus-path` (USB/PCI
  port), read by **binding the Device proxy and reading its INFO props** — the registry
  global only carries `device.name`/class, NOT the bus path (a real-hardware finding; an
  earlier registry-props read silently produced bare keys). Stream nodes (no device) and the
  mock keep bare keys. **Box-verified**: the 8-ch card keys as
  `…analog-surround-71.2@pci-0000:00:14.0-usb-0:2:1.0`. See the migration caveat below.

## Next — by priority

### P1 — make it usable day-to-day
1. **Runtime settings screen (app).** `AppConfig.dev` hardcodes `192.168.1.25` + `dev-token`.
   Add a settings screen to set server host/port/token at runtime (persist with
   shared_preferences). Today changing the server means editing `lib/config.dart`.
2. **Real token + run as a service (ops).** Replace `dev-token` in `audiozones.toml` with
   a long random token (chmod 600). Add a `systemd --user` unit so the server starts on
   login/boot instead of the current `setsid nohup` (see design doc Distribution Plan).

### P2 — correctness / fidelity
3. **External device-route volume watcher (backend).** `wpctl`/GNOME volume changes hit a
   separate layer (Q4) and aren't live-reflected. Watch the Device's route params (not just
   node `Props`) to pick these up. Until then the app only reflects its own volume changes.
4. **Optimistic zone toggle (app).** Zone on/off currently waits for the snapshot round-trip.
   Flip optimistically + revert on failure for snappier feel.

### P3 — scale / polish
5. **Event coalescing (backend).** `PipewireBackend` broadcasts a fresh snapshot per
   registry event (a burst at startup = many sends). Batch per pw-loop iteration
   (eng-review decision) — accumulate changes, flush one snapshot when the queue drains.
6. **Per-channel zone volume UI (app).** Backend + schema support per-channel now (#6);
   there's still no app surface for it — per-channel is TOML-only. A multi-channel card
   editor in the Graph lens would expose it.
7. **Graph lens polish (app).** Tablet/landscape layout, search/grouping; the full
   drag-to-connect editor (deferred from v1).

### Cross-cutting
- **Identity migration (#9 caveat).** Bus-path disambiguation changes real-hardware node
  keys — they now look like
  `Audio/Sink|alsa_output.usb-…analog-surround-71.2@pci-0000:00:14.0-usb-0:2:1.0`. The
  box's committed `zones.toml` still uses the old mock placeholder keys (`Audio/Sink|Patio
  Amp`), so its `patio` zone logs `create_link: unknown port key(s)` — it needs re-pointing
  to the real `@<bus-path>` keys from the live `/graph` before it links anything. Still
  untested against an actual two-card rig (only one card exists); the collision and the
  bare-key fallback are covered by unit tests.
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
