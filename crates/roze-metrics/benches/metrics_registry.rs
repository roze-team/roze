use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use roze_metrics::{MetricLabels, MetricRegistry};

fn labels(route: &str, status: &str) -> MetricLabels {
    MetricLabels::new()
        .insert("service", "bench")
        .insert("route", route)
        .insert("method", "GET")
        .insert("status", status)
}

fn bench_counter_write(c: &mut Criterion) {
    let registry = MetricRegistry::new();
    let labels = labels("/users/:id", "200");

    c.bench_function("metrics_counter_write_labeled", |b| {
        b.iter(|| {
            registry.inc_counter(
                black_box("roze_http_route_requests_total"),
                black_box(labels.clone()),
                black_box(1),
            )
        })
    });
}

fn bench_duration_write(c: &mut Criterion) {
    let registry = MetricRegistry::new();
    let labels = labels("/orders/:id", "200");

    c.bench_function("metrics_duration_write_labeled", |b| {
        b.iter(|| {
            registry.observe_duration(
                black_box("roze_http_route_request_duration"),
                black_box(labels.clone()),
                black_box(Duration::from_millis(12)),
            )
        })
    });
}

fn bench_render(c: &mut Criterion) {
    let registry = MetricRegistry::new();
    for route_idx in 0..128 {
        for status in ["200", "400", "500"] {
            registry.inc_counter(
                "roze_http_route_requests_total",
                labels(&format!("/bench/{route_idx}"), status),
                route_idx + 1,
            );
        }
    }

    c.bench_function("metrics_render_prometheus_text", |b| {
        b.iter(|| black_box(registry.render()))
    });
}

criterion_group!(
    benches,
    bench_counter_write,
    bench_duration_write,
    bench_render
);
criterion_main!(benches);
