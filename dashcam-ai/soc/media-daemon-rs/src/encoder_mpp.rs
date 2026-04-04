//! Rockchip MPP encoder — RV1106 hardware H.264 via librockchip_mpp.so
//! Compiled only when built with --features rockchip

use crate::capture::RawFrame;
use anyhow::{bail, Result};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

pub use crate::encoder_common::EncodedSegment;

// ── MPP FFI (links against librockchip_mpp.so) ───────────────────────────────
// Add to build.rs: println!("cargo:rustc-link-lib=rockchip_mpp");

#[allow(non_camel_case_types, dead_code)]
mod ffi {
    use std::ffi::c_void;
    pub type MppCtx    = *mut c_void;
    pub type MppApi    = *mut c_void;
    pub type MppFrame  = *mut c_void;
    pub type MppPacket = *mut c_void;
    pub type MppBuffer = *mut c_void;
    pub const MPP_CTX_ENC: u32          = 1;
    pub const MPP_VIDEO_CodingAVC: u32  = 7;

    extern "C" {
        pub fn mpp_create(ctx: *mut MppCtx, mpi: *mut MppApi) -> i32;
        pub fn mpp_init(ctx: MppCtx, t: u32, coding: u32) -> i32;
        pub fn mpp_destroy(ctx: MppCtx) -> i32;
        pub fn mpp_frame_init(frame: *mut MppFrame) -> i32;
        pub fn mpp_frame_set_width(frame: MppFrame, w: u32);
        pub fn mpp_frame_set_height(frame: MppFrame, h: u32);
        pub fn mpp_frame_set_hor_stride(frame: MppFrame, s: u32);
        pub fn mpp_frame_set_fmt(frame: MppFrame, fmt: u32);
        pub fn mpp_frame_set_buffer(frame: MppFrame, buf: MppBuffer);
        pub fn mpp_packet_get_pos(pkt: MppPacket) -> *mut u8;
        pub fn mpp_packet_get_length(pkt: MppPacket) -> usize;
        pub fn mpp_packet_deinit(pkt: *mut MppPacket) -> i32;
    }
}

struct MppEncoder {
    ctx:            ffi::MppCtx,
    segment_buf:    Vec<u8>,
    segment_start:  u64,
    segment_frames: u32,
    frames_per_seg: u32,
}

impl MppEncoder {
    fn new(fps: u32, bitrate_kbps: u32) -> Result<Self> {
        unsafe {
            let mut ctx: ffi::MppCtx = std::ptr::null_mut();
            let mut api: ffi::MppApi = std::ptr::null_mut();
            let ret = ffi::mpp_create(&mut ctx, &mut api);
            if ret != 0 { bail!("mpp_create failed: {ret}"); }
            let ret = ffi::mpp_init(ctx, ffi::MPP_CTX_ENC, ffi::MPP_VIDEO_CodingAVC);
            if ret != 0 { bail!("mpp_init failed: {ret}"); }
            // TODO: configure bitrate, fps, GOP via MpiCmd::MPP_ENC_SET_CFG
            info!("MPP encoder ready — {fps}fps {bitrate_kbps}kbps H.264");
            Ok(Self {
                ctx,
                segment_buf:    Vec::with_capacity(64 * 1024 * 1024),
                segment_start:  0,
                segment_frames: 0,
                frames_per_seg: fps * 60,
            })
        }
    }

    fn encode_frame(&mut self, frame: &RawFrame, pixel_data: &[u8]) -> Result<Option<EncodedSegment>> {
        if self.segment_frames == 0 { self.segment_start = frame.timestamp_us; }

        // TODO: wrap pixel_data in MppBuffer → MppFrame → mpi->encode_put_frame()
        // then drain mpi->encode_get_packet() into self.segment_buf

        self.segment_frames += 1;
        debug!("MPP encoded frame {}/{}", self.segment_frames, self.frames_per_seg);

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
        unsafe { ffi::mpp_destroy(self.ctx); }
    }
}

pub fn run(
    frame_rx: Receiver<RawFrame>,
    seg_tx:   Sender<EncodedSegment>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let mut enc = MppEncoder::new(30, 8_000)?;
    info!("encoder thread started (rockchip/MPP)");

    while !shutdown.load(Ordering::Relaxed) {
        let frame = match frame_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(f)  => f,
            Err(_) => continue,
        };
        let pixel_data: &[u8] = &[];   // TODO: read from shm ring slot
        match enc.encode_frame(&frame, pixel_data) {
            Ok(Some(seg)) => {
                info!("MPP segment ready — {} frames", seg.frame_count);
                if seg_tx.send(seg).is_err() { break; }
            }
            Ok(None) => {}
            Err(e) => warn!("MPP encode error: {e:#}"),
        }
    }

    info!("encoder thread exiting (rockchip/MPP)");
    Ok(())
}
