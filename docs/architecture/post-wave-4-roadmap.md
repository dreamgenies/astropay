# ASTROpay Post-Wave 4 Roadmap

This roadmap reflects the repository after the Stellar Wave 4 cleanup and hardening work. It is intentionally specific to this codebase: Next.js still owns the user-facing product and some payment runtime paths, while `rust-backend/` is the operational backend migration target.

## 1. Current System Architecture

ASTROpay is a hosted USDC payment-link and invoicing platform on Stellar.

Current runtime split:

- `usdc-payment-link-tool/` is the Next.js App Router application. It owns the merchant dashboard, public checkout pages, current checkout XDR build/submit path, TypeScript cron routes, and Vercel-facing deployment surface.
- `rust-backend/` is an Axum API service. It owns merchant auth/session APIs, invoice APIs, Horizon-backed reconciliation, Stellar webhook payment marking, migration execution, payout health/alert endpoints, and the shared money-state transition logic.
- PostgreSQL is the source of truth. SQL migrations live in `usdc-payment-link-tool/migrations/` and are applied by both `usdc-payment-link-tool/scripts/run-migrations.mjs` and `rust-backend/src/migrations.rs`.
- Stellar Horizon integration exists in both runtimes today. Rust matching/reconciliation lives in `rust-backend/src/stellar.rs`; TypeScript checkout and settlement helpers live in `usdc-payment-link-tool/lib/stellar.ts`.

Core domain tables and ownership:

- `merchants`, `sessions`, `invoices`, `payment_events`, `payouts`: created by `usdc-payment-link-tool/migrations/001_init.sql`.
- `schema_migrations`: maintained by both migration runners, with `applied_by` added by `015_schema_migrations_applied_by.sql`.
- `cron_runs`, retention config, webhook metrics, checkout attempts, and indexes are incremental Wave 4-era operational additions.

Critical backend modules:

- `rust-backend/src/auth.rs` and `rust-backend/src/handlers/auth.rs`: sessions, password hashing, login, refresh, logout.
- `rust-backend/src/handlers/invoices.rs`: invoice CRUD/status APIs.
- `rust-backend/src/money_state.rs`: atomic invoice-paid and payout-queued transition.
- `rust-backend/src/handlers/cron.rs`: reconcile, archive, retention purge, payout health, alert checks, replay, claim.
- `rust-backend/src/stellar.rs`: Horizon matching, timeout/retry behavior, memo/asset/amount validation.
- `rust-backend/src/main.rs`: routing, tracing, `x-correlation-id` propagation.

## 2. Rust Migration Status

Rust owns these production-relevant paths now:

- Merchant auth/session endpoints in `rust-backend/src/handlers/auth.rs`.
- Invoice list/create/detail/status endpoints in `rust-backend/src/handlers/invoices.rs`.
- Stellar webhook payment marking in `rust-backend/src/handlers/misc.rs`.
- Horizon reconciliation and replay in `rust-backend/src/handlers/cron.rs`.
- Shared invoice-paid/payout-queued state transition in `rust-backend/src/money_state.rs`.
- Migration runner in `rust-backend/src/migrations.rs`.
- Payout health, alert checks, orphan-payment scan, retention purge, and archival support in `rust-backend/src/handlers/cron.rs`.

Rust does not yet own these paths:

- Buyer checkout XDR build/submit. Rust currently exposes `unsupported_checkout` in `rust-backend/src/handlers/invoices.rs`; Next.js handles real checkout at `usdc-payment-link-tool/app/api/invoices/[id]/checkout/route.ts`.
- Merchant settlement signing/execution. The production settlement cron is still `usdc-payment-link-tool/app/api/cron/settle/route.ts`; Rust has settlement state validation in `rust-backend/src/settle.rs` but not full signing parity.
- Frontend rendering and wallet UX. These remain in `usdc-payment-link-tool/app/`.

The Rust migration should stay staged. Do not cut over checkout or settlement until the checklist in `docs/checkout-rust-cutover-checklist.md` is satisfied with e2e tests against testnet-compatible mocks or a controlled testnet environment.

## 3. Contribution Work Accepted Into Core

Accepted Wave 4 contributions that now add production value:

- Reconciliation keyset pagination: Rust cron scans pending invoices by `(created_at, id)` with `RECONCILE_PAGE_SIZE` in `rust-backend/src/handlers/cron.rs`; index coverage is guarded by `020_reconcile_pending_keyset_idx.sql`.
- Atomic money-state transition: `rust-backend/src/money_state.rs` centralizes `pending -> paid`, `payment_events`, and payout queueing with `ON CONFLICT (invoice_id) DO NOTHING`.
- Horizon timeout/retry hardening: `rust-backend/src/stellar.rs` uses bounded request timeout, retries 429s, and maps Horizon outages to `HORIZON_UNAVAILABLE`.
- Queue health and alert thresholds: `payout_health` and `alert_check` live in `rust-backend/src/handlers/cron.rs` and are backed by tests.
- Request correlation and secret-safe logs: `rust-backend/src/main.rs` propagates `x-correlation-id` without logging raw headers.
- Migration safety: `rust-backend/src/migrations.rs` uses `schema_migrations`, `applied_by`, and transaction-scoped advisory locking.
- Payment event retention: retention config/index migrations are guarded by `rust-backend/src/db.rs` tests; purge logic lives in `rust-backend/src/handlers/cron.rs`.
- Checkout attempt audit schema: `021_checkout_attempts.sql` and Next checkout audit writes provide useful operational visibility without changing payment behavior.

## 4. What Remains Experimental

Treat these as experimental until they have a live runtime owner, operator workflow, and tests:

- `webhook_deliveries_audit` from `014_webhook_deliveries_audit.sql`: schema exists, but the audit pipeline is not fully wired as a core operational path.
- Deployment event tracking from `008_deployment_events.sql` and `usdc-payment-link-tool/tests/deployment-events.test.js`: useful idea, but not yet a deployment gate.
- Broad issue backlog content in `docs/issue-backlog/astropay-250-issues.md`: planning source, not implementation truth.
- Observability specs in `docs/observability/`: useful direction, but dashboards/alerts must be connected to actual metrics and runbooks.
- Rust checkout and settlement parity: design intent exists, but production behavior still lives in Next.js.

## 5. Production Readiness Checklist

Backend:

- `cd rust-backend && cargo fmt --check`
- `cd rust-backend && cargo clippy -- -D warnings`
- `cd rust-backend && cargo test`
- Verify `rust-backend/src/main.rs` does not log raw request headers.
- Verify `CRON_SECRET`, `SESSION_SECRET`, `DATABASE_URL`, `ASSET_ISSUER`, and treasury keys are present in deployment env.
- Verify Sentry config is present when running production traffic.

Frontend/runtime:

- `cd usdc-payment-link-tool && npm test`
- `cd usdc-payment-link-tool && npm run typecheck`
- Run Playwright smoke tests for checkout, invoice creation, reconcile, and expiry before release.
- Verify `Authorization: Bearer <CRON_SECRET>` is used for cron calls.

Database:

- Apply migrations with one runner per environment.
- Run the clean migration test with `ASTROPAY_MIGRATION_TEST_ADMIN_DATABASE_URL` against a disposable Postgres admin database.
- Inspect indexes for reconcile, dashboard invoice lists, sessions expiry, queued payouts, payment event retention, and checkout attempts.
- Confirm rollback plans in migration comments before applying destructive follow-up migrations.

Payments:

- Confirm `transaction_hash` uniqueness via `007_invoice_transaction_hash_unique.sql`.
- Confirm all paid paths use the shared guarded transition: Rust `mark_invoice_paid_and_queue_payout`, TypeScript `markInvoicePaid`.
- Confirm invalid settlement public keys skip payout queueing but keep invoice payment state explicit.
- Confirm Horizon outage returns skip/retry behavior, not partial money-state mutation.

Security:

- Keep session cookies `HttpOnly`, `SameSite=Lax`, and `Secure` in HTTPS deployments.
- Keep login rate limits enabled unless a shared external limiter replaces them.
- Never log raw bearer tokens, cookies, database URLs, session secrets, or treasury secrets.

## 6. Settlement/Reconciliation Roadmap

Immediate:

- Keep Rust reconciliation authoritative for matching logic in `rust-backend/src/stellar.rs`.
- Keep settlement execution in `usdc-payment-link-tool/app/api/cron/settle/route.ts` until Rust has signing parity.
- Keep all money-state transitions routed through `rust-backend/src/money_state.rs` or TypeScript `markInvoicePaid`.

Next:

- Port settlement signing into Rust only after reproducing TypeScript behavior from `usdc-payment-link-tool/lib/stellar.ts`.
- Add integration tests for failed, submitted, confirmed, settled, and dead-letter payout states.
- Add replay-safe settlement confirmation for submitted payouts using bounded Horizon calls.
- Add explicit idempotency tests for concurrent reconcile, webhook, and replay against the same invoice.

Cutover:

- Run Next and Rust settlement paths side-by-side in dry-run/observe mode.
- Compare payout candidates, settlement memos, transaction hashes, and final state transitions.
- Cut traffic only after the Rust route emits the same lifecycle events and survives retry storms.

## 7. Observability Roadmap

Current useful signals:

- Request tracing and `x-correlation-id` propagation in `rust-backend/src/main.rs`.
- `cron_runs` audit rows for reconcile and retention jobs.
- `payment_events` lifecycle rows.
- Payout health endpoint in `rust-backend/src/handlers/cron.rs`.
- Webhook correlation metrics endpoint in `rust-backend/src/handlers/cron.rs`.
- Specs in `docs/observability/invoice-payout-lifecycle-metrics-spec.md` and `docs/observability/dashboard-payment-success-payout-latency.md`.

Next signals to operationalize:

- Payment success rate by invoice creation cohort.
- Time from invoice creation to paid.
- Time from paid to payout queued.
- Time from payout queued to settled.
- Horizon unavailable count and retry exhaustion count.
- Reconcile scanned count, match count, mismatch count, and skipped Horizon count.
- Queue age percentiles for queued/failed/submitted payouts.
- Checkout attempt failure rate from `checkout_attempts`.

Required runbooks:

- Horizon outage response.
- Stuck pending invoices.
- Stuck queued payouts.
- Settlement failure/dead-letter handling.
- Webhook replay/duplicate delivery investigation.
- Migration failure rollback.

## 8. AI Product Opportunities

AI should support operators and merchants; it should not make money-state decisions.

Practical opportunities:

- Merchant invoice assistant: draft invoice descriptions and checkout metadata inside the dashboard.
- Support copilot: summarize an invoice lifecycle from `invoices`, `payment_events`, `payouts`, `cron_runs`, and webhook metrics.
- Reconciliation investigator: explain why an invoice is pending, mismatched, expired, paid, queued, or dead-lettered using deterministic data.
- Risk explanation layer: flag suspicious mismatch patterns without automatically blocking payment.
- Operator runbook assistant: turn alert payloads into next-step checklists using the runbooks above.

Guardrails:

- AI output must not mutate invoice, payout, settlement, or auth state.
- AI summaries must cite concrete row IDs, event types, and timestamps.
- Never send treasury secret keys, session cookies, password hashes, or bearer tokens to an AI provider.

## 9. Scraping/Data Intelligence Opportunities

Scraping and data intelligence should improve reliability and market awareness, not replace Horizon or database truth.

Useful opportunities:

- Horizon health monitor: periodically sample Horizon latency/error rates and feed alert thresholds.
- Stellar asset issuer watch: monitor configured `ASSET_ISSUER` and public issuer metadata changes.
- Fee/latency intelligence: collect testnet/mainnet transaction confirmation latency for settlement planning.
- Competitor/market intelligence: track public pricing, payment method messaging, and onboarding flows for payment-link products.
- Documentation drift detection: monitor Stellar/Horizon docs for API changes relevant to payment operations and transaction lookup.

Constraints:

- Do not scrape private dashboards or authenticated user data.
- Do not use scraped data as payment proof.
- Persist raw external observations separately from money-state tables.
- Convert useful intelligence into alerts, tests, or docs before treating it as operational truth.

## 10. 30-Day, 60-Day, and 90-Day Roadmap

### 30 Days

- Stabilize the Rust backend surface already in production scope.
- Keep `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `npm test`, and `npm run typecheck` green.
- Finish checkout attempt audit verification around `021_checkout_attempts.sql`.
- Add e2e coverage for invoice paid, invoice expired, reconcile replay, and duplicate payment delivery.
- Build dashboards for payout queue age, stuck pending invoices, Horizon unavailable counts, and checkout attempt failures.
- Document operator runbooks for reconcile, settlement, Horizon outage, and dead-letter payouts.

### 60 Days

- Implement Rust settlement signing behind a feature flag or dry-run mode.
- Add side-by-side comparison between TypeScript settlement and Rust settlement candidates.
- Add integration tests for payout submitted/confirmed/settled transitions.
- Promote webhook audit/runtime delivery metrics from experimental to core if the route writes and runbooks are complete.
- Add migration CI that applies the full chain to clean disposable Postgres on a scheduled job.
- Move login rate limiting to shared storage if running more than one Rust backend instance.

### 90 Days

- Cut settlement execution to Rust if parity and rollback gates pass.
- Decide whether checkout XDR build/submit should move to Rust or remain in Next.js with a hardened service boundary.
- Make observability dashboards release-blocking for payment-critical deploys.
- Add AI support tools for operator investigation, not autonomous remediation.
- Add data intelligence jobs for Horizon health and Stellar ecosystem drift.
- Archive or remove unowned experimental contribution artifacts that have not gained runtime owners, tests, or operator workflows.

## Decision Rule

Wave 4 added useful parts, but production value only exists when a contribution is wired into a runtime path, covered by tests, documented for operators, and reversible. Anything else stays experimental until it earns those properties.
