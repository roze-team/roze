use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_local_cache::LocalCache;
use tokio::runtime::Runtime;

fn bench_insert(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let cache = LocalCache::with_capacity(1_000_000);
    let key_seq = AtomicU64::new(0);

    c.bench_function("local_cache_insert_unique", |b| {
        b.to_async(&runtime).iter(|| {
            let key = format!("key-{}", key_seq.fetch_add(1, Ordering::Relaxed));
            cache.insert(black_box(key), black_box("value".to_owned()))
        })
    });
}

fn bench_get_hit(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let cache = LocalCache::with_capacity(1024);
    let key = "key-hit".to_owned();
    runtime.block_on(cache.insert(key.clone(), "value".to_owned()));

    c.bench_function("local_cache_get_hit", |b| {
        b.to_async(&runtime).iter(|| cache.get(black_box(&key)))
    });
}

fn bench_get_or_insert_hit(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let cache = LocalCache::with_ttl(Duration::from_secs(60));
    let key = "key-existing".to_owned();
    runtime.block_on(cache.insert(key.clone(), "value".to_owned()));

    c.bench_function("local_cache_get_or_insert_hit", |b| {
        b.to_async(&runtime).iter(|| {
            cache.get_or_insert_with(black_box(key.clone()), || async { "computed".to_owned() })
        })
    });
}

criterion_group!(
    benches,
    bench_insert,
    bench_get_hit,
    bench_get_or_insert_hit
);
criterion_main!(benches);
