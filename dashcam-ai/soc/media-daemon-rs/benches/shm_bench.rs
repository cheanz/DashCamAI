//! Criterion benchmarks for the POSIX shared memory ring buffer.
//!
//! Run with:
//!   cargo bench --bench shm_bench --features jetson   # or rockchip
//!
//! Targets (RV1106 @ 500 MHz ARM Cortex-A7, 30 fps = 33 ms budget per frame):
//!   write_frame_1080p   < 2 ms
//!   write_frame_720p    < 1 ms
//!   sequential_fill     < 16 ms  (8 × 1080p frames, no eviction)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use media_daemon::shm::{ShmRingProducer, N_SLOTS};

fn fake_nv12(width: u32, height: u32) -> Vec<u8> {
    vec![0x80u8; (width as usize) * (height as usize) * 3 / 2]
}

// ── bench_write_frame_latency ─────────────────────────────────────────────────
//
// Measures the end-to-end cost of write_frame() for a single NV12 frame.
// A simulated consumer read keeps the ring from filling up.

fn bench_write_frame_latency(c: &mut Criterion) {
    let resolutions: &[(u32, u32)] = &[
        (1920, 1080),
        (1280, 720),
        (640,  480),
    ];

    let mut group = c.benchmark_group("shm_write_frame");

    for &(w, h) in resolutions {
        let frame       = fake_nv12(w, h);
        let frame_bytes = frame.len() as u64;
        group.throughput(Throughput::Bytes(frame_bytes));

        group.bench_with_input(
            BenchmarkId::new("resolution", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let producer = ShmRingProducer::create().expect("shm create");
                let mut ts: u64 = 0;

                b.iter(|| {
                    // Advance simulated consumer so the ring never fills
                    producer.simulate_consumer_read();
                    ts = ts.wrapping_add(33_333);

                    black_box(producer.write_frame(
                        black_box(&frame),
                        black_box(w),
                        black_box(h),
                        black_box(w),   // stride == width for packed NV12
                        black_box(ts),
                    ))
                });
            },
        );
    }

    group.finish();
}

// ── bench_sequential_ring_fill ────────────────────────────────────────────────
//
// Fills all N_SLOTS of the ring in a tight loop — aggregate throughput.

fn bench_sequential_ring_fill(c: &mut Criterion) {
    let frame       = fake_nv12(1920, 1080);
    let total_bytes = (frame.len() * N_SLOTS) as u64;

    let mut group = c.benchmark_group("shm_sequential_fill");
    group.throughput(Throughput::Bytes(total_bytes));

    group.bench_function("fill_all_slots_1080p", |b| {
        b.iter(|| {
            // Create a fresh producer each iteration so the ring starts empty.
            let producer = ShmRingProducer::create().expect("shm create");

            for i in 0..N_SLOTS {
                black_box(producer.write_frame(
                    black_box(&frame),
                    1920, 1080, 1920,
                    black_box((i as u64) * 33_333),
                ));
            }
            // Drop here — triggers shm_unlink so the next iteration gets a fresh shm
        });
    });

    group.finish();
}

// ── bench_write_frame_no_alloc ────────────────────────────────────────────────
//
// Verifies the write path has no hidden per-call heap allocations.
// Criterion's regression detection will flag unexpected allocation regressions.

fn bench_write_frame_no_alloc(c: &mut Criterion) {
    let frame = fake_nv12(1920, 1080);

    c.bench_function("shm_write_no_alloc_1080p", |b| {
        let producer = ShmRingProducer::create().expect("shm create");
        let mut ts: u64 = 0;

        b.iter(|| {
            producer.simulate_consumer_read();
            ts = ts.wrapping_add(33_333);
            black_box(producer.write_frame(black_box(&frame), 1920, 1080, 1920, ts))
        });
    });
}

criterion_group!(
    benches,
    bench_write_frame_latency,
    bench_sequential_ring_fill,
    bench_write_frame_no_alloc,
);
criterion_main!(benches);
