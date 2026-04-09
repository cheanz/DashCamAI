"""
YOLO collision detection pipeline.

Uses YOLOv8-nano for real-time object detection.
Collision score is derived from bounding box proximity to the camera
(box width as a fraction of frame width) × detection confidence.
"""

import logging
import numpy as np
from dataclasses import dataclass, field

log = logging.getLogger(__name__)

# ── Classes that matter for collision risk ────────────────────────────────────

COLLISION_CLASSES = {"car", "truck", "bus", "motorcycle", "person", "bicycle"}

# Thresholds: box_width / frame_width — bigger = closer = more dangerous
NEAR_MISS_THRESHOLD  = 0.30   # 30% of frame width → near-miss alert
COLLISION_THRESHOLD  = 0.50   # 50% of frame width → collision alert


@dataclass
class Detection:
    label:       str
    confidence:  float
    box:         tuple[float, float, float, float]   # x1, y1, x2, y2 (pixels)
    width_ratio: float   # box width / frame width — proxy for distance


@dataclass
class InferenceResult:
    detections:     list[Detection] = field(default_factory=list)
    collision_conf: float = 0.0   # 0..1 — highest collision risk score
    near_miss_conf: float = 0.0   # 0..1 — highest near-miss risk score

    @property
    def is_collision(self) -> bool:
        return self.collision_conf >= 0.55

    @property
    def is_near_miss(self) -> bool:
        return self.near_miss_conf >= 0.40 and not self.is_collision


# ── Pipeline ──────────────────────────────────────────────────────────────────

class YoloPipeline:
    def __init__(self, model_path: str = "yolov8n.pt", device: str = "cuda"):
        log.info(f"loading YOLO model {model_path} on {device}")
        from ultralytics import YOLO
        self.model  = YOLO(model_path)
        self.device = device
        # Warm up — first inference is slow due to CUDA JIT
        dummy = np.zeros((640, 640, 3), dtype=np.uint8)
        self.model(dummy, device=self.device, verbose=False)
        log.info("YOLO pipeline ready")

    def infer(self, frame_bgr: np.ndarray) -> InferenceResult:
        """Run inference on one BGR frame. Returns detections + risk scores."""
        h, w = frame_bgr.shape[:2]
        results = self.model(frame_bgr, device=self.device, verbose=False)

        result = InferenceResult()

        for r in results:
            for box in r.boxes:
                label = r.names[int(box.cls)]
                if label not in COLLISION_CLASSES:
                    continue

                conf = float(box.conf)
                x1, y1, x2, y2 = map(float, box.xyxy[0])
                width_ratio = (x2 - x1) / w

                det = Detection(
                    label=label,
                    confidence=conf,
                    box=(x1, y1, x2, y2),
                    width_ratio=width_ratio,
                )
                result.detections.append(det)

                # Risk score = confidence × proximity (how much of frame it fills)
                risk = conf * min(width_ratio / COLLISION_THRESHOLD, 1.0)

                if width_ratio >= COLLISION_THRESHOLD:
                    result.collision_conf = max(result.collision_conf, risk)
                elif width_ratio >= NEAR_MISS_THRESHOLD:
                    result.near_miss_conf = max(result.near_miss_conf, risk)

        return result
