#!/usr/bin/env bash
# ── DashCam AI — Jetson demo launcher ────────────────────────────────────────
#
# Usage:
#   ./run_demo.sh carla          # live CARLA simulation
#   ./run_demo.sh video demo.mp4 # pre-recorded video file
#   ./run_demo.sh webcam         # USB camera

set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

MODE="${1:-carla}"

# Install dependencies if needed
if ! python3 -c "import ultralytics" 2>/dev/null; then
    echo "Installing Python dependencies..."
    pip3 install -r requirements.txt --break-system-packages
fi

# Download YOLOv8n weights if not present
if [ ! -f "yolov8n.pt" ]; then
    echo "Downloading YOLOv8n weights..."
    python3 -c "from ultralytics import YOLO; YOLO('yolov8n.pt')"
fi

case "$MODE" in
  carla)
    echo "Starting ai-daemon with CARLA source..."
    python3 ai_daemon.py --source carla --display
    ;;
  video)
    VIDEO="${2:-demo.mp4}"
    echo "Starting ai-daemon with video: $VIDEO"
    python3 ai_daemon.py --source video --path "$VIDEO" --display
    ;;
  webcam)
    echo "Starting ai-daemon with webcam..."
    python3 ai_daemon.py --source webcam --display
    ;;
  *)
    echo "Usage: $0 [carla|video <path>|webcam]"
    exit 1
    ;;
esac
