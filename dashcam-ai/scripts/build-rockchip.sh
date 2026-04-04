#!/usr/bin/env bash
# build-rockchip.sh
# Cross-compile the media-daemon for RV1106 using Docker.
# Run this on your DEV MACHINE, not on the RV1106.
#
# Usage:
#   ./scripts/build-rockchip.sh              # build media-daemon
#   ./scripts/build-rockchip.sh --all        # build all SoC daemons
#   ./scripts/build-rockchip.sh --no-cache   # force full rebuild

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$REPO_ROOT/dist/rv1106"
IMAGE_NAME="dashcam-builder-rv1106"

ALL=false
NO_CACHE=""

for arg in "$@"; do
    case $arg in
        --all)       ALL=true ;;
        --no-cache)  NO_CACHE="--no-cache" ;;
    esac
done

mkdir -p "$DIST_DIR"

echo "==> Building cross-compiler image for RV1106 (armv7-unknown-linux-gnueabihf)"
docker build $NO_CACHE \
    -f "$REPO_ROOT/soc/media-daemon-rs/Dockerfile.rockchip" \
    -t "$IMAGE_NAME" \
    "$REPO_ROOT/soc/media-daemon-rs"

echo ""
echo "==> Extracting compiled binary to $DIST_DIR"
docker run --rm \
    -v "$DIST_DIR:/dist" \
    "$IMAGE_NAME"

echo ""
echo "==> Build output:"
ls -lh "$DIST_DIR"

echo ""
echo "==> Checking binary architecture"
file "$DIST_DIR/media-daemon"

echo ""
echo "✓ Done. Deploy with:"
echo "   ./scripts/deploy-rockchip.sh <RV1106_IP>"
