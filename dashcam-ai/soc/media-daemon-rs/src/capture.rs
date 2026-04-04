//! V4L2 camera capture using the `v4l` crate.
//!
//! Reads NV12 frames from the Sony IMX sensor via the RV1106 ISP pipeline,
//! writes each frame to the shared memory ring buffer (for ai-daemon),
//! and sends a lightweight `RawFrame` handle down the channel to the encoder.

use crate::shm::ShmRingProducer;
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};
use v4l::{
    buffer::Type,
    format::fourcc::FourCC,
    io::mmap::Stream,
    io::traits::CaptureStream,
    video::Capture,
    Device, Format,
};

// ── Types ─────────────────────────────────────────────────────────────────────

/// Lightweight frame descriptor sent to the encoder thread.
/// The actual pixel data lives in the shared memory ring slot.
#[derive(Debug)]
pub struct RawFrame {
    pub width:        u32,
    pub height:       u32,
    pub stride:       u32,
    pub size:         usize,
    pub timestamp_us: u64,
    /// Index of the shm ring slot holding the raw pixel data.
    /// ai-daemon reads from the same slot via the consumer API.
    pub shm_slot:     usize,
}

pub struct CaptureConfig {
    pub width:  u32,
    pub height: u32,
}

// ── V4L2 format negotiation ───────────────────────────────────────────────────

fn negotiate_format(dev: &Device, cfg: &CaptureConfig) -> Result<Format> {
    // Request NV12 (YUV 4:2:0 semi-planar) — native output of RV1106 ISP
    let mut fmt = dev.format().context("get current format")?;
    fmt.width  = cfg.width;
    fmt.height = cfg.height;
    fmt.fourcc = FourCC::new(b"NV12");

    let negotiated = dev.set_format(&fmt).context("set NV12 format")?;
    info!(
        "capture format: {}x{} {:?}",
        negotiated.width, negotiated.height, negotiated.fourcc
    );

    if negotiated.fourcc != FourCC::new(b"NV12") {
        warn!(
            "driver returned {:?} instead of NV12 — encoder may need conversion",
            negotiated.fourcc
        );
    }
    Ok(negotiated)
}

// ── Frame timestamp ───────────────────────────────────────────────────────────

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

// ── Capture loop ──────────────────────────────────────────────────────────────

pub fn run(
    device_path: &str,
    cfg: CaptureConfig,
    frame_tx: Sender<RawFrame>,
    shm: Arc<ShmRingProducer>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    info!("opening capture device {device_path}");
    let dev = Device::with_path(device_path)
        .with_context(|| format!("open {device_path}"))?;

    let fmt = negotiate_format(&dev, &cfg)?;

    // Memory-mapped streaming — zero-copy between kernel and userspace
    let mut stream = Stream::with_buffers(&dev, Type::VideoCapture, 4)
        .context("create mmap stream")?;

    info!("capture loop started — {}x{}", fmt.width, fmt.height);

    while !shutdown.load(Ordering::Relaxed) {
        // Block until the next frame is ready (typically < 33ms at 30fps)
        let (buf, meta) = stream.next().context("dequeue frame")?;

        let ts_us = meta
            .timestamp
            .map(|t| t.sec as u64 * 1_000_000 + t.usec as u64)
            .unwrap_or_else(now_us);

        // Write frame into the shm ring — ai-daemon reads from here
        let slot = match shm.write_frame(buf, fmt.width, fmt.height, fmt.stride, ts_us) {
            Some(s) => s,
            None => {
                // ai-daemon is falling behind — drop this frame rather than block
                warn!("shm ring full — dropping frame at {ts_us}");
                continue;
            }
        };

        let raw = RawFrame {
            width:        fmt.width,
            height:       fmt.height,
            stride:       fmt.stride,
            size:         buf.len(),
            timestamp_us: ts_us,
            shm_slot:     slot,
        };

        // Send to encoder — bounded channel; drop frame if encoder is stalled
        if frame_tx.try_send(raw).is_err() {
            debug!("encoder channel full — dropping frame");
        }
    }

    info!("capture loop exiting");
    Ok(())
}
