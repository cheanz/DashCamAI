//! Criterion benchmarks for EncodedSegment helper methods.
//!
//! Run with:
//!   cargo bench --bench encoder_common_bench --features jetson
//!
//! These are lightweight — the primary purpose is to confirm that helper
//! methods (duration_us, avg_bitrate_bps) are optimised away by the compiler
//! and add zero runtime overhead on the hot encoder path.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use media_daemon::encoder_common::EncodedSegment;

fn make_seg(n_bytes: usize) -> EncodedSegment {
    EncodedSegment {
        data:        vec![0u8; n_bytes],
        start_ts_us: 0,
        end_ts_us:   60_000_000,   // 60 s
        frame_count: 1800,
    }
}

fn bench_duration_us(c: &mut Criterion) {
    let seg = make_seg(0);
    c.bench_function("encoded_segment_duration_us", |b| {
        b.iter(|| black_box(seg.duration_us()))
    });
}

fn bench_avg_bitrate_bps(c: &mut Criterion) {
    // 30 MB at 60 s ≈ 4 Mbps
    let seg = make_seg(30 * 1024 * 1024);
    c.bench_function("encoded_segment_avg_bitrate_bps", |b| {
        b.iter(|| black_box(seg.avg_bitrate_bps()))
    });
}

fn bench_size_bytes(c: &mut Criterion) {
    let seg = make_seg(30 * 1024 * 1024);
    c.bench_function("encoded_segment_size_bytes", |b| {
        b.iter(|| black_box(seg.size_bytes()))
    });
}

criterion_group!(
    benches,
    bench_duration_us,
    bench_avg_bitrate_bps,
    bench_size_bytes,
);
criterion_main!(benches);
