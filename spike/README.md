# AudioZones spike — run this on the Linux box

Throwaway de-risking spike. **Not part of the real build.** Its only job is to answer
the four questions from the design doc before you invest in the server. Delete it after.

> Must run on Linux with PipeWire + WirePlumber active and your USB audio hardware
> attached. It cannot run on macOS (no PipeWire). Build deps: a Rust toolchain and
> the PipeWire dev headers (`libpipewire-0.3-dev` on Debian/Ubuntu, `pipewire-devel`
> on Fedora, `clang`/`pkg-config` present).

## Build & run

```bash
cd spike
cargo run
```

You'll see `+`/`- removed` lines as PipeWire objects appear/disappear.

---

## Q1 — Read path  ✅ when:

With the spike running, open **qpwgraph** (or run `pw-link <a> <b>` in another terminal)
and connect/disconnect two ports. You should see matching `+`/`- removed` lines stream in
the spike output **within a second**. That proves the registry + event subscription works.

## Q2 — Command path (cross-thread mutation)  ✅ when:

1. From the `+` lines (or `pw-dump`), pick an output node+port and an input node+port you
   want to link. Grab their numeric `id`s.
2. Re-run with them set:
   ```bash
   SPIKE_OUT_NODE=51 SPIKE_OUT_PORT=53 SPIKE_IN_NODE=40 SPIKE_IN_PORT=42 cargo run
   ```
3. After ~6 seconds the worker thread sends the command onto the loop thread and you see
   `Q2: link object created`. **✅** = the tokio↔pw-loop bridge model works (here it's a
   plain `std::thread` + `pw::channel`; in the server it's tokio + `pw::channel` — same shape).

## Q3 — WirePlumber coexistence (THE feasibility gate)  ⚠️

Right after `Q2: link object created`, **watch for a few seconds**:

- **✅ STICKS:** no `- removed` line for that link, and you can hear/see audio routed.
  WirePlumber leaves your link alone → the raw-PipeWire server design is viable.
- **❌ REVERTED:** a `- removed id=…` appears within a second or two, or the link silently
  stops passing audio. WirePlumber is reasserting policy → **the server must drive
  WirePlumber's API (Lua/metadata) instead of raw PipeWire.** This is a bottom-layer
  change to the design; better to know now.

Also check volume persistence: set a volume (see Q4), wait, and watch whether WP restores
the previous value.

## Q4 — Where does volume actually live?  (investigate with wpctl/pw-dump)

The binary doesn't touch volume — locating it is a triage you do by hand. Volume can live
on the sink node, the stream node, a mixer/`Props` param, or WirePlumber metadata. Find out:

```bash
# What wpctl thinks the default sink + its volume are:
wpctl status
wpctl get-volume @DEFAULT_AUDIO_SINK@

# Set it, then see which object's params changed:
wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.5
pw-dump | less     # search for "channelVolumes" / "volume" — note which object id holds it

# For your 8-channel USB card specifically, find its node id in `wpctl status`,
# then inspect its params:
pw-dump <node-id>  # look for Props -> channelVolumes (per-channel) vs a single volume
```

**Record the answer:** which object type holds the authoritative volume, and whether it's
per-channel (`channelVolumes`) or a single scalar. That decides the server's "set zone
volume" code path. If WP owns it (metadata), the server sets volume through WP, same
conclusion as Q3.

---

## Report back

Fill this in and the design doc's open risks close:

- Q1 read path: ☐ works
- Q2 cross-thread mutation: ☐ works
- Q3 WirePlumber: ☐ links stick  /  ☐ WP reverts (→ use WP API)
- Q4 volume lives on: ____________  (per-channel? ☐ / scalar? ☐)
