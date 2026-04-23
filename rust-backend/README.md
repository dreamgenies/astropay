# ASTROpay Rust Backend

This service is the beginning of the backend migration out of Next.js route handlers.

What it currently owns:

- merchant registration, login, logout, and cookie-backed sessions
- invoice creation, listing, detail lookup, and status lookup
- Horizon-backed reconciliation for pending invoices
- webhook-driven payment marking (`/api/webhooks/stellar`)
- a Rust migration runner that reuses the existing SQL migrations

Reconciliation and the Stellar webhook validate each merchant `settlement_public_key` with Stellar strkey decoding before inserting into `payouts`. Invalid keys skip payout queueing (invoice still marked paid) and emit a `payment_events` row with `event_type = payout_skipped_invalid_destination`. Run `cargo test` for strkey coverage.

What is intentionally not faked yet:

- buyer XDR generation/submission for checkout
- merchant settlement cron

Those routes return `501 Not Implemented` in the Rust service until the Stellar transaction logic is ported properly.

## Cron run audit (`cron_runs`)

Migration `004_cron_runs.sql` adds an append-only table keyed by `job_type` (`reconcile` \| `settle`) with JSONB `metadata` (response-shaped summary: `scanned` / `results` or `processed` / `results`) and optional `error_detail`. Successful Rust reconcile runs insert a row (if the handler returns `Ok` after scanning; Horizon or DB errors before that point skip audit persistence). Settle (still not implemented) inserts `success = false` with the error text before returning `501`. Audit insert failures are logged and do not change the HTTP status.

**Verification:** `cargo test` checks that `004_cron_runs.sql` defines the table and index; apply migrations with `cargo run --bin migrate` before relying on audit rows in development.
## Database migrations

SQL lives in `../usdc-payment-link-tool/migrations/`. Apply with `cargo run --bin migrate` from `rust-backend/`. The runner errors clearly if the migrations directory is missing or if a file’s SQL fails.

**Invoice `metadata` (JSONB):** migration `003_invoice_metadata_jsonb_index_plan.sql` records the indexing policy—no speculative GIN until real filter queries exist—and sets `COMMENT ON COLUMN invoices.metadata` for DB catalog visibility. See the Next.js README for the same guidance.

**Verification:** `cargo test` (includes a guard that 003 stays comment/plan-only without `CREATE INDEX`).
SQL lives in `../usdc-payment-link-tool/migrations/`. Apply in lexical order with:

```bash
cd rust-backend
cargo run --bin migrate
```

The runner aborts with a clear error if that directory is missing (for example when not run from `rust-backend/`) or if a migration file fails.

### `sessions` indexes

- **Request path**: Session validation loads the row by `sessions.id` (JWT `sid`); the primary key is the right index.
- **Cleanup path**: For jobs that delete or archive rows with `WHERE expires_at < $cutoff`, use migration `002_session_expiry_indexes.sql`, which builds `(expires_at, id)` for ordered batches and `(merchant_id, expires_at)` for merchant-scoped work. The composite on `merchant_id` replaces the standalone `merchant_id` index from `001_init.sql` after migration 002 runs.

**Verification:** `cargo test` (includes a guard that migration 002 defines the expected index names). With Postgres available, run `migrate` then inspect indexes, for example `psql "$DATABASE_URL" -c '\d sessions'`.

## Run locally

```bash
cd rust-backend
cargo run --bin migrate
cargo run
```

The service reads env vars from:

- `rust-backend/.env.local`
- `rust-backend/.env`
- `../usdc-payment-link-tool/.env.local`
- `../usdc-payment-link-tool/.env`
