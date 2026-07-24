# roze-kafka

Kafka providers for the stable `roze_mq::Publisher` and
`roze_mq::Subscriber` contracts.

## Providers

| Provider | Cargo feature | Native toolchain | Publish | Consumer groups | Manual ACK / offset commit | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Memory | `memory` | No | Yes | No | In-process settlement | Development/testing |
| rust-rdkafka | `rdkafka` | `librdkafka` | Yes | Yes | Yes | Production |
| rust-rdkafka + CMake | `rdkafka-cmake` | CMake and C/C++ tools | Yes | Yes | Yes | Production, Windows-friendly build |
| rskafka | `rskafka` | No | Yes | No | No | Experimental, publish-only |

The `rskafka` dependency is built without its default compression features so
it does not pull the native LZ4 or Zstandard libraries. Upstream rskafka does
not implement consumer groups, offset tracking, rebalance, or transactions.
Roze therefore rejects `build_runtime` for `rust-native` instead of presenting
unsafe or misleading ACK semantics. Publish-only applications can use
`build_publisher`.

## Configuration

```yaml
kafka:
  provider: rdkafka # memory | rdkafka | rust-native
  brokers: ["127.0.0.1:9092"]
  group: orders-workers
  enable_manual_ack: true
  retry_topic: orders.retry
  dead_letter_topic: orders.dlq
```

If `provider` is omitted, Roze preserves feature-based behavior: production
`rdkafka` is preferred when enabled, followed by `rskafka`, then `memory`.
Selecting a
provider without its Cargo feature fails at startup with the provider and
required feature in the error.

## Runtime construction

```rust
let runtime = roze_kafka::build_runtime(&config).await?;
let publisher = runtime.publisher;
let subscriber = runtime.subscriber;
```

Both handles expose only `roze_mq` traits. Applications do not need a Kafka
delivery adapter or broker-specific types in generated business code.

`KafkaCapabilities.transactions` is currently `false` for every provider
because the stable Roze runtime does not yet expose a transactional publisher,
even though librdkafka itself supports transactions.

Run the ignored pure-Rust round-trip against either Apache Kafka or Redpanda:

```bash
ROZE_KAFKA_BROKERS=127.0.0.1:9092 \
ROZE_KAFKA_TOPIC=roze.integration \
cargo test -p roze-kafka --no-default-features --features rskafka \
  rust_native_publish_round_trips_against_real_broker -- --ignored
```
