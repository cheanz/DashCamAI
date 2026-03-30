# DashCam AI — System Design Document

**Status:** Proposed
**Date:** 2026-03-30
**Author:** Chenge Zhang

---

## 1. Overview

DashCam AI is an embedded in-car device that combines continuous dashcam recording with an always-on AI assistant. It detects collisions in real time, backs up evidence to the cloud, and supports multilingual voice interaction — even when cellular connectivity is unavailable.

The device is designed around a dual-processor architecture: a low-power MCU that acts as a permanent sentinel, and a high-performance SoC that handles all video, audio, and AI workloads while the car is in motion.

---

## 2. Goals and Non-Goals

### Goals
- Continuous HD loop recording with automatic collision clip preservation
- Real-time object and collision detection running fully on-device
- Multilingual voice interaction (speech-to-text + cloud translation) as a core product feature
- Graceful degradation when LTE is unavailable: safety features and simple commands remain functional offline
- SoC BOM cost at or under $15

### Non-Goals
- Running a local large language model on-device (ruled out by BOM constraint)
- Sub-200ms end-to-end latency for cloud-dependent voice responses (not achievable on intermittent LTE; documented below)
- Replacing the phone as a navigation or media primary device

---

## 3. System Context

```
┌─────────────────────────────────────────────────────────┐
│                      In-Car Device                      │
│                                                         │
│   [Power Mgmt] → [MCU sentinel] ←→ [SoC + NPU]        │
│                        ↑                  ↓             │
│                   [G-Sensor]       [Camera / Mic]       │
│                                         ↓               │
│                                  [eMMC storage]         │
│                                         ↓               │
│                                   [LTE module]          │
└─────────────────────────────────────────────────────────┘
                              ↕ (intermittent)
              ┌───────────────────────────────┐
              │         Cloud Services        │
              │  Cloud LLM  |  AWS S3 Vault   │
              └───────────────────────────────┘
```

**Primary users:** Driver and passengers
**Connectivity assumption:** Intermittent 4G LTE — urban coverage is good, rural and tunnel scenarios must be handled gracefully
**Operating environments:** Driving (SoC always ON) and parked/idle (SoC in suspend-to-RAM, MCU active)

---

## 4. Hardware Subsystems

### 4.1 Power Management

| Component | Role |
|-----------|------|
| Car Battery (12V) | Primary power source |
| Hardwire Kit / PMIC | Voltage regulation, dual output, charge control |
| Supercapacitor | Explosion-proof backup — provides seconds of hold-up power for safe shutdown on sudden power loss |
| Power Distribution | Routes micro-amp continuous current to MCU; switches high current to SoC |

The supercapacitor is intentionally chosen over a lithium cell: no thermal runaway risk, no degradation over temperature cycles, and sufficient hold-up time to flush the active recording buffer and write metadata before shutdown.

### 4.2 MCU — Low-Power Sentinel

**Chip:** ESP32 or STM32
**OS:** FreeRTOS
**Power draw:** Micro-amp level (always ON)

The MCU has one job: watch for events when the SoC is asleep and wake it when needed. It never handles video, audio, or AI inference.

| Function | Detail |
|----------|--------|
| G-Sensor monitoring | Polls accelerometer for impact or vibration exceeding threshold |
| Wake-word detection (parked) | Runs a lightweight keyword-spotting model (KWS) locally — used only when the SoC is in suspend-to-RAM |
| SoC wake signal | Pulls a GPIO pin HIGH to bring the SoC out of suspend |

### 4.3 SoC — High-Performance Core

**Chip:** Rockchip RV1106
**OS:** Linux
**Power state:** Always ON while the car is driving; suspend-to-RAM when parked/idle

The RV1106 integrates an NPU, ISP, and audio codec on a single die, which keeps BOM cost within the $15 target while providing sufficient compute for the edge AI workload defined in Section 5.

| Peripheral | Interface | Notes |
|------------|-----------|-------|
| HD Camera (Sony IMX) | MIPI-CSI | High-speed lane; ISP pipeline runs inside SoC |
| Noise-canceling Mic Array | I2S audio bus | Multi-mic beamforming handled in software |
| In-car Speaker | Audio codec (built-in) | TTS playback and alert tones |
| DDR Memory | On-board | Working memory for Linux + active inference buffers |
| eMMC / SD Card | — | Loop recording destination, offline translation queue, evidence clip buffer |
| 4G LTE Module | USB / UART | Cloud connectivity; treated as best-effort |

### 4.4 Edge-to-Cloud Connectivity

| Service | Purpose | Dependency |
|---------|---------|------------|
| 4G LTE Module | Transport layer for all cloud traffic | Required for cloud features |
| Cloud LLM | Complex multi-turn dialogue; multilingual translation (core) | LTE |
| AWS S3 Cloud Vault | Damage-proof evidence clip backup | LTE |

Cloud features degrade gracefully when LTE is unavailable. See Section 6.3 for the offline fallback model.

---

## 5. Architecture Decisions

### ADR-1: SoC Power State Strategy

**Decision:** The SoC runs in one of two states — fully ON while driving, and suspend-to-RAM when parked. It does not power off completely.

**Rationale:**

Full power-off was considered but rejected. Cold boot time on the RV1106 is 2–4 seconds, which is unacceptable for two scenarios: a parked collision (hit-and-run) where evidence must be captured within 1 second of impact, and a user waking the device by voice from idle. Suspend-to-RAM reduces wake latency to approximately 100–300ms from GPIO interrupt to camera active, which is within the <1 second target.

Full ON at all times was also rejected due to power draw. Keeping the SoC active while parked would drain the car battery within hours.

The chosen model — always ON while driving, suspend-to-RAM when parked — cleanly separates the two use cases:

| State | Trigger in | SoC | MCU | Camera | NPU |
|-------|-----------|-----|-----|--------|-----|
| **Driving** | Ignition / motion detected | Fully ON | Monitoring | Recording | Continuously inferring |
| **Parked idle** | Engine off | Suspend-to-RAM | Active, watching G-sensor + KWS | Off | Off |
| **Parked alert** | G-sensor or wake-word | Woken via GPIO (100–300ms) | Hands off | Active | Active |

**Consequence:** The MCU must reliably detect the transition from driving to parked and trigger the SoC suspend sequence. A debounce window (suggested: 60 seconds of inactivity) should be used to avoid thrashing on stop-and-go traffic.

---

### ADR-2: NPU Workload Partition — Vision and Intent Only

**Decision:** The RV1106 NPU handles object detection, speech-to-text, intent classification, and wake-word detection (while driving). It does not run a local large language model.

**Rationale:**

The RV1106 NPU delivers approximately 0.5–1 TOPS. At this compute level, running a local LLM (e.g., Phi-3 mini at 3.8B parameters, Gemma 2B) would produce unacceptable latency (5–15 seconds per response) or require aggressive quantization that degrades quality below a usable threshold. A more capable NPU (e.g., RK3588 at 6 TOPS) would bring the SoC BOM to $25–40, violating the $15 constraint.

The workload is therefore bounded to models that run comfortably within 0.5 TOPS:

| Task | Model | Target Latency | Runs when |
|------|-------|---------------|-----------|
| Object & collision detection | YOLO-nano INT8 quantized | 30–80ms per frame | Continuously while driving |
| Speech-to-text | Whisper tiny (quantized) | 150–250ms per utterance | On voice activity |
| Intent classification | Small ONNX classifier | 20–50ms | After STT |
| Wake-word detection | KWS model (NPU quality) | <50ms | While driving (on SoC) |
| Wake-word detection | Lightweight KWS | <50ms | While parked (on MCU) |

**Consequence:** Complex dialogue and all multilingual translation must be handled by the Cloud LLM. This creates an LTE dependency for those features. The offline fallback strategy (ADR-3) is designed to contain the impact of this dependency.

---

### ADR-3: Voice Pipeline — Hybrid Routing with Offline Queue

**Decision:** Voice requests are processed on-device up to intent classification, then routed to either an edge response or the Cloud LLM based on intent complexity and LTE availability. Translation requests that cannot reach the cloud are queued on eMMC and flushed when connectivity is restored.

**Rationale:**

Multilingual translation is a core product feature and cannot be dropped when LTE is unavailable — it must degrade to "pending, will complete when reconnected" rather than silently failing. This requires a durable local queue.

Simple commands (navigation, playback, volume, emergency) can be served entirely on-device with deterministic latency. Routing these through the cloud would add unnecessary round-trip overhead and introduce an LTE dependency for functionality that does not need it.

**Voice pipeline flow:**

```
Mic Array
   │
   ▼ (I2S)
  SoC
   │
   ▼ (audio stream)
 STT — Whisper tiny on NPU (~150–250ms)
   │
   ▼ (transcript)
 Intent Classifier — ONNX on NPU (~20–50ms)
   │
   ├──► Simple intent → Edge Response → Speaker        [ <400ms, fully offline ]
   │
   └──► Complex / multilingual intent
            │
            ├── LTE available → Cloud LLM → response → SoC → Speaker  [ 800ms–1.2s ]
            │
            └── LTE unavailable → Transcription queued on eMMC
                                   └── Flushed to Cloud LLM on reconnect
                                        └── Response delivered when available
```

**Known latency trade-off:** End-to-end latency for cloud-routed voice responses is 800ms–1.2s under normal LTE conditions (STT ~200ms + LTE round-trip ~150–400ms + LLM inference ~200–500ms + TTS ~100–200ms). Sub-200ms is not achievable for cloud-dependent responses on this hardware profile. This is a documented constraint, not a bug.

**Offline queue design:**
- Queue stored as append-only timestamped records on eMMC
- Each record contains: utterance transcript, detected language, timestamp, session context
- On LTE reconnect, queue is flushed in FIFO order
- Translated responses are delivered as a notification (audio + optional display) upon return
- Queue is capped at a configurable maximum (suggested: 50 entries / 7 days) with FIFO eviction

---

## 6. Data Flows

### 6.1 Collision Detection and Evidence Path

This path runs continuously while driving. The SoC is always ON; no wake latency applies.

```
Sony IMX Camera
   │ MIPI-CSI (high-speed lane)
   ▼
Built-in ISP (image processing pipeline inside RV1106)
   │
   ▼
Main SoC (RV1106)
   │ video frames
   ▼
NPU — YOLO-nano INT8 (~30–80ms per frame)
   │
   ├── No event → frames overwritten in loop buffer on eMMC
   │
   └── Collision / near-miss detected
            │
            ▼
         eMMC — collision clip preserved (pre-event buffer + post-event)
            │
            ├── LTE available → upload to AWS S3 immediately
            │
            └── LTE unavailable → clip held on eMMC, uploaded on reconnect
```

**Pre-event buffer:** A rolling buffer of 10–30 seconds prior to the detected event is preserved alongside the post-event clip. This captures the lead-up to the incident.

### 6.2 Voice Interaction Path

```
Noise-Canceling Mic Array
   │ I2S audio bus
   ▼
SoC — Voice Activity Detection (VAD)
   │ audio stream (on voice activity only)
   ▼
NPU — Whisper tiny STT (~150–250ms)
   │ transcript
   ▼
NPU — Intent Classifier (~20–50ms)
   │ classified intent + detected language
   ▼
Intent Router
   │
   ├── Simple command (offline capable)
   │      ▼
   │   Edge Response → Audio Codec → In-Car Speaker   [ <400ms total ]
   │
   └── Complex dialogue or multilingual
          │
          ├── LTE available
          │      ▼
          │   4G LTE → Cloud LLM
          │      ▼
          │   Response → SoC → TTS → Speaker          [ 800ms–1.2s total ]
          │
          └── LTE unavailable
                 ▼
              Queued on eMMC (see ADR-3)
```

### 6.3 Offline Fallback Model

When LTE is unavailable, the device operates in a reduced but safe mode:

| Feature | LTE available | LTE unavailable |
|---------|-------------|----------------|
| Loop recording | ✅ | ✅ |
| Collision detection | ✅ | ✅ |
| Evidence clip preservation | ✅ (local + cloud) | ✅ (local only, queued for upload) |
| Simple voice commands | ✅ | ✅ |
| Complex voice dialogue | ✅ | ❌ (graceful failure with spoken notice) |
| Multilingual translation | ✅ | ⏳ (queued, delivered on reconnect) |
| Evidence cloud backup | ✅ | ⏳ (queued, uploaded on reconnect) |

---

## 7. Latency Budget

| Path | Budget | Achievable | Notes |
|------|--------|-----------|-------|
| Collision detection (driving) | <100ms | ✅ ~30–80ms | NPU only, SoC already ON |
| Parked collision → camera active | <1s | ✅ ~100–300ms | MCU GPIO → SoC suspend-to-RAM exit |
| Simple voice command | <400ms | ✅ ~350–400ms | STT + intent + edge response, no cloud |
| Cloud voice response | <1.5s | ✅ ~800ms–1.2s | STT + LTE + LLM + TTS |
| Cloud video upload (per clip) | Best-effort | ⚠️ LTE dependent | Not latency-sensitive |

---

## 8. Open Questions

1. **Collision sensitivity threshold:** The G-sensor threshold for triggering the parked-state wake (and clip preservation while driving) needs calibration. Too sensitive causes false positives on rough roads; too coarse misses low-speed impacts. A configurable threshold with a default tuned to parking lot speeds (5–15 km/h) is recommended.

2. **eMMC queue eviction policy:** The offline translation queue and evidence clip buffer share the same storage medium as the loop recording. Storage pressure management — especially when driving in a no-coverage area for an extended period — needs a defined priority order (suggested: evidence clips > queued transcriptions > oldest loop footage).

3. **Language detection accuracy:** The Intent Classifier must detect the spoken language to route multilingual requests correctly. Accuracy on short utterances (<3 words) in languages close to each other (e.g., Mandarin vs. Cantonese) may require a dedicated lightweight language-ID model rather than relying on Whisper's built-in detection.

4. **KWS model parity:** The driving-state wake-word model (on NPU) and the parked-state model (on MCU) will have different accuracy profiles. The MCU model is more constrained. Acceptable false-negative rate for the parked KWS should be defined before selecting the MCU-side model.

5. **Privacy and data retention:** Video clips uploaded to S3 and voice transcripts queued on eMMC may be subject to regional data protection regulations (GDPR, PIPL, CCPA). Retention policies, at-rest encryption on eMMC, and in-transit encryption to cloud endpoints should be specified before the device ships in regulated markets.

---

## 9. Action Items

1. [ ] Validate YOLO-nano INT8 accuracy on RV1106 NPU against target collision/object detection benchmarks
2. [ ] Benchmark Whisper tiny (quantized) latency on RV1106 NPU across language samples including Mandarin, Cantonese, and English
3. [ ] Define G-sensor threshold matrix for driving vs. parked states
4. [ ] Specify eMMC storage allocation: loop buffer size, evidence clip cap, translation queue cap
5. [ ] Select and benchmark MCU-side KWS model (ESP32 / STM32 target)
6. [ ] Define acceptable false-positive / false-negative rates for collision detection
7. [ ] Confirm RV1106 suspend-to-RAM wake latency under production firmware (target: <300ms GPIO to camera active)
8. [ ] Specify at-rest encryption requirements for eMMC (evidence clips + offline queue)
9. [ ] Define Cloud LLM provider and API contract for translation and dialogue endpoints
