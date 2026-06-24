use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_singleflight::SingleFlightGroup;
use tokio::runtime::Runtime;

fn bench_unique_keys(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let group = SingleFlightGroup::new();
    let key_seq = AtomicU64::new(0);

    c.bench_function("singleflight_unique_keys", |b| {
        b.to_async(&runtime).iter(|| {
            let key = format!("key-{}", key_seq.fetch_add(1, Ordering::Relaxed));
            group.do_call(black_box(key), || async { Ok::<_, String>(42u64) })
        })
    });
}

fn bench_cached_same_key(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let group = SingleFlightGroup::new();
    runtime
        .block_on(group.do_call("shared-key", || async { Ok::<_, String>(42u64) }))
        .expect("seed singleflight result");

    c.bench_function("singleflight_cached_same_key", |b| {
        b.to_async(&runtime).iter(|| {
            group.do_call(black_box("shared-key"), || async {
                Err::<u64, _>("loader should not run".to_owned())
            })
        })
    });
}

fn bench_reset(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let group = SingleFlightGroup::new();
    let key_seq = AtomicU64::new(0);

    c.bench_function("singleflight_reset", |b| {
        b.to_async(&runtime).iter(|| async {
            let key = format!("reset-{}", key_seq.fetch_add(1, Ordering::Relaxed));
            group
                .do_call(black_box(&key), || async { Ok::<_, String>(42u64) })
                .await
                .expect("singleflight call");
            group.reset(black_box(&key)).await;
        })
    });
}

criterion_group!(
    benches,
    bench_unique_keys,
    bench_cached_same_key,
    bench_reset
);
criterion_main!(benches);
