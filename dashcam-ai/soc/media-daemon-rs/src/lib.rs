//! media-daemon library — exposes internal modules for benchmarks and integration tests.
//!
//! The binary entry point is in `main.rs`.  This lib re-exports the same
//! module tree so that `[[bench]]` targets and `tests/` crates can import
//! from `media_daemon::shm`, `media_daemon::event_bus`, etc.

pub mod capture;
pub mod encoder_common;
#[cfg(feature = "jetson")]   pub mod encoder_gstreamer;
#[cfg(feature = "rockchip")] pub mod encoder_mpp;
pub mod encoder;
pub mod loop_writer;
pub mod audio;
pub mod shm;
pub mod event_bus;
