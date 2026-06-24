use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_mq::{InMemoryBroker, Message, Publisher, Subscriber};
use tokio::runtime::Runtime;

fn bench_subscribe_existing_topic(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let broker = InMemoryBroker::new();
    runtime
        .block_on(broker.subscribe("events"))
        .expect("seed topic");

    c.bench_function("mq_subscribe_existing_topic", |b| {
        b.to_async(&runtime)
            .iter(|| broker.subscribe(black_box("events")))
    });
}

fn bench_publish_existing_topic(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let broker = InMemoryBroker::new();
    runtime
        .block_on(broker.subscribe("events"))
        .expect("seed topic");

    c.bench_function("mq_publish_existing_topic", |b| {
        b.to_async(&runtime).iter(|| {
            broker.publish(black_box(Message::new(
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
