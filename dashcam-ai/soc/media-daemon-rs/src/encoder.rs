//! H.264 encoding via Rockchip MPP (Media Process Platform).
//!
//! MPP is Rockchip's hardware codec library. There is no stable Rust crate
//! for it yet, so we call it via a thin `unsafe` FFI block.
//! The encoder receives raw NV12 frames from the capture channel,
//! encodes them in hardware, and emits complete 1-minute segments.

use crate::capture::RawFrame;
use anyhow::{bail, Context, Result};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

// ── MPP FFI ───────────────────────────────────────────────────────────────────
// Link against librockchip_mpp.so from the RV1106 SDK.
// Add to build.rs: println!("cargo:rustc-link-lib=rockchip_mpp");

#[allow(non_camel_case_types, dead_code)]
mod mpp_ffi {
    use std::ffi::c_void;

    pub type MppCtx    = *mut c_void;
    pub type MppApi    = *mut c_void;
    pub type MppFrame  = *mut c_void;
    pub type MppPacket = *mut c_void;
    pub type MppBuffer = *mut c_void;

    pub const MPP_VIDEO_CodingAVC: u32 = 7;   /* H.264 */
    pub const MPP_ENC_RC_MODE_CBR: u32 = 1;   /* constant bitrate */

    extern "C" {
        pub fn mpp_create(ctx: *mut MppCtx, mpi: *mut MppApi) -> i32;
        pub fn mpp_init(ctx: MppCtx, codec_type: u32, coding: u32) -> i32;
        pub fn mpp_destroy(ctx: MppCtx) -> i32;

        pub fn mpp_frame_init(frame: *mut MppFrame) -> i32;
        pub fn mpp_frame_set_width(frame: MppFrame, width: u32);
        pub fn mpp_frame_set_height(frame: MppFrame, height: u32);
        pub fn mpp_frame_set_hor_stride(frame: MppFrame, stride: u32);
        pub fn mpp_frame_set_fmt(frame: MppFrame, fmt: u32);   /* MPP_FMT_YUV420SP = NV12 */
        pub fn mpp_frame_set_buffer(frame: MppFrame, buf: MppBuffer);

        pub fn mpp_packet_init_with_buffer(pkt: *mut MppPacket, buf: MppBuffer) -> i32;
        pub fn mpp_packet_get_pos(pkt: MppPacket) -> *mut u8;
        pub fn mpp_packet_get_length(pkt: MppPacket) -> usize;
        pub fn mpp_packet_deinit(pkt: *mut MppPacket) -> i32;
    }
}

// ── Encoded output ────────────────────────────────────────────────────────────

/// One complete loop-recording segment (~60s of H.264 NAL units).
pub struct EncodedSegment {
    pub data:        Vec<u8>,
    pub start_ts_us: u64,
    pub end_ts_us:   u64,
    pub frame_count: u32,
}

// ── Encoder state ─────────────────────────────────────────────────────────────

struct MppEncoder {
    ctx: mpp_ffi::MppCtx,
    #[allow(dead_code)]
    api: mpp_ffi::MppApi,
    /// Output buffer — grows segment-by-segment
    segment_buf:     Vec<u8>,
    segment_start:   u64,
    segment_frames:  u32,
    frames_per_seg:  u32,   /* = fps * segment_duration_secs */
}

impl MppEncoder {
    fn new(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Result<Self> {
        unsafe {
            let mut ctx: mpp_ffi::MppCtx = std::ptr::null_mut();
            let mut api: mpp_ffi::MppApi = std::ptr::null_mut();

            let ret = mpp_ffi::mpp_create(&mut ctx, &mut api);
            if ret != 0 { bail!("mpp_create failed: {ret}"); }

            // MPP_CTX_ENC = 1, MPP_VIDEO_CodingAVC = 7
            let ret = mpp_ffi::mpp_init(ctx, 1, mpp_ffi::MPP_VIDEO_CodingAVC);
            if ret != 0 { bail!("mpp_init failed: {ret}"); }

            // TODO: configure encoder params via MpiCmd::MPP_ENC_SET_CFG:
            //   - width, height, format (NV12)
            //   - fps, bitrate, rc_mode = CBR
            //   - gop = fps * 2 (keyframe every 2s)
            info!(
                "MPP encoder ready — {width}x{height} @ {fps}fps {bitrate_kbps}kbps H.264 CBR"
            );

            Ok(Self {
                ctx,
                api,
                segment_buf:    Vec::with_capacity(64 * 1024 * 1024),   // 64MB pre-alloc
                segment_start:  0,
                segment_frames: 0,
                frames_per_seg: fps * 60,   // 60-second segments
            })
        }
    }

    /// Encode one NV12 frame. Returns a completed segment when the
    /// segment duration has elapsed, otherwise returns None.
    fn encode_frame(&mut self, frame: &RawFrame, pixel_data: &[u8])
        -> Result<Option<EncodedSegment>>
    {
        if self.segment_frames == 0 {
            self.segment_start = frame.timestamp_us;
        }

        // TODO: wrap pixel_data in MppBuffer, set on MppFrame, call mpi->encode_put_frame()
        // then call mpi->encode_get_packet() in a loop until no more output:
        //
        //   let mut pkt = std::ptr::null_mut();
        //   while mpi.encode_get_packet(ctx, &mut pkt) == 0 && !pkt.is_null() {
        //       let ptr = mpp_ffi::mpp_packet_get_pos(pkt);
        //       let len = mpp_ffi::mpp_packet_get_length(pkt);
        //       let slice = std::slice::from_raw_parts(ptr, len);
        //       self.segment_buf.extend_from_slice(slice);
        //       mpp_ffi::mpp_packet_deinit(&mut pkt);
        //   }

        self.segment_frames += 1;
        debug!("encoded frame {} / {}", self.segment_frames, self.frames_per_seg);

        if self.segment_frames >= self.frames_per_seg {
            let seg = EncodedSegment {
                data:        std::mem::take(&mut self.segment_buf),
                start_ts_us: self.segment_start,
                end_ts_us:   frame.timestamp_us,
                frame_count: self.segment_frames,
            };
            self.segment_buf    = Vec::with_capacity(64 * 1024 * 1024);
            self.segment_frames = 0;
            return Ok(Some(seg));
        }
        Ok(None)
    }
}

impl Drop for MppEncoder {
    fn drop(&mut self) {
        unsafe { mpp_ffi::mpp_destroy(self.ctx); }
    }
}

// ── Run loop ──────────────────────────────────────────────────────────────────

pub fn run(
    frame_rx: Receiver<RawFrame>,
    seg_tx:   Sender<EncodedSegment>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut encoder = MppEncoder::new(
        1920, 1080,
        /*fps*/ 30,
        /*bitrate_kbps*/ 8_000,
    )?;

    info!("encoder thread started");

    while !shutdown.load(Ordering::Relaxed) {
        let frame = match frame_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(f)  => f,
            Err(_) => continue,   // timeout — check shutdown and retry
        };

        // TODO: read pixel data from shm ring slot
        // let pixel_data = shm.read_slot(frame.shm_slot);
        let pixel_data: &[u8] = &[];   // placeholder

        match encoder.encode_frame(&frame, pixel_data) {
            Ok(Some(segment)) => {
                info!(
                    "segment ready — {} frames, {:.1}MB",
                    segment.frame_count,
                    segment.data.len() as f64 / 1024.0 / 1024.0
                );
                if seg_tx.send(segment).is_err() {
                    warn!("loop_writer channel closed — exiting encoder");
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!("encode_frame error: {e:#}");
            }
        }
    }

    info!("encoder thread exiting");
    Ok(())
}
