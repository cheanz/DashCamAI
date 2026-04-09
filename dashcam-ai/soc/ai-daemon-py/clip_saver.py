"""
Collision clip saver — pre-roll buffer + evidence preservation.

Keeps a rolling buffer of the last N seconds of frames.
When tag_collision() is called, flushes the buffer to disk as an MP4.
"""

import logging
import os
import time
import threading
from collections import deque
from pathlib import Path
import cv2
import numpy as np

log = logging.getLogger(__name__)

EVIDENCE_DIR  = Path("/mnt/emmc/evidence")   # production path
FALLBACK_DIR  = Path("/tmp/dashcam_evidence") # fallback if eMMC not mounted
PRE_ROLL_SECS = 10    # seconds of pre-collision footage to preserve
FPS           = 30


class ClipSaver:
    """Thread-safe rolling frame buffer with collision clip export."""

    def __init__(
        self,
        pre_roll_secs: int = PRE_ROLL_SECS,
        fps: int = FPS,
        output_dir: Path | None = None,
    ):
        self.fps       = fps
        self.max_frames = pre_roll_secs * fps
        self._buf: deque[np.ndarray] = deque(maxlen=self.max_frames)
        self._lock     = threading.Lock()
        self._clip_idx = 0

        # Choose output directory
        if output_dir:
            self.output_dir = output_dir
        elif EVIDENCE_DIR.parent.exists():
            self.output_dir = EVIDENCE_DIR
        else:
            self.output_dir = FALLBACK_DIR

        self.output_dir.mkdir(parents=True, exist_ok=True)
        log.info(f"ClipSaver: saving to {self.output_dir}")

    def push_frame(self, frame: np.ndarray):
        """Add a frame to the rolling buffer. Thread-safe."""
        with self._lock:
            self._buf.append(frame.copy())

    def tag_collision(self, post_roll_frames: list[np.ndarray] | None = None) -> Path:
        """
        Flush the pre-roll buffer + optional post-roll frames to an MP4.
        Returns the path of the saved clip.
        """
        with self._lock:
            frames = list(self._buf)

        if post_roll_frames:
            frames.extend(post_roll_frames)

        if not frames:
            log.warning("tag_collision called but buffer is empty")
            return self.output_dir

        self._clip_idx += 1
        ts      = int(time.time())
        outpath = self.output_dir / f"collision_{ts}_{self._clip_idx:04d}.mp4"

        h, w = frames[0].shape[:2]
        fourcc = cv2.VideoWriter_fourcc(*"mp4v")
        writer = cv2.VideoWriter(str(outpath), fourcc, self.fps, (w, h))

        for f in frames:
            writer.write(f)
        writer.release()

        log.info(
            f"saved collision clip → {outpath}  "
            f"({len(frames)} frames, {len(frames)/self.fps:.1f}s)"
        )
        return outpath
