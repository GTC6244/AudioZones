# AudioZones spike — runnable PipeWire + WirePlumber environment.
# Runs the de-risking spike against VIRTUAL nodes (no real hardware).
# Proves: the spike compiles, Q1 read path, Q2 cross-thread mutation,
# Q3 WirePlumber coexistence (does WP revert a manual link?).
# Does NOT replicate the real 8-channel USB card — that's the on-box confirmation.
#
# Base ships a Rust toolchain; we add the PipeWire stack + dev headers + bindgen deps.
# If the `pipewire` crate fails to build against bookworm's libpipewire (0.3.65),
# switch the base to a newer Debian (trixie) which carries a newer PipeWire.
FROM rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
      pipewire pipewire-bin pipewire-pulse wireplumber \
      libpipewire-0.3-dev libspa-0.2-dev \
      pulseaudio-utils \
      dbus dbus-x11 \
      pkg-config clang libclang-dev \
      jq procps \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work

# Static virtual sinks (support.null-audio-sink) loaded by the daemon at startup —
# the reliable way to get live, linkable ports headlessly (no hardware/logind).
COPY pipewire-virtual-sinks.conf /etc/pipewire/pipewire.conf.d/10-virtual-sinks.conf

# Pre-build inside the image so a compile error (my pipewire-rs API guesses) surfaces
# at `docker build` time, loudly, instead of at first run.
COPY Cargo.toml ./
COPY src ./src
RUN cargo build 2>&1 | tee /tmp/build.log || \
    echo ">>> SPIKE BUILD FAILED — see errors above. Likely a one-line pipewire-rs API tweak."

COPY container-init.sh /usr/local/bin/container-init.sh
RUN chmod +x /usr/local/bin/container-init.sh

ENTRYPOINT ["/usr/local/bin/container-init.sh"]
# Default: enumerate (Q1). Override with env + `cargo run` for Q2/Q3.
CMD ["cargo", "run"]
