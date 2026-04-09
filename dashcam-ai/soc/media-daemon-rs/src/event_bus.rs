//! Event bus client — async Unix domain socket pub/sub.
//!
//! Wire protocol (same as the C shared/event_bus.h):
//!   [u32 LE: event_type] [u64 LE: timestamp_us] [u32 LE: payload_len] [payload bytes (JSON)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::debug;

// ── Event types — mirrors events.h ───────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    // media-daemon
    VoiceActivityStart   = 0x01,
    VoiceActivityEnd     = 0x02,
    CollisionPrerollTag  = 0x03,

    // ai-daemon
    CollisionDetected    = 0x10,
    ObjectDetected       = 0x11,
    TranscriptReady      = 0x12,
    IntentClassified     = 0x13,
    WakeWordDetected     = 0x14,

    // cloud-daemon
    LteConnected         = 0x20,
    LteDisconnected      = 0x21,
    LlmResponseReady     = 0x22,
    UploadComplete       = 0x23,

    // power-daemon
    SystemDriving        = 0x30,
    SystemParked         = 0x31,
    SystemResumed        = 0x32,
    SuspendRequested     = 0x33,
    SuspendAck           = 0x34,
}

// ── Payload types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CollisionPayload {
    pub confidence: f32,
    pub clip_id:    u32,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct IntentPayload {
    pub transcript: String,
    pub lang:       String,
    pub intent:     u32,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WakePayload {
    pub reason: u32,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LlmPayload {
    pub response: String,
    pub lang:     String,
}

// ── Event envelope ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct DashcamEvent {
    pub event_type:   EventType,
    pub timestamp_us: u64,
    pub collision:    Option<CollisionPayload>,
    pub intent:       Option<IntentPayload>,
    pub wake:         Option<WakePayload>,
    pub llm:          Option<LlmPayload>,
}

impl DashcamEvent {
    pub fn suspend_ack() -> Self {
        Self {
            event_type:   EventType::SuspendAck,
            timestamp_us: now_us(),
            collision:    None,
            intent:       None,
            wake:         None,
            llm:          None,
        }
    }
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

// ── Wire encoding ─────────────────────────────────────────────────────────────

fn encode_event(evt: &DashcamEvent) -> Vec<u8> {
    let payload = match evt.event_type {
        EventType::CollisionDetected | EventType::CollisionPrerollTag =>
            serde_json::to_vec(&evt.collision).unwrap_or_default(),
        EventType::IntentClassified =>
            serde_json::to_vec(&evt.intent).unwrap_or_default(),
        EventType::WakeWordDetected =>
            serde_json::to_vec(&evt.wake).unwrap_or_default(),
        EventType::LlmResponseReady =>
            serde_json::to_vec(&evt.llm).unwrap_or_default(),
        _ => vec![],
    };

    let mut buf = Vec::with_capacity(16 + payload.len());
    buf.extend_from_slice(&(evt.event_type as u32).to_le_bytes());
    buf.extend_from_slice(&evt.timestamp_us.to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&payload);
    buf
}

async fn decode_event(stream: &mut UnixStream) -> Result<DashcamEvent> {
    let mut header = [0u8; 16];
    stream.read_exact(&mut header).await.context("read event header")?;

    let type_id      = u32::from_le_bytes(header[0..4].try_into().unwrap());
    let timestamp_us = u64::from_le_bytes(header[4..12].try_into().unwrap());
    let payload_len  = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;

    let mut payload_bytes = vec![0u8; payload_len];
    if payload_len > 0 {
        stream.read_exact(&mut payload_bytes).await.context("read event payload")?;
    }

    let event_type = match type_id {
        0x01 => EventType::VoiceActivityStart,
        0x03 => EventType::CollisionPrerollTag,
        0x10 => EventType::CollisionDetected,
        0x11 => EventType::ObjectDetected,
        0x12 => EventType::TranscriptReady,
        0x13 => EventType::IntentClassified,
        0x14 => EventType::WakeWordDetected,
        0x20 => EventType::LteConnected,
        0x21 => EventType::LteDisconnected,
        0x22 => EventType::LlmResponseReady,
        0x33 => EventType::SuspendRequested,
        0x34 => EventType::SuspendAck,
        _    => EventType::SystemDriving,   // fallback
    };

    let collision = if matches!(event_type, EventType::CollisionDetected | EventType::CollisionPrerollTag) {
        serde_json::from_slice(&payload_bytes).ok()
    } else { None };

    Ok(DashcamEvent { event_type, timestamp_us, collision, intent: None, wake: None, llm: None })
}

// ── EventBus client ───────────────────────────────────────────────────────────

pub struct EventBus {
    stream: UnixStream,
}

impl EventBus {
    pub async fn connect(path: &str) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .await
            .with_context(|| format!("connect to event bus at {path}"))?;
        Ok(Self { stream })
    }

    /// Subscribe to an event type — sends a subscribe message to the broker.
    pub async fn subscribe(&mut self, event_type: EventType) -> Result<()> {
        // Subscribe wire format: 0xFF marker + event_type u32
        let mut msg = [0u8; 5];
        msg[0] = 0xFF;
        msg[1..5].copy_from_slice(&(event_type as u32).to_le_bytes());
        self.stream.write_all(&msg).await.context("write subscribe")?;
        Ok(())
    }

    /// Publish an event to all subscribers.
    pub async fn publish(&mut self, evt: DashcamEvent) -> Result<()> {
        let encoded = encode_event(&evt);
        self.stream.write_all(&encoded).await.context("write event")?;
        self.stream.flush().await?;
        debug!("published {:?}", evt.event_type);
        Ok(())
    }

    /// Block until the next subscribed event arrives.
    pub async fn next_event(&mut self) -> Result<DashcamEvent> {
        decode_event(&mut self.stream).await
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;
    use std::os::unix::fs::PermissionsExt;

    // ── encode_event wire format ──────────────────────────────────────────────

    #[test]
    fn test_encode_event_header_layout() {
        let evt = DashcamEvent {
            event_type:   EventType::SuspendAck,
            timestamp_us: 0x0102_0304_0506_0708,
            collision:    None,
            intent:       None,
            wake:         None,
            llm:          None,
        };

        let buf = encode_event(&evt);

        // Bytes 0..4 — event_type as u32 LE
        let type_id = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        assert_eq!(type_id, EventType::SuspendAck as u32);

        // Bytes 4..12 — timestamp_us as u64 LE
        let ts = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        assert_eq!(ts, 0x0102_0304_0506_0708);

        // Bytes 12..16 — payload_len as u32 LE (SuspendAck has no payload)
        let payload_len = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        assert_eq!(payload_len, 0);
        assert_eq!(buf.len(), 16, "no-payload event must be exactly 16 bytes");
    }

    #[test]
    fn test_encode_collision_event_includes_payload() {
        let evt = DashcamEvent {
            event_type:   EventType::CollisionDetected,
            timestamp_us: 1_000_000,
            collision: Some(CollisionPayload { confidence: 0.95, clip_id: 7 }),
            intent: None, wake: None, llm: None,
        };

        let buf = encode_event(&evt);
        assert!(buf.len() > 16, "collision event must carry a JSON payload");

        let payload_len = u32::from_le_bytes(buf[12..16].try_into().unwrap()) as usize;
        assert_eq!(buf.len(), 16 + payload_len);

        // The payload must round-trip through serde
        let payload: CollisionPayload = serde_json::from_slice(&buf[16..]).unwrap();
        assert!((payload.confidence - 0.95_f32).abs() < 1e-4);
        assert_eq!(payload.clip_id, 7);
    }

    #[test]
    fn test_encode_intent_event_includes_payload() {
        let evt = DashcamEvent {
            event_type:   EventType::IntentClassified,
            timestamp_us: 2_000_000,
            collision:    None,
            intent: Some(IntentPayload {
                transcript: "navigate home".into(),
                lang:       "en".into(),
                intent:     3,
            }),
            wake: None, llm: None,
        };

        let buf = encode_event(&evt);
        let payload: IntentPayload = serde_json::from_slice(&buf[16..]).unwrap();
        assert_eq!(payload.transcript, "navigate home");
        assert_eq!(payload.lang, "en");
        assert_eq!(payload.intent, 3);
    }

    #[test]
    fn test_encode_llm_event_includes_payload() {
        let evt = DashcamEvent {
            event_type:   EventType::LlmResponseReady,
            timestamp_us: 3_000_000,
            collision:    None,
            intent:       None,
            wake:         None,
            llm: Some(LlmPayload {
                response: "Turn left in 200 metres".into(),
                lang:     "zh".into(),
            }),
        };

        let buf = encode_event(&evt);
        let payload: LlmPayload = serde_json::from_slice(&buf[16..]).unwrap();
        assert_eq!(payload.lang, "zh");
    }

    // ── encode / decode round-trip via in-process socket pair ─────────────────

    #[tokio::test]
    async fn test_encode_decode_round_trip_suspend_ack() {
        use tokio::io::AsyncWriteExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let sock_path = tmp.path().join("event_bus.sock");

        let listener = UnixListener::bind(&sock_path).unwrap();

        let path_clone = sock_path.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            decode_event(&mut stream).await
        });

        let mut client = tokio::net::UnixStream::connect(&path_clone).await.unwrap();
        let evt = DashcamEvent::suspend_ack();
        let encoded = encode_event(&evt);
        client.write_all(&encoded).await.unwrap();

        let decoded = server.await.unwrap().unwrap();
        assert_eq!(decoded.event_type, EventType::SuspendAck);
    }

    #[tokio::test]
    async fn test_encode_decode_round_trip_collision() {
        use tokio::io::AsyncWriteExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let sock_path = tmp.path().join("event_bus2.sock");

        let listener = UnixListener::bind(&sock_path).unwrap();

        let path_clone = sock_path.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            decode_event(&mut stream).await
        });

        let mut client = tokio::net::UnixStream::connect(&path_clone).await.unwrap();
        let evt = DashcamEvent {
            event_type:   EventType::CollisionDetected,
            timestamp_us: 99_000_000,
            collision: Some(CollisionPayload { confidence: 0.88, clip_id: 42 }),
            intent: None, wake: None, llm: None,
        };
        let encoded = encode_event(&evt);
        client.write_all(&encoded).await.unwrap();

        let decoded = server.await.unwrap().unwrap();
        assert_eq!(decoded.event_type, EventType::CollisionDetected);
        assert_eq!(decoded.timestamp_us, 99_000_000);
        let col = decoded.collision.unwrap();
        assert_eq!(col.clip_id, 42);
        assert!((col.confidence - 0.88_f32).abs() < 1e-4);
    }

    // ── Subscribe wire format ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_subscribe_sends_0xff_marker_and_event_type() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let tmp = tempfile::TempDir::new().unwrap();
        let sock_path = tmp.path().join("event_bus3.sock");

        let listener = UnixListener::bind(&sock_path).unwrap();

        let path_clone = sock_path.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            buf
        });

        // EventBus::subscribe sends [0xFF, event_type u32 LE]
        let stream = tokio::net::UnixStream::connect(&path_clone).await.unwrap();
        let mut bus = EventBus { stream };
        bus.subscribe(EventType::CollisionDetected).await.unwrap();

        let received = server.await.unwrap();
        assert_eq!(received[0], 0xFF, "subscribe marker must be 0xFF");
        let et = u32::from_le_bytes(received[1..5].try_into().unwrap());
        assert_eq!(et, EventType::CollisionDetected as u32);
    }

    // ── EventType values match the C header constants ─────────────────────────

    #[test]
    fn test_event_type_discriminants_match_c_header() {
        assert_eq!(EventType::VoiceActivityStart  as u32, 0x01);
        assert_eq!(EventType::VoiceActivityEnd    as u32, 0x02);
        assert_eq!(EventType::CollisionPrerollTag as u32, 0x03);
        assert_eq!(EventType::CollisionDetected   as u32, 0x10);
        assert_eq!(EventType::ObjectDetected      as u32, 0x11);
        assert_eq!(EventType::TranscriptReady     as u32, 0x12);
        assert_eq!(EventType::IntentClassified    as u32, 0x13);
        assert_eq!(EventType::WakeWordDetected    as u32, 0x14);
        assert_eq!(EventType::LteConnected        as u32, 0x20);
        assert_eq!(EventType::LteDisconnected     as u32, 0x21);
        assert_eq!(EventType::LlmResponseReady    as u32, 0x22);
        assert_eq!(EventType::UploadComplete      as u32, 0x23);
        assert_eq!(EventType::SystemDriving       as u32, 0x30);
        assert_eq!(EventType::SystemParked        as u32, 0x31);
        assert_eq!(EventType::SystemResumed       as u32, 0x32);
        assert_eq!(EventType::SuspendRequested    as u32, 0x33);
        assert_eq!(EventType::SuspendAck          as u32, 0x34);
    }

    // ── suspend_ack constructor ───────────────────────────────────────────────

    #[test]
    fn test_suspend_ack_has_no_payloads() {
        let evt = DashcamEvent::suspend_ack();
        assert_eq!(evt.event_type, EventType::SuspendAck);
        assert!(evt.collision.is_none());
        assert!(evt.intent.is_none());
        assert!(evt.wake.is_none());
        assert!(evt.llm.is_none());
        assert!(evt.timestamp_us > 0, "timestamp must be set");
    }

    // ── now_us monotonicity ───────────────────────────────────────────────────

    #[test]
    fn test_now_us_is_monotonic() {
        let t1 = now_us();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let t2 = now_us();
        assert!(t2 >= t1, "now_us must be non-decreasing");
    }
}
