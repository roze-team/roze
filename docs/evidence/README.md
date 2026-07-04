# Production Evidence Reports

This directory stores reproducible long-run evidence reports.

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
