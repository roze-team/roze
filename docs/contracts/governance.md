# Governance Policy Contract

`roze_config::GovernanceConfig` is the configuration source of truth.
Transports and background runtimes must not merge global and scoped fields
independently. Call one of these methods at the application boundary:

- `resolve_policy(key)` for one route, RPC method, topic, consumer, or job name.
- `resolve_policy_for(keys)` when a transport has an explicit precedence list.

Both methods return `roze_config::GovernancePolicy`, re-exported from the lower
`roze-resilience` execution layer so MQ and Job do not depend on configuration
loading or create a dependency cycle.

## Merge Rules

For each field, the first matching scoped policy overrides the global value.
An omitted scoped field inherits its global value. Scoped keys themselves are
matched in caller-provided order. A fallback is returned only when the effective
entry has `enabled: true`; disabled fallback never silently inherits as enabled.

Gateway explicit route fields remain higher priority than the resolved shared
policy. Gateway resolves scoped keys in path, path-without-leading-slash, then
service-name order.

## Runtime Coverage

- REST applies timeout, rate limit, breaker, shedding, and fallback.
- RPC applies timeout, retry budget, rate limit, breaker, shedding, and fallback.
- Gateway applies explicit route fields first, then the resolved shared policy.
- MQ `spawn_consumer_with_governance` applies timeout, bounded full-jitter retry,
  retry budget, rate limit, breaker, and shedding before ack/nack settlement.
- Job `add_governed`, `spawn_with_governance`, and
  `spawn_once_with_governance` apply the same executable controls per job.

Fallback is response-oriented. MQ and Job do not reinterpret fallback as ack,
successful completion, or suppressed failure; failed work remains failed.

All governed boundaries emit `roze_resilience_decisions_total` with a bounded
`boundary` value (`rest`, `rpc`, `gateway`, `mq`, or `job`). Policy state remains
local to the data path except when a boundary explicitly selects an external
state provider. REST, RPC, and Gateway use the shared memory/Redis contract in
[`distributed-rate-limit.md`](distributed-rate-limit.md); Redis-backed rate
limiting deliberately consults the data store for each decision and applies the
configured timeout and unavailable policy.
