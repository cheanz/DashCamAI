#!/usr/bin/env bash
# deploy-rockchip.sh
# Deploy compiled binaries and configs to a running RV1106 device over SSH.
#
# Usage:
#   ./scripts/deploy-rockchip.sh <RV1106_IP>
#   ./scripts/deploy-rockchip.sh 192.168.1.100
#
# Prerequisites on the RV1106:
#   - SSH server running (dropbear or openssh)
#   - /usr/bin writable (or rootfs remounted rw)
#   - librknnrt.so already present at /usr/lib/librknnrt.so
#     (comes with the Rockchip SDK rootfs — see RV1106 BSP)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$REPO_ROOT/dist/rv1106"
TARGET_IP="${1:-}"
TARGET_USER="root"
SSH_KEY="${SSH_KEY:-}"   # set SSH_KEY=/path/to/key to use a specific identity

if [ -z "$TARGET_IP" ]; then
    echo "Usage: $0 <RV1106_IP>"
    exit 1
fi

SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=10"
if [ -n "$SSH_KEY" ]; then
    SSH_OPTS="$SSH_OPTS -i $SSH_KEY"
fi

SCP="scp $SSH_OPTS"
SSH="ssh $SSH_OPTS ${TARGET_USER}@${TARGET_IP}"

echo "==> Checking connection to ${TARGET_IP}"
$SSH "uname -a" || { echo "ERROR: Cannot connect to ${TARGET_IP}"; exit 1; }

echo ""
echo "==> Remounting rootfs read-write (if needed)"
$SSH "mount -o remount,rw / 2>/dev/null || true"

echo ""
echo "==> Deploying media-daemon binary"
$SCP "$DIST_DIR/media-daemon" "${TARGET_USER}@${TARGET_IP}:/usr/bin/media-daemon"
$SSH "chmod +x /usr/bin/media-daemon"

echo ""
echo "==> Deploying RKNN runtime library (if not already present)"
$SSH "[ -f /usr/lib/librknnrt.so ] && echo 'librknnrt.so already on device' || true"
if [ -f "$DIST_DIR/librknnrt.so" ]; then
    $SSH "[ -f /usr/lib/librknnrt.so ]" \
        || $SCP "$DIST_DIR/librknnrt.so" "${TARGET_USER}@${TARGET_IP}:/usr/lib/librknnrt.so"
fi

echo ""
echo "==> Deploying config files"
$SSH "mkdir -p /etc/dashcam"
$SCP "$REPO_ROOT/config/gsensor-thresholds.yaml" \
     "${TARGET_USER}@${TARGET_IP}:/etc/dashcam/gsensor-thresholds.yaml"
$SCP "$REPO_ROOT/config/storage-policy.yaml" \
     "${TARGET_USER}@${TARGET_IP}:/etc/dashcam/storage-policy.yaml"
$SCP "$REPO_ROOT/config/cloud-endpoints.yaml" \
     "${TARGET_USER}@${TARGET_IP}:/etc/dashcam/cloud-endpoints.yaml"

echo ""
echo "==> Deploying model weights"
$SSH "mkdir -p /usr/share/dashcam/models"
for model in yolo-nano-int8.rknn whisper-tiny.rknn intent-classifier.onnx kws-driving.rknn; do
    if [ -f "$REPO_ROOT/models/$model" ]; then
        echo "    Deploying $model"
        $SCP "$REPO_ROOT/models/$model" \
             "${TARGET_USER}@${TARGET_IP}:/usr/share/dashcam/models/$model"
    else
        echo "    WARNING: $model not found in models/ — skipping"
    fi
done

echo ""
echo "==> Installing systemd-style init script (using procd for Buildroot)"
$SCP "$REPO_ROOT/scripts/rv1106-init/S90media-daemon" \
     "${TARGET_USER}@${TARGET_IP}:/etc/init.d/S90media-daemon" 2>/dev/null \
     || echo "    (no init script found — start manually)"

echo ""
echo "==> Creating runtime directories"
$SSH "mkdir -p /var/run/dashcam /mnt/emmc/loop /mnt/emmc/evidence"

echo ""
echo "==> Verifying binary on device"
$SSH "/usr/bin/media-daemon --version 2>/dev/null || file /usr/bin/media-daemon"

echo ""
echo "✓ Deployment complete."
echo ""
echo "Start on device:"
echo "   ssh ${TARGET_USER}@${TARGET_IP}"
echo "   RUST_LOG=info /usr/bin/media-daemon &"
echo ""
echo "Or restart the init service:"
echo "   ssh ${TARGET_USER}@${TARGET_IP} '/etc/init.d/S90media-daemon restart'"
