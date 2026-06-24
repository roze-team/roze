use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_eventbus::{EventEnvelope, EventPublisher, EventSubscriber, InMemoryEventBus};
use tokio::runtime::Runtime;

fn bench_subscribe_existing_topic(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let bus = InMemoryEventBus::new();
    runtime
        .block_on(bus.subscribe("events"))
        .expect("seed topic");

    c.bench_function("eventbus_subscribe_existing_topic", |b| {
        b.to_async(&runtime)
            .iter(|| bus.subscribe(black_box("events")))
    });
}

fn bench_publish_existing_topic(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let bus = InMemoryEventBus::new();
    runtime
        .block_on(bus.subscribe("events"))
        .expect("seed topic");

    c.bench_function("eventbus_publish_existing_topic", |b| {
        b.to_async(&runtime).iter(|| {
            bus.publish(black_box(EventEnvelope::new(
                "events",
                serde_json::json!({"id": 1}),
            )))
        })
    });
}

criterion_group!(
    benches,
    bench_subscribe_existing_topic,
    bench_publish_existing_topic
);
criterion_main!(benches);
