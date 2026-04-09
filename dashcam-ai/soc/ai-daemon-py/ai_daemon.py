"""
DashCam AI Daemon — Python edition for Jetson demo.

Reads frames from a pluggable source (CARLA / video / webcam),
runs YOLO collision detection, saves evidence clips, and publishes
events to the event bus using the same wire protocol as the Rust daemon.

Usage:
  python ai_daemon.py --source carla
  python ai_daemon.py --source video --path /path/to/demo.mp4
  python ai_daemon.py --source webcam --device 0
  python ai_daemon.py --source video --path demo.mp4 --display
"""

import argparse
import asyncio
import logging
import signal
import sys
import threading
import time
from pathlib import Path

import cv2
import numpy as np

from clip_saver   import ClipSaver
from event_bus    import EventBus, EventType
from frame_source import CarlaSource, VideoSource, WebcamSource
from yolo_pipeline import YoloPipeline, InferenceResult

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s  %(levelname)-7s  %(name)s — %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("ai_daemon")

# ── Tunables ──────────────────────────────────────────────────────────────────

COLLISION_COOLDOWN_SECS = 5.0    # minimum gap between collision events
POST_ROLL_FRAMES        = 60     # ~2 s of footage after collision trigger
DISPLAY_SCALE           = 0.6    # resize display window to this fraction


# ── Overlay drawing ───────────────────────────────────────────────────────────

def draw_overlay(
    frame: np.ndarray,
    result: InferenceResult,
    fps: float,
    collision_active: bool,
    clip_count: int,
) -> np.ndarray:
    out = frame.copy()
    h, w = out.shape[:2]

    # Draw bounding boxes
    for det in result.detections:
        x1, y1, x2, y2 = map(int, det.box)
        # Colour by risk level
        if det.width_ratio >= 0.50:
            colour = (0, 0, 255)    # red — collision
        elif det.width_ratio >= 0.30:
            colour = (0, 165, 255)  # orange — near-miss
        else:
            colour = (0, 255, 0)    # green — safe distance

        cv2.rectangle(out, (x1, y1), (x2, y2), colour, 2)
        label = f"{det.label} {det.confidence:.2f}"
        cv2.putText(out, label, (x1, y1 - 8),
                    cv2.FONT_HERSHEY_SIMPLEX, 0.55, colour, 2)

    # Status bar background
    cv2.rectangle(out, (0, 0), (w, 52), (20, 20, 20), -1)

    # FPS
    cv2.putText(out, f"FPS: {fps:.1f}", (12, 32),
                cv2.FONT_HERSHEY_SIMPLEX, 0.8, (200, 200, 200), 2)

    # Risk indicator
    if collision_active or result.is_collision:
        risk_text   = "!! COLLISION !!"
        risk_colour = (0, 0, 255)
    elif result.is_near_miss:
        risk_text   = "NEAR MISS"
        risk_colour = (0, 165, 255)
    else:
        risk_text   = "SAFE"
        risk_colour = (0, 220, 0)

    cv2.putText(out, risk_text, (w // 2 - 100, 36),
                cv2.FONT_HERSHEY_SIMPLEX, 1.0, risk_colour, 3)

    # Clip counter
    cv2.putText(out, f"Clips saved: {clip_count}", (w - 220, 32),
                cv2.FONT_HERSHEY_SIMPLEX, 0.7, (200, 200, 200), 2)

    # Big red flash on collision
    if collision_active or result.is_collision:
        overlay = out.copy()
        cv2.rectangle(overlay, (0, 0), (w, h), (0, 0, 200), -1)
        cv2.addWeighted(overlay, 0.15, out, 0.85, 0, out)

    return out


# ── Main async event loop ─────────────────────────────────────────────────────

async def run_event_loop(bus: EventBus, frame_q: asyncio.Queue):
    """
    Async loop: receives inference results from the main thread via queue,
    publishes events to the bus.
    """
    clip_id = 0
    last_collision_ts = 0.0

    while True:
        item = await frame_q.get()
        if item is None:
            break   # shutdown sentinel

        result, clip_path = item

        now = time.time()

        if result.is_collision:
            cooldown_ok = (now - last_collision_ts) >= COLLISION_COOLDOWN_SECS
            if cooldown_ok:
                clip_id += 1
                last_collision_ts = now
                await bus.publish_collision(result.collision_conf, clip_id)
                if clip_path:
                    log.info(f"collision clip {clip_id} saved → {clip_path}")

        elif result.is_near_miss:
            await bus.publish_object_detected("near_miss", result.near_miss_conf)

        for det in result.detections:
            if det.confidence >= 0.6:
                await bus.publish_object_detected(det.label, det.confidence)


# ── Main entry point ──────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="DashCam AI Daemon (Python)")
    parser.add_argument("--source",  choices=["carla", "video", "webcam"],
                        default="carla")
    parser.add_argument("--path",    default=None,
                        help="Video file path (--source video)")
    parser.add_argument("--device",  default=0, type=int,
                        help="Webcam device index (--source webcam)")
    parser.add_argument("--carla-host", default="localhost")
    parser.add_argument("--carla-port", default=2000, type=int)
    parser.add_argument("--model",   default="yolov8n.pt",
                        help="YOLO model file (yolov8n.pt / yolov8s.pt / ...)")
    parser.add_argument("--display", action="store_true",
                        help="Show live annotated video window")
    parser.add_argument("--no-gpu",  action="store_true",
                        help="Use CPU inference (slow — for testing only)")
    args = parser.parse_args()

    device = "cpu" if args.no_gpu else "cuda"

    # ── Frame source ──────────────────────────────────────────────────────────
    if args.source == "carla":
        source = CarlaSource(host=args.carla_host, port=args.carla_port)
        log.info("frame source: CARLA simulation")
    elif args.source == "video":
        if not args.path:
            parser.error("--path required for --source video")
        source = VideoSource(args.path, loop=True)
        log.info(f"frame source: video file {args.path}")
    else:
        source = WebcamSource(device=args.device)
        log.info(f"frame source: webcam {args.device}")

    # ── Inference + clip saving ────────────────────────────────────────────────
    yolo  = YoloPipeline(model_path=args.model, device=device)
    saver = ClipSaver()

    # ── Event bus (async) ─────────────────────────────────────────────────────
    frame_q: asyncio.Queue = asyncio.Queue(maxsize=64)
    bus     = EventBus()

    loop    = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    loop.run_until_complete(bus.connect())

    event_task = loop.create_task(run_event_loop(bus, frame_q))
    bus_thread = threading.Thread(
        target=loop.run_forever, daemon=True, name="event-bus"
    )
    bus_thread.start()

    # ── Shutdown handler ──────────────────────────────────────────────────────
    running = [True]

    def _shutdown(sig, _frame):
        log.info(f"received {signal.Signals(sig).name} — shutting down")
        running[0] = False

    signal.signal(signal.SIGTERM, _shutdown)
    signal.signal(signal.SIGINT,  _shutdown)

    # ── Main inference loop ───────────────────────────────────────────────────
    log.info("ai-daemon started — press Ctrl+C to stop")

    collision_active   = False
    collision_cooldown = 0.0
    post_roll_buf: list[np.ndarray] = []
    clip_count = 0

    fps_t0     = time.perf_counter()
    fps_frames = 0
    fps_val    = 0.0

    with source if hasattr(source, "__enter__") else _noop_ctx(source) as src_iter:
        for frame in src_iter:
            if not running[0]:
                break

            # ── Inference ─────────────────────────────────────────────────────
            result = yolo.infer(frame)

            # ── Clip saver ────────────────────────────────────────────────────
            saver.push_frame(frame)

            clip_path = None

            now = time.time()
            if result.is_collision and (now - collision_cooldown) >= COLLISION_COOLDOWN_SECS:
                collision_active   = True
                collision_cooldown = now
                post_roll_buf      = []
                log.warning(
                    f"COLLISION detected! conf={result.collision_conf:.2f}"
                )

            if collision_active:
                post_roll_buf.append(frame)
                if len(post_roll_buf) >= POST_ROLL_FRAMES:
                    clip_path       = saver.tag_collision(post_roll_buf)
                    clip_count     += 1
                    collision_active = False
                    post_roll_buf   = []

            # ── Event bus publish (non-blocking) ─────────────────────────────
            try:
                frame_q.put_nowait((result, clip_path))
            except asyncio.QueueFull:
                pass   # bus is backed up — drop this frame's events

            # ── FPS counter ───────────────────────────────────────────────────
            fps_frames += 1
            if fps_frames >= 30:
                elapsed   = time.perf_counter() - fps_t0
                fps_val   = fps_frames / elapsed
                fps_t0    = time.perf_counter()
                fps_frames = 0

            # ── Display (optional) ────────────────────────────────────────────
            if args.display:
                annotated = draw_overlay(
                    frame, result, fps_val, collision_active, clip_count
                )
                small = cv2.resize(
                    annotated,
                    None,
                    fx=DISPLAY_SCALE,
                    fy=DISPLAY_SCALE,
                )
                cv2.imshow("DashCam AI", small)
                if cv2.waitKey(1) & 0xFF == ord("q"):
                    break

    # ── Cleanup ───────────────────────────────────────────────────────────────
    log.info("shutting down...")
    asyncio.run_coroutine_threadsafe(frame_q.put(None), loop).result(timeout=2)
    loop.call_soon_threadsafe(loop.stop)
    bus_thread.join(timeout=3)
    bus.close()
    cv2.destroyAllWindows()
    log.info(f"done — {clip_count} collision clip(s) saved to {saver.output_dir}")


class _noop_ctx:
    """Context manager shim for sources that aren't context managers."""
    def __init__(self, obj): self.obj = obj
    def __enter__(self): return iter(self.obj)
    def __exit__(self, *_): pass


if __name__ == "__main__":
    main()
