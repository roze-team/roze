use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_rpc::{AttemptOutcome, EwmaP2cBalancer, EwmaP2cConfig, ServiceInstance};

fn instances(count: usize) -> Vec<ServiceInstance> {
    (0..count)
        .map(|index| ServiceInstance::new("bench", format!("127.0.0.1:{}", 10_000 + index)))
        .collect()
}

fn bench_ewma_p2c(c: &mut Criterion) {
    let candidates = instances(64);
    let balancer = EwmaP2cBalancer::new(EwmaP2cConfig {
        force_pick_after: Duration::from_secs(60),
        ..EwmaP2cConfig::default()
    });

    c.bench_function("ewma_p2c_pick_and_finish_64", |b| {
        b.iter(|| {
            let lease = balancer
                .pick_tracked(black_box(&candidates))
                .expect("non-empty candidates");
            lease.finish(AttemptOutcome::Success);
        })
    });

    c.bench_function("ewma_p2c_churn_64", |b| {
        b.iter(|| {
            balancer.synchronize(black_box(&candidates[..32]), 1);
            balancer.synchronize(black_box(&candidates[32..]), 2);
            black_box(balancer.state_len());
        })
    });
}

criterion_group!(benches, bench_ewma_p2c);
criterion_main!(benches);
