use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_ws::{WsHub, WsSession};

fn bench_register(c: &mut Criterion) {
    let hub = WsHub::default();
    let seq = AtomicU64::new(0);

    c.bench_function("ws_hub_register_unique", |b| {
        b.iter(|| {
            let id = format!("session-{}", seq.fetch_add(1, Ordering::Relaxed));
            hub.register(black_box(WsSession::new(id)));
        })
    });
}

fn bench_get_hit(c: &mut Criterion) {
    let hub = WsHub::default();
    hub.register(WsSession::new("session-hit"));

    c.bench_function("ws_hub_get_hit", |b| {
        b.iter(|| black_box(hub.get(black_box("session-hit"))))
    });
}

fn bench_disconnect(c: &mut Criterion) {
    let hub = WsHub::default();
    let seq = AtomicU64::new(0);

    c.bench_function("ws_hub_disconnect", |b| {
        b.iter(|| {
            let id = format!("disconnect-{}", seq.fetch_add(1, Ordering::Relaxed));
            hub.register(WsSession::new(id.clone()));
            black_box(hub.disconnect(black_box(&id)));
        })
    });
}

criterion_group!(benches, bench_register, bench_get_hit, bench_disconnect);
criterion_main!(benches);
