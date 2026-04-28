# Dashboard Spec: Payment Success Rate and Payout Latency

**Issue:** AP-244
**Labels:** `area:observability`, `type:docs`, `difficulty:starter`
**Done when:** The team has a concrete monitoring dashboard design instead of vague "we should monitor this" language.

---

## Purpose

This document defines the panels, queries, and layout for a monitoring dashboard covering two operational concerns:

1. **Payment success rate** — are buyers completing payments at a healthy rate, and are failures being caught?
2. **Payout latency** — are settled invoices being paid out to merchants within expected windows?

Both concerns span the current split runtime (Next.js + Rust). Panels are keyed to the metric names defined in [`invoice-payout-lifecycle-metrics-spec.md`](./invoice-payout-lifecycle-metrics-spec.md). No new metrics are introduced here — this spec only describes how to visualize the ones already defined.

---

## Dashboard Layout

The dashboard is organized into three rows:

```
┌─────────────────────────────────────────────────────────────────┐
│  ROW 1: Payment Success Rate                                    │
│  [Success Rate %] [Payment Attempts] [Failure Breakdown]        │
│  [Duplicate / Stale Detections]                                 │
├─────────────────────────────────────────────────────────────────┤
│  ROW 2: Payout Latency                                          │
│  [Paid→Settled P50/P95] [Queue Drain Rate] [Oldest Queued Age]  │
│  [Payout Status Backlog]                                        │
├─────────────────────────────────────────────────────────────────┤
│  ROW 3: System Health                                           │
│  [Invoice Backlog by Status] [Cron Run Health] [Error Rate]     │
└─────────────────────────────────────────────────────────────────┘
```

---

## Row 1: Payment Success Rate

### Panel 1.1 — Payment Success Rate (%)

**Type:** Stat / Gauge

**Description:** Percentage of payment detection attempts that resulted in a successful `pending→paid` transition over the selected time window.

**Query (PromQL-style):**
```
rate(astropay_invoice_payment_detection_total{result="success"}[5m])
/
rate(astropay_invoice_payment_detection_total[5m])
* 100
```

**Thresholds:**
- Green: ≥ 95%
- Yellow: 80–94%
- Red: < 80%

**Notes:**
- Denominator includes `success`, `error`, `duplicate`, and `stale` results.
- `duplicate` and `stale` are expected at low rates; a spike in either warrants investigation but does not indicate a payment failure.

---

### Panel 1.2 — Payment Detection Attempts (Rate)

**Type:** Time-series line chart

**Description:** Rate of payment detection attempts broken down by result, showing volume trends over time.

**Query:**
```
rate(astropay_invoice_payment_detection_total[5m]) by (result, source)
```

**Series:** `success`, `error`, `duplicate`, `stale` — each as a separate line.

**Notes:**
- A rising `error` line without a corresponding `success` rise is the primary alert signal.
- `source` dimension helps distinguish webhook-driven vs. reconcile-driven detections.

---

### Panel 1.3 — Payment Failure Breakdown

**Type:** Bar chart (stacked)

**Description:** Count of non-success payment detection results broken down by `reason` label, over the selected window.

**Query:**
```
sum by (reason) (
  increase(astropay_invoice_payment_detection_total{result="error"}[1h])
)
```

**Expected `reason` values:**
- `invoice_not_found`
- `invoice_not_pending`
- `expired_before_payment`
- `db_error`
- `auth_error`
- `upstream_horizon_error`

**Notes:**
- A spike in `upstream_horizon_error` points to Stellar network issues, not application bugs.
- `invoice_not_found` spikes may indicate webhook delivery for invoices that were deleted or never created.

---

### Panel 1.4 — Duplicate and Stale Detection Rate

**Type:** Time-series line chart

**Description:** Rate of duplicate and stale payment detections. Low steady-state values are normal; spikes indicate replay or reconcile loop issues.

**Query:**
```
rate(astropay_invoice_payment_detection_total{result=~"duplicate|stale"}[5m]) by (result)
```

**Thresholds:** No hard threshold — use for anomaly detection relative to baseline.

---

## Row 2: Payout Latency

### Panel 2.1 — Paid-to-Settled Latency (P50 / P95)

**Type:** Stat (two values side by side)

**Description:** Histogram percentiles for `astropay_invoice_paid_to_settled_seconds` — the time from invoice payment detection to merchant settlement. This is the primary customer-impact latency metric.

**Queries:**
```
# P50
histogram_quantile(0.50, rate(astropay_invoice_paid_to_settled_seconds_bucket[1h]))

# P95
histogram_quantile(0.95, rate(astropay_invoice_paid_to_settled_seconds_bucket[1h]))
```

**Thresholds (example — tune to actual SLA):**
- P50 Green: < 300s (5 min), Yellow: 300–900s, Red: > 900s
- P95 Green: < 900s (15 min), Yellow: 900–3600s, Red: > 3600s

**Notes:**
- This metric is only recorded on first successful `paid→settled` transition, so retries do not inflate it.
- If the histogram is empty, no invoices have completed the full lifecycle in the window — check the invoice backlog panel.

---

### Panel 2.2 — Payout Queue Drain Rate

**Type:** Time-series line chart

**Description:** Rate of payout status transitions, showing how fast the queue is moving from `queued` through `submitted` to `settled`.

**Query:**
```
rate(astropay_payout_status_transition_total[5m]) by (status_from, status_to)
```

**Key series to watch:**
- `queued→submitted`: cron settle job is picking up work
- `submitted→settled`: chain confirmation is completing
- `queued→failed` or `failed→submitted`: retry activity

---

### Panel 2.3 — Oldest Queued Payout Age

**Type:** Stat / Gauge

**Description:** Age in seconds of the oldest payout still in `queued` state. The primary alert signal for a stuck settlement queue.

**Query:**
```
astropay_payout_oldest_queued_age_seconds
```

**Thresholds (tune to cron cadence):**
- Green: < 300s
- Yellow: 300–900s
- Red: > 900s (payout queue is likely stuck)

---

### Panel 2.4 — Payout Status Backlog

**Type:** Bar chart (grouped by status)

**Description:** Current snapshot of payout counts by status.

**Query:**
```
astropay_payouts_by_status by (status)
```

**Statuses:** `queued`, `submitted`, `settled`, `failed`

**Notes:**
- A growing `queued` count without a corresponding `submitted` growth means the settle cron is not running or is failing silently.
- A persistent `failed` count requires operator review — check `astropay_payout_oldest_failed_age_seconds`.

---

## Row 3: System Health

### Panel 3.1 — Invoice Backlog by Status

**Type:** Bar chart (grouped by status)

**Description:** Current snapshot of invoice counts by lifecycle status.

**Query:**
```
astropay_invoices_by_status by (status)
```

**Statuses:** `pending`, `paid`, `expired`, `settled`, `failed`

**Notes:**
- A growing `paid` count without a corresponding `settled` growth is the invoice-side signal for settlement lag.
- `astropay_invoice_oldest_paid_unsettled_age_seconds` is the companion alert metric for this panel.

---

### Panel 3.2 — Cron Job Health

**Type:** Time-series line chart

**Description:** Rate of cron job invocations broken down by `job_type` and `result`.

**Query:**
```
rate(astropay_cron_run_total[5m]) by (job_type, result)
```

**Series:** `reconcile/success`, `reconcile/error`, `settle/success`, `settle/error`

**Notes:**
- Both `reconcile` and `settle` jobs should show regular `success` pulses matching their configured schedule.
- A gap in `success` pulses without a corresponding `error` pulse may mean the cron trigger itself is not firing.

---

### Panel 3.3 — Cron Job Duration

**Type:** Time-series line chart

**Description:** P95 duration of reconcile and settle cron runs. Useful for detecting slow scans before they cause timeouts.

**Query:**
```
histogram_quantile(0.95, rate(astropay_cron_run_duration_seconds_bucket[5m])) by (job_type)
```

---

### Panel 3.4 — Invoice and Payout Error Rate

**Type:** Time-series line chart

**Description:** Combined error rate across invoice and payout operations, for a single-pane error health view.

**Query:**
```
sum(rate(astropay_invoice_payment_detection_total{result="error"}[5m]))
+
sum(rate(astropay_payout_settlement_attempt_total{result="error"}[5m]))
```

---

## Edge Cases and Error Handling

| Scenario | Expected dashboard behavior |
|---|---|
| Webhook delivers payment for already-paid invoice | `duplicate` result visible in Panel 1.4; no change to success rate numerator |
| Reconcile targets an expired invoice | `stale` result in Panel 1.4; no lifecycle counter change |
| Settle cron runs but treasury key is missing | `settle/error` in Panel 3.2; `queued→failed` in Panel 2.2 |
| Payout insert conflicts (already queued) | `duplicate` in payout queue attempt counter; Panel 2.4 backlog unchanged |
| Horizon is unreachable | `upstream_horizon_error` spike in Panel 1.3; success rate drops in Panel 1.1 |
| No invoices completed full lifecycle in window | Panel 2.1 histogram is empty; check Panel 3.1 for `paid` backlog |
| Cron trigger stops firing | Gap in Panel 3.2 success pulses; `oldest_queued_age` climbs in Panel 2.3 |

---

## Verification Steps

Before treating this dashboard as production-ready:

1. Confirm `astropay_invoice_payment_detection_total` is emitted from both `nextjs_api` and `rust_api` sources and that the `source` label is populated correctly.
2. Confirm `astropay_invoice_paid_to_settled_seconds` is only recorded once per invoice (on first `paid→settled` transition) — verify by checking that retried settle runs do not re-emit the histogram observation.
3. Confirm `astropay_payout_oldest_queued_age_seconds` is updated on every cron run, not only when a payout is processed.
4. Confirm `astropay_cron_run_total` increments for both `reconcile` and `settle` job types and that `result` is always one of `success` or `error`.
5. Confirm no panel query uses high-cardinality labels (`invoice_id`, `public_id`, wallet address, email, transaction hash).
6. Run a test payment end-to-end in staging and verify that Panels 1.1, 1.2, 2.1, and 2.2 all reflect the transition within one scrape interval.

---

## Alerting Tie-In

This dashboard is the visual companion to the alert thresholds defined in AP-243. The panels map to alerts as follows:

| Panel | Corresponding alert |
|---|---|
| 1.1 Payment Success Rate | Alert when rate drops below 80% for > 5 min |
| 2.3 Oldest Queued Payout Age | Alert when value exceeds expected settlement cadence |
| 3.1 Invoice Backlog (`paid` count) | Alert via `astropay_invoice_oldest_paid_unsettled_age_seconds` |
| 3.2 Cron Job Health | Alert when `error` result appears or success pulses gap |

---

## Related Documents

- [`invoice-payout-lifecycle-metrics-spec.md`](./invoice-payout-lifecycle-metrics-spec.md) — metric definitions this dashboard is built on
- [`docs/issue-backlog/astropay-250-issues.md`](../issue-backlog/astropay-250-issues.md) — AP-243 (alert thresholds), AP-245 (incident runbook)
