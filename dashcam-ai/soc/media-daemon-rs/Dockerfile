# ── Stage 1: Builder ──────────────────────────────────────────────────────────
#
# Use NVIDIA's L4T base image — matches JetPack and includes the NVIDIA
# userspace libraries needed for hardware video encoding.
#
# Match this tag to your JetPack version:
#   JetPack 5.1.x  →  r35.x.x
#   JetPack 6.0    →  r36.x.x
#
# Check yours with: cat /etc/nv_tegra_release
#
FROM nvcr.io/nvidia/l4t-base:r35.3.1 AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    # Rust toolchain bootstrap
    curl \
    ca-certificates \
    # C build tools (needed by some crates)
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    # ALSA dev headers — audio.rs
    libasound2-dev \
    # V4L2 dev headers — capture.rs
    libv4l-dev \
    # GStreamer dev headers — encoder.rs (replaces Rockchip MPP)
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    # Clean up apt cache
    && rm -rf /var/lib/apt/lists/*

# Install Rust (stable)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

# Cache dependencies before copying source
# This layer is rebuilt only when Cargo.toml / Cargo.lock changes.
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src

# Copy source and build for real
COPY src ./src
# Touch main.rs so Cargo sees the change
RUN touch src/main.rs && cargo build --release


# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
#
# Minimal runtime image — only what's needed to run the binary.
#
FROM nvcr.io/nvidia/l4t-base:r35.3.1 AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    # ALSA runtime
    libasound2 \
    # V4L2 runtime + camera debug tools
    libv4l-0 \
    v4l-utils \
    # GStreamer runtime + Jetson hardware encoder plugins
    gstreamer1.0-tools \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    # Jetson-specific GStreamer plugins (nvv4l2h264enc, nvvidconv)
    # These come pre-installed in the L4T image — listed here for clarity
    && rm -rf /var/lib/apt/lists/*

# Create runtime directories
RUN mkdir -p /var/run/dashcam /mnt/emmc/loop /mnt/emmc/evidence

# Copy only the compiled binary from the builder stage
COPY --from=builder /build/target/release/media-daemon /usr/local/bin/media-daemon

# Drop root — run as a dedicated user
RUN useradd -r -s /sbin/nologin dashcam \
    && chown -R dashcam:dashcam /var/run/dashcam /mnt/emmc
USER dashcam

ENV RUST_LOG=info

ENTRYPOINT ["/usr/local/bin/media-daemon"]
