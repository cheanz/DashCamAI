# DashCam AI — Software Architecture

**Status:** Proposed
**Date:** 2026-03-30
**Author:** Chenge Zhang
**Companion doc:** [Architecture.md](./Architecture.md)

---

## 1. Overview

The software system is split across two processor domains with a thin GPIO/UART boundary between them. The MCU runs a minimal FreeRTOS task set as a permanent sentinel. The SoC runs Linux with a set of cooperating single-responsibility daemons connected by a lightweight IPC layer.

---

## 2. MCU — FreeRTOS Task Model

**Target:** ESP32 / STM32
**OS:** FreeRTOS
**Principle:** Do as little as possible. The MCU's sole job is to watch for events when the SoC is asleep and wake it when needed. No video, audio, or AI inference runs here.

### Tasks

| Task | Priority | Stack | Responsibility |
|------|----------|-------|----------------|
| `power_task` | High | 2KB | Watches ignition signal; drives the driving/parked state machine; pulls GPIO HIGH to wake SoC |
| `gsensor_task` | High | 1KB | Services G-sensor interrupt; applies threshold filter; enqueues wake event on breach |
| `kws_task` | Medium | 4KB | Runs lightweight KWS model inference; active in parked state only; enqueues wake event on keyword |
| `comm_task` | Low | 2KB | UART bridge to SoC; transmits state change notifications; receives ACKs |

### Inter-task Communication

Tasks communicate via FreeRTOS queues and binary semaphores only — no shared global state. `gsensor_task` and `kws_task` both write to a shared `wake_event_queue` consumed by `power_task`, which serializes all wake decisions.

```
gsensor_task ──┐
               ├──► wake_event_queue ──► power_task ──► GPIO / comm_task
kws_task    ──┘
```

### State Machine

```
         ignition ON
[PARKED] ──────────────────► [DRIVING]
   ▲                              │
   │       ignition OFF +         │
   └──────── 60s debounce ────────┘
   │
   │  gsensor / KWS event
   └──► [PARKED_ALERT] ──► GPIO HIGH ──► SoC wakes
              │
              └──► returns to PARKED after SoC ACK
```

The 60-second debounce on the driving→parked transition prevents SoC suspend thrashing in stop-and-go traffic.

---

## 3. SoC — Linux Daemon Architecture

**Target:** Rockchip RV1106
**OS:** Linux
**Principle:** One daemon per concern. Each daemon can crash and restart independently without taking down recording — critical for a safety device.

### Process Map

```
┌──────────────────────────────────────────────────────────────┐
│                        power-daemon                          │
│         driving ↔ parked state machine | suspend coord       │
└────────────────────────────┬─────────────────────────────────┘
                             │ event bus (Unix domain sockets)
          ┌──────────────────┼──────────────────┐
          ▼                  ▼                  ▼
  ┌──────────────┐   ┌──────────────┐   ┌──────────────────┐
  │ media-daemon │   │  ai-daemon   │   │  cloud-daemon    │
  │              │──►│  (RKNN SDK)  │   │  (LTE + LLM + S3)│
  │ V4L2 + ALSA  │   │              │   │                  │
  └──────────────┘   └──────┬───────┘   └────────┬─────────┘
          │                 │                    │
          │                 ▼                    │
          │         ┌──────────────┐             │
          └────────►│ voice-daemon │─────────────┘
                    │ router + TTS │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │storage-daemon│
                    │ eMMC manager │
                    │ offline queue│
                    └──────────────┘
```

### 3.1 media-daemon

Owns the camera and microphone. Runs continuously while the SoC is active.

**Responsibilities:**
- V4L2 capture loop → H.264 encode → eMMC ring buffer write
- ALSA audio capture → VAD → audio chunk push to `ai-daemon` (on voice activity only, not a constant stream)
- On collision event signal: tags pre-event buffer (10–30s rolling) for preservation by `storage-daemon`

**Key design choice:** VAD gating is applied before audio reaches `ai-daemon`. This prevents the STT pipeline from running on silence and keeps NPU utilization low during normal driving.

### 3.2 ai-daemon

Owns the NPU via the RKNN SDK. Runs three concurrent inference pipelines.

**Responsibilities:**
- **Vision pipeline:** Consumes video frames from `media-daemon` shared memory ring buffer → YOLO-nano INT8 inference → emits `collision_detected` / `object_detected` events
- **Audio pipeline:** Consumes audio chunks from `media-daemon` → Whisper tiny STT → emits `transcript_ready` events with detected language
- **Intent pipeline:** Consumes transcripts → ONNX intent classifier → emits `intent_classified` events
- **KWS (driving mode):** Runs NPU-quality wake-word model; emits `wake_word_detected` event

**Does not touch:** Storage, network, TTS, conversation state.

### 3.3 voice-daemon

Orchestrates the voice pipeline end-to-end. The only daemon with a full view of a voice interaction.

**Responsibilities:**
- Receives `intent_classified` events from `ai-daemon`
- Routing decision: edge response (simple command, offline capable) or cloud LLM request (complex / multilingual)
- Manages multi-turn session context (last N exchanges, detected language, user preferences)
- Runs local TTS engine (Piper TTS) for edge responses
- Handles graceful offline failure: plays "queued for later" response when LTE is unavailable and intent requires cloud
- Hands translation/dialogue requests to `cloud-daemon`

**Latency targets:**
- Edge path: <400ms end-to-end (STT + intent + TTS)
- Cloud path: 800ms–1.2s (STT + LTE round-trip + LLM + TTS)

### 3.4 cloud-daemon

Owns all network I/O. The only daemon that touches the LTE module.

**Responsibilities:**
- LTE connection management via ModemManager
- Cloud LLM API client (HTTP/WebSocket) for dialogue and translation requests
- AWS S3 multipart upload client for evidence clips
- Offline queue flush: on LTE reconnect, drains the eMMC queue in FIFO order
- Publishes `lte_connected` / `lte_disconnected` events to the bus so other daemons can adapt

**Interface to other daemons:**
- `voice-daemon` sends requests synchronously (waits for LLM response or timeout)
- `storage-daemon` pushes clip upload jobs asynchronously (fire and forget)

### 3.5 storage-daemon

The eMMC gatekeeper. Arbitrates all writes to prevent conflicts between the recording loop and event-driven writes.

**Responsibilities:**
- Loop recording file rotation (create, seal, delete oldest)
- Evidence clip preservation: moves tagged pre/post-event segments to a protected directory
- Offline translation queue: append-only timestamped log (transcript, language, session context, timestamp)
- Clip upload job queue for `cloud-daemon`
- Storage pressure management — enforces eviction priority order:
  1. Evidence clips (never evicted automatically)
  2. Queued transcriptions (evicted after configurable TTL, default 7 days)
  3. Oldest loop footage (evicted first)

### 3.6 power-daemon

Mirrors the MCU state machine on the Linux side and coordinates the SoC suspend/resume sequence.

**Responsibilities:**
- Watches ignition signal and motion state (from MCU via UART)
- Applies 60-second debounce before triggering suspend
- On suspend: signals all daemons to flush and checkpoint; waits for ACKs; calls `systemctl suspend`
- On resume (GPIO wake): restores daemon state; publishes `system_resumed` with wake reason (gsensor / KWS / scheduled)

---

## 4. IPC Model

### High-bandwidth path — Shared Memory Ring Buffer

Video frames between `media-daemon` and `ai-daemon` are passed via a POSIX shared memory ring buffer. Passing frames over a socket would saturate the Unix socket and add copy overhead on a memory-constrained device. The ring buffer is allocated once at startup; `media-daemon` is the producer, `ai-daemon` is the consumer.

```
media-daemon                     ai-daemon
  [write ptr] ──► shm ring ──► [read ptr]
               (POSIX shm)
```

### Control & event path — Unix Domain Sockets

All control messages, commands, and events between daemons use Unix domain sockets. A minimal publish-subscribe broker (or nanomsg if a library dependency is acceptable) handles one-to-many fan-out for system events like `collision_detected`, `lte_connected`, and `system_resumed`.

### Summary

| Path | Mechanism | Rationale |
|------|-----------|-----------|
| Video frames (media → ai) | POSIX shared memory ring buffer | Zero-copy, high throughput |
| Audio chunks (media → ai) | Unix socket | Low bandwidth, simplicity |
| Events (any → any) | Pub-sub over Unix sockets | Decoupled, one-to-many |
| LLM requests (voice → cloud) | Unix socket RPC | Synchronous request/response |
| Upload jobs (storage → cloud) | Unix socket queue | Async, fire and forget |
| State sync (MCU ↔ SoC) | UART | Hardware boundary |

---

## 5. Model Inventory

| Model | Format | Est. Size | NPU runtime | Used by |
|-------|--------|-----------|-------------|---------|
| YOLO-nano INT8 | `.rknn` | ~2MB | RKNN SDK | ai-daemon (vision) |
| Whisper tiny (quantized) | `.rknn` | ~40MB | RKNN SDK | ai-daemon (STT) |
| Intent classifier | `.onnx` | <1MB | ONNX Runtime | ai-daemon (intent) |
| KWS — driving mode | `.rknn` | <1MB | RKNN SDK | ai-daemon (KWS) |
| KWS — parked mode | Binary | <100KB | MCU runtime | MCU kws_task |
| Piper TTS voice | `.onnx` | ~60MB | ONNX Runtime | voice-daemon |

All RKNN models are compiled for the RV1106 NPU target using the Rockchip RKNN Toolkit. ONNX models run on CPU via ONNX Runtime (lightweight build).

---

## 6. Repository Structure

```
dashcam-ai/
│
├── mcu/                        # FreeRTOS firmware (C)
│   ├── CMakeLists.txt
│   ├── main.c                  # FreeRTOS scheduler entry, task creation
│   └── tasks/
│       ├── gsensor_task.h/.c   # G-sensor interrupt handler + threshold filter
│       ├── kws_task.h/.c       # Lightweight KWS inference (parked mode)
│       ├── power_task.h/.c     # Driving/parked state machine, GPIO wake
│       └── comm_task.h/.c      # UART bridge to SoC
│
├── soc/                        # Linux userspace daemons (C/C++)
│   ├── CMakeLists.txt
│   ├── media-daemon/
│   │   ├── CMakeLists.txt
│   │   ├── main.c
│   │   ├── capture.h/.c        # V4L2 camera capture
│   │   ├── encoder.h/.c        # H.264 software/HW encode
│   │   ├── loop_writer.h/.c    # eMMC ring buffer write
│   │   └── vad.h/.c            # Voice activity detection
│   ├── ai-daemon/
│   │   ├── CMakeLists.txt
│   │   ├── main.c
│   │   ├── vision_pipeline.h/.c   # YOLO-nano inference via RKNN SDK
│   │   ├── stt_pipeline.h/.c      # Whisper tiny inference
│   │   ├── intent_pipeline.h/.c   # ONNX intent classifier
│   │   └── kws_pipeline.h/.c      # KWS driving mode
│   ├── voice-daemon/
│   │   ├── CMakeLists.txt
│   │   ├── main.c
│   │   ├── router.h/.c         # Edge vs. cloud routing logic
│   │   ├── session.h/.c        # Multi-turn session context
│   │   └── tts.h/.c            # Piper TTS wrapper
│   ├── cloud-daemon/
│   │   ├── CMakeLists.txt
│   │   ├── main.c
│   │   ├── lte_manager.h/.c    # ModemManager / connection lifecycle
│   │   ├── llm_client.h/.c     # Cloud LLM HTTP/WebSocket client
│   │   ├── s3_client.h/.c      # AWS S3 multipart upload
│   │   └── queue_flush.h/.c    # Offline queue drain on reconnect
│   ├── storage-daemon/
│   │   ├── CMakeLists.txt
│   │   ├── main.c
│   │   ├── loop_manager.h/.c   # Loop file rotation + eviction
│   │   ├── clip_store.h/.c     # Evidence clip preservation
│   │   └── offline_queue.h/.c  # Append-only translation queue
│   ├── power-daemon/
│   │   ├── CMakeLists.txt
│   │   ├── main.c
│   │   ├── state_machine.h/.c  # Driving/parked transitions + debounce
│   │   └── suspend.h/.c        # Coordinated suspend/resume sequence
│   └── shared/
│       ├── event_bus.h/.c      # Pub-sub broker over Unix sockets
│       ├── shm_ring_buffer.h/.c# POSIX shared memory ring buffer (video)
│       ├── ipc.h/.c            # Unix socket helpers (RPC + queue)
│       └── events.h            # Canonical event type definitions
│
├── models/                     # Quantized model weights (not in git — use LFS or fetch script)
│   ├── yolo-nano-int8.rknn
│   ├── whisper-tiny.rknn
│   ├── intent-classifier.onnx
│   ├── kws-driving.rknn
│   ├── kws-parked.bin
│   └── piper-tts.onnx
│
├── config/
│   ├── gsensor-thresholds.yaml # Impact thresholds for driving vs. parked
│   ├── storage-policy.yaml     # Loop size, clip cap, queue TTL, eviction order
│   └── cloud-endpoints.yaml    # LLM API + S3 endpoints (no secrets — use env)
│
└── CMakeLists.txt              # Top-level build
```

---

## 7. Open Questions

1. **RKNN Toolkit compatibility:** Whisper tiny at ~40MB is near the upper edge of what the RV1106 NPU can load. Confirm model fits within NPU memory budget before committing to it — fallback is CPU inference with higher latency (~600ms).
2. **Piper TTS language coverage:** Verify Piper TTS has voice models for all target languages. Some lower-resource languages may require a cloud TTS fallback even for edge responses.
3. **ONNX Runtime build size:** ONNX Runtime's full build is ~50MB. A minimal build (CPU only, no training ops) may be needed to fit within the RV1106 rootfs budget.
4. **MCU–SoC UART protocol:** Define framing, baud rate, and message schema for the MCU↔SoC UART link. A simple length-prefixed binary protocol (e.g., protobuf-nano) is preferred over plain text for robustness.
5. **Daemon restart policy:** Define systemd unit restart policies for each daemon. `media-daemon` and `ai-daemon` should restart immediately on crash; `cloud-daemon` should use exponential backoff to avoid hammering a flaky LTE connection.
