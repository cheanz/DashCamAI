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
