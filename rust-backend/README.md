# ASTROpay Rust Backend

This service is the beginning of the backend migration out of Next.js route handlers.

What it currently owns:

- merchant registration, login, logout, session refresh, and cookie-backed sessions
- invoice creation, listing, detail lookup, and status lookup
- Horizon-backed reconciliation for pending invoices
- webhook-driven payment marking (`/api/webhooks/stellar`)
- a Rust migration runner that reuses the existing SQL migrations

Reconciliation and the Stellar webhook validate each merchant `settlement_public_key` with Stellar strkey decoding before inserting into `payouts`. Invalid keys skip payout queueing (invoice still marked paid) and emit a `payment_events` row with `event_type = payout_skipped_invalid_destination`. Run `cargo test` for strkey coverage.

What is intentionally not faked yet:

- buyer XDR generation/submission for checkout
- merchant settlement cron

Those routes return `501 Not Implemented` in the Rust service until the Stellar transaction logic is ported properly.

## Session cookie security

The `Secure` flag on the `astropay_session` cookie is driven by `APP_URL`:

- `APP_URL` starts with `https://` → `secure_cookies = true` → `Secure` flag is set.
- `APP_URL` starts with `http://` (local dev) → `secure_cookies = false` → `Secure` flag is absent.

The cookie is always `HttpOnly` and `SameSite=Lax` regardless of environment.

**Verify:** `cargo test session_cookie` runs four unit tests covering the secure flag, insecure flag, http-only invariant, and name/path stability — no running server or database required.

## Session refresh (`POST /api/auth/refresh`)

A valid session can be extended without re-authenticating:

```
POST /api/auth/refresh
Cookie: astropay_session=<token>
```

Behavior:
- If the cookie is present and the session row is unexpired, `expires_at` is extended by 30 days and a fresh JWT cookie is returned with `{ "ok": true }`.
- If the cookie is missing or the session is expired/invalid, returns `401 AUTH_SESSION_REQUIRED`.
- Does **not** create a new session row — only extends the existing one. Logout still invalidates the same row.
- Does **not** accept an already-expired token (JWT `exp` validation runs first).

**Verify:** call `POST /api/auth/refresh` with a valid session cookie and confirm the response sets a new cookie with a later expiry. Call it without a cookie and confirm `401`.

## Login rate limiting (`POST /api/auth/login`)

Login attempts are throttled with **two independent in-memory sliding windows** (per process). They are tuned so a few wrong passwords do not brick an account: only **failed** logins count toward the email bucket, while the IP bucket limits overall volume from a single client. Successful login **clears** the failure streak for that normalized email. Set the corresponding `*_MAX` env var to **`0`** to disable either limiter.

| Variable | Default | Meaning |
| --- | --- | --- |
| `LOGIN_RATE_IP_WINDOW_SECS` | `600` | Window length for per-IP counting (seconds). |
| `LOGIN_RATE_IP_MAX` | `80` | Max login POSTs from one IP within the IP window (`0` = off). |
| `LOGIN_RATE_EMAIL_WINDOW_SECS` | `900` | Window for failed attempts per trimmed/lowercased email. |
| `LOGIN_RATE_EMAIL_FAIL_MAX` | `12` | Max **failed** credential checks per email in that window (`0` = off). |

When a limit is exceeded the API returns **429 Too Many Requests** with:
## Merchant registration and wallet keys

`POST /api/auth/register` trims `stellar_public_key` and `settlement_public_key` before storage. Registration returns **409 Conflict** if either incoming key already appears on **any** existing merchant in **either** column (same rule for both keys so a key cannot be “shared” as someone else’s business wallet or settlement wallet). That prevents ambiguous ownership of payouts and identity.

Apply migration `002_merchant_wallet_key_indexes.sql` (via `cargo run --bin migrate`) so those lookups stay fast as the merchant table grows.

**Verify:** `cargo test wallet_conflict` (pure logic). With Postgres running, register a merchant, then register again with the same stellar or settlement key (or swap roles) and expect **409** with the conflict message.
## HTTP 401 (authentication) JSON contract

All **401 Unauthorized** responses use a single structured shape so clients can branch on `error.code` instead of parsing free-form strings:

```json
{
  "error": {
    "code": "AUTH_RATE_LIMITED",
    "message": "Too many login attempts. Please wait before trying again.",
    "retryAfterSeconds": 42
  }
}
```

and a **`Retry-After`** header (seconds). The client IP comes from Axum `ConnectInfo` (the TCP peer). Behind a reverse proxy you may need to forward the real client IP or swap this for `X-Forwarded-For` in a later change. For multiple app instances, use a shared store (e.g. Redis) instead of this in-memory map.

**Verify:** `cargo test login_rate_limit` (unit tests for IP / email windows and disabled mode).
    "code": "AUTH_INVALID_CREDENTIALS",
    "message": "Invalid credentials"
  }
}
```

Stable `code` values (do not depend on `message` for logic):

| `code` | When |
| --- | --- |
| `AUTH_INVALID_CREDENTIALS` | Login rejected (unknown email or wrong password; same response for both). |
| `AUTH_SESSION_REQUIRED` | Cookie session missing, invalid, expired, or not accepted for the route (e.g. `/api/auth/me`, invoice routes). |
| `AUTH_CRON_SECRET_MISMATCH` | `Authorization: Bearer …` does not match `CRON_SECRET` for cron/webhook routes. |

Other HTTP errors (400, 404, 409, 501, 500) still use the legacy form `{ "error": "<string>" }` until migrated.

**Quick check:** with the server running, `curl -s -o /dev/stderr -w "%{http_code}" http://127.0.0.1:8080/api/auth/me` should print `401` and a JSON body whose `error.code` is `AUTH_SESSION_REQUIRED`.
## Cron run audit (`cron_runs`)

Migration `004_cron_runs.sql` adds an append-only table keyed by `job_type` (`reconcile` \| `settle`) with JSONB `metadata` (response-shaped summary: `scanned` / `results` or `processed` / `results`) and optional `error_detail`. Successful Rust reconcile runs insert a row (if the handler returns `Ok` after scanning; Horizon or DB errors before that point skip audit persistence). Settle (still not implemented) inserts `success = false` with the error text before returning `501`. Audit insert failures are logged and do not change the HTTP status.

**Verification:** `cargo test` checks that `004_cron_runs.sql` defines the table and index; apply migrations with `cargo run --bin migrate` before relying on audit rows in development.

## Payout settlement retry tracking (`payouts`)

### Column definitions

Migrations `005_payout_dead_letter.sql` and `007_payout_attempt_counters_and_last_error.sql` enable operators to debug and investigate payout failures:

| Column | Type | Purpose |
| --- | --- | --- |
| `failure_count` | `INTEGER` | Number of settlement attempts that failed for this payout (added by migration 005). |
| `last_failure_at` | `TIMESTAMPTZ` | Timestamp of the most recent failed attempt (added by migration 005). |
| `last_failure_reason` | `TEXT` | Error message from the most recent failure, for debugging and operator visibility (added by migration 007). |

### Retry and dead-letter workflow

1. **Initial payout:** Invoice is marked `paid` after Horizon webhook detection. Reconciliation inserts a payout row with `status = 'queued'` and all failure fields `NULL`.

2. **Settlement failure:** When a settlement attempt fails (still not fully implemented in Rust), the `failure_reason` is captured and the payout status is set to `'failed'`.

3. **Cron settle handler retry:** `GET /api/cron/settle` (with `Authorization: Bearer <CRON_SECRET>`) scans payouts with `status = 'failed'`:
   - Increments `failure_count` by 1.
   - Sets `last_failure_at = NOW()`.
   - Sets `last_failure_reason` to the stored error message.
   - If `failure_count < PAYOUT_DEAD_LETTER_THRESHOLD` (currently 5), resets status to `'queued'` for the next settlement attempt.
   - If `failure_count >= 5`, escalates to `status = 'dead_lettered'` and inserts a row into `payout_dead_letters` for operator review.

4. **Operator resolution:** Dead-lettered payouts require manual investigation and resolution (e.g., invalid destination key, network issues, insufficient funds). They do not auto-retry.

### Indexing

Migration `007` creates an index on `payouts(last_failure_at DESC NULLS LAST)` to support queries that discover recently-failed payouts for monitoring or manual retry workflows.

### Example queries for operators

```sql
-- Find payouts that are not yet dead-lettered but have failed at least once:
SELECT id, invoice_id, merchant_id, failure_count, last_failure_at, last_failure_reason
FROM payouts
WHERE failure_count > 0 AND status != 'dead_lettered'
ORDER BY last_failure_at DESC;

-- Find all dead-lettered payouts for a merchant:
SELECT p.id, p.invoice_id, p.failure_count, p.last_failure_reason,
       dl.created_at
FROM payouts p
INNER JOIN payout_dead_letters dl ON p.id = dl.payout_id
WHERE p.merchant_id = $1
ORDER BY dl.created_at DESC;
```

**Verification:** `cargo test` checks that migration 007 defines `last_failure_reason` and the index; apply migrations with `cargo run --bin migrate`.

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
