//! POSIX shared memory ring buffer — producer side (media-daemon).
//!
//! ai-daemon opens the same shm region as a consumer and reads frames
//! from it without any copies. This is the zero-copy path for video.

use anyhow::{bail, Context, Result};
use nix::sys::mman::{mmap, munmap, shm_open, shm_unlink, MapFlags, ProtFlags};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;
use std::num::NonZeroUsize;
use std::os::unix::io::AsRawFd;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

// ── Layout ────────────────────────────────────────────────────────────────────

pub const SHM_NAME:       &str  = "/dashcam_video_ring";
pub const N_SLOTS:        usize = 8;
pub const FRAME_MAX_BYTES: usize = 1920 * 1080 * 3 / 2;   // NV12 at 1080p

/// Header at the start of the shared memory region.
#[repr(C)]
struct ShmHeader {
    write_idx: AtomicU32,
    read_idx:  AtomicU32,
}

/// One frame slot in the ring.
#[repr(C)]
pub struct ShmSlot {
    pub width:        u32,
    pub height:       u32,
    pub stride:       u32,
    pub size:         u32,
    pub timestamp_us: u64,
    pub data:         [u8; FRAME_MAX_BYTES],
}

const SHM_TOTAL: usize = std::mem::size_of::<ShmHeader>()
    + N_SLOTS * std::mem::size_of::<ShmSlot>();

// ── Producer ──────────────────────────────────────────────────────────────────

pub struct ShmRingProducer {
    ptr:  NonNull<u8>,
    size: usize,
}

// SAFETY: ShmRingProducer is Send — raw pointer is to a shared memory region
// managed by us and only written from the capture thread.
unsafe impl Send for ShmRingProducer {}
unsafe impl Sync for ShmRingProducer {}

impl ShmRingProducer {
    pub fn create() -> Result<Self> {
        // Remove stale shm from a previous run
        let _ = shm_unlink(SHM_NAME);

        let fd = shm_open(
            SHM_NAME,
            nix::fcntl::OFlag::O_CREAT | nix::fcntl::OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IWGRP,
        ).context("shm_open create")?;

        ftruncate(fd.as_raw_fd(), SHM_TOTAL as i64)
            .context("ftruncate shm")?;

        let ptr = unsafe {
            mmap(
                None,
                NonZeroUsize::new(SHM_TOTAL).unwrap(),
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            ).context("mmap shm producer")?
        };

        // Zero-initialise header
        unsafe {
            let hdr = ptr.as_ptr() as *mut ShmHeader;
            (*hdr).write_idx.store(0, Ordering::Relaxed);
            (*hdr).read_idx .store(0, Ordering::Relaxed);
        }

        Ok(Self { ptr, size: SHM_TOTAL })
    }

    fn header(&self) -> &ShmHeader {
        unsafe { &*(self.ptr.as_ptr() as *const ShmHeader) }
    }

    fn slot_ptr(&self, idx: usize) -> *mut ShmSlot {
        let base = self.ptr.as_ptr() as usize + std::mem::size_of::<ShmHeader>();
        (base + (idx % N_SLOTS) * std::mem::size_of::<ShmSlot>()) as *mut ShmSlot
    }

    /// Write a frame into the next ring slot.
    /// Returns the slot index, or None if the consumer is too far behind.
    pub fn write_frame(
        &self,
        data:         &[u8],
        width:        u32,
        height:       u32,
        stride:       u32,
        timestamp_us: u64,
    ) -> Option<usize> {
        let hdr  = self.header();
        let widx = hdr.write_idx.load(Ordering::Acquire) as usize;
        let ridx = hdr.read_idx .load(Ordering::Acquire) as usize;

        // Ring is full if we're N_SLOTS ahead of the reader
        if widx.wrapping_sub(ridx) >= N_SLOTS {
            return None;
        }

        let slot = unsafe { &mut *self.slot_ptr(widx) };
        let copy_len = data.len().min(FRAME_MAX_BYTES);

        slot.width        = width;
        slot.height       = height;
        slot.stride       = stride;
        slot.size         = copy_len as u32;
        slot.timestamp_us = timestamp_us;
        slot.data[..copy_len].copy_from_slice(&data[..copy_len]);

        // Publish the write — release ordering ensures slot data is visible
        hdr.write_idx.fetch_add(1, Ordering::Release);
        Some(widx % N_SLOTS)
    }
}

impl Drop for ShmRingProducer {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(self.ptr, self.size);
        }
        let _ = shm_unlink(SHM_NAME);
    }
}
