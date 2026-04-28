# Chaos-Test Plan: Duplicate Webhooks and Duplicate Cron Triggers

Covers the scenarios where the same payment event is delivered more than once — either via the webhook endpoint, the cron reconcile endpoint, or both simultaneously — and verifies that no duplicate state corruption results.

Background: `docs/reconciliation/webhook-assumptions.md` documents the idempotency guarantees the code is supposed to provide. This plan tests those guarantees under adversarial timing.

---

## Scope

| Scenario | Risk |
|---|---|
| Webhook fires twice for the same invoice | Duplicate `payment_events` row or double payout queue |
| Cron reconcile runs overlap (two instances start before the first finishes) | Same invoice marked paid twice, duplicate payout |
| Webhook and cron reconcile race on the same invoice | Same as above |
| Cron settle runs overlap | Same payout submitted to Stellar twice |
| Duplicate `X-Delivery-Id` within replay window | Replay-window dedup bypassed |
| Duplicate `X-Delivery-Id` outside replay window | Second delivery treated as new |

---

## Idempotency invariants under test

These are the properties that must hold regardless of delivery count or timing:

1. An invoice transitions from `pending` to `paid` exactly once. A second `UPDATE ... WHERE status = 'pending'` is a no-op.
2. The `payouts` table has a `UNIQUE` constraint on `invoice_id`. A second `INSERT ... ON CONFLICT (invoice_id) DO NOTHING` inserts nothing.
3. `payment_events` is append-only. Duplicate events are allowed as audit records but must not trigger additional payouts.
4. `processing_worker_id` on `payouts` prevents two settle workers from claiming the same row simultaneously.

---

## Test scenarios

### Scenario 1 — Duplicate webhook delivery (same `X-Delivery-Id`)

**Goal:** Verify the replay-window dedup rejects the second delivery.

**Steps:**

```bash
INVOICE_ID="inv_$(openssl rand -hex 8)"
DELIVERY_ID="dlv_$(uuidgen)"
TX_HASH="$(openssl rand -hex 32)"

# First delivery — should succeed
curl -s -X POST https://<host>/api/webhooks/stellar \
  -H "Authorization: Bearer $CRON_SECRET" \
  -H "X-Delivery-Id: $DELIVERY_ID" \
  -H "Content-Type: application/json" \
  -d "{\"publicId\":\"$INVOICE_ID\",\"transactionHash\":\"$TX_HASH\"}"

# Second delivery within 5 minutes — must be rejected as duplicate
curl -s -X POST https://<host>/api/webhooks/stellar \
  -H "Authorization: Bearer $CRON_SECRET" \
  -H "X-Delivery-Id: $DELIVERY_ID" \
  -H "Content-Type: application/json" \
  -d "{\"publicId\":\"$INVOICE_ID\",\"transactionHash\":\"$TX_HASH\"}"
```

**Expected:** Second response contains `"duplicate": true`. Invoice status is `paid` exactly once. `payment_events` has one row for this invoice. `payouts` has one row for this invoice.

**Verify:**

```sql
SELECT status FROM invoices WHERE public_id = '<INVOICE_ID>';
-- expected: paid

SELECT count(*) FROM payment_events WHERE invoice_id = (SELECT id FROM invoices WHERE public_id = '<INVOICE_ID>');
-- expected: 1

SELECT count(*) FROM payouts WHERE invoice_id = (SELECT id FROM invoices WHERE public_id = '<INVOICE_ID>');
-- expected: 1
```

---

### Scenario 2 — Duplicate webhook delivery (no `X-Delivery-Id`)

**Goal:** Verify idempotency falls back to invoice status check when no delivery ID is sent.

**Steps:** Same as Scenario 1 but omit the `X-Delivery-Id` header on both requests.

**Expected:** Second delivery returns `"received": true` with `"status": "paid"`. No additional `payment_events` or `payouts` rows are created.

---

### Scenario 3 — Concurrent cron reconcile runs

**Goal:** Verify two overlapping reconcile runs do not double-mark an invoice paid.

**Steps (simulate with two parallel curl calls):**

```bash
# Fire two reconcile runs simultaneously against the same pending invoice
curl -s "https://<host>/api/cron/reconcile" \
  -H "Authorization: Bearer $CRON_SECRET" &

curl -s "https://<host>/api/cron/reconcile" \
  -H "Authorization: Bearer $CRON_SECRET" &

wait
```

**Expected:** Exactly one run reports `action: "paid"` for the invoice. The other reports `action: "already_paid"` or `action: "skipped"`. DB state: one `payment_events` row, one `payouts` row.

**Why it is safe:** The `UPDATE invoices SET status = 'paid' WHERE status = 'pending'` is atomic at the Postgres row level. Only one concurrent UPDATE wins the row lock; the other sees zero rows affected and skips payout insertion.

---

### Scenario 4 — Webhook and cron reconcile race

**Goal:** Verify that a webhook delivery and a cron reconcile run arriving simultaneously for the same invoice do not corrupt state.

**Steps:**

```bash
# Fire webhook and reconcile simultaneously
curl -s -X POST https://<host>/api/webhooks/stellar \
  -H "Authorization: Bearer $CRON_SECRET" \
  -H "Content-Type: application/json" \
  -d '{"publicId":"<INVOICE_ID>","transactionHash":"<TX_HASH>"}' &

curl -s "https://<host>/api/cron/reconcile" \
  -H "Authorization: Bearer $CRON_SECRET" &

wait
```

**Expected:** Same as Scenario 3 — exactly one `paid` transition, one payout row.

---

### Scenario 5 — Concurrent settle runs (duplicate payout submission)

**Goal:** Verify two settle workers cannot claim and submit the same payout row.

**Mechanism:** `db::claim_payout_for_processing` uses:

```sql
UPDATE payouts
SET processing_worker_id = $1, processing_started_at = NOW()
WHERE id = $2 AND status = 'queued' AND processing_worker_id IS NULL
```

Only one UPDATE wins; the other sees zero rows affected and skips.

**Steps:**

```bash
curl -s "https://<host>/api/cron/settle" \
  -H "Authorization: Bearer $CRON_SECRET" &

curl -s "https://<host>/api/cron/settle" \
  -H "Authorization: Bearer $CRON_SECRET" &

wait
```

**Expected:** Each payout row is submitted to Stellar at most once. No `DUPLICATE_TRANSACTION` errors from Horizon (Stellar itself also rejects duplicate sequence numbers, providing a second safety net).

**Verify:**

```sql
SELECT id, status, processing_worker_id, transaction_hash
FROM payouts
WHERE invoice_id = (SELECT id FROM invoices WHERE public_id = '<INVOICE_ID>');
-- expected: status = 'settled', exactly one non-null transaction_hash
```

---

### Scenario 6 — Replay-window boundary (delivery ID reused after window expires)

**Goal:** Verify a delivery ID reused after `WEBHOOK_REPLAY_WINDOW_SECS` (default 300 s) is treated as a new delivery, not a duplicate.

**Steps:**

1. Send first delivery with `X-Delivery-Id: dlv_old`.
2. Wait > 300 seconds (or temporarily set `WEBHOOK_REPLAY_WINDOW_SECS=5` in a test environment).
3. Send second delivery with the same `X-Delivery-Id: dlv_old`.

**Expected:** Second delivery is not rejected as a duplicate. Because the invoice is already `paid`, the status check prevents a second mutation. Response: `"received": true`, `"status": "paid"`.

---

## Automated test harness (integration test outline)

For environments where a test database is available, the following Rust integration test structure covers Scenarios 1–4 without network calls:

```rust
// rust-backend/tests/duplicate_delivery.rs  (outline — requires test DB)
//
// 1. Insert a pending invoice into the test DB.
// 2. Spawn two tokio tasks that both call mark_invoice_paid_and_queue_payout
//    for the same invoice_id concurrently.
// 3. Join both tasks.
// 4. Assert: exactly one task returned InvoicePaidOutcome::Paid,
//    the other returned InvoicePaidOutcome::AlreadyTransitioned.
// 5. Assert: SELECT count(*) FROM payouts WHERE invoice_id = ? returns 1.
// 6. Assert: SELECT count(*) FROM payment_events WHERE invoice_id = ? returns 1.
```

This test does not require a live Stellar network. It only needs a Postgres instance with the schema applied (`cargo run --bin migrate`).

---

## Checklist before marking this issue resolved

- [ ] Scenario 1 executed against staging; second delivery returns `"duplicate": true`
- [ ] Scenario 2 executed; no extra DB rows created
- [ ] Scenario 3 executed; DB shows exactly one `payment_events` and one `payouts` row
- [ ] Scenario 4 executed; same result as Scenario 3
- [ ] Scenario 5 executed; no duplicate Stellar submission
- [ ] Scenario 6 executed; boundary behaviour confirmed
- [ ] `webhook-assumptions.md` updated if any guarantee was found to be incorrect

---

## Related files

- `rust-backend/src/handlers/cron.rs` — reconcile and settle handlers
- `rust-backend/src/handlers/misc.rs` — webhook handler
- `rust-backend/src/money_state.rs` — `mark_invoice_paid_and_queue_payout`
- `rust-backend/src/db.rs` — `claim_payout_for_processing`
- `docs/reconciliation/webhook-assumptions.md` — idempotency guarantees reference
