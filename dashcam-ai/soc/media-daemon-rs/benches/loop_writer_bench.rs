//! Criterion benchmarks for the loop writer — disk I/O path.
//!
//! Run with:
//!   cargo bench --bench loop_writer_bench --features jetson
//!
//! Disk benchmarks use tmpfs (/dev/shm on Linux) to eliminate storage variance.
//! On the RV1106, /mnt/emmc is eMMC — sequential write throughput target is
//! ≥ 25 MB/s (4 Mbps × headroom / 8 = 0.5 MB/s, well within eMMC limits).
//!
//! Targets:
//!   write_segment_4mb    < 200 ms  (eMMC worst-case @ 20 MB/s)
//!   write_segment_30mb   < 1500 ms (one full 4 Mbps 60 s segment)
//!   eviction_100segs     < 5 ms    (eviction must not block the encoder)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use media_daemon::encoder_common::EncodedSegment;
use media_daemon::loop_writer::{Config, LoopWriter};
use tempfile::TempDir;

fn fake_seg(n_bytes: usize, idx: u64) -> EncodedSegment {
    EncodedSegment {
        data:        vec![0xAAu8; n_bytes],
        start_ts_us: idx * 60_000_000,
        end_ts_us:   (idx + 1) * 60_000_000,
        frame_count: 1800,
    }
}

fn tmpfs_dir() -> TempDir {
    // Prefer /dev/shm (tmpfs) for low-variance I/O benchmarks.
    // Falls back to the system temp dir if /dev/shm is not available.
    if std::path::Path::new("/dev/shm").exists() {
        TempDir::new_in("/dev/shm").expect("tmpfs TempDir")
    } else {
        TempDir::new().expect("TempDir")
    }
}

fn make_writer(tmp: &TempDir, max_gb: u64) -> LoopWriter {
    LoopWriter::new(Config {
        root_dir:       tmp.path().join("loop"),
        segment_secs:   60,
        max_storage_gb: max_gb,
        pre_roll_secs:  30,
    }).expect("LoopWriter::new")
}

// ── write_segment throughput ──────────────────────────────────────────────────

fn bench_write_segment_sizes(c: &mut Criterion) {
    let sizes_mb: &[usize] = &[4, 16, 30];

    let mut group = c.benchmark_group("loop_writer_write_segment");

    for &mb in sizes_mb {
        let n_bytes = mb * 1024 * 1024;
        group.throughput(Throughput::Bytes(n_bytes as u64));

        group.bench_with_input(
            BenchmarkId::new("size_mb", mb),
            &mb,
            |b, _| {
                b.iter(|| {
                    let tmp = tmpfs_dir();
                    let mut writer = make_writer(&tmp, 100); // 100 GB — no eviction
                    let seg = fake_seg(n_bytes, 0);
                    writer.write_segment(black_box(seg)).unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── eviction cost ─────────────────────────────────────────────────────────────
//
// Eviction removes files from disk — benchmark worst-case latency when
// writing the (N+1)-th segment beyond the storage limit.

fn bench_eviction(c: &mut Criterion) {
    let mut group = c.benchmark_group("loop_writer_eviction");

    for n_pre in [10u64, 50, 100] {
        group.bench_with_input(
            BenchmarkId::new("pre_existing_segments", n_pre),
            &n_pre,
            |b, &n| {
                b.iter(|| {
                    let tmp = tmpfs_dir();
                    // Fill the ring with n × 1 MB segments (total = n MB)
                    let mut writer = make_writer(&tmp, 0); // 0 GB → evict everything
                    writer.max_bytes = n * 1024 * 1024;   // allow n MB

                    // Pre-fill n segments of 1 MB each
                    for i in 0..n {
                        writer.write_segment(fake_seg(1024 * 1024, i)).unwrap();
                    }

                    // This write triggers eviction
                    writer.write_segment(black_box(fake_seg(1024 * 1024, n))).unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── sequential write throughput (simulates real recording) ────────────────────

fn bench_sequential_60s_segments(c: &mut Criterion) {
    // Write 10 × 30 MB segments sequentially — simulates 10 minutes of recording
    let n_segs = 10usize;
    let mb_per_seg = 30usize;

    let mut group = c.benchmark_group("loop_writer_sequential");
    group.throughput(Throughput::Bytes((n_segs * mb_per_seg * 1024 * 1024) as u64));
    group.sample_size(10); // fewer samples — this is a slow bench

    group.bench_function("10x30mb_sequential", |b| {
        b.iter(|| {
            let tmp = tmpfs_dir();
            let mut writer = make_writer(&tmp, 100);
            for i in 0..n_segs {
                writer.write_segment(black_box(fake_seg(mb_per_seg * 1024 * 1024, i as u64))).unwrap();
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_write_segment_sizes,
    bench_eviction,
    bench_sequential_60s_segments,
);
criterion_main!(benches);
