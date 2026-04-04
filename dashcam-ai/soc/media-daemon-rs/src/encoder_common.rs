//! Types shared between both encoder backends.

/// One complete loop-recording segment (~60s of H.264 NAL units).
pub struct EncodedSegment {
    pub data:        Vec<u8>,
    pub start_ts_us: u64,
    pub end_ts_us:   u64,
    pub frame_count: u32,
}
