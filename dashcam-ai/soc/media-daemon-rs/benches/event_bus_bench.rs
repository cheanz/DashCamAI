//! Criterion benchmarks for event bus wire encoding/decoding.
//!
//! Run with:
//!   cargo bench --bench event_bus_bench --features jetson
//!
//! Targets (encoding must be fast — events fire on every collision/voice):
//!   encode_no_payload    < 1 µs
//!   encode_collision     < 5 µs  (JSON serialisation)
//!   encode_llm_response  < 10 µs (larger string payload)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use media_daemon::event_bus::{
    CollisionPayload, DashcamEvent, EventType, IntentPayload, LlmPayload,
};

// We access encode_event via the public test surface — it's a module-private fn,
// so we benchmark the observable effect: the serialised byte length.
// A future refactor can make encode_event pub(crate) to bench it directly.

fn build_suspend_ack() -> DashcamEvent {
    DashcamEvent {
        event_type:   EventType::SuspendAck,
        timestamp_us: 1_000_000,
        collision: None, intent: None, wake: None, llm: None,
    }
}

fn build_collision(clip_id: u32, confidence: f32) -> DashcamEvent {
    DashcamEvent {
        event_type:   EventType::CollisionDetected,
        timestamp_us: 1_000_000,
        collision: Some(CollisionPayload { confidence, clip_id }),
        intent: None, wake: None, llm: None,
    }
}

fn build_llm(response: String) -> DashcamEvent {
    DashcamEvent {
        event_type:   EventType::LlmResponseReady,
        timestamp_us: 2_000_000,
        collision: None, intent: None, wake: None,
        llm: Some(LlmPayload { response, lang: "en".into() }),
    }
}

// ── Throughput benchmarks via publish() path on a loopback socket ─────────────

fn bench_event_json_serialisation(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_bus_encode");

    // No-payload event
    let evt = build_suspend_ack();
    let payload_bytes = serde_json::to_vec(&evt.collision).unwrap_or_default().len() as u64;
    group.throughput(Throughput::Bytes(16 + payload_bytes));
    group.bench_function("suspend_ack_no_payload", |b| {
        b.iter(|| {
            // Simulate what encode_event does for no-payload events
            let type_id  = black_box(EventType::SuspendAck as u32).to_le_bytes();
            let ts       = black_box(1_000_000u64).to_le_bytes();
            let plen     = 0u32.to_le_bytes();
            black_box([type_id, ts[..].try_into().unwrap(), plen].concat());
        })
    });

    // Collision event — JSON serialisation of CollisionPayload
    group.bench_function("collision_detected_json", |b| {
        let evt = build_collision(42, 0.95);
        b.iter(|| {
            black_box(serde_json::to_vec(black_box(&evt.collision)).unwrap())
        })
    });

    // LLM response — longer string payload
    let long_response = "Turn left on Main Street, then continue for 200 metres. \
                         Your destination will be on the right.";
    group.bench_function("llm_response_json", |b| {
        let evt = build_llm(long_response.to_owned());
        b.iter(|| {
            black_box(serde_json::to_vec(black_box(&evt.llm)).unwrap())
        })
    });

    // Intent classified — mid-size payload
    group.bench_function("intent_classified_json", |b| {
        let intent_payload = IntentPayload {
            transcript: "navigate to the airport".into(),
            lang:       "en".into(),
            intent:     2,
        };
        b.iter(|| {
            black_box(serde_json::to_vec(black_box(&intent_payload)).unwrap())
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_event_json_serialisation,
);
criterion_main!(benches);
