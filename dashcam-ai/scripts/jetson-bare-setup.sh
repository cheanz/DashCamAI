#!/usr/bin/env bash
# jetson-bare-setup.sh
# Install all dependencies and build media-daemon natively on Jetson.
# Run this directly on the Jetson — no Docker needed.
#
# Tested on: JetPack 5.x (Ubuntu 20.04 aarch64)
# Usage: bash scripts/jetson-bare-setup.sh

set -euo pipefail

echo "==> JetPack version"
cat /etc/nv_tegra_release
echo ""

# ── 1. System dependencies ────────────────────────────────────────────────────
echo "==> Installing system dependencies"
sudo apt-get update && sudo apt-get install -y \
    # Rust bootstrap
    curl \
    # C build tools (some crates need them)
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    # ALSA — audio capture
    libasound2-dev \
    # V4L2 — camera capture
    libv4l-dev \
    v4l-utils \
    # GStreamer — H.264 encoding (hardware via nvv4l2h264enc)
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-tools \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    # ONNX Runtime (for ai-daemon on Jetson — replaces RKNN)
    # Download from: https://github.com/microsoft/onnxruntime/releases
    # pick the aarch64 build
    # Optional system utils
    socat \        # event bus broker stand-in during development
    net-tools

echo ""

# ── 2. Rust ───────────────────────────────────────────────────────────────────
echo "==> Installing Rust (native aarch64)"
if command -v rustc &>/dev/null; then
    echo "Rust already installed: $(rustc --version)"
else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
fi
rustc --version
cargo --version

echo ""

# ── 3. Verify GStreamer hardware encoder ──────────────────────────────────────
echo "==> Checking GStreamer encoder availability"
if gst-inspect-1.0 nvv4l2h264enc &>/dev/null; then
    echo "✓ nvv4l2h264enc available (Jetson hardware H.264)"
    ENCODER="nvv4l2h264enc"
elif gst-inspect-1.0 x264enc &>/dev/null; then
    echo "⚠ nvv4l2h264enc not found — falling back to x264enc (software)"
    echo "  Install: sudo apt install gstreamer1.0-plugins-ugly"
    ENCODER="x264enc"
else
    echo "✗ No H.264 encoder found — install gstreamer1.0-plugins-ugly"
    ENCODER="none"
fi
echo "Encoder: $ENCODER"

echo ""

# ── 4. Build media-daemon ─────────────────────────────────────────────────────
echo "==> Building media-daemon (native aarch64)"
cd "$(dirname "$0")/../soc/media-daemon-rs"

# Set encoder backend via feature flag or env for Jetson
GSTREAMER_ENCODER=$ENCODER cargo build --release

echo ""
echo "==> Binary info"
file ./target/release/media-daemon
ls -lh ./target/release/media-daemon

echo ""

# ── 5. Install ────────────────────────────────────────────────────────────────
echo "==> Installing binary"
sudo cp ./target/release/media-daemon /usr/local/bin/media-daemon
sudo chmod +x /usr/local/bin/media-daemon

echo ""

# ── 6. Runtime directories ────────────────────────────────────────────────────
echo "==> Creating runtime directories"
sudo mkdir -p /var/run/dashcam /mnt/emmc/loop /mnt/emmc/evidence
sudo useradd -r -s /sbin/nologin dashcam 2>/dev/null || true
sudo chown -R dashcam:dashcam /var/run/dashcam /mnt/emmc

echo ""
echo "✓ Setup complete."
echo ""
echo "Next steps:"
echo "  1. Install systemd service:"
echo "     sudo cp scripts/jetson-bare/media-daemon.service /etc/systemd/system/"
echo "     sudo systemctl daemon-reload"
echo "     sudo systemctl enable --now media-daemon"
echo ""
echo "  2. Or run manually:"
echo "     RUST_LOG=info /usr/local/bin/media-daemon"
