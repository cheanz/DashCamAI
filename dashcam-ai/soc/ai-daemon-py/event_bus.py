"""
Event bus client — matches the Rust/C wire protocol exactly.

Wire format (per event):
  [u32 LE: event_type] [u64 LE: timestamp_us] [u32 LE: payload_len] [JSON payload]
"""

import asyncio
import json
import struct
import time
import logging

log = logging.getLogger(__name__)

# ── Event type constants — mirrors event_bus.rs and events.h ─────────────────

class EventType:
    VOICE_ACTIVITY_START  = 0x01
    VOICE_ACTIVITY_END    = 0x02
    COLLISION_PREROLL_TAG = 0x03

    COLLISION_DETECTED    = 0x10
    OBJECT_DETECTED       = 0x11
    TRANSCRIPT_READY      = 0x12
    INTENT_CLASSIFIED     = 0x13
    WAKE_WORD_DETECTED    = 0x14

    LTE_CONNECTED         = 0x20
    LTE_DISCONNECTED      = 0x21
    LLM_RESPONSE_READY    = 0x22
    UPLOAD_COMPLETE       = 0x23

    SYSTEM_DRIVING        = 0x30
    SYSTEM_PARKED         = 0x31
    SYSTEM_RESUMED        = 0x32
    SUSPEND_REQUESTED     = 0x33
    SUSPEND_ACK           = 0x34


def now_us() -> int:
    return int(time.time() * 1_000_000)


def encode_event(event_type: int, payload: dict | None = None) -> bytes:
    """Encode an event to the wire format."""
    payload_bytes = json.dumps(payload).encode() if payload else b""
    header = struct.pack("<IQI", event_type, now_us(), len(payload_bytes))
    return header + payload_bytes


# ── EventBus client ───────────────────────────────────────────────────────────

class EventBus:
    """Async Unix socket event bus client."""

    SOCK_PATH = "/var/run/dashcam/event_bus.sock"

    def __init__(self, sock_path: str = SOCK_PATH):
        self.sock_path = sock_path
        self._reader: asyncio.StreamReader | None = None
        self._writer: asyncio.StreamWriter | None = None
        self._connected = False

    async def connect(self) -> bool:
        try:
            self._reader, self._writer = await asyncio.open_unix_connection(
                self.sock_path
            )
            self._connected = True
            log.info(f"EventBus connected to {self.sock_path}")
            return True
        except (FileNotFoundError, ConnectionRefusedError) as e:
            log.warning(f"EventBus not available ({e}) — running in offline mode")
            self._connected = False
            return False

    async def publish(self, event_type: int, payload: dict | None = None) -> bool:
        if not self._connected:
            log.debug(f"EventBus offline — dropped event {event_type:#x}")
            return False
        try:
            data = encode_event(event_type, payload)
            self._writer.write(data)
            await self._writer.drain()
            log.debug(f"published event {event_type:#x}")
            return True
        except Exception as e:
            log.error(f"EventBus publish error: {e}")
            self._connected = False
            return False

    async def publish_collision(self, confidence: float, clip_id: int) -> bool:
        return await self.publish(
            EventType.COLLISION_DETECTED,
            {"confidence": round(confidence, 4), "clip_id": clip_id},
        )

    async def publish_object_detected(self, label: str, confidence: float) -> bool:
        return await self.publish(
            EventType.OBJECT_DETECTED,
            {"label": label, "confidence": round(confidence, 4)},
        )

    def close(self):
        if self._writer:
            self._writer.close()
        self._connected = False
