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
    fn new(fps: u32) -> Self {
        Self {
            buf:            Vec::with_capacity(64 * 1024 * 1024),
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
            let seg = EncodedSegment {
                data:        std::mem::take(&mut self.buf),
                start_ts_us: self.start_ts,
                end_ts_us:   frame_ts,
                frame_count: self.frame_count,
            };
            self.buf         = Vec::with_capacity(64 * 1024 * 1024);
            self.frame_count = 0;
            return Some(seg);
        }
        None
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
