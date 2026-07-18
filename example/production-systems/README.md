# Generated Production Reference Systems

These contracts are the authoritative inputs for Roze's generated-system
verification. Generated output belongs under a temporary evidence workspace and
must never be edited as source.

The three systems exercise distinct production paths:

1. `rest-crud`: REST, SQL model, search, cache-consistency ownership, reports,
   deployment assets, and migration/rollback policy.
2. `service-mesh`: REST to managed RPC dependency, SQL model, Redis-backed
   readiness, context propagation, deadline, retry budget, and tracing.
3. `event-commerce`: REST, stream worker, Gateway/registry topology, reliable
   event envelope, inbox/outbox, Saga/TCC, object storage, and replay recovery.

Run the compile and regeneration matrix with:

```bash
bash scripts/generated-reference-systems.sh
```

The script is part of `scripts/generated-target-matrix.sh` and therefore the
release gate. It performs create/update generation, managed dependency
canonicalization, operations-asset checks, and `cargo check` for all five
generated crates.

Runtime evidence must use `docker-compose.integration.yml` and record the
success, dependency-loss, recovery, duplicate-delivery, rollback, and graceful
drain scenarios declared in each `topology.yaml`. A compile pass is not runtime
or long-run evidence.

Run the real dependency integration path on a Linux host with Docker:

```bash
bash scripts/reference-systems-integration.sh
```

It verifies generated compilation, Redis and NATS round trips, Etcd and Consul
registration, PostgreSQL/MySQL migration rollback, Kafka produce/consume,
Elasticsearch indexing, and explicit Redis/NATS/Etcd disconnect and recovery.
