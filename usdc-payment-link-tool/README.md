# ASTROpay v2

ASTROpay is a hosted USDC payment-link and invoicing product on Stellar.

This repo now also contains a Rust backend under `../rust-backend` for the API migration away from Next.js route handlers. That migration is real but not complete yet: auth, sessions, invoice CRUD, webhook payment marking, and SQL migrations are implemented in Rust; Stellar checkout XDR generation and the cron settlement/reconciliation flows are not.

This v2 upgrade replaces the fake-MVP shortcuts with real product foundations:
- PostgreSQL-backed merchants, sessions, invoices, payouts, and audit events
- merchant authentication
- persistent invoices and hosted checkout pages
- Horizon-based cron reconciliation
- platform-custody fee capture and merchant net settlement
- deployment config for Vercel or Railway

## What changed from v1

The original MVP let buyers pay directly to the merchant wallet and used browser polling to detect settlement. That is fine for a demo, but it breaks the moment you try to capture platform fees, audit payments, or reconcile invoices after the checkout tab closes.

ASTROpay v2 changes the money flow:
1. Buyer pays the invoice **gross amount** in USDC to the **platform treasury**.
2. ASTROpay verifies the on-chain payment by destination, asset, amount, and transaction memo.
3. ASTROpay marks the invoice as `paid` and creates a queued payout.
4. Settlement cron sends the **net amount** to the merchant settlement wallet.
5. ASTROpay retains the platform fee.

This is the only honest way to implement fee-splitting without lying to yourself in the UI.

## Tech stack

- Next.js App Router + TypeScript
- Node runtime for the current frontend and legacy route handlers
- Rust + Axum for the backend migration service in `../rust-backend`
- PostgreSQL via `pg`
- Stellar SDK + Horizon API
- Freighter wallet for buyer signing
- Zod for request validation
- JOSE + signed httpOnly cookies for merchant sessions

## Core entities

### merchants
Stores merchant identity and settlement wallet.

### sessions
Stores long-lived merchant sessions (JWT references `sessions.id`). Auth checks use the row’s primary key plus `expires_at > NOW()`.

Indexes evolve across migrations: `001_init.sql` creates starter btree indexes; **`002_session_expiry_indexes.sql`** replaces them with `(expires_at, id)` for global expiry sweeps and `(merchant_id, expires_at)` for merchant-scoped cleanup (the latter’s left prefix still supports `WHERE merchant_id = $1` alone). Run `npm run db:migrate` or `cargo run --bin migrate` from `rust-backend` so 002 is applied in production.

### invoices
Stores hosted invoice details and lifecycle state.

**`public_id` format** — `inv_[0-9a-f]{16}` (the prefix `inv_` followed by exactly 16 lowercase hex characters, e.g. `inv_3f8a1b2c4d5e6f7a`). Migration **`016_invoice_public_id_format.sql`** adds a `CHECK` constraint enforcing this shape at the database level. Both generators already produce conforming values (`generatePublicId()` in `lib/security.ts` and `generate_public_id()` in `rust-backend/src/auth.rs`).

**`metadata` (JSONB)** — arbitrary key/value data for integrations. Today nothing in this repo filters invoices in SQL by `metadata`; default rows use a small object (for example `{"product":"ASTROpay"}`). Migration **`003_invoice_metadata_jsonb_index_plan.sql`** documents when to add expression indexes vs `GIN (metadata jsonb_path_ops)` vs key-specific btree indexes, and installs a `COMMENT ON COLUMN` pointer for operators. **Do not add JSONB indexes preemptively** (write amplification and unused GIN are common pitfalls); add a migration alongside the first real `WHERE metadata …` query.

### payment_events
Append-only audit trail for payment and settlement events.

### payouts
Tracks merchant settlement jobs and outcomes.

### cron_runs
Append-only audit of each **`GET /api/cron/reconcile`** and **`GET /api/cron/settle`** run (after auth succeeds). `metadata` stores the same shape as the JSON response body (`scanned` + `results` or `processed` + `results`); `error_detail` is set when the handler returns `500` (for example DB or config errors). `recordCronRun` swallows insert failures so a broken audit table cannot break cron. Add retention or partitioning separately if volume grows.

## Invoice lifecycle

- `pending` — created but not paid
- `paid` — on-chain gross payment detected
- `expired` — invoice expired before payment
- `settled` — merchant net payout completed
- `failed` — terminal failure state

## Local setup

1. Copy env file:
   ```bash
   cp .env.example .env.local
   ```
2. Start PostgreSQL locally.
3. Set `DATABASE_URL` and Stellar treasury variables.
4. Install dependencies:
   ```bash
   npm install
   ```
5. Run migrations:
   ```bash
   npm run db:migrate
   ```
6. Start dev server:
   ```bash
   npm run dev
   ```

### Buyer checkout (Freighter) and payment errors

On `/pay/[publicId]`, failed wallet connect, signing, XDR build, or Horizon submit steps show a **dedicated payment failure panel** (title, short explanation, bullet actions, optional technical detail) instead of a single raw error line. API contracts for `POST /api/invoices/:id/checkout` are unchanged.

**Verify**

- Automated: `npm run test` (covers shared formatting, validation, HTTP helpers, and buyer-facing payment failure copy).
- Manual: open a checkout link with Freighter disconnected → use **Connect Freighter** → expect the “Freighter is not available” style panel, **Dismiss**, then connect and use **Pay now** → cancel the Freighter signature → expect cancellation copy and **Pay now** again.

## Rust backend setup

1. Copy the Rust backend env file:
   ```bash
   cd ../rust-backend
   cp .env.example .env.local
   ```
2. Reuse the same Postgres database and core env vars as the Next app.
3. Run the Rust migration runner:
   ```bash
   cargo run --bin migrate
   ```
4. Start the Rust API service:
   ```bash
   cargo run
   ```

Current Rust backend coverage:
- `POST /api/auth/register`
- `POST /api/auth/login`
- `POST /api/auth/logout`
- `GET /api/auth/me`
- `GET /api/invoices`
- `POST /api/invoices`
- `GET /api/invoices/:id`
- `GET /api/invoices/:id/status`
- `POST /api/webhooks/stellar`

Current Rust backend gaps:
- `POST /api/invoices/:id/checkout`
- `GET /api/cron/reconcile`
- `GET /api/cron/settle`

Those still need a proper Stellar transaction port and currently return `501 Not Implemented` in the Rust service.

## Environment variables

- `APP_URL`
- `NEXT_PUBLIC_APP_URL`
- `DATABASE_URL`
- `PGSSL`
- `SESSION_SECRET`
- `CRON_SECRET`
- `STELLAR_NETWORK`
- `NEXT_PUBLIC_STELLAR_NETWORK`
- `HORIZON_URL`
- `NETWORK_PASSPHRASE`
- `ASSET_CODE`
- `ASSET_ISSUER`
- `PLATFORM_TREASURY_PUBLIC_KEY`
- `PLATFORM_TREASURY_SECRET_KEY`
- `PLATFORM_FEE_BPS`
- `INVOICE_EXPIRY_HOURS`

## API surface

### Merchant auth
- `POST /api/auth/register`
- `POST /api/auth/login`
- `POST /api/auth/logout`

### Invoices
- `GET /api/invoices`
- `POST /api/invoices`
- `GET /api/invoices/[id]`
- `POST /api/invoices/[id]/checkout`
- `GET /api/invoices/[id]/status`

### Reconciliation and settlement
- `GET /api/cron/reconcile`
- `GET /api/cron/settle`
- `POST /api/webhooks/stellar`

When an invoice is marked paid, the server validates the merchant `settlement_public_key` as a Stellar Ed25519 account strkey (checksum-valid `G...`) before inserting a `payouts` row. If the key is missing or invalid, the invoice still becomes paid (funds were received on-chain), but no payout is queued; a `payment_events` row is written with `event_type = payout_skipped_invalid_destination` instead.

Successful reconcile entries for paid invoices include `payoutQueued` (boolean) and `payoutSkipReason` (`null`, `invalid_settlement_public_key`, or `payout_already_queued` when the payout row already existed). If another worker wins the same `pending` to `paid` race, the Rust path reports `invoice_already_transitioned` and does not write duplicate payment events or payouts. The Stellar webhook response includes the same `payoutQueued` / `payoutSkipReason` fields when it transitions an invoice from `pending` to `paid`.

The settlement cron rejects queued payouts whose stored `destination_public_key` fails the same validation and marks them failed so they are not submitted to Horizon.

**Verification:** `cd rust-backend && cargo test` runs StrKey validation unit tests. `cd usdc-payment-link-tool && npm run typecheck` checks the TypeScript integration.

## Vercel deployment

1. Push the repo to GitHub.
2. Import the repo into Vercel.
3. Provision Postgres through a Vercel Marketplace provider, or connect an external Postgres instance. Vercel injects database environment variables when connected through its Postgres integrations.
4. Set all env vars from `.env.example`.
5. Configure the cron secret in Vercel env vars.
6. Deploy.

Notes:
- Vercel cron jobs call your function paths on a schedule configured in `vercel.json`.
- Vercel Functions are created from App Router route handlers under `app/api/.../route.ts`.
- On Hobby, Vercel cron jobs have important restrictions, including at-most-daily execution for many schedules and relaxed timing within the scheduled hour. Check your plan before relying on high-frequency reconciliation there.

Because of that last point, Railway or another worker-friendly host is usually better for tighter reconciliation intervals.

## Railway deployment

1. Create a new Railway project.
2. Add a Postgres service or connect an external Postgres instance.
3. Add the app service from this repo.
4. Set all required environment variables.
5. Railway can host Next.js directly, and its deployment templates support Next.js app deployment.
6. Run `npm run db:migrate` before first production traffic, or allow the provided start command to do it.

## Security notes

- Merchant sessions are stored in httpOnly cookies.
- Passwords are hashed with Node `crypto.scrypt`.
- Cron and webhook endpoints require `Authorization: Bearer <CRON_SECRET>`.
- The client never gets to declare an invoice paid.
- Merchant registration rejects malformed Stellar keys; payout destinations are validated again when payouts are queued so bad data cannot create settlement jobs.
- Production treasury secrets must remain server-only. See [`docs/treasury-key-custody.md`](../docs/treasury-key-custody.md) for storage, rotation, and verification requirements.

## Operational caveats

This is a strong v2, not the final state.

Still missing before serious scale:
- rate limiting
- observability and alerting
- invoice search/filtering
- email receipts
- dispute handling
- SEP-10 or wallet-native merchant auth
- idempotency keys for checkout and admin actions
- robust Horizon pagination/streaming for high volume
- better payout retry strategy and dead-letter handling
- compliance and custody review before handling meaningful funds

## Docs used for current implementation assumptions

- Next.js App Router route handlers are the supported way to implement server endpoints in the `app` directory.
- Next.js environment variables are loaded from `.env*`, and browser-exposed vars must use the `NEXT_PUBLIC_` prefix.
- Stellar exposes transaction and payment data through Horizon, including transaction listing and memo retrieval through transaction endpoints.
- Freighter provides web app APIs for connecting a wallet and signing Stellar transactions.
