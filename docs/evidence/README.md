# Production Evidence Reports

This directory stores reproducible long-run evidence reports.

It can also store scoped generator verification notes when they clearly state
their evidence boundary. These notes support release confidence, but they do
not replace the long-run reports required before broad production-stability
claims.

Current scoped verification notes:

- [2026-07-04 generation verification](2026-07-04-generation-verification.md)

Generate a new report scaffold with:

```bash
bash scripts/production-evidence.sh \
  --area gateway \
  --duration 24h \
  --workload "proxy traffic with retries, fallback, rate limit, breaker, load shedding, and hot reload" \
  --failure-injection "upstream 5xx, timeout, slow response, config reload, upstream recovery" \
  --command "bash scripts/production-soak-gateway.sh --duration 24h"
```

Reports are intentionally explicit about missing data. Do not mark a report
`pass` until measurements, failure timeline, leak checks, and artifacts are
filled in.

Current soak harnesses:

```bash
bash scripts/production-soak-mq.sh 300
bash scripts/production-soak-config-center.sh 300
```

Use `ROZE_MQ_SOAK_SECONDS`, `ROZE_MQ_SOAK_MESSAGES`,
`ROZE_CONFIG_CENTER_SOAK_SECONDS`, and `ROZE_CONFIG_CENTER_SOAK_UPDATES` for
24h/72h runs.
