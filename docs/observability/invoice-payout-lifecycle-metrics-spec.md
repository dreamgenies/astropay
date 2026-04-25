# Invoice And Payout Lifecycle Metrics Spec

This spec defines the counters, gauges, and timings that matter for ASTROpay invoice and payout operations as the repo exists today.

It is intentionally grounded in the current split runtime:

- Next.js still owns checkout XDR generation and payout settlement execution.
- Rust owns invoice CRUD, webhook payment marking, and reconciliation for the migration path.
- Both runtimes touch shared invoice and payout tables, so metrics must be keyed to lifecycle transitions, not to "which app logged it".

## Goals

- Measure whether invoices are being created, paid, expired, and settled at healthy rates.
- Measure whether payouts are being queued and drained without getting stuck.
- Separate true business-state transitions from duplicate scans, retries, and stale records.
- Preserve enough dimensions for debugging without leaking payment-sensitive or secret data.

## Metric Design Rules

- Emit metrics when the system attempts or completes a lifecycle transition, not on every page view.
- Prefer low-cardinality labels such as `source`, `status`, `result`, and `reason`.
- Never label by raw `invoice_id`, `public_id`, transaction hash, wallet address, email, cookie, or secret.
- Use the same lifecycle vocabulary across runtimes: invoice `pending|paid|expired|settled|failed`; payout `queued|submitted|settled|failed`.
- Count duplicate or stale events separately from successful transitions so retries do not look like money movement.

## Common Labels

Use only the labels that apply to a metric:

- `source`: `nextjs_api`, `rust_api`, `webhook`, `cron_reconcile`, `cron_settle`
- `result`: `success`, `error`, `duplicate`, `stale`, `skipped`
- `reason`: bounded values such as `invalid_settlement_public_key`, `payout_already_queued`, `invoice_not_found`, `invoice_not_pending`, `expired_before_payment`, `settlement_submit_failed`, `upstream_horizon_error`, `db_error`, `auth_error`
- `status_from`: prior lifecycle state when known
- `status_to`: resulting lifecycle state when known
- `job_type`: `reconcile`, `settle`

## Invoice Metrics

### Counters

`astropay_invoice_created_total`

- Increment when a new invoice row is inserted successfully.
- Labels: `source`, `result=success`

`astropay_invoice_status_transition_total`

- Increment once per successful invoice status change.
- Labels:
  `source`, `status_from`, `status_to`, `result=success`
- Required transitions today:
  `pending->paid`, `pending->expired`, `paid->settled`
- Optional terminal transition if introduced later:
  `pending->failed`, `paid->failed`

`astropay_invoice_payment_detection_total`

- Count payment detection attempts from reconcile scans and webhook delivery.
- Labels:
  `source`, `result`, `reason`
- Recommended meanings:
  `success` for a transition from `pending` to `paid`
  `duplicate` when the invoice is already `paid` or `settled`
  `stale` when the invoice is already `expired` or `failed`
  `error` for DB, auth, or upstream failures

`astropay_invoice_expiration_total`

- Count invoice expirations caused by lifecycle processing, not passive reads.
- Labels:
  `source`, `result`, `reason`
- `reason` should normally be `expired_before_payment`

### Gauges

`astropay_invoices_by_status`

- Snapshot of current invoice backlog by status.
- Labels: `status`
- Minimum statuses: `pending`, `paid`, `expired`, `settled`, `failed`

`astropay_invoice_oldest_pending_age_seconds`

- Age in seconds of the oldest `pending` invoice.
- No labels, or only `source=shared_db`
- Use for alerting on stuck pending invoices.

`astropay_invoice_oldest_paid_unsettled_age_seconds`

- Age in seconds of the oldest invoice still `paid` but not `settled`.
- No labels, or only `source=shared_db`
- This is the queue-health signal for settlement lag.

### Timings

`astropay_invoice_time_to_payment_seconds`

- Histogram from invoice creation to invoice payment detection (`paid_at - created_at`).
- Labels: `source`
- Record only on first successful `pending->paid` transition.

`astropay_invoice_time_to_expiry_seconds`

- Histogram from invoice creation to `expired`.
- Labels: `source`
- Useful for checking whether configured expiry windows match real behavior.

`astropay_invoice_time_to_settlement_seconds`

- Histogram from invoice creation to invoice `settled`.
- Labels: `source`

`astropay_invoice_paid_to_settled_seconds`

- Histogram from `paid_at` to `settled_at`.
- Labels: `source`
- This is the cleanest customer-impact latency for merchant settlement.

## Payout Metrics

### Counters

`astropay_payout_queue_attempt_total`

- Increment when invoice payment handling decides whether a payout should be queued.
- Labels:
  `source`, `result`, `reason`
- Expected meanings:
  `success` when a payout row is newly inserted
  `duplicate` with `reason=payout_already_queued`
  `skipped` with `reason=invalid_settlement_public_key`
  `error` for DB failures

`astropay_payout_status_transition_total`

- Increment once per successful payout state change.
- Labels:
  `source`, `status_from`, `status_to`, `result=success`
- Required transitions today:
  `queued->submitted`, `submitted->settled`, `queued->failed`, `failed->submitted`, `failed->settled`

`astropay_payout_settlement_attempt_total`

- Count settlement execution attempts.
- Labels:
  `source`, `result`, `reason`
- Recommended meanings:
  `success` when the payout is submitted or settled
  `stale` when the related invoice is no longer in a payable state
  `error` when submission fails or config is invalid

### Gauges

`astropay_payouts_by_status`

- Snapshot of payout backlog by status.
- Labels: `status`
- Minimum statuses: `queued`, `submitted`, `settled`, `failed`

`astropay_payout_oldest_queued_age_seconds`

- Age in seconds of the oldest `queued` payout.
- Primary alert signal for stuck payout processing.

`astropay_payout_oldest_failed_age_seconds`

- Age in seconds of the oldest unresolved `failed` payout.
- Useful for identifying retry or operator backlog.

### Timings

`astropay_payout_queue_to_submit_seconds`

- Histogram from payout creation to `submitted`.
- Labels: `source`

`astropay_payout_queue_to_settled_seconds`

- Histogram from payout creation to `settled`.
- Labels: `source`

`astropay_payout_submit_to_settled_seconds`

- Histogram from submission to chain-confirmed settlement when both timestamps are available.
- Labels: `source`

## Cron And Job Metrics

These support lifecycle metrics without replacing them.

`astropay_cron_run_total`

- Count each reconcile or settle job invocation that reaches handler logic.
- Labels:
  `job_type`, `result`

`astropay_cron_run_duration_seconds`

- Histogram for full reconcile and settle handler duration.
- Labels:
  `job_type`, `result`

`astropay_cron_items_scanned`

- Histogram for invoice or payout batch size per run.
- Labels:
  `job_type`

## Edge-Case Handling

The metrics contract must handle awkward states explicitly:

- Duplicate payment detection:
  If webhook delivery or reconcile sees a payment for an already-paid invoice, increment `astropay_invoice_payment_detection_total{result="duplicate"}` and do not increment transition timings again.
- Stale invoice state:
  If a payment or settlement attempt targets `expired`, `failed`, or otherwise disconnected records, count `result="stale"` and keep lifecycle counters unchanged.
- Invalid settlement destination:
  If the merchant settlement key is missing or invalid, count `astropay_payout_queue_attempt_total{result="skipped",reason="invalid_settlement_public_key"}`. The invoice may still transition to `paid`.
- Payout already queued:
  If the payout insert conflicts on `invoice_id`, count `result="duplicate",reason="payout_already_queued"` rather than pretending queue growth happened.
- Partial success:
  If invoice payment is marked but payout queue insertion is skipped or duplicated, record the invoice `pending->paid` transition and separately record the payout queue outcome.
- Cron audit failure:
  If `cron_runs` persistence fails, log the error but do not count the business transition twice. Cron/job metrics should still reflect the handler result.
- Missing invoice:
  For webhooks or status transitions against unknown invoices, use error counters with `reason="invoice_not_found"`; do not create phantom lifecycle transitions.
- Unknown lifecycle value:
  If a runtime encounters an unsupported status string, map it to `result="error",reason="unknown_status"` and alert through logs. Do not coerce it into a known business state.

## Alerting Guidance

These metrics support later alert work:

- Alert when `astropay_invoice_oldest_pending_age_seconds` exceeds the expected invoice expiry window by a meaningful buffer.
- Alert when `astropay_payout_oldest_queued_age_seconds` exceeds the expected settlement cadence.
- Alert when `astropay_payout_queue_attempt_total{result="skipped",reason="invalid_settlement_public_key"}` spikes, because that points to merchant configuration issues.
- Alert when `astropay_invoice_payment_detection_total{result="error"}` or `astropay_payout_settlement_attempt_total{result="error"}` crosses a sustained threshold.

## Verification

- Confirm every documented transition exists in code paths under `usdc-payment-link-tool/lib/data.ts`, `app/api/cron/*`, `app/api/webhooks/stellar/route.ts`, and the Rust handler equivalents.
- Confirm label values are bounded and do not include invoice IDs, hashes, wallet addresses, email addresses, cookies, or secrets.
- Confirm first-success timings are only recorded once per lifecycle transition and are not re-emitted on retries or duplicate deliveries.

## Log Redaction Rules

Logs are useful for debugging but must never expose secrets or high-risk identifiers. The following rules apply to both runtimes.

### What Must Never Appear in Logs

| Category | Examples | Risk |
|---|---|---|
| Wallet secret keys | `PLATFORM_TREASURY_SECRET_KEY`, any `S...` Stellar key | Full fund loss if leaked |
| Session tokens | Raw JWT from `astropay_session` cookie | Account takeover |
| Cookie header values | `Cookie: astropay_session=eyJ...` | Session hijack |
| Bearer tokens | `Authorization: Bearer <CRON_SECRET>` value | Cron/webhook auth bypass |
| Database credentials | `postgres://user:pass@host/db` | DB access |
| Webhook rotation secret | `WEBHOOK_SECRET_SECONDARY` | Webhook auth bypass |

### What Is Safe to Log

- Stellar **public** keys (`G...` 56-char strkeys) — these are on-chain and public
- Invoice `public_id` (`inv_<hex>`) — opaque, not a secret
- Transaction hashes — on-chain and public
- Invoice status strings (`pending`, `paid`, `settled`, `expired`, `failed`)
- Payout IDs (UUIDs) — internal references, not secrets
- HTTP status codes, error codes, and structured error messages that do not embed raw values

### Rust Backend — `redact` Module

`rust-backend/src/redact.rs` provides:

- **`Redacted<T>`** — newtype wrapper; `Debug` and `Display` emit `[REDACTED]`. Applied to `Config` fields: `session_secret`, `cron_secret`, `database_url`, `platform_treasury_secret_key`.
- **`redact_authorization_header(value)`** — returns `"Bearer [REDACTED]"` or `"[REDACTED]"`.
- **`redact_cookie_header(header)`** — preserves cookie names, replaces all values with `[REDACTED]`.
- **`redact_connection_string(dsn)`** — replaces `user:pass@` with `[REDACTED]@`, keeps host and db name.

The startup log in `main.rs` uses `redact_connection_string` so the `DATABASE_URL` host is visible but credentials are not.

### Next.js — `lib/redact.ts`

`usdc-payment-link-tool/lib/redact.ts` provides:

- **`redactSecret(value)`** — redacts bearer tokens and raw secrets.
- **`redactCookieHeader(header)`** — preserves cookie names, replaces values.
- **`redactConnectionString(dsn)`** — same as Rust helper.
- **`redactWalletKey(key)`** — passes through public keys (`G...` 56 chars), redacts everything else.

### Verification Checklist

- [ ] `Config` debug output does not contain `session_secret`, `cron_secret`, or `database_url` credentials — covered by `Redacted<T>` wrapper and tests in `config.rs`.
- [ ] No `tracing::info!` or `tracing::error!` call passes a raw secret as a field value.
- [ ] Cookie headers are never logged verbatim — use `redact_cookie_header` before any log call.
- [ ] Authorization headers are never logged verbatim — use `redact_authorization_header`.
- [ ] Sentry `beforeSend` scrubbing is configured separately if structured event data needs scrubbing.
- [ ] Tests in `rust-backend/src/redact.rs` and `usdc-payment-link-tool/lib/redact.test.ts` pass in CI.
