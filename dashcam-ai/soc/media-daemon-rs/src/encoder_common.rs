//! Types shared between both encoder backends.

/// One complete loop-recording segment (~60s of H.264 NAL units).
pub struct EncodedSegment {
    pub data:        Vec<u8>,
    pub start_ts_us: u64,
    pub end_ts_us:   u64,
    pub frame_count: u32,
}

impl EncodedSegment {
    /// Duration of the segment in microseconds.
    pub fn duration_us(&self) -> u64 {
        self.end_ts_us.saturating_sub(self.start_ts_us)
    }

    /// Size of the encoded data in bytes.
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }

    /// Average bitrate in bits per second.
    /// Returns 0 if duration is zero.
    pub fn avg_bitrate_bps(&self) -> u64 {
        let dur_us = self.duration_us();
        if dur_us == 0 { return 0; }
        // bits = bytes * 8 ; convert µs → s: * 1_000_000
        (self.data.len() as u64).saturating_mul(8).saturating_mul(1_000_000) / dur_us
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_seg(data_len: usize, start_us: u64, end_us: u64, frames: u32) -> EncodedSegment {
        EncodedSegment {
            data:        vec![0u8; data_len],
            start_ts_us: start_us,
            end_ts_us:   end_us,
            frame_count: frames,
        }
    }

    // ── EncodedSegment field correctness ──────────────────────────────────────

    #[test]
    fn test_segment_stores_fields_correctly() {
        let seg = make_seg(1024, 1_000_000, 61_000_000, 1800);
        assert_eq!(seg.data.len(),    1024);
        assert_eq!(seg.start_ts_us,  1_000_000);
        assert_eq!(seg.end_ts_us,    61_000_000);
        assert_eq!(seg.frame_count,  1800);
    }

    // ── duration_us ───────────────────────────────────────────────────────────

    #[test]
    fn test_duration_us_normal() {
        let seg = make_seg(0, 1_000_000, 61_000_000, 0);
        assert_eq!(seg.duration_us(), 60_000_000, "60 seconds in µs");
    }

    #[test]
    fn test_duration_us_zero_when_equal() {
        let seg = make_seg(0, 42, 42, 0);
        assert_eq!(seg.duration_us(), 0);
    }

    #[test]
    fn test_duration_us_saturates_on_underflow() {
        // end < start — saturating_sub should return 0, not wrap
        let seg = make_seg(0, 100, 50, 0);
        assert_eq!(seg.duration_us(), 0);
    }

    // ── size_bytes ────────────────────────────────────────────────────────────

    #[test]
    fn test_size_bytes_matches_data_len() {
        let seg = make_seg(999, 0, 1, 0);
        assert_eq!(seg.size_bytes(), 999);
    }

    #[test]
    fn test_size_bytes_zero_for_empty_segment() {
        let seg = make_seg(0, 0, 1, 0);
        assert_eq!(seg.size_bytes(), 0);
    }

    // ── avg_bitrate_bps ───────────────────────────────────────────────────────

    #[test]
    fn test_avg_bitrate_typical_1080p() {
        // 8 Mbps @ 60 s → 60 MB segment
        let bytes_60s = 60 * 1_000_000u64; // 60 MB (simplify: 1 byte ≈ 8 Mbps / 8)
        let seg = make_seg(bytes_60s as usize, 0, 60_000_000, 1800);
        let bps = seg.avg_bitrate_bps();
        // 60_000_000 bytes * 8 / 60_000_000 µs * 1_000_000 = 8_000_000 bps
        assert_eq!(bps, 8_000_000, "should be exactly 8 Mbps");
    }

    #[test]
    fn test_avg_bitrate_zero_duration() {
        let seg = make_seg(1024, 5, 5, 0);
        assert_eq!(seg.avg_bitrate_bps(), 0, "must not divide by zero");
    }

    #[test]
    fn test_avg_bitrate_zero_data() {
        let seg = make_seg(0, 0, 60_000_000, 0);
        assert_eq!(seg.avg_bitrate_bps(), 0);
    }

    // ── Memory size sanity ────────────────────────────────────────────────────

    #[test]
    fn test_typical_segment_fits_in_rv1106_ram() {
        // At 4 Mbps / 60 s, a segment is ~30 MB.
        // RV1106 has ~256 MB RAM; we cap ENCODED_SEG_CAP at 4 → max 120 MB in flight.
        let bytes_per_seg = 4_000_000u64 * 60 / 8; // 30 MB
        assert!(
            bytes_per_seg < 64 * 1024 * 1024,
            "at 4 Mbps a 60 s segment should be under 64 MB"
        );
        // Cap: 4 segments in flight
        assert!(
            bytes_per_seg * 4 < 256 * 1024 * 1024,
            "4 segments at 4 Mbps must fit within 256 MB"
        );
    }
}
