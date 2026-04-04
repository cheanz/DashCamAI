//! ALSA audio capture with WebRTC VAD gating.
//!
//! Only voice-active frames are forwarded to ai-daemon over a Unix socket —
//! silence is dropped before it ever reaches the STT pipeline.

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tracing::{debug, info, warn};
use webrtc_vad::{Vad, VadMode, SampleRate};

// ALSA is a C library — we call it via the `alsa` crate
use alsa::{
    pcm::{Access, Format, HwParams, PCM},
    Direction, ValueOr,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const SAMPLE_RATE:    u32 = 16_000;   // Hz — Whisper tiny expects 16kHz
const CHANNELS:       u32 = 1;        // Mono
const FRAME_MS:       u32 = 30;       // VAD frame size (10 / 20 / 30ms supported)
const FRAME_SAMPLES:  u32 = SAMPLE_RATE * FRAME_MS / 1000;   // = 480 samples
const FRAMES_PER_CHUNK: usize = 34;   // ~1 second of audio per chunk sent to ai-daemon

/// Unix socket path where ai-daemon's audio receiver listens.
const AI_DAEMON_AUDIO_SOCK: &str = "/var/run/dashcam/ai_audio.sock";

// ── ALSA setup ────────────────────────────────────────────────────────────────

fn open_alsa(device: &str) -> Result<PCM> {
    let pcm = PCM::new(device, Direction::Capture, false)
        .with_context(|| format!("open ALSA device {device}"))?;

    let hwp = HwParams::any(&pcm).context("alloc HwParams")?;
    hwp.set_channels(CHANNELS).context("set channels")?;
    hwp.set_rate(SAMPLE_RATE, ValueOr::Nearest).context("set sample rate")?;
    hwp.set_format(Format::s16()).context("set S16_LE format")?;
    hwp.set_access(Access::RWInterleaved).context("set interleaved access")?;
    // Buffer = 4 frames, period = 1 frame — low latency capture
    hwp.set_buffer_size(4 * FRAME_SAMPLES as i64).context("set buffer size")?;
    hwp.set_period_size(FRAME_SAMPLES as i64, ValueOr::Nearest).context("set period size")?;
    pcm.hw_params(&hwp).context("apply HwParams")?;

    pcm.start().context("start PCM capture")?;
    info!("ALSA capture: {device} @ {SAMPLE_RATE}Hz mono S16_LE");
    Ok(pcm)
}

// ── Capture + VAD loop ────────────────────────────────────────────────────────

pub fn run(
    device:   &str,
    chunk_tx: Sender<Vec<i16>>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let pcm = open_alsa(device)?;
    let io  = pcm.io_i16().context("get S16 IO")?;

    let mut vad = Vad::new_with_rate_and_mode(
        SampleRate::Rate16kHz,
        VadMode::Quality,   // Quality = lowest false-positive rate
    );

    let mut frame_buf  = vec![0i16; FRAME_SAMPLES as usize];
    let mut chunk_buf: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES as usize * FRAMES_PER_CHUNK);
    let mut voice_frames = 0usize;
    let mut total_frames = 0usize;

    info!("audio capture loop started");

    while !shutdown.load(Ordering::Relaxed) {
        // Read exactly one VAD frame from ALSA (blocking)
        let n = io.readi(&mut frame_buf).context("ALSA readi")?;
        if n != FRAME_SAMPLES as usize {
            warn!("short ALSA read: {n} / {FRAME_SAMPLES}");
            continue;
        }

        total_frames += 1;

        // VAD decision — is this frame voice-active?
        let is_voice = vad.is_voice_segment(&frame_buf)
            .unwrap_or(false);

        if is_voice {
            voice_frames += 1;
            chunk_buf.extend_from_slice(&frame_buf);
        }

        // Emit a chunk to ai-daemon when we have ~1s of voice audio
        if chunk_buf.len() >= FRAME_SAMPLES as usize * FRAMES_PER_CHUNK {
            debug!(
                "audio chunk ready — {voice_frames}/{total_frames} voice frames",
            );
            let chunk = std::mem::take(&mut chunk_buf);
            chunk_buf.reserve(FRAME_SAMPLES as usize * FRAMES_PER_CHUNK);

            if chunk_tx.try_send(chunk).is_err() {
                warn!("audio chunk channel full — dropping");
            }

            voice_frames = 0;
            total_frames = 0;
        }
    }

    info!("audio capture loop exiting");
    Ok(())
}

// ── Async chunk forwarder — runs inside tokio ─────────────────────────────────

/// Receives VAD-gated audio chunks from the capture thread and sends them
/// to ai-daemon via a Unix domain socket.
///
/// Wire format (length-prefixed):
///   [u32 little-endian: n_samples] [n_samples × i16 little-endian samples]
pub async fn forward_chunks(chunk_rx: Receiver<Vec<i16>>) -> Result<()> {
    let mut stream = UnixStream::connect(AI_DAEMON_AUDIO_SOCK).await
        .with_context(|| format!("connect to {AI_DAEMON_AUDIO_SOCK}"))?;

    info!("audio forwarder connected to ai-daemon");

    loop {
        // Check for a new chunk without blocking the async runtime
        let chunk = tokio::task::spawn_blocking({
            let rx = chunk_rx.clone();
            move || rx.recv_timeout(std::time::Duration::from_millis(200))
        })
        .await?;

        let chunk = match chunk {
            Ok(c)  => c,
            Err(_) => continue,   // timeout — poll again
        };

        // Serialize: 4-byte length header + raw i16 samples
        let n = chunk.len() as u32;
        stream.write_all(&n.to_le_bytes()).await.context("write length")?;

        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 2)
        };
        stream.write_all(bytes).await.context("write samples")?;
        stream.flush().await?;

        debug!("forwarded {n} audio samples to ai-daemon");
    }
}
