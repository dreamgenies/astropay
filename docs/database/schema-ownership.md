# Schema Ownership Across Next.js And Rust

This document records which runtime currently owns each part of the ASTROpay schema, where that ownership is shared, and where the split can drift.

It is intentionally based on code that exists today, not the target architecture.

## Current Rule Of Thumb

- SQL migrations are canonical under `usdc-payment-link-tool/migrations/`.
- The Next.js app still owns checkout and payout settlement behavior.
- The Rust backend owns merchant auth/session handling, invoice CRUD, Rust-side reconcile, and Rust-side webhook payment marking.
- Several tables are in a shared-write transition state. When that is true, this document says so explicitly instead of pretending ownership is exclusive.

## Migration Ownership

Canonical migration source:

- `usdc-payment-link-tool/migrations/*.sql`

Runtimes that apply those migrations:

- Next.js via `usdc-payment-link-tool/scripts/run-migrations.mjs` and `npm run db:migrate`
- Rust via `rust-backend/src/bin/migrate.rs` and `cargo run --bin migrate`

Current ownership decision:

- The Next.js tree owns migration files and migration ordering.
- The Rust migration binary is a runner over the same migration directory, not a separate migration authority.

Risk:

- If one runtime starts generating migrations in a different directory or with different ordering rules, schema drift will become invisible until deploy time.

## Table Ownership

### `schema_migrations`

Primary owner:

- Shared operational ownership

Writers:

- Next.js migration runner inserts applied migration IDs.
- Rust migration runner inserts applied migration IDs.

Notes:

- Both runtimes must agree on lexical ordering and idempotency.
- Ownership is intentionally shared because both runtimes can bootstrap the same database.

### `merchants`

Primary owner:

- Shared application ownership, with Rust leading the migration path

Writers:

- Next.js creates merchants through current auth flows in `usdc-payment-link-tool/lib/data.ts`.
- Rust creates merchants in `rust-backend/src/handlers/auth.rs`.

Readers:

- Both runtimes read merchants for login, current session resolution, invoice ownership, and payout destination lookup.

Notes:

- Merchant wallet uniqueness and validation rules must stay aligned across runtimes.
- Rust appears intended to become the long-term owner, but Next.js still writes this table today.

### `sessions`

Primary owner:

- Shared application ownership, with Rust leading the migration path

Writers:

- Next.js creates session rows in `usdc-payment-link-tool/lib/auth.ts`.
- Rust creates session rows in `rust-backend/src/auth.rs`.

Readers:

- Next.js resolves current merchant sessions in `usdc-payment-link-tool/lib/auth.ts`.
- Rust resolves current merchant sessions in `rust-backend/src/auth.rs`.

Notes:

- Both runtimes assume the same cookie signing secret and the same `sessions.id`/`merchant_id` lookup pattern.
- Session cleanup remains an operational concern rather than an explicitly owned runtime feature.

### `invoices`

Primary owner:

- Shared ownership

Writers:

- Next.js creates invoices in `usdc-payment-link-tool/lib/data.ts`.
- Rust creates invoices in `rust-backend/src/handlers/invoices.rs`.
- Next.js updates invoice status during reconcile and settlement flows.
- Rust updates invoice status during Rust reconcile and webhook flows.

Readers:

- Both runtimes read invoices for merchant dashboards, public pay routes, status polling, reconciliation, settlement, and webhook handling.

Notes:

- This is the highest-coupling table in the repo.
- Both runtimes rely on the same lifecycle strings: `pending`, `paid`, `expired`, `settled`, `failed`.
- `metadata` is shared JSONB with a documented no-speculative-index rule; both runtimes assume that contract.

### `payment_events`

Primary owner:

- Shared ownership

Writers:

- Next.js appends events when payment detection, payout skip, or merchant settlement occurs.
- Rust appends events during reconcile and webhook payment marking.

Readers:

- No dedicated API reader is obvious today; the table acts as append-only audit history for operators and future features.

Notes:

- Event names such as `payment_detected`, `payout_skipped_invalid_destination`, and `merchant_settled` are a shared contract.
- Any rename here is effectively a schema-contract change for observability and ops tooling.

### `payouts`

Primary owner:

- Shared ownership, but Next.js currently owns payout execution

Writers:

- Next.js inserts queued payouts during payment marking and updates payout status during settlement.
- Rust inserts queued payouts during Rust reconcile and Rust webhook payment marking.
- Rust does not yet own actual payout submission/settlement execution in production terms; its settle route is still intentionally incomplete.

Readers:

- Next.js reads queued payouts for cron settlement.
- Rust reads payouts for future settlement parity and current code/tests around reconcile behavior.

Notes:

- Queue insertion is shared.
- Execution ownership is still effectively Next.js because `app/api/cron/settle/route.ts` contains the real settlement path.

### `cron_runs`

Primary owner:

- Shared ownership

Writers:

- Next.js writes cron audit rows through `recordCronRun` in `usdc-payment-link-tool/lib/data.ts`.
- Rust writes cron audit rows from `rust-backend/src/handlers/cron.rs`.

Readers:

- No dedicated reader surface yet; this is an operator/audit table.

Notes:

- Both runtimes assume the same `job_type`, `success`, `metadata`, and `error_detail` shape.
- The response summary contract inside `metadata` is hidden coupling between runtimes.

## Hidden Coupling And Drift Risks

### Shared lifecycle enums are implicit, not centralized

The invoice and payout status strings live in SQL checks, Next.js code, and Rust code, but there is no single schema enum source. A new state added in one runtime can silently break the other.

### Next.js still owns real payout settlement

Rust has reconcile and webhook payment marking, but settlement execution remains in the Next.js cron route. Any document that says Rust owns payouts end to end would be overstating the current system.

### Migration authority is directory-based, not tool-enforced

Both runtimes can apply migrations, but only the Next.js tree currently stores them. That works as long as contributors keep using the shared migration directory.

### Session assumptions are shared but invisible

Both runtimes assume the same cookie name, JWT claim layout, and session lookup semantics. Breaking one side can invalidate logins for the other without a schema migration ever changing.

### Audit/event payloads are shared operational contracts

`payment_events.payload`, `invoices.metadata`, and `cron_runs.metadata` are schema-flexible JSONB, but the code still relies on stable meanings. Drift here is harder to detect than drift in typed columns.

## Error And Edge-Case Handling

- If runtime ownership becomes stale:
  Document the uncertainty instead of guessing exclusive ownership. Shared-write tables should stay marked shared until one runtime's write path is removed.
- If a route is intentionally incomplete:
  Do not assign ownership based on file presence alone. For example, Rust has a settle handler, but it is not yet the authoritative payout executor.
- If both runtimes can write the same row family:
  Treat ownership as shared and call out idempotency expectations, especially for `invoices`, `payment_events`, `payouts`, and `cron_runs`.
- If JSONB shape is inferred:
  Note that the column exists without strict DB-level typing and that downstream readers must tolerate absent keys.

## Verification

- Confirmed against:
  `usdc-payment-link-tool/migrations/001_init.sql`
  `usdc-payment-link-tool/scripts/run-migrations.mjs`
  `usdc-payment-link-tool/lib/data.ts`
  `usdc-payment-link-tool/lib/auth.ts`
  `usdc-payment-link-tool/app/api/cron/reconcile/route.ts`
  `usdc-payment-link-tool/app/api/cron/settle/route.ts`
  `rust-backend/src/bin/migrate.rs`
  `rust-backend/src/db.rs`
  `rust-backend/src/models.rs`
  `rust-backend/src/auth.rs`
  `rust-backend/src/handlers/auth.rs`
  `rust-backend/src/handlers/invoices.rs`
  `rust-backend/src/handlers/cron.rs`
  `rust-backend/src/handlers/misc.rs`
