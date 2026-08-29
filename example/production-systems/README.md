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
   event envelope, inbox/outbox, application compensation, object storage, and
   replay recovery. Distributed Saga/TCC evidence belongs to the independently
   versioned [`roze-dtm`](https://github.com/roze-team/roze-dtm) matrix and is
   not implied by this reference system.

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

Set `ROZE_REFERENCE_EVIDENCE_DIR` to retain a machine-readable `run.json` and
summary for the integration attempt. A `passed` status records only the
success/failure/recovery workflow; it is not a substitute for signed 24h/72h
long-run evidence.

For a side-effect-free environment check, run:

```bash
bash scripts/reference-systems-preflight.sh
```

It verifies generated compilation, Redis and NATS round trips, Etcd and Consul
registration, PostgreSQL/MySQL migration rollback, Kafka produce/consume,
Elasticsearch indexing, and explicit Redis/NATS/Etcd disconnect and recovery.
