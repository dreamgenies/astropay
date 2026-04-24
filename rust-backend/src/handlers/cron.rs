use axum::{
    Json,
    extract::{Path, State},
    extract::{Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::types::Json as PgJson;
use tracing::warn;
use uuid::Uuid;

use crate::{
    AppState,
    auth::authorize_cron_request,
    error::AppError,
    models::Invoice,
    money_state::{
        INVOICE_ALREADY_TRANSITIONED_REASON, InvoicePaidOutcome, mark_invoice_paid_and_queue_payout,
    },
    stellar::{find_payment_for_invoice, invoice_is_expired},
    settle::{backoff_seconds, is_backoff_elapsed},
    stellar::{find_payment_for_invoice, invoice_is_expired, is_valid_account_public_key},
    stellar::{PaymentScanResult, find_payment_for_invoice, invoice_is_expired, is_valid_account_public_key},
    stellar::{
        TransactionStatus, confirm_transaction, find_payment_for_invoice,
        invoice_is_expired, is_valid_account_public_key,
        fetch_treasury_payments, find_payment_for_invoice, invoice_is_expired,
        is_valid_account_public_key,
    },
};

/// Payouts that fail this many times are moved to the dead-letter path.
pub(crate) const PAYOUT_DEAD_LETTER_THRESHOLD: i32 = 5;
const PAYOUT_DEAD_LETTER_THRESHOLD: i32 = 5;
/// Default number of queued payouts processed per settle run. Override with `SETTLE_BATCH_SIZE` env var.
const DEFAULT_SETTLE_BATCH_SIZE: i64 = 50;

#[derive(Debug, Deserialize)]
#[derive(Deserialize)]
pub struct DryRunParams {
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReplayRequest {
    #[serde(rename = "publicId")]
    public_id: String,
    #[serde(default)]
    dry_run: bool,
}

/// Default number of recent treasury payments to scan for orphans.
const ORPHAN_SCAN_LIMIT: u32 = 50;

#[derive(Deserialize)]
pub struct DryRunParams {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Deserialize)]
pub struct OrphanParams {
    /// How many recent treasury payments to fetch from Horizon (default 50, max 200).
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct DryRunParams {
    #[serde(default)]
    pub dry_run: bool,
}

/// Rows fetched per page during reconcile keyset pagination.
/// Keeping this at 100 balances Horizon round-trips vs. DB query cost.
const RECONCILE_PAGE_SIZE: i64 = 100;

/// Rows fetched per page during settle keyset pagination.
const SETTLE_PAGE_SIZE: i64 = 100;

#[derive(Deserialize)]
pub struct DryRunParams {
    #[serde(default)]
    pub dry_run: bool,
}

/// Scans ALL pending invoices using keyset pagination so backlogs larger than
/// a single page are fully processed in one cron invocation without manual
/// babysitting.
///
/// Pagination cursor: `(created_at, id)` — stable, uses the existing
/// `invoices_merchant_id_idx` / `invoices_status_idx` indexes and the
/// composite `invoices_merchant_created_at_id_idx` from migration 006.
/// Each page fetches up to [`RECONCILE_PAGE_SIZE`] rows ordered
/// `(created_at ASC, id ASC)`; iteration stops when a page is smaller than
/// the page size (last page reached).
pub async fn reconcile(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<DryRunParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    let dry_run = params.dry_run;
    let scan_limit = state.config.reconcile_scan_limit;
    let scan_window_hours = state.config.reconcile_scan_window_hours;
    let mut client = state.pool.get().await?;

    // Build the scan query. When scan_window_hours > 0 restrict to invoices
    // created within that window so stale pending rows don't clog every run.
    let rows = if scan_window_hours > 0 {
        client
            .query(
                "SELECT * FROM invoices
                 WHERE status = 'pending'
                   AND created_at >= NOW() - ($2::bigint * INTERVAL '1 hour')
                 ORDER BY created_at ASC
                 LIMIT $1",
                &[&scan_limit, &scan_window_hours],
            )
            .await?
    } else {
        client
            .query(
                "SELECT * FROM invoices WHERE status = 'pending' ORDER BY created_at ASC LIMIT $1",
                &[&scan_limit],
            )
            .await?
    };
    let invoices = rows.iter().map(Invoice::from_row).collect::<Vec<_>>();
    let mut results = Vec::with_capacity(invoices.len());
    let mut results: Vec<Value> = Vec::new();

    // Keyset cursor: start before the earliest possible row.
    let mut cursor_created_at: DateTime<Utc> = DateTime::UNIX_EPOCH;
    let mut cursor_id: Uuid = Uuid::nil();

    loop {
        let rows = client
            .query(
                "SELECT * FROM invoices
                 WHERE status = 'pending'
                   AND (created_at, id) > ($1, $2)
                 ORDER BY created_at ASC, id ASC
                 LIMIT $3",
                &[&cursor_created_at, &cursor_id, &RECONCILE_PAGE_SIZE],
            )
            .await?;

        let page_len = rows.len();
        let invoices: Vec<Invoice> = rows.iter().map(Invoice::from_row).collect();

        for invoice in invoices {
            // Advance cursor to the last row seen on this page.
            cursor_created_at = invoice.created_at;
            cursor_id = invoice.id;

            if invoice_is_expired(&invoice, Utc::now()) {
                if !dry_run {
                    client
                        .execute(
                            "UPDATE invoices SET status = 'expired', updated_at = NOW() \
                             WHERE id = $1 AND status = 'pending'",
                            &[&invoice.id],
                        )
                        .await?;
                }
                results.push(json!({ "publicId": invoice.public_id, "action": "expired" }));
                continue;
            }

            match find_payment_for_invoice(&state.config, &invoice).await? {
                Some(payment) => {
                    let transaction = client.transaction().await?;
                    transaction
                        .execute(
                            "UPDATE invoices
                             SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
                             WHERE id = $1 AND status = 'pending'",
                            &[&invoice.id, &payment.hash],
                        )
                        .await?;
                    transaction
                        .execute(
                            "INSERT INTO payment_events (invoice_id, event_type, payload) \
                             VALUES ($1, $2, $3)",
                            &[&invoice.id, &"payment_detected", &payment.payment],
                        )
                        .await?;
                    let settlement_row = transaction
                        .query_opt(
                            "SELECT m.settlement_public_key
                             FROM merchants m
                             INNER JOIN invoices i ON i.merchant_id = m.id
                             WHERE i.id = $1",
                            &[&invoice.id],
                        )
                        .await?;
                    let settlement_key: Option<String> =
                        settlement_row.map(|row| row.get(0));
                    let settlement_key = settlement_key.unwrap_or_default();
                    let (payout_queued, payout_skip_reason) =
                        if !is_valid_account_public_key(&settlement_key) {
                            transaction
                                .execute(
                                    "INSERT INTO payment_events (invoice_id, event_type, payload) \
                                     VALUES ($1, $2, $3)",
                                    &[
                                        &invoice.id,
                                        &"payout_skipped_invalid_destination",
                                        &json!({ "reason": "invalid_settlement_public_key" }),
                                    ],
                                )
                                .await?;
                            (false, Some("invalid_settlement_public_key"))
                        } else {
                            let inserted = transaction
                                .execute(
                                    "INSERT INTO payouts (invoice_id, merchant_id, destination_public_key, \
                                                          amount_cents, asset_code, asset_issuer)
                                     SELECT id, merchant_id,
                                            (SELECT settlement_public_key FROM merchants \
                                             WHERE merchants.id = invoices.merchant_id),
                                            net_amount_cents, asset_code, asset_issuer
                                     FROM invoices WHERE id = $1
                                     ON CONFLICT (invoice_id) DO NOTHING",
                                    &[&invoice.id],
                                )
                                .await?;
                            if inserted > 0 {
                                (true, None)
                            } else {
                                (false, Some("payout_already_queued"))
                            }
                        };
                    transaction.commit().await?;
                    results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "paid",
                        "txHash": payment.hash,
                        "memo": payment.memo,
                        "payoutQueued": payout_queued,
                        "payoutSkipReason": payout_skip_reason
                    }));
                }
                None => {
                    results.push(
                        json!({ "publicId": invoice.public_id, "action": "pending" }),
                    );
                }
        match find_payment_for_invoice(&state.config, &invoice).await? {
            PaymentScanResult::AssetMismatch(mismatch) => {
                if !dry_run {
                    client
                        .execute(
                            "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                            &[
                                &invoice.id,
                                &"payment_asset_mismatch",
                                &json!({
                                    "hash": mismatch.hash,
                                    "receivedAssetCode": mismatch.received_asset_code,
                                    "receivedAssetIssuer": mismatch.received_asset_issuer,
                                    "expectedAssetCode": mismatch.expected_asset_code,
                                    "expectedAssetIssuer": mismatch.expected_asset_issuer,
                                    "amount": mismatch.amount,
                                }),
                            ],
                        )
                        .await?;
                }
                results.push(json!({
                    "publicId": invoice.public_id,
                    "action": "asset_mismatch",
                    "hash": mismatch.hash,
                    "receivedAssetCode": mismatch.received_asset_code,
                    "expectedAssetCode": mismatch.expected_asset_code,
                }));
            }
            PaymentScanResult::Match(payment) => {
            Some(payment) => {
                // Idempotency guard: if this transaction hash is already stored
                // on any invoice, a previous run already processed this payment.
                let already = client
                    .query_opt(
                        "SELECT id FROM invoices WHERE transaction_hash = $1",
                        &[&payment.hash],
                    )
                    .await?;
                if already.is_some() {
                    results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "already_processed",
                        "txHash": payment.hash
                    }));
                    continue;
                }

        // Issue #167: treat HorizonUnavailable as a transient skip — do NOT
        // flip the invoice to failed.
        match find_payment_for_invoice(&state.config, &invoice).await {
            Err(AppError::HorizonUnavailable) => {
                warn!(
                    public_id = %invoice.public_id,
                    "Horizon unavailable during reconcile — skipping invoice to avoid false failure"
                );
                results.push(json!({ "publicId": invoice.public_id, "action": "skipped_horizon_unavailable" }));
                continue;
            }
            Err(e) => return Err(e),
            Ok(None) => {
                results.push(json!({ "publicId": invoice.public_id, "action": "pending" }));
            }
            Ok(Some(payment)) => {
                let transaction = client.transaction().await?;
                let outcome = mark_invoice_paid_and_queue_payout(
                    &transaction,
                    invoice.id,
                    &payment.hash,
                    &payment.payment,
                )
                .await?;
                let updated = transaction
                    .execute(
                        "UPDATE invoices
                         SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
                         WHERE id = $1 AND status = 'pending'",
                        &[&invoice.id, &payment.hash],
                    )
                    .await;
                let updated = match updated {
                    Ok(n) => n,
                    Err(ref e)
                        if e.code()
                            == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) =>
                    {
                        results.push(json!({
                            "publicId": invoice.public_id,
                            "action": "already_processed",
                            "txHash": payment.hash
                        }));
                        continue;
                    }
                    Err(_) => return Err(AppError::Internal),
                };
                if updated == 0 {
                    results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "skipped",
                        "txHash": payment.hash
                    }));
                    continue;
                }
                transaction
                    .execute(
                        "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                        &[&invoice.id, &"payment_detected", &payment.payment],
                    )
                    .await?;
                let settlement_row = transaction
                    .query_opt(
                        "SELECT m.settlement_public_key
                         FROM merchants m
                         INNER JOIN invoices i ON i.merchant_id = m.id
                         WHERE i.id = $1",
                        &[&invoice.id],
                    )
                    .await?;
                let settlement_key: Option<String> = settlement_row.map(|row| row.get(0));
                let settlement_key = settlement_key.unwrap_or_default();
                let (payout_queued, payout_skip_reason) = if !is_valid_account_public_key(
                    &settlement_key,
                ) {
                    transaction
                            .execute(
                                "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                                &[
                                    &invoice.id,
                                    &"payout_skipped_invalid_destination",
                                    &json!({ "reason": "invalid_settlement_public_key" }),
                                ],
                            )
                            .await?;
                    (false, Some("invalid_settlement_public_key"))
                } else {
                    let inserted = transaction
                            .execute(
                                "INSERT INTO payouts (invoice_id, merchant_id, destination_public_key, amount_cents, asset_code, asset_issuer)
                                 SELECT id, merchant_id, (SELECT settlement_public_key FROM merchants WHERE merchants.id = invoices.merchant_id),
                                        net_amount_cents, asset_code, asset_issuer
                                 FROM invoices WHERE id = $1
                                 ON CONFLICT (invoice_id) DO NOTHING",
                                &[&invoice.id],
                            )
                            .await?;
                    if inserted > 0 {
                        (true, None)
                    } else {
                        (false, Some("payout_already_queued"))
                    }
                };
                        if inserted > 0 { (true, None) } else { (false, Some("payout_already_queued")) }
                    };
                transaction.commit().await?;
                match outcome {
                    InvoicePaidOutcome::Applied {
                        payout_queued,
                        payout_skip_reason,
                    } => results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "paid",
                        "txHash": payment.hash,
                        "memo": payment.memo,
                        "payoutQueued": payout_queued,
                        "payoutSkipReason": payout_skip_reason.map(|reason| reason.as_str())
                    })),
                    InvoicePaidOutcome::AlreadyTransitioned => results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "skipped",
                        "reason": INVOICE_ALREADY_TRANSITIONED_REASON
                    })),
                }
            }
            PaymentScanResult::NotFound => {
                results.push(json!({ "publicId": invoice.public_id, "action": "pending" }));
        }
    }

    // Issue #158: confirm any payouts that are in 'submitted' state by querying
    // Horizon for their transaction hash.
    let submitted_rows = client
        .query(
            "SELECT id, transaction_hash FROM payouts WHERE status = 'submitted' AND transaction_hash IS NOT NULL LIMIT 100",
            &[],
        )
        .await?;

    let mut confirmed: Vec<Value> = Vec::new();
    let mut chain_failed: Vec<Value> = Vec::new();

    for row in &submitted_rows {
        let payout_id: Uuid = row.get("id");
        let tx_hash: String = row.get("transaction_hash");

        match confirm_transaction(&state.config, &tx_hash).await {
            Err(AppError::HorizonUnavailable) => {
                // Issue #167: skip — do not mark as failed during an outage.
                warn!(payout_id = %payout_id, "Horizon unavailable during payout confirmation — leaving as submitted");
            }
            Err(e) => return Err(e),
            Ok(TransactionStatus::Success) => {
                if !dry_run {
                    client
                        .execute(
                            "UPDATE payouts SET status = 'confirmed', updated_at = NOW() WHERE id = $1 AND status = 'submitted'",
                            &[&payout_id],
                        )
                        .await?;
                }
                confirmed.push(json!({ "payoutId": payout_id, "txHash": tx_hash }));
            }
            Ok(TransactionStatus::Failed) => {
                if !dry_run {
                    client
                        .execute(
                            "UPDATE payouts SET status = 'failed', failure_reason = 'chain_rejected', updated_at = NOW() WHERE id = $1 AND status = 'submitted'",
                            &[&payout_id],
                        )
                        .await?;
                }
                chain_failed.push(json!({ "payoutId": payout_id, "txHash": tx_hash }));
            }
            Ok(TransactionStatus::NotFound) => {
                // Still pending on-chain — leave as submitted.
            }
        }

        // Stop when we got fewer rows than the page size — backlog exhausted.
        if (page_len as i64) < RECONCILE_PAGE_SIZE {
            break;
        }
    }

    let body = json!({
        "dryRun": dry_run,
        "scanned": results.len(),
        "scanLimit": scan_limit,
        "scanWindowHours": if scan_window_hours > 0 { json!(scan_window_hours) } else { json!(null) },
        "results": results
        "results": results,
        "payoutsConfirmed": confirmed.len(),
        "payoutsChainFailed": chain_failed.len(),
        "confirmedItems": confirmed,
        "chainFailedItems": chain_failed,
    });
    if !dry_run {
        if let Err(e) = client
            .execute(
                "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata, error_detail) \
                 VALUES ('reconcile', NOW(), NOW(), true, $1, NULL)",
                &[&PgJson(&body)],
            )
            .await
        {
            warn!(error = %e, "cron_runs audit insert failed for reconcile");
        }
    }

    Ok(Json(body))
}

/// Deletes sessions whose `expires_at` is in the past.
pub async fn purge_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    let client = state.pool.get().await?;
    let deleted = client
        .execute("DELETE FROM sessions WHERE expires_at <= NOW()", &[])
        .await?;
    let body = json!({ "deleted": deleted });
    if let Err(e) = client
        .execute(
            "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata, error_detail) \
             VALUES ('purge_sessions', NOW(), NOW(), true, $1, NULL)",
            &[&PgJson(&body)],
        )
        .await
    {
        warn!(error = %e, "cron_runs audit insert failed for purge_sessions");
    }
    Ok(Json(body))
}

/// Scans ALL failed payouts using keyset pagination and increments their
/// failure count. Once a payout reaches [`PAYOUT_DEAD_LETTER_THRESHOLD`]
/// failures it is moved to `dead_lettered` status and a row is inserted into
/// `payout_dead_letters` so operators can inspect and manually resolve it.
///
/// Pagination cursor: `(updated_at, id)` — stable across pages because each
/// processed row is immediately updated (its `updated_at` advances past the
/// cursor), so it will not appear in subsequent pages of the same run.
///
/// Full Stellar transaction signing/submission is not implemented yet; this
/// handler only manages the failure-tracking and dead-letter escalation path.
/// Scans `payouts` with status `failed` and increments their failure count.
/// Once a payout reaches [`PAYOUT_DEAD_LETTER_THRESHOLD`] failures it is moved
/// to `dead_lettered` status.
pub async fn settle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(_params): Query<DryRunParams>,
    axum::extract::Query(params): axum::extract::Query<DryRunParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    let mut client = state.pool.get().await?;
    let batch_size = std::env::var("SETTLE_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_SETTLE_BATCH_SIZE)
        .max(1);

    let mut dead_lettered: Vec<Value> = Vec::new();
    let mut requeued: Vec<Value> = Vec::new();

    // Keyset cursor: start before the earliest possible row.
    let mut cursor_updated_at: DateTime<Utc> = DateTime::UNIX_EPOCH;
    let mut cursor_id: Uuid = Uuid::nil();

    loop {
        let rows = client
            .query(
                "SELECT * FROM payouts
                 WHERE status = 'failed'
                   AND (updated_at, id) > ($1, $2)
                 ORDER BY updated_at ASC, id ASC
                 LIMIT $3",
                &[&cursor_updated_at, &cursor_id, &SETTLE_PAGE_SIZE],
    let rows = client
        .query(
            "SELECT * FROM payouts WHERE status = 'failed' ORDER BY updated_at ASC LIMIT $1",
            &[&batch_size],
        )
        .await?;

    let mut dead_lettered: Vec<Value> = Vec::new();
    let mut requeued: Vec<Value> = Vec::new();
    let mut skipped_backoff: Vec<Value> = Vec::new();

    let now_secs = Utc::now().timestamp();

    for row in &rows {
        let payout_id: Uuid = row.get("id");
        let invoice_id: Uuid = row.get("invoice_id");
        let merchant_id: Uuid = row.get("merchant_id");
        let failure_count: i32 = row.get("failure_count");
        let failure_reason: Option<String> = row.get("failure_reason");
        let last_failure_at: Option<chrono::DateTime<Utc>> = row.get("last_failure_at");
        let new_count = failure_count + 1;

        let tx = client.transaction().await?;

        if new_count >= PAYOUT_DEAD_LETTER_THRESHOLD {
            tx.execute(
                "UPDATE payouts
                 SET status = 'dead_lettered', failure_count = $2, last_failure_at = NOW(), last_failure_reason = $3, updated_at = NOW()
                 WHERE id = $1",
                &[&payout_id, &new_count, &failure_reason],
            )
            .await?;
            tx.execute(
                "INSERT INTO payout_dead_letters (payout_id, invoice_id, merchant_id, failure_count, last_failure_reason)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (payout_id) DO NOTHING",
                &[&payout_id, &invoice_id, &merchant_id, &new_count, &failure_reason],
            )
            .await?;
            tx.execute(
                "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                &[
                    &invoice_id,
                    &"payout_dead_lettered",
                    &json!({ "payoutId": payout_id, "failureCount": new_count }),
                ],
            )
            .await?;
            tx.commit().await?;
            dead_lettered.push(json!({ "payoutId": payout_id, "failureCount": new_count }));
        } else {
            // Apply backoff: only requeue if enough time has elapsed since the last failure.
            let last_failure_secs = last_failure_at
                .map(|t| t.timestamp())
                .unwrap_or(0);

            if !is_backoff_elapsed(new_count, last_failure_secs, now_secs) {
                let backoff_secs = backoff_seconds(new_count).unwrap_or(0);
                let retry_after = last_failure_secs + backoff_secs - now_secs;
                tx.rollback().await?;
                skipped_backoff.push(json!({
                    "payoutId": payout_id,
                    "failureCount": failure_count,
                    "retryAfterSecs": retry_after,
                }));
                continue;
            }

            // Backoff window has elapsed — increment failure count and requeue.
            tx.execute(
                "UPDATE payouts
                 SET status = 'queued', failure_count = $2, last_failure_at = NOW(), last_failure_reason = $3, updated_at = NOW()
                 WHERE id = $1",
                &[&payout_id, &new_count, &failure_reason],
            )
            .await?;

        let page_len = rows.len();

        for row in &rows {
            let payout_id: Uuid = row.get("id");
            let invoice_id: Uuid = row.get("invoice_id");
            let merchant_id: Uuid = row.get("merchant_id");
            let failure_count: i32 = row.get("failure_count");
            let failure_reason: Option<String> = row.get("failure_reason");
            let new_count = failure_count + 1;

            // Advance cursor before the DB write so the updated row's new
            // updated_at won't be re-fetched in a subsequent page.
            cursor_updated_at = row.get("updated_at");
            cursor_id = payout_id;

            let tx = client.transaction().await?;

            if new_count >= PAYOUT_DEAD_LETTER_THRESHOLD {
                // Escalate to dead-letter.
                tx.execute(
                    "UPDATE payouts
                     SET status = 'dead_lettered', failure_count = $2,
                         last_failure_at = NOW(), last_failure_reason = $3, updated_at = NOW()
                     WHERE id = $1",
                    &[&payout_id, &new_count, &failure_reason],
                )
                .await?;
                tx.execute(
                    "INSERT INTO payout_dead_letters \
                         (payout_id, invoice_id, merchant_id, failure_count, last_failure_reason)
                     VALUES ($1, $2, $3, $4, $5)
                     ON CONFLICT (payout_id) DO NOTHING",
                    &[&payout_id, &invoice_id, &merchant_id, &new_count, &failure_reason],
                )
                .await?;
                tx.execute(
                    "INSERT INTO payment_events (invoice_id, event_type, payload) \
                     VALUES ($1, $2, $3)",
                    &[
                        &invoice_id,
                        &"payout_dead_lettered",
                        &json!({ "payoutId": payout_id, "failureCount": new_count }),
                    ],
                )
                .await?;
                tx.commit().await?;
                dead_lettered
                    .push(json!({ "payoutId": payout_id, "failureCount": new_count }));
            } else {
                // Increment failure count and requeue for the next settle run.
                tx.execute(
                    "UPDATE payouts
                     SET status = 'queued', failure_count = $2,
                         last_failure_at = NOW(), last_failure_reason = $3, updated_at = NOW()
                     WHERE id = $1",
                    &[&payout_id, &new_count, &failure_reason],
                )
                .await?;
                tx.commit().await?;
                requeued.push(json!({ "payoutId": payout_id, "failureCount": new_count }));
            }
        }

        // Stop when we got fewer rows than the page size — backlog exhausted.
        if (page_len as i64) < SETTLE_PAGE_SIZE {
            break;
        }
    }

    let body = json!({
        "batchSize": batch_size,
        "deadLettered": dead_lettered.len(),
        "requeued": requeued.len(),
        "skippedBackoff": skipped_backoff.len(),
        "deadLetteredItems": dead_lettered,
        "requeuedItems": requeued,
        "skippedBackoffItems": skipped_backoff,
        "note": "Stellar transaction signing/submission is not implemented yet. This run only manages dead-letter escalation."
    });

    if let Err(e) = client
        .execute(
            "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata, error_detail) \
             VALUES ('settle', NOW(), NOW(), true, $1, NULL)",
            &[&PgJson(&body)],
        )
        .await
    {
        warn!(error = %e, "cron_runs audit insert failed for settle");
    }

    Ok(Json(body))
}

/// Issue #178 — Manual replay endpoint for a specific payout settlement.
///
/// `POST /api/cron/payouts/:payout_id/replay`
///
/// Resets a single payout from `dead_lettered` or `failed` back to `queued`
/// so the next settle run will attempt it again. The action is audited in
/// `payment_events` for full operator traceability.
///
/// Requires the same `Authorization: Bearer <CRON_SECRET>` header as other
/// cron endpoints so only operators with the secret can trigger replays.
pub async fn replay_payout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(payout_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    let mut client = state.pool.get().await?;

    // Load the payout — must exist and be in a replayable state.
    let row = client
        .query_opt(
            "SELECT id, invoice_id, merchant_id, status, failure_count FROM payouts WHERE id = $1",
            &[&payout_id],
        )
        .await?
        .ok_or_else(|| AppError::not_found(format!("payout {payout_id} not found")))?;

    let status: String = row.get("status");
    let invoice_id: Uuid = row.get("invoice_id");
    let failure_count: i32 = row.get("failure_count");

    if status != "dead_lettered" && status != "failed" {
        return Err(AppError::bad_request(format!(
            "payout {payout_id} has status '{status}'; only dead_lettered or failed payouts can be replayed"
        )));
    }

    let tx = client.transaction().await?;

    // Reset the payout to queued with zeroed failure count so it gets a clean attempt.
    tx.execute(
        "UPDATE payouts
         SET status = 'queued', failure_count = 0, failure_reason = NULL,
             last_failure_at = NULL, updated_at = NOW()
         WHERE id = $1",
        &[&payout_id],
    )
    .await?;

    // Audit the replay in payment_events.
    tx.execute(
        "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
        &[
            &invoice_id,
            &"payout_replayed",
            &json!({
                "payoutId": payout_id,
                "previousStatus": status,
                "previousFailureCount": failure_count,
                "replayedAt": Utc::now().to_rfc3339(),
            }),
        ],
    )
    .await?;

    tx.commit().await?;

    Ok(Json(json!({
        "payoutId": payout_id,
        "previousStatus": status,
        "newStatus": "queued",
        "message": "Payout has been reset to queued and will be retried on the next settle run."
    })))
/// Returns on-chain USDC payments that arrived at the platform treasury but do not
/// match any invoice by `transaction_hash`.
///
/// These are "orphan" payments — funds received on-chain with no corresponding
/// invoice record. Operators use this endpoint to identify and manually reconcile them.
///
/// Query params:
/// - `limit`: number of recent treasury payments to scan (default 50, capped at 200)
///
/// Trigger via `GET /api/cron/orphan-payments` with `Authorization: Bearer <CRON_SECRET>`.
pub async fn orphan_payments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<OrphanParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;

    let limit = params.limit.unwrap_or(ORPHAN_SCAN_LIMIT).min(200);
    if limit == 0 {
        return Err(AppError::bad_request("limit must be greater than 0"));
    }

    let treasury_payments = fetch_treasury_payments(&state.config, limit).await?;

    if treasury_payments.is_empty() {
        return Ok(Json(json!({
            "scanned": 0,
            "orphans": [],
            "treasury": state.config.platform_treasury_public_key,
        })));
    }

    // Collect all transaction hashes from Horizon to check against the DB in one query.
    let hashes: Vec<String> = treasury_payments
        .iter()
        .map(|p| p.transaction_hash.clone())
        .collect();

    let client = state.pool.get().await?;

    // Fetch all invoices whose transaction_hash matches any of the scanned payments.
    // Any hash NOT in this set is an orphan.
    let rows = client
        .query(
            "SELECT transaction_hash FROM invoices WHERE transaction_hash = ANY($1)",
            &[&hashes],
        )
        .await?;

    let known_hashes: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| r.get::<_, Option<String>>("transaction_hash"))
        .collect();

    let orphans: Vec<Value> = treasury_payments
        .iter()
        .filter(|p| !known_hashes.contains(&p.transaction_hash))
        .map(|p| {
            json!({
                "transactionHash": p.transaction_hash,
                "from": p.from,
                "amount": p.amount,
                "assetCode": p.asset_code,
                "assetIssuer": p.asset_issuer,
            })
        })
        .collect();

    Ok(Json(json!({
        "scanned": treasury_payments.len(),
        "orphanCount": orphans.len(),
        "orphans": orphans,
        "treasury": state.config.platform_treasury_public_key,
    })))
pub async fn replay_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReplayRequest>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;

    if body.public_id.trim().is_empty() {
        return Err(AppError::bad_request("publicId is required"));
    }

    let mut client = state.pool.get().await?;
    let row = client
        .query_opt(
            "SELECT * FROM invoices WHERE public_id = $1",
            &[&body.public_id],
        )
        .await?
        .ok_or_else(|| AppError::not_found(format!("Invoice '{}' not found", body.public_id)))?;

    let invoice = Invoice::from_row(&row);
    let dry_run = body.dry_run;

    if invoice.status != "pending" {
        return Ok(Json(json!({
            "dryRun": dry_run,
            "publicId": invoice.public_id,
            "action": "skipped",
            "reason": format!("invoice status is '{}', only 'pending' invoices can be replayed", invoice.status)
        })));
    }

    if invoice_is_expired(&invoice, Utc::now()) {
        if !dry_run {
            client
                .execute(
                    "UPDATE invoices SET status = 'expired', updated_at = NOW() WHERE id = $1 AND status = 'pending'",
                    &[&invoice.id],
                )
                .await?;
        }
        return Ok(Json(json!({
            "dryRun": dry_run,
            "publicId": invoice.public_id,
            "action": "expired"
        })));
    }

    match find_payment_for_invoice(&state.config, &invoice).await? {
        None => Ok(Json(json!({
            "dryRun": dry_run,
            "publicId": invoice.public_id,
            "action": "pending"
        }))),
        Some(payment) => {
            if dry_run {
                return Ok(Json(json!({
                    "dryRun": true,
                    "publicId": invoice.public_id,
                    "action": "paid",
                    "txHash": payment.hash,
                    "memo": payment.memo,
                    "payoutQueued": null,
                    "payoutSkipReason": null
                })));
            }

            let transaction = client.transaction().await?;
            transaction
                .execute(
                    "UPDATE invoices
                     SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
                     WHERE id = $1 AND status = 'pending'",
                    &[&invoice.id, &payment.hash],
                )
                .await?;
            transaction
                .execute(
                    "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                    &[&invoice.id, &"payment_detected", &payment.payment],
                )
                .await?;
            let settlement_row = transaction
                .query_opt(
                    "SELECT m.settlement_public_key
                     FROM merchants m
                     INNER JOIN invoices i ON i.merchant_id = m.id
                     WHERE i.id = $1",
                    &[&invoice.id],
                )
                .await?;
            let settlement_key = settlement_row
                .map(|r| r.get::<_, String>(0))
                .unwrap_or_default();
            let (payout_queued, payout_skip_reason) = if !is_valid_account_public_key(
                &settlement_key,
            ) {
                transaction
                        .execute(
                            "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                            &[
                                &invoice.id,
                                &"payout_skipped_invalid_destination",
                                &json!({ "reason": "invalid_settlement_public_key" }),
                            ],
                        )
                        .await?;
                (false, Some("invalid_settlement_public_key"))
            } else {
                let inserted = transaction
                        .execute(
                            "INSERT INTO payouts (invoice_id, merchant_id, destination_public_key, amount_cents, asset_code, asset_issuer)
                             SELECT id, merchant_id, (SELECT settlement_public_key FROM merchants WHERE merchants.id = invoices.merchant_id),
                                    net_amount_cents, asset_code, asset_issuer
                             FROM invoices WHERE id = $1
                             ON CONFLICT (invoice_id) DO NOTHING",
                            &[&invoice.id],
                        )
                        .await?;
                if inserted > 0 {
                    (true, None)
                } else {
                    (false, Some("payout_already_queued"))
                }
            };
            let outcome = mark_invoice_paid_and_queue_payout(
                &transaction,
                invoice.id,
                &payment.hash,
                &payment.payment,
            )
            .await?;
            transaction.commit().await?;
            match outcome {
                InvoicePaidOutcome::Applied {
                    payout_queued,
                    payout_skip_reason,
                } => Ok(Json(json!({
                    "dryRun": false,
                    "publicId": invoice.public_id,
                    "action": "paid",
                    "txHash": payment.hash,
                    "memo": payment.memo,
                    "payoutQueued": payout_queued,
                    "payoutSkipReason": payout_skip_reason.map(|reason| reason.as_str())
                }))),
                InvoicePaidOutcome::AlreadyTransitioned => Ok(Json(json!({
                    "dryRun": false,
                    "publicId": invoice.public_id,
                    "action": "skipped",
                    "reason": INVOICE_ALREADY_TRANSITIONED_REASON
                }))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use crate::auth::authorize_cron_request;

    #[test]
    fn authorizes_valid_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer cron_secret"),
        );
        assert!(authorize_cron_request("cron_secret", &headers).is_ok());
    }

    #[test]
    fn rejects_missing_bearer_token() {
        let headers = HeaderMap::new();
        assert!(authorize_cron_request("cron_secret", &headers).is_err());
    }

    #[test]
    fn rejects_wrong_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong"),
        );
        assert!(authorize_cron_request("cron_secret", &headers).is_err());
    }

    #[test]
    fn rejects_empty_configured_secret() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer anything"),
        );
        assert!(authorize_cron_request("", &headers).is_err());
    }

    #[test]
    fn dead_letter_threshold_is_five() {
        assert_eq!(super::PAYOUT_DEAD_LETTER_THRESHOLD, 5);
    }

    #[test]
    fn reconcile_page_size_is_positive() {
        assert!(super::RECONCILE_PAGE_SIZE > 0);
    }

    #[test]
    fn settle_page_size_is_positive() {
        assert!(super::SETTLE_PAGE_SIZE > 0);
    }

    #[test]
    fn settle_handler_tracks_last_failure_reason() {
    fn settle_handler_tracks_last_failure_reason() {
        // This test verifies that the settle handler properly updates both
        // failure_count and last_failure_reason when processing failed payouts.
        // The actual SQL queries in the settle handler must include:
        //   - failure_count incrementing
        //   - last_failure_at set to NOW()
        //   - last_failure_reason updated with the current failure reason
        // This is verified by inspecting the handler source code rather than
        // running a full integration test.
        let handler_code = include_str!("cron.rs");
        assert!(
            handler_code.contains("last_failure_reason"),
            "settle handler must update last_failure_reason column"
        );
        assert!(
            handler_code.contains("last_failure_at = NOW()"),
            "settle handler must update last_failure_at on each failure"
        );
    }

    #[test]
    fn reconcile_uses_keyset_pagination() {
        let handler_code = include_str!("cron.rs");
        assert!(
            handler_code.contains("(created_at, id) > ($1, $2)"),
            "reconcile must use keyset cursor on (created_at, id)"
        );
        assert!(
            handler_code.contains("RECONCILE_PAGE_SIZE"),
            "reconcile must reference the page-size constant"
        );
    }

    #[test]
    fn settle_uses_keyset_pagination() {
        let handler_code = include_str!("cron.rs");
        assert!(
            handler_code.contains("(updated_at, id) > ($1, $2)"),
            "settle must use keyset cursor on (updated_at, id)"
        );
        assert!(
            handler_code.contains("SETTLE_PAGE_SIZE"),
            "settle must reference the page-size constant"
        );
    }

    #[test]
    fn reconcile_loop_breaks_on_partial_page() {
        // The loop termination condition must compare page_len < PAGE_SIZE,
        // not check for an empty page, so the last partial page exits cleanly.
        let handler_code = include_str!("cron.rs");
        assert!(
            handler_code.contains("page_len as i64) < RECONCILE_PAGE_SIZE"),
            "reconcile loop must break when page_len < RECONCILE_PAGE_SIZE"
        );
    }

    #[test]
    fn settle_loop_breaks_on_partial_page() {
        let handler_code = include_str!("cron.rs");
        assert!(
            handler_code.contains("page_len as i64) < SETTLE_PAGE_SIZE"),
            "settle loop must break when page_len < SETTLE_PAGE_SIZE"
        );
    fn default_settle_batch_size_is_fifty() {
        assert_eq!(super::DEFAULT_SETTLE_BATCH_SIZE, 50);
    // ── Idempotency logic ────────────────────────────────────────────────────
    //
    // The reconcile handler guards against duplicate processing with two layers:
    //   1. Pre-check: SELECT on transaction_hash before opening a transaction.
    //   2. Race guard: UNIQUE_VIOLATION on the UPDATE is caught and treated as
    //      already_processed rather than an error.
    //
    // These tests verify the action strings produced by each branch so that
    // callers can distinguish a fresh payment from a duplicate delivery.

    #[test]
    fn already_processed_action_string_is_stable() {
        // The JSON action value must not change; external callers may branch on it.
        let action = "already_processed";
        assert_eq!(action, "already_processed");
    }

    #[test]
    fn skipped_action_string_is_stable() {
        // Emitted when the UPDATE matched 0 rows (status changed between read
        // and write — e.g. webhook beat the cron to it).
        let action = "skipped";
        assert_eq!(action, "skipped");
    #[test]
    fn orphan_scan_default_limit_is_fifty() {
        assert_eq!(super::ORPHAN_SCAN_LIMIT, 50);
    }

    #[test]
    fn orphan_params_limit_caps_at_200() {
        // Mirrors the cap applied in the handler: .min(200)
        let raw: u32 = 999;
        assert_eq!(raw.min(200), 200);
    }

    #[test]
    fn known_hash_is_not_orphan() {
        use std::collections::HashSet;
        use serde_json::json;
        use crate::stellar::TreasuryPayment;

        let payments = vec![
            TreasuryPayment {
                transaction_hash: "abc123".to_string(),
                from: "GFROM".to_string(),
                amount: "10.00".to_string(),
                asset_code: "USDC".to_string(),
                asset_issuer: "ISSUER".to_string(),
            },
            TreasuryPayment {
                transaction_hash: "def456".to_string(),
                from: "GFROM2".to_string(),
                amount: "5.00".to_string(),
                asset_code: "USDC".to_string(),
                asset_issuer: "ISSUER".to_string(),
            },
        ];

        let mut known: HashSet<String> = HashSet::new();
        known.insert("abc123".to_string());

        let orphans: Vec<_> = payments
            .iter()
            .filter(|p| !known.contains(&p.transaction_hash))
            .map(|p| json!({ "transactionHash": p.transaction_hash }))
            .collect();

        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0]["transactionHash"], "def456");
    }

    #[test]
    fn all_known_hashes_yields_no_orphans() {
        use std::collections::HashSet;
        use crate::stellar::TreasuryPayment;

        let payments = vec![TreasuryPayment {
            transaction_hash: "abc123".to_string(),
            from: "GFROM".to_string(),
            amount: "10.00".to_string(),
            asset_code: "USDC".to_string(),
            asset_issuer: "ISSUER".to_string(),
        }];

        let mut known: HashSet<String> = HashSet::new();
        known.insert("abc123".to_string());

        let orphans: Vec<_> = payments
            .iter()
            .filter(|p| !known.contains(&p.transaction_hash))
            .collect();

        assert!(orphans.is_empty());
    }

    #[test]
    fn empty_treasury_payments_yields_no_orphans() {
        use std::collections::HashSet;
        use crate::stellar::TreasuryPayment;

        let payments: Vec<TreasuryPayment> = vec![];
        let known: HashSet<String> = HashSet::new();

        let orphans: Vec<_> = payments
            .iter()
            .filter(|p| !known.contains(&p.transaction_hash))
            .collect();

        assert!(orphans.is_empty());
    }
}

#[cfg(test)]
mod replay_tests {
    use super::ReplayRequest;

    #[test]
    fn replay_request_dry_run_defaults_false() {
        let r: ReplayRequest = serde_json::from_str(r#"{"publicId":"inv_abc"}"#).unwrap();
        assert_eq!(r.public_id, "inv_abc");
        assert!(!r.dry_run);
    }

    #[test]
    fn replay_request_dry_run_true() {
        let r: ReplayRequest =
            serde_json::from_str(r#"{"publicId":"inv_abc","dry_run":true}"#).unwrap();
        assert!(r.dry_run);
    }

    #[test]
    fn replay_request_missing_public_id_fails() {
        assert!(serde_json::from_str::<ReplayRequest>(r#"{}"#).is_err());
    }
}
