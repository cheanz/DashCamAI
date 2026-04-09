"""
Frame sources — pluggable backends for the ai-daemon.

Three sources are supported:
  CarlaSource   — live frames from a running CARLA simulation
  VideoSource   — pre-recorded video file (MP4 / AVI / etc.)
  WebcamSource  — USB or built-in camera via V4L2

All sources implement the same iterator protocol:
  for frame_bgr in source:
      ...   # frame_bgr is a numpy BGR array (H × W × 3, uint8)
"""

import logging
import time
import numpy as np
import cv2

log = logging.getLogger(__name__)


# ── CARLA source ──────────────────────────────────────────────────────────────

class CarlaSource:
    """
    Connects to a running CARLA server, spawns a camera sensor on the
    ego vehicle, and yields BGR frames in real time.

    CARLA server must be running before this is created.
    Default assumes CARLA on localhost:2000.
    """

    def __init__(
        self,
        host: str = "localhost",
        port: int = 2000,
        width: int = 1920,
        height: int = 1080,
        fps: int = 30,
        fov: float = 90.0,
    ):
        self.host   = host
        self.port   = port
        self.width  = width
        self.height = height
        self.fps    = fps
        self.fov    = fov
        self._frame_queue: list[np.ndarray] = []
        self._world  = None
        self._camera = None
        self._vehicle = None

    def __enter__(self):
        import carla  # type: ignore  # installed separately

        log.info(f"connecting to CARLA at {self.host}:{self.port}")
        client = carla.Client(self.host, self.port)
        client.set_timeout(10.0)
        self._world = client.get_world()

        # Use the first available vehicle actor as the ego vehicle,
        # or spawn one if none exists.
        vehicles = self._world.get_actors().filter("vehicle.*")
        if vehicles:
            self._vehicle = vehicles[0]
            log.info(f"attaching to existing vehicle {self._vehicle.id}")
        else:
            bp_lib    = self._world.get_blueprint_library()
            vehicle_bp = bp_lib.find("vehicle.tesla.model3")
            spawn_pts  = self._world.get_map().get_spawn_points()
            self._vehicle = self._world.spawn_actor(vehicle_bp, spawn_pts[0])
            self._vehicle.set_autopilot(True)
            log.info(f"spawned vehicle {self._vehicle.id} with autopilot")

        # Attach RGB camera sensor
        bp_lib    = self._world.get_blueprint_library()
        camera_bp = bp_lib.find("sensor.camera.rgb")
        camera_bp.set_attribute("image_size_x", str(self.width))
        camera_bp.set_attribute("image_size_y", str(self.height))
        camera_bp.set_attribute("fov",          str(self.fov))
        camera_bp.set_attribute("sensor_tick",  str(1.0 / self.fps))

        import carla
        transform = carla.Transform(
            carla.Location(x=1.5, z=2.4),   # front of vehicle, 2.4m up
            carla.Rotation(pitch=-15.0),     # slight downward angle
        )
        self._camera = self._world.spawn_actor(
            camera_bp, transform, attach_to=self._vehicle
        )

        def _on_frame(image):
            array = np.frombuffer(image.raw_data, dtype=np.uint8)
            array = array.reshape((image.height, image.width, 4))   # BGRA
            bgr   = array[:, :, :3]                                  # drop alpha
            self._frame_queue.append(bgr.copy())
            if len(self._frame_queue) > 4:
                self._frame_queue.pop(0)   # drop oldest if we're falling behind

        self._camera.listen(_on_frame)
        log.info("CARLA camera sensor active")
        return self

    def __iter__(self):
        frame_interval = 1.0 / self.fps
        while True:
            t0 = time.perf_counter()
            if self._frame_queue:
                yield self._frame_queue.pop(0)
            else:
                time.sleep(0.001)   # brief spin until next frame arrives
                continue
            elapsed = time.perf_counter() - t0
            remaining = frame_interval - elapsed
            if remaining > 0:
                time.sleep(remaining)

    def __exit__(self, *_):
        if self._camera:
            self._camera.stop()
            self._camera.destroy()
        log.info("CARLA camera destroyed")


# ── Video file source ─────────────────────────────────────────────────────────

class VideoSource:
    """
    Reads frames from a video file (MP4, AVI, etc.) at real-time speed.
    Loops the video — useful for repeatable pitch demos.
    """

    def __init__(self, path: str, loop: bool = True, target_fps: float = 30.0):
        self.path       = path
        self.loop       = loop
        self.target_fps = target_fps

    def __iter__(self):
        cap = cv2.VideoCapture(self.path)
        if not cap.isOpened():
            raise RuntimeError(f"Cannot open video file: {self.path}")

        src_fps  = cap.get(cv2.CAP_PROP_FPS) or self.target_fps
        interval = 1.0 / src_fps

        log.info(f"VideoSource: {self.path} @ {src_fps:.1f} fps")

        try:
            while True:
                t0 = time.perf_counter()
                ok, frame = cap.read()
                if not ok:
                    if self.loop:
                        cap.set(cv2.CAP_PROP_POS_FRAMES, 0)
                        continue
                    break
                yield frame
                elapsed = time.perf_counter() - t0
                sleep   = interval - elapsed
                if sleep > 0:
                    time.sleep(sleep)
        finally:
            cap.release()


# ── Webcam source ─────────────────────────────────────────────────────────────

class WebcamSource:
    """Live webcam or V4L2 device."""

    def __init__(self, device: int | str = 0, width: int = 1920, height: int = 1080):
        self.device = device
        self.width  = width
        self.height = height

    def __iter__(self):
        cap = cv2.VideoCapture(self.device)
        cap.set(cv2.CAP_PROP_FRAME_WIDTH,  self.width)
        cap.set(cv2.CAP_PROP_FRAME_HEIGHT, self.height)
        if not cap.isOpened():
            raise RuntimeError(f"Cannot open camera: {self.device}")
        log.info(f"WebcamSource: device {self.device}")
        try:
            while True:
                ok, frame = cap.read()
                if not ok:
                    log.warning("webcam read failed — retrying")
                    time.sleep(0.033)
                    continue
                yield frame
        finally:
            cap.release()
