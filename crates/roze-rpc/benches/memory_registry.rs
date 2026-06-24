use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_rpc::registry::{MemoryRegistry, Registry, ServiceInstance};
use tokio::runtime::Runtime;

fn bench_register(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let registry = MemoryRegistry::default();
    let addr_seq = AtomicU64::new(0);

    c.bench_function("memory_registry_register_unique", |b| {
        b.to_async(&runtime).iter(|| {
            let seq = addr_seq.fetch_add(1, Ordering::Relaxed);
            let instance =
                ServiceInstance::new("bench.rpc.UserService", format!("127.0.0.1:{seq}"));
            registry.register(black_box(instance))
        })
    });
}

fn bench_discover(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let registry = MemoryRegistry::default();
    runtime.block_on(async {
        for idx in 0..128 {
            registry
                .register(ServiceInstance::new(
                    "bench.rpc.UserService",
                    format!("127.0.0.1:{}", 10_000 + idx),
                ))
                .await
                .expect("register instance");
        }
    });

    c.bench_function("memory_registry_discover_128_instances", |b| {
        b.to_async(&runtime)
            .iter(|| registry.discover(black_box("bench.rpc.UserService")))
    });
}

fn bench_register_deregister(c: &mut Criterion) {
    let runtime = Runtime::new().expect("create tokio runtime");
    let registry = MemoryRegistry::default();
    let addr_seq = AtomicU64::new(20_000);

    c.bench_function("memory_registry_register_deregister", |b| {
        b.to_async(&runtime).iter(|| async {
            let seq = addr_seq.fetch_add(1, Ordering::Relaxed);
            let addr = format!("127.0.0.1:{seq}");
            registry
                .register(ServiceInstance::new("bench.rpc.UserService", addr.clone()))
                .await
                .expect("register instance");
            registry
                .deregister(black_box("bench.rpc.UserService"), black_box(&addr))
                .await
                .expect("deregister instance");
        })
    });
}

criterion_group!(
    benches,
    bench_register,
    bench_discover,
    bench_register_deregister
);
criterion_main!(benches);
