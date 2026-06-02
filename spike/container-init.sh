#!/usr/bin/env bash
# Boots a headless PipeWire + WirePlumber stack with two virtual null-sinks,
# prints the port map, then execs the given command (default: the spike).
set -euo pipefail

export XDG_RUNTIME_DIR=/run/user/0
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# WirePlumber wants a session bus. Re-exec ourselves inside one if needed.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  exec dbus-run-session -- "$0" "$@"
fi

echo "== starting pipewire + wireplumber (no real hardware) =="
# pipewire loads the two static null-audio-sink adapters (ZoneA/ZoneB) from
# /etc/pipewire/pipewire.conf.d/10-virtual-sinks.conf — always-process, so their
# ports exist at startup and persist. wireplumber still applies its policy on top,
# which is what makes the Q3 coexistence test meaningful.
pipewire &
sleep 2
wireplumber &
sleep 3

echo
echo "== wpctl status =="
wpctl status || true

echo
echo "== PORTS:  portId  nodeId  portName  dir  path =="
echo "   (pick an out-port on ZoneA's monitor and an in-port on ZoneB's playback)"
pw-dump 2>/dev/null | jq -r '
  .[] | select(.type=="PipeWire:Interface:Port")
  | [ .id,
      (.info.props["node.id"]),
      (.info.props["port.name"]),
      (.info.props["port.direction"]),
      (.info.props["object.path"]) ] | @tsv' 2>/dev/null || \
  echo "   (jq/pw-dump parse failed — fall back to: pw-link -I -o ; pw-link -I -i)"

echo
echo "== running: $* =="
echo "   For Q2/Q3 re-run with the four ids, e.g.:"
echo "   docker exec -it <container> env SPIKE_OUT_NODE=.. SPIKE_OUT_PORT=.. \\"
echo "        SPIKE_IN_NODE=.. SPIKE_IN_PORT=.. cargo run"
echo
exec "$@"
