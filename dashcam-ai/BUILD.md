# Build & Run Guide

This project targets two hardware platforms. The encoder backend is selected
at compile time via a Cargo feature flag — everything else is identical.

| Platform | Encoder | Feature flag | Where to build |
|----------|---------|-------------|----------------|
| Jetson (bare Linux) | GStreamer `nvv4l2h264enc` | `--features jetson` | On the Jetson itself |
| RV1106 (production) | Rockchip MPP | `--features rockchip` | Dev machine (cross-compile) |

---

## Jetson — Bare Linux (Native Build)

Jetson runs full Ubuntu (JetPack) with systemd. Build and run natively — no
cross-compilation or Docker needed.

### 1. One-time setup

```bash
bash scripts/jetson-bare-setup.sh
```

This installs Rust, ALSA/V4L2/GStreamer dev libraries, builds the binary,
and installs it to `/usr/local/bin/media-daemon`.

### 2. Manual build

```bash
cd soc/media-daemon-rs
cargo build --release --features jetson
```

Binary lands at `target/release/media-daemon`.

### 3. Run manually

```bash
RUST_LOG=info ./target/release/media-daemon
```

### 4. Run as a systemd service

```bash
# Install services
sudo cp scripts/jetson-bare/dashcam-event-bus.service /etc/systemd/system/
sudo cp scripts/jetson-bare/media-daemon.service      /etc/systemd/system/

sudo systemctl daemon-reload
sudo systemctl enable --now dashcam-event-bus
sudo systemctl enable --now media-daemon
```

### 5. Common commands

```bash
# Status
sudo systemctl status media-daemon

# Live logs
journalctl -u media-daemon -f

# Restart
sudo systemctl restart media-daemon

# Stop
sudo systemctl stop media-daemon
```

### 6. Verify hardware before running

```bash
# Check camera
v4l2-ctl --list-devices
v4l2-ctl -d /dev/video0 --list-formats-ext

# Check audio
arecord -l

# Check GStreamer encoder
gst-inspect-1.0 nvv4l2h264enc   # hardware (preferred)
gst-inspect-1.0 x264enc         # software fallback
```

---

## RV1106 — Cross-Compile via Docker

The RV1106 runs minimal Buildroot Linux. Docker does not run on the device.
Build on your dev machine, deploy to the device over SSH.

### Prerequisites (dev machine)

- Docker Desktop (or Docker Engine on Linux)
- SSH access to the RV1106

### 1. Build

```bash
# Using the helper script
./scripts/build-rockchip.sh

# Or directly with Docker Compose
docker compose -f docker-compose.rockchip.yml run media-daemon-build
```

Binary and runtime library land in `dist/rv1106/`:

```
dist/rv1106/
├── media-daemon      ← armv7 binary
└── librknnrt.so      ← RKNN runtime (copy to /usr/lib on device)
```

### 2. Deploy

```bash
./scripts/deploy-rockchip.sh <RV1106_IP>
# e.g.
./scripts/deploy-rockchip.sh 192.168.1.100
```

This copies the binary, runtime library, config files, and model weights
to the device over SCP, and installs the init script.

### 3. Run on device

```bash
ssh root@<RV1106_IP>

# Start via init script
/etc/init.d/S90media-daemon start

# Or run manually
RUST_LOG=info /usr/bin/media-daemon &

# Tail logs
tail -f /var/log/media-daemon.log
```

### 4. Init script commands

```bash
/etc/init.d/S90media-daemon start
/etc/init.d/S90media-daemon stop
/etc/init.d/S90media-daemon restart
/etc/init.d/S90media-daemon status
```

### 5. Force full rebuild (no cache)

```bash
./scripts/build-rockchip.sh --no-cache
```

---

## Jetson — Docker (Alternative)

If you prefer running everything in containers on the Jetson:

```bash
# Build the media-daemon image
docker compose build media-daemon

# Start all services
docker compose up -d

# Logs
docker compose logs -f media-daemon

# Stop everything
docker compose down
```

Requires NVIDIA Container Runtime (`nvidia-container-toolkit`).
Run `scripts/jetson-setup.sh` first for one-time Docker setup.

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level: `error`, `warn`, `info`, `debug`, `trace` |
| `DASHCAM_VIDEO_DEVICE` | `/dev/video0` | V4L2 camera device |
| `DASHCAM_AUDIO_DEVICE` | `hw:0,0` | ALSA capture device |
| `DASHCAM_LOOP_DIR` | `/mnt/emmc/loop` | Loop recording directory |
| `LLM_API_KEY` | — | Cloud LLM API key (cloud-daemon) |
| `AWS_ACCESS_KEY_ID` | — | AWS credentials (cloud-daemon) |
| `AWS_SECRET_ACCESS_KEY` | — | AWS credentials (cloud-daemon) |

Copy `.env.example` to `.env` and fill in secrets before running cloud-daemon.

---

## Model Weights

Model files are not in git. See `models/README.md` for the fetch script and
full model inventory. Place all models in `models/` before deploying.

```bash
# Expected files
models/
├── yolo-nano-int8.rknn      # collision detection (RV1106 NPU)
├── whisper-tiny.rknn        # speech-to-text     (RV1106 NPU)
├── intent-classifier.onnx   # intent routing     (CPU, both targets)
├── kws-driving.rknn         # wake-word          (RV1106 NPU)
├── kws-parked.bin           # wake-word          (MCU)
└── piper-tts.onnx           # text-to-speech     (CPU, both targets)
```

---

## Troubleshooting

**Camera not found**
```bash
ls /dev/video*
# If missing, check MIPI-CSI driver: dmesg | grep -i csi
```

**Audio device not found**
```bash
arecord -l
# Try: DASHCAM_AUDIO_DEVICE=hw:1,0 ./media-daemon
```

**GStreamer hardware encoder not available (Jetson)**
```bash
# Falls back to x264enc automatically. To check:
gst-inspect-1.0 nvv4l2h264enc
# If missing on JetPack 5+: sudo apt install nvidia-l4t-gstreamer
```

**RKNN library not found (RV1106)**
```bash
# On device:
ls /usr/lib/librknnrt.so
# If missing, redeploy: ./scripts/deploy-rockchip.sh <IP>
```

**Shared memory permission error**
```bash
# User needs access to /dev/shm
ls -la /dev/shm
# Or run with --ipc=host if using Docker
```
