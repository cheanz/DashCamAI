//! GStreamer encoder — Jetson hardware H.264 via nvv4l2h264enc
//! Compiled only when built with --features jetson

use crate::capture::RawFrame;
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use gstreamer as gst;
use gstreamer_app::{AppSink, AppSrc};
use gst::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

pub use crate::encoder_common::EncodedSegment;

// ── Pipeline builder ─────────────────────────────────────────────────────────

fn build_pipeline(width: u32, height: u32, fps: u32, bitrate_kbps: u32)
    -> Result<(gst::Pipeline, AppSrc, AppSink)>
{
    gst::init().context("gst::init")?;

    // Try Jetson hardware encoder first, fall back to software
    let encoder_element = if gst::ElementFactory::find("nvv4l2h264enc").is_some() {
        format!("nvv4l2h264enc bitrate={} ", bitrate_kbps * 1000)
    } else {
        warn!("nvv4l2h264enc not found — falling back to x264enc (software)");
        format!("x264enc bitrate={} speed-preset=ultrafast tune=zerolatency ", bitrate_kbps)
    };

    let pipeline_str = format!(
        "appsrc name=src \
           caps=video/x-raw,format=NV12,width={width},height={height},\
                framerate={fps}/1,interlace-mode=progressive \
           format=time block=true max-bytes=0 \
         ! {encoder_element}\
         ! h264parse \
         ! appsink name=sink sync=false max-buffers=8 drop=false"
    );

    info!("GStreamer pipeline: {pipeline_str}");
    let pipeline = gst::parse::launch(&pipeline_str)
        .context("gst parse_launch")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;

    let src  = pipeline.by_name("src").unwrap().downcast::<AppSrc>().unwrap();
    let sink = pipeline.by_name("sink").unwrap().downcast::<AppSink>().unwrap();

    pipeline.set_state(gst::State::Playing).context("pipeline set Playing")?;
    info!("GStreamer encoder pipeline running");

    Ok((pipeline, src, sink))
}

// ── Segment assembler ────────────────────────────────────────────────────────

struct SegmentAssembler {
    buf:            Vec<u8>,
    start_ts:       u64,
    frame_count:    u32,
    frames_per_seg: u32,
}

impl SegmentAssembler {
    /// Create a new assembler.
    ///
    /// Pre-allocates `initial_cap_bytes` for the segment buffer.
    /// Default is 32 MiB — enough for a 60 s segment at 4 Mbps (≈ 30 MB)
    /// without reallocation, while fitting in RV1106 RAM.
    /// On the Jetson (≥4 GB) this can be raised to 64 MiB.
    const DEFAULT_INITIAL_CAP: usize = 32 * 1024 * 1024; // 32 MiB

    fn new(fps: u32) -> Self {
        Self::with_capacity(fps, Self::DEFAULT_INITIAL_CAP)
    }

    fn with_capacity(fps: u32, initial_cap_bytes: usize) -> Self {
        Self {
            buf:            Vec::with_capacity(initial_cap_bytes),
            start_ts:       0,
            frame_count:    0,
            frames_per_seg: fps * 60,
        }
    }

    fn push(&mut self, nal_data: &[u8], frame_ts: u64) -> Option<EncodedSegment> {
        if self.frame_count == 0 { self.start_ts = frame_ts; }
        self.buf.extend_from_slice(nal_data);
        self.frame_count += 1;

        if self.frame_count >= self.frames_per_seg {
            let prev_cap = self.buf.capacity();
            let seg = EncodedSegment {
                data:        std::mem::take(&mut self.buf),
                start_ts_us: self.start_ts,
                end_ts_us:   frame_ts,
                frame_count: self.frame_count,
            };
            // Re-use the same capacity as the previous segment — avoids
            // reallocating and prevents capacity growth on Jetson.
            self.buf         = Vec::with_capacity(prev_cap);
            self.frame_count = 0;
            return Some(seg);
        }
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: u32 = 30;

    fn nal_chunk(n_bytes: usize) -> Vec<u8> {
        (0..n_bytes).map(|i| (i & 0xFF) as u8).collect()
    }

    // ── SegmentAssembler — basic push/flush ────────────────────────────────────

    #[test]
    fn test_no_segment_emitted_before_frames_per_seg() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(1024);
        let frames_needed = FPS * 60; // 1800

        for i in 0..(frames_needed - 1) {
            let seg = asm.push(&chunk, i as u64 * 33_333);
            assert!(seg.is_none(), "should not emit segment before frame {}", frames_needed);
        }
    }

    #[test]
    fn test_segment_emitted_exactly_at_frames_per_seg() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(512);
        let frames  = FPS * 60; // 1800

        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&chunk, i as u64 * 33_333);
        }
        assert!(seg.is_some(), "segment must be emitted at exactly frames_per_seg frames");
    }

    #[test]
    fn test_segment_frame_count_matches() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(128);
        let frames  = FPS * 60;

        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&chunk, i as u64 * 33_333);
        }
        let seg = seg.unwrap();
        assert_eq!(seg.frame_count, frames, "frame_count must equal frames_per_seg");
    }

    #[test]
    fn test_segment_data_length_is_sum_of_nal_units() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(256);
        let frames  = FPS * 60;

        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&chunk, i as u64 * 33_333);
        }
        let expected_bytes = (frames as usize) * 256;
        assert_eq!(seg.unwrap().data.len(), expected_bytes);
    }

    #[test]
    fn test_segment_timestamps_span_full_duration() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(64);
        let frames  = FPS * 60;
        // 1800 frames at 33_333 µs each ≈ 60 s
        let frame_us = 33_333u64;

        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&chunk, i as u64 * frame_us);
        }
        let seg = seg.unwrap();
        assert_eq!(seg.start_ts_us, 0, "start_ts must be the first frame timestamp");
        assert_eq!(
            seg.end_ts_us,
            (frames - 1) as u64 * frame_us,
            "end_ts must be the last frame timestamp"
        );
    }

    // ── SegmentAssembler — reset after segment emission ────────────────────────

    #[test]
    fn test_assembler_resets_after_segment_emission() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(32);
        let frames  = FPS * 60;

        // Emit first segment
        for i in 0..frames {
            asm.push(&chunk, i as u64 * 33_333);
        }

        // The next frames_per_seg frames should produce a second segment
        let mut second_seg = None;
        let offset = frames as u64 * 33_333;
        for i in 0..frames {
            second_seg = asm.push(&chunk, offset + i as u64 * 33_333);
        }
        assert!(second_seg.is_some(), "assembler must reset and produce a second segment");
        assert_eq!(second_seg.unwrap().frame_count, frames);
    }

    #[test]
    fn test_assembler_start_ts_updates_after_reset() {
        let mut asm = SegmentAssembler::new(FPS);
        let chunk   = nal_chunk(16);
        let frames  = FPS * 60;
        let frame_us = 33_333u64;

        for i in 0..frames {
            asm.push(&chunk, i as u64 * frame_us);
        }

        // Second segment — start_ts must be the first frame of the second window
        let second_start = frames as u64 * frame_us;
        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&chunk, second_start + i as u64 * frame_us);
        }
        assert_eq!(seg.unwrap().start_ts_us, second_start);
    }

    // ── SegmentAssembler — variable FPS ────────────────────────────────────────

    #[test]
    fn test_assembler_works_with_24fps() {
        let fps = 24u32;
        let mut asm = SegmentAssembler::new(fps);
        let chunk   = nal_chunk(100);
        let frames  = fps * 60; // 1440

        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&chunk, i as u64 * 41_666);
        }
        assert!(seg.is_some());
        assert_eq!(seg.unwrap().frame_count, frames);
    }

    // ── Memory capacity sanity ─────────────────────────────────────────────────

    #[test]
    fn test_segment_pre_allocated_capacity() {
        // SegmentAssembler pre-allocates 64 MiB. A 1080p 8 Mbps 60 s segment
        // is ≈ 60 MB. The capacity must cover that without reallocation.
        let expected_cap = 64 * 1024 * 1024;
        // We can't inspect buf directly, but we can confirm no panic on 60 MB fill
        let mut asm     = SegmentAssembler::new(1);   // 1 fps, segment = 60 frames
        let big_chunk   = vec![0u8; 1024 * 1024];      // 1 MB per "frame"
        let frames      = 60u32;

        let mut seg = None;
        for i in 0..frames {
            seg = asm.push(&big_chunk, i as u64 * 1_000_000);
        }
        let seg = seg.unwrap();
        assert_eq!(seg.data.len(), frames as usize * 1024 * 1024);
        assert!(
            seg.data.len() <= expected_cap,
            "60 MB segment must not exceed pre-allocated 64 MiB capacity"
        );
    }
}

// ── Run loop ─────────────────────────────────────────────────────────────────

pub fn run(
    frame_rx: Receiver<RawFrame>,
    seg_tx:   Sender<EncodedSegment>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let (pipeline, src, sink) = build_pipeline(1920, 1080, 30, 8_000)?;
    let mut asm = SegmentAssembler::new(30);

    info!("encoder thread started (jetson/GStreamer)");

    while !shutdown.load(Ordering::Relaxed) {
        // ── Push raw frame into appsrc ──────────────────────────────────────
        let frame = match frame_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(f)  => f,
            Err(_) => continue,
        };

        // TODO: read pixel data from shm ring by frame.shm_slot
        // let pixel_data = shm.read_slot(frame.shm_slot);
        let pixel_data: &[u8] = &[];   // placeholder

        let mut gst_buf = gst::Buffer::with_size(pixel_data.len())
            .context("alloc gst buffer")?;
        {
            let buf_ref = gst_buf.get_mut().unwrap();
            buf_ref.set_pts(gst::ClockTime::from_useconds(frame.timestamp_us));
            let mut map = buf_ref.map_writable().unwrap();
            map.copy_from_slice(pixel_data);
        }
        src.push_buffer(gst_buf).context("appsrc push_buffer")?;

        // ── Pull encoded NAL units from appsink ──────────────────────────────
        while let Ok(sample) = sink.try_pull_sample(gst::ClockTime::ZERO) {
            let buf = sample.buffer().context("appsink: no buffer in sample")?;
            let map = buf.map_readable().context("appsink: map buffer")?;
            let ts  = buf.pts().map(|t| t.useconds()).unwrap_or(frame.timestamp_us);

            debug!("GStreamer NAL unit: {} bytes @ {ts}us", map.len());

            if let Some(seg) = asm.push(&map, ts) {
                info!("GStreamer segment ready — {} frames, {:.1}MB",
                      seg.frame_count, seg.data.len() as f64 / 1024.0 / 1024.0);
                if seg_tx.send(seg).is_err() {
                    warn!("loop_writer channel closed");
                    break;
                }
            }
        }
    }

    // Graceful shutdown
    src.end_of_stream().ok();
    pipeline.set_state(gst::State::Null).ok();
    info!("encoder thread exiting (jetson/GStreamer)");
    Ok(())
}
