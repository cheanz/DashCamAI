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
pub struct SegmentMeta {
    pub path:       PathBuf,
    pub size_bytes: u64,
    pub start_ts:   u64,
    pub end_ts:     u64,
}

// ── Writer state ──────────────────────────────────────────────────────────────

pub struct LoopWriter {
    cfg:           Config,
    /// Ring of written segments, oldest first.
    pub written:     VecDeque<SegmentMeta>,
    pub total_bytes: u64,
    pub max_bytes:   u64,
    segment_idx:   u64,
}

impl LoopWriter {
    pub fn new(cfg: Config) -> Result<Self> {
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

    pub fn write_segment(&mut self, seg: EncodedSegment) -> Result<()> {
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
    pub fn evict_if_needed(&mut self) -> Result<()> {
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
    pub fn check_collision_tag(&self, current_ts: u64) -> Result<()> {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::EncodedSegment;
    use crossbeam_channel::bounded;
    use std::sync::Arc;
    use tempfile::TempDir;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn fake_segment(size: usize, start_us: u64, end_us: u64) -> EncodedSegment {
        EncodedSegment {
            data:        vec![0xAAu8; size],
            start_ts_us: start_us,
            end_ts_us:   end_us,
            frame_count: 1800,
        }
    }

    fn test_config(root: &TempDir) -> Config {
        Config {
            root_dir:       root.path().join("loop"),
            segment_secs:   60,
            max_storage_gb: 1,       // 1 GB limit — generous for unit tests
            pre_roll_secs:  30,
        }
    }

    // ── LoopWriter creation ───────────────────────────────────────────────────

    #[test]
    fn test_creates_root_dir_on_init() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp);
        let dir = cfg.root_dir.clone();

        LoopWriter::new(cfg).unwrap();
        assert!(dir.exists(), "root_dir must be created");
    }

    // ── write_segment — basic persistence ────────────────────────────────────

    #[test]
    fn test_write_segment_creates_file_on_disk() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();

        let seg = fake_segment(1024, 0, 60_000_000);
        writer.write_segment(seg).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path().join("loop"))
            .unwrap()
            .collect();
        assert_eq!(entries.len(), 1, "exactly one file written");
    }

    #[test]
    fn test_write_segment_file_content_matches() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();

        let mut data = vec![0u8; 512];
        data[0] = 0xFF; data[511] = 0xEE;
        let seg = EncodedSegment { data: data.clone(), start_ts_us: 0, end_ts_us: 1, frame_count: 1 };
        writer.write_segment(seg).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path().join("loop"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let on_disk = std::fs::read(&entries[0].path()).unwrap();
        assert_eq!(on_disk[0], 0xFF);
        assert_eq!(on_disk[511], 0xEE);
    }

    #[test]
    fn test_segment_index_increments_monotonically() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();

        for i in 0..5u64 {
            writer.write_segment(fake_segment(64, i * 60_000_000, (i + 1) * 60_000_000)).unwrap();
        }

        let mut names: Vec<String> = std::fs::read_dir(tmp.path().join("loop"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();

        for (i, name) in names.iter().enumerate() {
            assert!(
                name.starts_with(&format!("seg_{:08}", i)),
                "segment {i} must have monotonic filename prefix, got {name}"
            );
        }
    }

    #[test]
    fn test_total_bytes_tracks_written_data() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();

        writer.write_segment(fake_segment(1000, 0, 1)).unwrap();
        assert_eq!(writer.total_bytes, 1000);

        writer.write_segment(fake_segment(2000, 1, 2)).unwrap();
        assert_eq!(writer.total_bytes, 3000);
    }

    // ── evict_if_needed ───────────────────────────────────────────────────────

    #[test]
    fn test_eviction_removes_oldest_segment_when_over_limit() {
        let tmp = TempDir::new().unwrap();
        let mut cfg = test_config(&tmp);
        // Set a tiny 1-byte limit to force eviction after the first segment
        cfg.max_storage_gb = 0;  // 0 GB → max_bytes = 0 → evict immediately
        let mut writer = LoopWriter::new(cfg).unwrap();

        writer.write_segment(fake_segment(100, 0, 1)).unwrap();
        // After write+evict, the segment should be gone
        assert_eq!(writer.written.len(), 0, "segment must be evicted");
        assert_eq!(writer.total_bytes, 0);
    }

    #[test]
    fn test_eviction_keeps_newest_when_over_limit() {
        let tmp = TempDir::new().unwrap();
        // Allow exactly 200 bytes — first two segments of 100 bytes each fit,
        // writing the third (100 bytes) should evict the oldest.
        let mut cfg = test_config(&tmp);
        cfg.max_storage_gb = 0; // will be overridden below — patch max_bytes via a helper
        let mut writer = LoopWriter::new(cfg).unwrap();
        // Force max_bytes to 250 — fits 2 × 100 byte segments, evicts on 3rd
        writer.max_bytes = 250;

        writer.write_segment(fake_segment(100, 0,   60_000_000)).unwrap(); // seg 0
        writer.write_segment(fake_segment(100, 1,   61_000_000)).unwrap(); // seg 1
        writer.write_segment(fake_segment(100, 2,   62_000_000)).unwrap(); // seg 2 → evicts seg 0

        assert_eq!(writer.written.len(), 2, "only 2 segments should remain");
        // Oldest remaining must be seg 1 (start_ts = 1)
        assert_eq!(writer.written.front().unwrap().start_ts, 1);
    }

    #[test]
    fn test_eviction_does_not_underflow_total_bytes() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();
        writer.max_bytes = 0; // force evict everything

        for i in 0..10u64 {
            writer.write_segment(fake_segment(512, i, i + 1)).unwrap();
        }
        assert_eq!(writer.total_bytes, 0, "total_bytes must not underflow");
    }

    // ── tag_preroll / check_collision_tag ────────────────────────────────────

    #[test]
    fn test_tag_preroll_is_zero_initially() {
        // Reset the global before testing
        TAGGED_CLIP_ID.store(0, Ordering::Relaxed);
        let clip = TAGGED_CLIP_ID.load(Ordering::Relaxed);
        assert_eq!(clip, 0);
    }

    #[test]
    fn test_tag_preroll_stores_clip_id() {
        tag_preroll(99);
        assert_eq!(TAGGED_CLIP_ID.load(Ordering::Relaxed), 99);
        TAGGED_CLIP_ID.store(0, Ordering::Relaxed); // cleanup
    }

    #[test]
    fn test_collision_tag_copies_preroll_segments() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();

        // Write 3 segments spanning 0..180 s
        for i in 0..3u64 {
            writer.write_segment(fake_segment(
                512,
                i * 60_000_000,
                (i + 1) * 60_000_000,
            )).unwrap();
        }

        // Tag clip 5 for the last segment (current_ts = 120_000_000)
        TAGGED_CLIP_ID.store(5, Ordering::Relaxed);
        writer.check_collision_tag(180_000_000).unwrap();

        // Evidence directory for clip 5 must exist and contain files
        let evidence = tmp.path().join("evidence").join("clip_00000005");
        assert!(evidence.exists(), "evidence dir must be created");
        let files: Vec<_> = std::fs::read_dir(&evidence)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!files.is_empty(), "at least one preroll segment must be preserved");
    }

    #[test]
    fn test_collision_tag_clears_after_processing() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();
        writer.write_segment(fake_segment(64, 0, 60_000_000)).unwrap();

        TAGGED_CLIP_ID.store(7, Ordering::Relaxed);
        writer.check_collision_tag(60_000_000).unwrap();

        // The global must be zero again
        assert_eq!(TAGGED_CLIP_ID.load(Ordering::Relaxed), 0, "clip_id must be cleared after processing");
    }

    #[test]
    fn test_no_evidence_copy_when_tag_is_zero() {
        let tmp = TempDir::new().unwrap();
        let mut writer = LoopWriter::new(test_config(&tmp)).unwrap();
        writer.write_segment(fake_segment(64, 0, 60_000_000)).unwrap();

        TAGGED_CLIP_ID.store(0, Ordering::Relaxed);
        writer.check_collision_tag(60_000_000).unwrap();

        let evidence = tmp.path().join("evidence");
        assert!(!evidence.exists(), "no evidence dir without a collision tag");
    }

    // ── flush_and_checkpoint global ───────────────────────────────────────────

    #[test]
    fn test_flush_requested_atomic() {
        FLUSH_REQUESTED.store(false, Ordering::Relaxed);
        flush_and_checkpoint();
        assert!(FLUSH_REQUESTED.load(Ordering::Relaxed));
        FLUSH_REQUESTED.store(false, Ordering::Relaxed); // cleanup
    }

    // ── run() integration via channel ─────────────────────────────────────────

    #[test]
    fn test_run_writes_segment_and_exits_cleanly() {
        let tmp = TempDir::new().unwrap();
        let cfg = Config {
            root_dir:       tmp.path().join("loop"),
            segment_secs:   60,
            max_storage_gb: 1,
            pre_roll_secs:  30,
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx)  = bounded(4);

        // Send one segment then signal shutdown
        tx.send(fake_segment(1024, 0, 60_000_000)).unwrap();
        drop(tx);  // close the sender — recv_timeout will drain then get Disconnected

        // Shutdown after short delay
        let sd = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            sd.store(true, Ordering::Relaxed);
        });

        run(rx, cfg, shutdown).expect("run must exit cleanly");

        let files: Vec<_> = std::fs::read_dir(tmp.path().join("loop"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "one segment must be on disk after run()");
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
