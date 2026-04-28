# Database Connection-Pool Tuning Guide

Covers the `deadpool-postgres` pool used by the Rust backend (`rust-backend/src/db.rs`).
The Next.js side uses the `pg` npm package with its own pool; see `usdc-payment-link-tool/` for that config.

## How the pool is created

```rust
// rust-backend/src/db.rs
Pool::builder(manager)
    .runtime(Runtime::Tokio1)
    .max_size(16)   // hard ceiling on open connections
    .build()
```

`deadpool-postgres` uses `RecyclingMethod::Fast` (ping-free recycle) and `tokio_postgres::NoTls` by default.
SSL is controlled by the `PGSSL` env var, which is validated at startup against the list of valid PostgreSQL SSL modes.

## Sizing by environment

| Environment | Recommended `max_size` | Notes |
|---|---|---|
| Local dev | 5 | One developer, no concurrency pressure |
| Railway Starter / Hobby | 5–10 | Railway free-tier Postgres caps at ~25 total connections |
| Railway Pro | 16 (default) | Adequate for moderate traffic; raise if p99 latency climbs |
| Production (dedicated PG) | `(2 × vCPU) + 1` per replica | Standard rule of thumb; tune down if connection overhead dominates |

The current hard-coded default is `16`. Override it by setting `DATABASE_POOL_MAX_SIZE` in the environment and wiring it through `Config` when you need a value other than 16.

## Key environment variables

| Variable | Purpose | Example |
|---|---|---|
| `DATABASE_URL` | Full Postgres connection string | `postgres://user:pass@host:5432/astropay` |
| `PGSSL` | SSL mode passed to `tokio-postgres` | `require` (prod), `disable` (local) |

Valid `PGSSL` values: `disable`, `allow`, `prefer`, `require`, `verify-ca`, `verify-full`.
The server rejects any other value at startup with a clear error message.

## Symptoms of pool exhaustion

- Requests hang waiting for a connection and eventually time out.
- Logs show `PoolError::Timeout` from `deadpool`.
- DB server shows many idle connections near its `max_connections` limit.

When you see these, either raise `max_size` (if the DB can handle more connections) or add a connection pooler (PgBouncer / pgpool) in front of Postgres.

## Using PgBouncer in transaction mode

If you add PgBouncer in **transaction** pooling mode:

- Set `RecyclingMethod::Fast` (already the default) — do **not** use `RecyclingMethod::Verified` because that issues a `SELECT 1` ping which is unnecessary with a pooler.
- Prepared statements do not survive across PgBouncer transaction-mode connections. Switch to unprepared queries (`client.query_raw` / `client.execute`) or use PgBouncer's **session** mode instead.
- Set `max_size` to the number of PgBouncer server connections allocated to this service, not the raw Postgres `max_connections`.

## Railway-specific notes

Railway injects `DATABASE_URL` automatically when a Postgres plugin is attached.
The free-tier plugin allows roughly 25 simultaneous connections shared across all services.
Keep `max_size ≤ 10` on the free tier to leave headroom for migrations and ad-hoc queries.

For the Railway Pro plan, the connection limit depends on the plan's resource allocation.
Check `SHOW max_connections;` on the DB to confirm the ceiling before raising `max_size`.

## Verifying pool health locally

```bash
# Check how many connections are open right now
psql $DATABASE_URL -c "SELECT count(*), state FROM pg_stat_activity WHERE datname = current_database() GROUP BY state;"

# Watch pool stats during a load test
watch -n2 'psql $DATABASE_URL -c "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database();"'
```

## Changing `max_size` without a code deploy

The pool size is set at startup from `Config`. To change it:

1. Add `DATABASE_POOL_MAX_SIZE` to `Config` and `config.rs` (parse from env, default `16`).
2. Pass it to `Pool::builder(...).max_size(config.pool_max_size)`.
3. Redeploy — the pool is created once at startup and cannot be resized at runtime.

Until that wiring is done, change the literal `16` in `db.rs` and redeploy.

## Related files

- `rust-backend/src/db.rs` — pool creation and `validate_ssl_mode`
- `rust-backend/src/config.rs` — `Config` struct and env parsing
- `docs/database/schema-ownership.md` — index and migration ownership
- `docs/database/retention-policy.md` — session and audit table cleanup
