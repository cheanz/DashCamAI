//! Encoder — thin feature-flag dispatcher.
//!
//! Selects the backend at compile time:
//!   --features jetson   → encoder_gstreamer (nvv4l2h264enc)
//!   --features rockchip → encoder_mpp       (Rockchip MPP)

#[cfg(not(any(feature = "jetson", feature = "rockchip")))]
compile_error!(
    "No encoder backend selected.\n\
     Build with one of:\n\
       cargo build --features jetson    # Jetson bare Linux\n\
       cargo build --features rockchip  # RV1106 cross-compile"
);

#[cfg(all(feature = "jetson", feature = "rockchip"))]
compile_error!("Cannot enable both 'jetson' and 'rockchip' features at the same time.");

// ── Re-export the active backend as `encoder` ─────────────────────────────────

#[cfg(feature = "jetson")]
pub use crate::encoder_gstreamer::*;

#[cfg(feature = "rockchip")]
pub use crate::encoder_mpp::*;
