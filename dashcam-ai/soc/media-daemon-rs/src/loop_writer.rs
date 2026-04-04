//! Segment rotation writer — persists encoded H.264 segments to eMMC
//! and handles loop eviction, pre-roll tagging, and collision clip preservation.

use crate::encoder::EncodedSegment;
use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

// ── Globals shared with event handler (called from tokio task) ────────────────

static TAGGED_CLIP_ID: AtomicU32 = AtomicU32::new(0);
static FLUSH_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Called by the event bus handler when EVT_COLLISION_DETECTED fires.
/// Thread-safe — uses atomics only.
pub fn tag_preroll(clip_id: u32) {
    TAGGED_CLIP_ID.store(clip_id, Ordering::Relaxed);
}

/// Called on EVT_SUSPEND_REQUESTED — signals the writer to checkpoint.
pub fn flush_and_checkpoint() {
    FLUSH_REQUESTED.store(true, Ordering::Relaxed);
}

// ── Config ────────────────────────────────────────────────────────────────────

pub struct Config {
    /// Directory on eMMC where loop segments are written.
    pub root_dir:       PathBuf,
    /// Seconds per segment (must match encoder's `frames_per_seg / fps`).
    pub segment_secs:   u64,
    /// Maximum loop storage in GB — oldest segments evicted when exceeded.
    pub max_storage_gb: u64,
    /// Pre-roll duration to preserve on collision (seconds).
    pub pre_roll_secs:  u64,
}

// ── Segment metadata ──────────────────────────────────────────────────────────

#[derive(Debug)]
struct SegmentMeta {
    path:       PathBuf,
    size_bytes: u64,
    start_ts:   u64,
    end_ts:     u64,
}

// ── Writer state ──────────────────────────────────────────────────────────────

struct LoopWriter {
    cfg:           Config,
    /// Ring of written segments, oldest first.
    written:       VecDeque<SegmentMeta>,
    total_bytes:   u64,
    max_bytes:     u64,
    segment_idx:   u64,
}

impl LoopWriter {
    fn new(cfg: Config) -> Result<Self> {
        fs::create_dir_all(&cfg.root_dir)
            .with_context(|| format!("create loop dir {:?}", cfg.root_dir))?;

        let max_bytes = cfg.max_storage_gb * 1024 * 1024 * 1024;
        Ok(Self {
            cfg,
            written: VecDeque::new(),
            total_bytes: 0,
            max_bytes,
            segment_idx: 0,
        })
    }

    fn write_segment(&mut self, seg: EncodedSegment) -> Result<()> {
        let fname = format!(
            "seg_{:08}_{}.h264",
            self.segment_idx, seg.start_ts_us
        );
        let path = self.cfg.root_dir.join(&fname);

        let mut f = fs::File::create(&path)
            .with_context(|| format!("create segment {path:?}"))?;
        f.write_all(&seg.data)
            .with_context(|| format!("write segment {path:?}"))?;
        f.sync_data()?;   // fsync data — important before power loss

        let size = seg.data.len() as u64;
        self.total_bytes += size;
        self.written.push_back(SegmentMeta {
            path,
            size_bytes: size,
            start_ts:   seg.start_ts_us,
            end_ts:     seg.end_ts_us,
        });

        info!(
            "wrote segment {} — {:.1}MB — total {:.1}GB / {:.1}GB",
            fname,
            size as f64 / 1024.0 / 1024.0,
            self.total_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
            self.max_bytes  as f64 / 1024.0 / 1024.0 / 1024.0,
        );

        self.segment_idx += 1;
        self.evict_if_needed()?;
        self.check_collision_tag(seg.start_ts_us)?;

        Ok(())
    }

    /// Evict oldest segments until total storage is under the limit.
    fn evict_if_needed(&mut self) -> Result<()> {
        while self.total_bytes > self.max_bytes {
            if let Some(oldest) = self.written.pop_front() {
                warn!("evicting loop segment {:?}", oldest.path);
                fs::remove_file(&oldest.path)
                    .with_context(|| format!("remove {:?}", oldest.path))?;
                self.total_bytes -= oldest.size_bytes;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// If a collision clip was tagged, copy pre-roll segments to the
    /// evidence directory. storage-daemon will pick them up from there.
    fn check_collision_tag(&self, current_ts: u64) -> Result<()> {
        let clip_id = TAGGED_CLIP_ID.swap(0, Ordering::Relaxed);
        if clip_id == 0 { return Ok(()); }

        let pre_roll_us = self.cfg.pre_roll_secs * 1_000_000;
        let cutoff_ts   = current_ts.saturating_sub(pre_roll_us);

        let evidence_dir = self.cfg.root_dir
            .parent().unwrap_or(Path::new("/mnt/emmc"))
            .join("evidence")
            .join(format!("clip_{clip_id:08}"));
        fs::create_dir_all(&evidence_dir)?;

        for seg in self.written.iter().filter(|s| s.end_ts >= cutoff_ts) {
            let dest = evidence_dir.join(seg.path.file_name().unwrap());
            fs::copy(&seg.path, &dest)
                .with_context(|| format!("copy preroll {:?} → {:?}", seg.path, dest))?;
            info!("preserved preroll segment {:?} for clip {clip_id}", dest);
        }
        Ok(())
    }
}

// ── Run loop ──────────────────────────────────────────────────────────────────

pub fn run(
    seg_rx:   Receiver<EncodedSegment>,
    cfg:      Config,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut writer = LoopWriter::new(cfg)?;
    info!("loop-writer thread started");

    while !shutdown.load(Ordering::Relaxed) {
        if FLUSH_REQUESTED.swap(false, Ordering::Relaxed) {
            info!("flush requested — all segments fsynced");
            // Segments are already fsynced per-write; nothing extra needed
        }

        match seg_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(seg) => {
                if let Err(e) = writer.write_segment(seg) {
                    warn!("write_segment error: {e:#}");
                }
            }
            Err(_) => {}   // timeout — check shutdown and retry
        }
    }

    info!("loop-writer thread exiting");
    Ok(())
}
