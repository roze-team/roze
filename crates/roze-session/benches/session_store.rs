use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_session::{InMemorySessionStore, Session, SessionStore};
use tokio::runtime::Runtime;

fn bench_upsert(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let store = InMemorySessionStore::new();
    let seq = AtomicU64::new(0);

    c.bench_function("session_store_upsert_unique", |b| {
        b.to_async(&runtime).iter(|| {
            let id = format!("sid-{}", seq.fetch_add(1, Ordering::Relaxed));
            store.upsert(black_box(Session::new(id, "subject")))
        })
    });
}

fn bench_get_hit(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let store = InMemorySessionStore::new();
    runtime
        .block_on(store.upsert(Session::new("sid-hit", "subject")))
        .expect("seed session");

    c.bench_function("session_store_get_hit", |b| {
        b.to_async(&runtime)
            .iter(|| store.get(black_box("sid-hit")))
    });
}

criterion_group!(benches, bench_upsert, bench_get_hit);
criterion_main!(benches);
