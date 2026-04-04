#!/usr/bin/env bash
# jetson-setup.sh — one-time setup on the Jetson before first Docker run
set -euo pipefail

echo "==> Checking JetPack version"
cat /etc/nv_tegra_release

echo ""
echo "==> Installing NVIDIA Container Toolkit (if not already installed)"
if ! command -v nvidia-ctk &> /dev/null; then
    distribution=$(. /etc/os-release; echo $ID$VERSION_ID)
    curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey \
        | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
    curl -s -L https://nvidia.github.io/libnvidia-container/$distribution/libnvidia-container.list \
        | sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' \
        | sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list
    sudo apt-get update && sudo apt-get install -y nvidia-container-toolkit
    sudo nvidia-ctk runtime configure --runtime=docker
    sudo systemctl restart docker
    echo "NVIDIA Container Toolkit installed."
else
    echo "NVIDIA Container Toolkit already installed."
fi

echo ""
echo "==> Checking camera"
if [ -e /dev/video0 ]; then
    v4l2-ctl --list-devices
    echo "Camera formats:"
    v4l2-ctl -d /dev/video0 --list-formats-ext | head -30
else
    echo "WARNING: /dev/video0 not found. Connect camera and check driver."
fi

echo ""
echo "==> Checking audio"
aplay -l 2>/dev/null || echo "WARNING: No ALSA playback devices found."
arecord -l 2>/dev/null || echo "WARNING: No ALSA capture devices found."

echo ""
echo "==> Creating eMMC mount directory"
sudo mkdir -p /mnt/emmc/loop /mnt/emmc/evidence

echo ""
echo "==> Checking GStreamer Jetson plugins"
gst-inspect-1.0 nvv4l2h264enc > /dev/null 2>&1 \
    && echo "nvv4l2h264enc: OK (hardware H.264)" \
    || echo "WARNING: nvv4l2h264enc not found — will fall back to x264enc (software)"

echo ""
echo "==> Verifying Docker NVIDIA runtime"
docker run --rm --runtime nvidia nvcr.io/nvidia/l4t-base:r35.3.1 \
    nvidia-smi 2>/dev/null \
    && echo "NVIDIA runtime: OK" \
    || echo "WARNING: NVIDIA runtime not working — check nvidia-ctk setup"

echo ""
echo "==> Setup complete. Next steps:"
echo "    1. cp .env.example .env && edit .env with your API keys"
echo "    2. docker compose build"
echo "    3. docker compose up -d media-daemon"
echo "    4. docker compose logs -f media-daemon"
