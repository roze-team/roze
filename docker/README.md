# Docker Verification Environment

This directory contains local dependencies for Roze verification.

## Services

- Postgres: `127.0.0.1:15432`, database `roze`, user `postgres`, password `postgres`
- MySQL: `127.0.0.1:13306`, database `roze`, user `root`, password `root`
- Redis: `127.0.0.1:16379`
- Kafka: `127.0.0.1:19092`
- NATS: `127.0.0.1:14222`
- NATS monitor: `http://127.0.0.1:18222`
- MongoDB: `127.0.0.1:27018`
- Elasticsearch: `http://127.0.0.1:19200`
- OpenSearch: `http://127.0.0.1:19201`
- Meilisearch: `http://127.0.0.1:17700`
- Etcd: `http://127.0.0.1:12379`
- Consul: `http://127.0.0.1:18500`

Copy `.env.example` to `.env` if local ports need to change.

## Start

```bash
docker compose --env-file docker/.env.example -f docker/docker-compose.yml up -d postgres mysql redis kafka
```

Start all services:

```bash
docker compose --env-file docker/.env.example -f docker/docker-compose.yml up -d
```

## Status

```bash
docker compose --env-file docker/.env.example -f docker/docker-compose.yml ps
```

## Verify

```bash
bash docker/verify.sh
```

## Cleanup

Stop services and keep data:

```bash
docker compose --env-file docker/.env.example -f docker/docker-compose.yml down
```

Stop services and remove verification volumes:

```bash
docker compose --env-file docker/.env.example -f docker/docker-compose.yml down -v
```

## Example `rozectl model inspect`

```bash
rozectl model inspect users \
  --db-kind mysql \
  --db-url mysql://root:root@127.0.0.1:13306/roze \
  --out services/user-rpc
```

```bash
rozectl model inspect users \
  --db-kind postgres \
  --db-url postgres://postgres:postgres@127.0.0.1:15432/roze \
  --schema public \
  --out services/user-rpc
```
