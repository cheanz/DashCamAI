// Modules are declared in lib.rs; the binary imports them via `crate::`.
use crate::capture;
use crate::encoder;
use crate::loop_writer;
use crate::audio;
use crate::shm;
use crate::event_bus::{EventBus, DashcamEvent, EventType};

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use tokio::runtime::Runtime;
use tracing::{info, error};

use capture::RawFrame;
use encoder::EncodedSegment;

// ── Channel capacities ────────────────────────────────────────────────────────

/// Raw frame ring — camera → encoder + shm writer.
/// Bounded to avoid unbounded memory growth if the encoder falls behind.
const RAW_FRAME_CAP: usize  = 4;

/// Encoded segment ring — encoder → loop writer.
///
/// Memory budget (RV1106, ~256 MB RAM):
///   At 4 Mbps / 60 s each segment is ≈ 30 MB.
///   ENCODED_SEG_CAP = 4 → 4 × 30 MB = 120 MB peak in-flight — safe.
///
///   The previous value of 8 (× 64 MB pre-alloc = 512 MB) exceeded the
///   RV1106's physical RAM.  On the Jetson (≥4 GB) 8 is fine, but we
///   keep the conservative value for both targets.
const ENCODED_SEG_CAP: usize = 4;   // was 8 — reduced to fit RV1106 RAM budget

/// Audio chunk ring — ALSA → ai-daemon socket sender.
const AUDIO_CHUNK_CAP: usize = 16;

fn main() -> Result<()> {
    // Structured logging — level controlled by RUST_LOG env var
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("media_daemon=info".parse()?)
        )
        .init();

    info!("media-daemon starting");

    // Shared shutdown flag — set on SIGTERM / SIGINT
    let shutdown = Arc::new(AtomicBool::new(false));
    install_signal_handler(Arc::clone(&shutdown));

    // ── Channels ──────────────────────────────────────────────────────────────
    let (raw_tx,  raw_rx):  (Sender<RawFrame>,       Receiver<RawFrame>)       = bounded(RAW_FRAME_CAP);
    let (enc_tx,  enc_rx):  (Sender<EncodedSegment>, Receiver<EncodedSegment>) = bounded(ENCODED_SEG_CAP);
    let (audio_tx, audio_rx) = bounded::<Vec<i16>>(AUDIO_CHUNK_CAP);

    // ── Shared memory ring (producer side) ───────────────────────────────────
    let shm_ring = Arc::new(shm::ShmRingProducer::create()?);
    let shm_ring_clone = Arc::clone(&shm_ring);

    // ── Spawn OS threads ─────────────────────────────────────────────────────

    // 1. Camera capture thread — V4L2 blocking read loop
    let raw_tx2    = raw_tx.clone();
    let sd_capture = Arc::clone(&shutdown);
    let capture_handle = thread::Builder::new()
        .name("capture".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            if let Err(e) = capture::run(
                "/dev/video0",
                capture::CaptureConfig { width: 1920, height: 1080 },
                raw_tx2,
                shm_ring_clone,
                sd_capture,
            ) {
                error!("capture thread error: {e:#}");
            }
        })?;

    // 2. Encoder thread — raw NV12 → H.264 via Rockchip MPP or GStreamer
    let sd_encoder = Arc::clone(&shutdown);
    let encoder_handle = thread::Builder::new()
        .name("encoder".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Err(e) = encoder::run(raw_rx, enc_tx, sd_encoder) {
                error!("encoder thread error: {e:#}");
            }
        })?;

    // 3. Loop writer thread — segment rotation on eMMC
    let sd_writer = Arc::clone(&shutdown);
    let writer_handle = thread::Builder::new()
        .name("loop-writer".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Err(e) = loop_writer::run(
                enc_rx,
                loop_writer::Config {
                    root_dir:        "/mnt/emmc/loop".into(),
                    segment_secs:    60,
                    max_storage_gb:  28,
                    pre_roll_secs:   30,
                },
                sd_writer,
            ) {
                error!("loop-writer thread error: {e:#}");
            }
        })?;

    // 4. Audio thread — ALSA capture + VAD gating
    let sd_audio = Arc::clone(&shutdown);
    let audio_handle = thread::Builder::new()
        .name("audio".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Err(e) = audio::run("hw:0,0", audio_tx, sd_audio) {
                error!("audio thread error: {e:#}");
            }
        })?;

    // 5. Tokio runtime — async tasks: event bus + audio chunk forwarding
    let rt = Runtime::new()?;
    rt.block_on(async move {
        let mut bus = EventBus::connect("/var/run/dashcam/event_bus.sock").await?;

        // Subscribe to collision events so we can tag the pre-roll buffer
        bus.subscribe(EventType::CollisionDetected).await?;
        bus.subscribe(EventType::CollisionPrerollTag).await?;

        // Forward VAD-gated audio chunks to ai-daemon over a Unix socket
        let audio_fwd = tokio::spawn(async move {
            audio::forward_chunks(audio_rx).await
        });

        // Event dispatch loop
        loop {
            tokio::select! {
                event = bus.next_event() => {
                    match event {
                        Ok(DashcamEvent { event_type: EventType::CollisionDetected, collision, .. }) => {
                            let clip_id = collision.map(|c| c.clip_id).unwrap_or(0);
                            loop_writer::tag_preroll(clip_id);
                            info!("collision detected — clip {clip_id} tagged for preservation");
                        }
                        Ok(DashcamEvent { event_type: EventType::SuspendRequested, .. }) => {
                            info!("suspend requested — flushing and checkpointing");
                            loop_writer::flush_and_checkpoint();
                            bus.publish(DashcamEvent::suspend_ack()).await.ok();
                            break;
                        }
                        Ok(_) => {}
                        Err(e) => { error!("event bus error: {e}"); break; }
                    }
                }
            }
        }

        audio_fwd.abort();
        Ok::<(), anyhow::Error>(())
    })?;

    // ── Shutdown ──────────────────────────────────────────────────────────────
    shutdown.store(true, Ordering::Relaxed);
    let _ = capture_handle.join();
    let _ = encoder_handle.join();
    let _ = writer_handle.join();
    let _ = audio_handle.join();

    info!("media-daemon exited cleanly");
    Ok(())
}

fn install_signal_handler(shutdown: Arc<AtomicBool>) {
    // SIGTERM / SIGINT → set shutdown flag via sigprocmask + sigwait
    thread::spawn(move || {
        use nix::sys::signal::{self, Signal};
        let mut mask = signal::SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGINT);
        signal::sigprocmask(signal::SigmaskHow::SIG_BLOCK, Some(&mask), None).ok();
        loop {
            if let Ok(sig) = mask.wait() {
                tracing::warn!("received signal {:?} — shutting down", sig);
                shutdown.store(true, Ordering::Relaxed);
                break;
            }
        }
    });
}
