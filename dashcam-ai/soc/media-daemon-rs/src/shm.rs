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
pub struct ShmHeader {
    pub write_idx: AtomicU32,
    pub read_idx:  AtomicU32,
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

    /// Expose the header for benchmark/test use — allows callers to simulate
    /// a consumer advancing the read index without a real consumer process.
    #[doc(hidden)]
    pub fn header_for_bench(&self) -> &ShmHeader {
        self.header()
    }

    /// Simulate a consumer reading one slot (advances read_idx by 1).
    /// For use in tests and benchmarks only.
    #[doc(hidden)]
    pub fn simulate_consumer_read(&self) {
        self.header()
            .read_idx
            .fetch_add(1, Ordering::Release);
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic NV12 frame of the given dimensions.
    fn fake_nv12(width: u32, height: u32) -> Vec<u8> {
        let luma  = (width * height) as usize;
        let chroma = luma / 2;
        let mut v = vec![0x80u8; luma];    // Y plane
        v.extend(vec![0x80u8; chroma]);    // UV plane
        v
    }

    // Helper: create a producer and ensure the shm name differs between
    // parallel test runs by using a unique name via std::process::id().
    // Since SHM_NAME is a constant, tests must be serialised — use
    // `-- --test-threads=1` or patch the name.  We rely on the Drop impl
    // cleaning up between tests.

    #[test]
    fn test_create_and_drop() {
        // Should not panic — creation and cleanup round-trip
        let p = ShmRingProducer::create().expect("create shm ring");
        drop(p);
        // After drop the shm name should be gone; a second create must succeed
        let p2 = ShmRingProducer::create().expect("re-create shm ring after drop");
        drop(p2);
    }

    #[test]
    fn test_write_single_frame_returns_slot_zero() {
        let p    = ShmRingProducer::create().expect("create");
        let data = fake_nv12(1920, 1080);

        let slot = p.write_frame(&data, 1920, 1080, 1920, 1_000_000);
        assert_eq!(slot, Some(0), "first write should land in slot 0");
        drop(p);
    }

    #[test]
    fn test_write_wraps_around_correctly() {
        let p = ShmRingProducer::create().expect("create");
        let data = fake_nv12(1920, 1080);

        // Write N_SLOTS frames — each should succeed
        for i in 0..N_SLOTS {
            let slot = p.write_frame(&data, 1920, 1080, 1920, i as u64 * 33_333);
            assert_eq!(slot, Some(i), "slot index should match iteration");
        }
        drop(p);
    }

    #[test]
    fn test_ring_full_returns_none() {
        let p    = ShmRingProducer::create().expect("create");
        let data = fake_nv12(1920, 1080);

        // Fill the ring without advancing read_idx
        for _ in 0..N_SLOTS {
            p.write_frame(&data, 1920, 1080, 1920, 0);
        }

        // One more write — ring is full, consumer hasn't read anything
        let result = p.write_frame(&data, 1920, 1080, 1920, 0);
        assert_eq!(result, None, "should refuse write when ring is full");
        drop(p);
    }

    #[test]
    fn test_slot_metadata_is_written_correctly() {
        let p    = ShmRingProducer::create().expect("create");
        let data = fake_nv12(1920, 1080);

        p.write_frame(&data, 1920, 1080, 1920, 42_000_000)
            .expect("write should succeed");

        // Read back metadata via slot_ptr
        let slot = unsafe { &*p.slot_ptr(0) };
        assert_eq!(slot.width,        1920);
        assert_eq!(slot.height,       1080);
        assert_eq!(slot.stride,       1920);
        assert_eq!(slot.timestamp_us, 42_000_000);
        assert_eq!(slot.size as usize, data.len());
        drop(p);
    }

    #[test]
    fn test_oversized_frame_is_clamped() {
        let p = ShmRingProducer::create().expect("create");
        // Frame larger than the slot max — must be clamped, not panic
        let huge = vec![0u8; FRAME_MAX_BYTES + 1024];

        let slot = p.write_frame(&huge, 1920, 1080, 1920, 0);
        assert_eq!(slot, Some(0));

        let s = unsafe { &*p.slot_ptr(0) };
        assert_eq!(s.size as usize, FRAME_MAX_BYTES, "size must be clamped");
        drop(p);
    }

    #[test]
    fn test_write_idx_monotonically_increases() {
        let p    = ShmRingProducer::create().expect("create");
        let data = fake_nv12(640, 480);

        for i in 0..4u32 {
            p.write_frame(&data, 640, 480, 640, i as u64 * 33_333);
            let widx = p.header().write_idx.load(Ordering::Acquire);
            assert_eq!(widx, i + 1, "write_idx should increment after each push");
        }
        drop(p);
    }

    #[test]
    fn test_slot_data_matches_input() {
        let p    = ShmRingProducer::create().expect("create");
        let mut data = fake_nv12(1920, 1080);
        // Stamp distinctive pattern at start and end
        data[0] = 0xDE;
        data[1] = 0xAD;
        *data.last_mut().unwrap() = 0xBE;

        p.write_frame(&data, 1920, 1080, 1920, 0);

        let slot = unsafe { &*p.slot_ptr(0) };
        assert_eq!(slot.data[0], 0xDE);
        assert_eq!(slot.data[1], 0xAD);
        assert_eq!(slot.data[data.len() - 1], 0xBE);
        drop(p);
    }
}
