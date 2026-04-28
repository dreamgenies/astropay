# Webhook provider assumptions and failure modes

This document covers what the webhook integration does and does not guarantee, what assumptions the code makes about the Stellar/Horizon provider, and how each failure mode is handled. Read this before changing any of the reconciliation, settle, or webhook routes.

## Overview of the payment detection pipeline

There are two independent paths that can mark an invoice `paid`:

| Path | Entry point | Who calls it |
|---|---|---|
| Cron reconcile | `GET /api/cron/reconcile` | Scheduled job (Vercel cron, Railway cron, etc.) |
| Stellar webhook | `POST /api/webhooks/stellar` | External webhook provider (e.g. Stellar Quest, custom relay) |
| Manual replay | `POST /api/cron/reconcile/replay` | Operator, on demand |

All three paths converge on the same `markInvoicePaid` / DB transaction logic. The invoice status update uses `WHERE status = 'pending'` so a double-fire from two paths arriving simultaneously is safe — only one write wins.

## What the webhook endpoint does

`POST /api/webhooks/stellar` (TS: `app/api/webhooks/stellar/route.ts`, Rust: `handlers/misc.rs`) receives a JSON body from an external provider and marks the matching invoice paid.

**What it does:**
- Authenticates the caller with `Authorization: Bearer <CRON_SECRET>` or `<WEBHOOK_SECRET_SECONDARY>` (issue #159: secret rotation support).
- Issue #162: Replay detection — if the caller sends an `X-Delivery-Id` header, the service checks if that delivery ID was already processed within the replay window (default 5 minutes). Duplicate deliveries are rejected with `{"received": true, "duplicate": true}` before any invoice mutation occurs.
- Looks up the invoice by `publicId`.
- If the invoice is `pending`, calls `markInvoicePaid` which atomically updates the invoice, inserts a `payment_events` row, and queues a payout row.
- Returns `received: true` regardless of whether the invoice was already paid (idempotent read; no mutation if not pending).

**What it does not do:**
- It does not verify the `transactionHash` against Horizon. The caller is trusted to supply a valid hash.
- It does not check that the payment amount, asset, or memo match the invoice. That validation only happens in the cron reconcile path via `find_payment_for_invoice`.
- It does not deduplicate on `transactionHash` alone. If the same hash is sent twice for the same invoice, the second call is a no-op because the invoice is no longer `pending` after the first. However, if the caller sends an `X-Delivery-Id` header, the service will reject duplicate deliveries within the replay window even if the invoice is still pending.

## What the cron reconcile endpoint does

`GET /api/cron/reconcile` (TS and Rust) polls Horizon directly and is the authoritative payment detection path.

**Horizon query assumptions:**
- Calls `GET /accounts/{destination_public_key}/payments?order=desc&limit=50` for each pending invoice.
- Only inspects the most recent 50 payment operations. A payment older than the 50th most recent operation on that account will not be detected by this scan.
- Only matches operations with `type = "payment"` — path payments, claimable balances, and other operation types are ignored.
- Fetches the transaction record separately to read the `memo` field. If the transaction fetch fails, the payment is skipped for that run.

**Match criteria (all must be true):**
- `to` or `account` field equals `invoice.destination_public_key`
- `asset_code` equals `invoice.asset_code`
- `asset_issuer` equals `invoice.asset_issuer`
- `amount` equals `gross_amount_cents / 100` formatted to two decimal places (e.g. `"12.50"`)
- Transaction `memo` equals `invoice.memo`

If any field mismatches, the payment is skipped and the invoice stays `pending` until the next run.

## What the cron settle endpoint does

`GET /api/cron/settle` processes queued payouts and submits settlement transactions to Stellar.

**Current state:**
- The TypeScript implementation in `app/api/cron/settle/route.ts` is functional and handles payout submission.
- The Rust implementation returns `501 Not Implemented`. Settlement execution has not been ported to Rust yet. Do not route production settle traffic to the Rust backend until that is done.

## Failure modes and how they are handled

### Horizon is unreachable or returns a non-2xx response

- **Reconcile**: `find_payment_for_invoice` returns `AppError::Internal` / throws. The entire reconcile run fails with 500. No invoices are mutated. The `cron_runs` audit row is not written (the error happens before the audit insert).
- **Webhook**: Not applicable — the webhook path does not call Horizon.
- **Recovery**: The next scheduled reconcile run will retry all still-pending invoices.

### Horizon returns fewer than 50 records and the payment is older

- The payment will not be found by the current scan window.
- **Recovery**: Use `POST /api/cron/reconcile/replay` with `dry_run=true` first to confirm the payment is visible, then without `dry_run` to mark it paid. Alternatively, use `POST /api/webhooks/stellar` with the known `transactionHash` to mark it paid directly (operator must verify the hash manually).

### Transaction memo fetch fails after payment record is found

- The payment record is skipped for that invoice on that run.
- The invoice stays `pending` and will be retried on the next reconcile run.

### Invoice is already paid when webhook fires

- `markInvoicePaid` is not called (guarded by `invoice.status === 'pending'` check).
- Response returns `received: true` with the current `status` field so the caller can observe the state.
- No duplicate `payment_events` row is inserted.

### Invoice is already paid when reconcile runs

- The SQL `UPDATE ... WHERE status = 'pending'` is a no-op.
- The payout `INSERT ... ON CONFLICT (invoice_id) DO NOTHING` is also a no-op.
- The result entry shows `action: "paid"` with the transaction hash from Horizon.

### Invoice expires before payment is detected

- Reconcile marks it `expired` via `UPDATE ... WHERE status = 'pending'`.
- Webhook: if the webhook fires for an expired invoice, the status check (`invoice.status === 'pending'`) is false, so `markInvoicePaid` is not called. The response returns `received: true` with `status: "expired"`. The payment is not recorded.
- **Implication**: a payment that arrives on-chain after expiry will not be credited automatically. An operator must manually investigate and decide whether to credit the merchant out-of-band.

### Merchant has an invalid or missing `settlement_public_key`

- The invoice is still marked `paid`.
- Payout queueing is skipped.
- A `payment_events` row is inserted with `event_type = "payout_skipped_invalid_destination"` and `payload.reason = "invalid_settlement_public_key"`.
- The `payoutSkipReason` field in the response is `"invalid_settlement_public_key"`.
- **Recovery**: Fix the merchant's `settlement_public_key`, then manually insert a payout row or re-trigger settlement logic once the key is corrected. There is no automated retry for this case.

### Payout already queued for this invoice

- The `INSERT INTO payouts ... ON CONFLICT (invoice_id) DO NOTHING` is a no-op.
- `payoutSkipReason` in the response is `"payout_already_queued"`.
- This is not an error — it means a previous run already queued the payout.

### Settlement transaction submission fails (TS settle path)

- `markPayoutFailed` is called with the error message.
- The payout row is marked `failed` in the DB.
- The result entry shows `action: "failed"` with the reason.
- The invoice remains `paid`; only the payout is failed.
- **Recovery**: Investigate the Stellar submission error. If the destination account does not exist or is not funded, the merchant must correct their settlement key. There is no automatic retry — a failed payout must be manually re-queued or re-triggered.

### Cron secret is missing or wrong

- All cron and webhook routes return `401 AUTH_CRON_SECRET_MISMATCH`.
- Set `CRON_SECRET` in the environment and pass it as `Authorization: Bearer <secret>`.

### DB connection pool exhausted or query fails mid-reconcile

- The reconcile run fails at the point of failure. Invoices processed before the failure retain their mutations (the DB transaction per invoice is committed individually). Invoices not yet reached are unaffected and will be retried on the next run.

## Guarantees the integration provides

- Payment detection is **at-least-once**: a paid invoice will eventually be detected as long as the payment appears in the most recent 50 operations on the destination account and the memo matches.
- Invoice status transitions are **idempotent**: marking a `paid` invoice `paid` again is a no-op at the SQL level.
- Payout queueing is **idempotent**: `ON CONFLICT (invoice_id) DO NOTHING` prevents duplicate payout rows.
- The `payment_events` table is an append-only audit log. Rows are never deleted or updated by application code.

## Guarantees the integration does not provide

- **Exactly-once webhook delivery**: the webhook endpoint deduplicates on `X-Delivery-Id` within the replay window (default 5 min). Outside that window, or if no `X-Delivery-Id` is sent, a second delivery is a no-op only because the invoice is no longer `pending`.
- **Payment amount/asset verification via webhook**: the webhook path trusts the caller. Only the cron reconcile path verifies amount, asset, issuer, and memo against Horizon.
- **Detection of payments older than 50 operations**: the Horizon query window is fixed at 50. High-volume destination accounts may miss older payments.
- **Automatic recovery for expired invoices with on-chain payments**: no automated path exists. Operator intervention is required.
- **Rust settle parity**: the Rust backend does not execute settlement transactions yet. The TypeScript settle route is the only production-ready settlement path.

## Secret rotation procedure (issue #159)

To rotate `CRON_SECRET` / webhook secret without downtime:

1. Set `WEBHOOK_SECRET_SECONDARY=<new_secret>` and deploy. Both old and new secrets are now accepted.
2. Update all webhook callers to use `<new_secret>`.
3. Promote: set `CRON_SECRET=<new_secret>` and clear `WEBHOOK_SECRET_SECONDARY`, then deploy.

During step 1–2 both secrets are valid. No requests are dropped.

## Replay detection window (issue #162)

Webhook callers should send a unique `X-Delivery-Id` header per delivery attempt. The service stores each delivery ID in `webhook_deliveries` and rejects duplicates within `WEBHOOK_REPLAY_WINDOW_SECS` (default 300 seconds / 5 minutes).

- If `X-Delivery-Id` is absent, replay detection is skipped and idempotency falls back to the `transaction_hash` uniqueness check.
- Stale entries outside the window are purged lazily on each delivery.
- The `webhook_deliveries` table can be queried to audit recent deliveries.

## Webhook-to-invoice correlation metrics (AP-162)

Every webhook delivery is counted by outcome in the `webhook_correlation_metrics` table (migration `020_webhook_correlation_metrics.sql`). Operators can query aggregated counts via:

```
GET /api/cron/webhook-metrics?window_hours=24
Authorization: Bearer <CRON_SECRET>
```

Response shape:

```json
{
  "metric": "astropay_webhook_invoice_correlation",
  "windowHours": 24,
  "totals": {
    "resolved":   120,
    "duplicate":  5,
    "stale":      2,
    "miss":       3,
    "mismatch":   1,
    "auth_error": 0,
    "error":      0
  },
  "resolutionRate": 0.923
}
```

Outcome definitions:

| Outcome | Meaning |
|---|---|
| `resolved` | Invoice was `pending` → `paid` by this delivery |
| `duplicate` | Invoice already `paid`/`settled`; no mutation |
| `stale` | Invoice `expired` or `failed`; payment cannot be applied |
| `miss` | No invoice found for the `publicId` supplied |
| `mismatch` | Replay-window duplicate (`X-Delivery-Id` rejected) |
| `auth_error` | Unauthorized delivery attempt |
| `error` | Unexpected DB or runtime failure |

`resolutionRate` = `resolved / (resolved + miss + stale + error)`. It is `null` when there are no attempts. Duplicates and mismatches are excluded from the denominator because they are noise, not failed resolution attempts.

Alert guidance:
- A sustained drop in `resolutionRate` below expected baseline indicates webhook callers are sending bad `publicId` values or invoices are expiring before delivery.
- A spike in `auth_error` indicates a misconfigured or compromised caller.
- A spike in `mismatch` indicates the caller is retrying within the replay window without backing off.

## Memo mismatch events (issue #172)

When the cron reconcile path finds a payment where destination + asset + amount all match an invoice but the memo is wrong or missing, it inserts a `payment_events` row with `event_type = 'payment_memo_mismatch'` and payload:

```json
{ "hash": "<tx_hash>", "receivedMemo": "<actual_memo>", "expectedMemo": "<invoice_memo>" }
```

The invoice stays `pending`. This event is intended for support and fraud review. The reconcile response includes `action: "memo_mismatch"` for that invoice.

## Partial index for pending invoices (issue #191)

Migration `012_pending_invoices_expiry_idx.sql` adds:

```sql
CREATE INDEX IF NOT EXISTS invoices_pending_expires_at_idx
  ON invoices (expires_at ASC)
  WHERE status = 'pending';
```

This index is used by reconcile queries that locate expiring invoices. Only live pending rows are indexed, so it stays small as invoices transition to terminal states.

## Operator runbook

### Check if a specific invoice was paid on-chain but not in the DB

```bash
# dry-run first — no mutations
curl -s -X POST https://<host>/api/cron/reconcile/replay \
  -H "Authorization: Bearer $CRON_SECRET" \
  -H "Content-Type: application/json" \
  -d '{"publicId":"inv_abc123","dry_run":true}'

# if action=paid in dry-run, apply for real
curl -s -X POST https://<host>/api/cron/reconcile/replay \
  -H "Authorization: Bearer $CRON_SECRET" \
  -H "Content-Type: application/json" \
  -d '{"publicId":"inv_abc123"}'
```

### Force-mark an invoice paid via webhook (operator-verified hash)

Only use this when you have independently confirmed the transaction hash on Horizon.

```bash
curl -s -X POST https://<host>/api/webhooks/stellar \
  -H "Authorization: Bearer $CRON_SECRET" \
  -H "Content-Type: application/json" \
  -d '{"publicId":"inv_abc123","transactionHash":"<verified_hash>"}'
```

### Run reconcile in dry-run to preview what would change

```bash
curl -s "https://<host>/api/cron/reconcile?dry_run=true" \
  -H "Authorization: Bearer $CRON_SECRET"
```

### Audit recent cron runs

```sql
SELECT job_type, started_at, success, metadata, error_detail
FROM cron_runs
ORDER BY started_at DESC
LIMIT 20;
```
