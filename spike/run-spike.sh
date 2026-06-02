#!/usr/bin/env bash
# Build the spike image and run it. Requires the Docker daemon to be running
# (Docker Desktop, or `colima start`).
#
#   ./run-spike.sh             # Q1: boot stack, print ports, stream events
#   ./run-spike.sh bash        # drop into a shell with the stack running
#
# For Q2/Q3, watch the printed port map, then in another terminal:
#   docker exec -it audiozones-spike-run env \
#     SPIKE_OUT_NODE=.. SPIKE_OUT_PORT=.. SPIKE_IN_NODE=.. SPIKE_IN_PORT=.. cargo run
set -euo pipefail
cd "$(dirname "$0")"

IMAGE=audiozones-spike
NAME=audiozones-spike-run

echo "== building $IMAGE (first build compiles pipewire-sys, ~1-3 min) =="
docker build -t "$IMAGE" .

echo "== running container '$NAME' =="
# Mount the live source so edits don't require a rebuild; keep target/ inside
# the container (anon volume) so host and container don't fight over artifacts.
# If PipeWire complains about permissions, add --privileged.
docker run --rm -it \
  --name "$NAME" \
  -v "$PWD":/work \
  -v /work/target \
  "$IMAGE" "$@"
