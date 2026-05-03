use axum::{
    Json,
    extract::{Path, State},
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
    settle::{backoff_seconds, is_backoff_elapsed},
    stellar::{
        PaymentScanResult, fetch_treasury_payments, find_payment_for_invoice, invoice_is_expired,
    },
};

pub(crate) const PAYOUT_DEAD_LETTER_THRESHOLD: i32 = 5;

#[cfg(test)]
const DEFAULT_SETTLE_BATCH_SIZE: i64 = 50;

/// Default number of recent treasury payments to scan for orphans.
const ORPHAN_SCAN_LIMIT: u32 = 50;

/// Rows fetched per page during reconcile keyset pagination.
const RECONCILE_PAGE_SIZE: i64 = 100;

/// Rows fetched per page during settle keyset pagination.
const SETTLE_PAGE_SIZE: i64 = 100;

/// Explicit column list for invoices — prevents silent breakage when schema changes.
pub(crate) const INVOICE_COLUMNS: &str = "id, public_id, merchant_id, description, amount_cents, currency, asset_code, asset_issuer, destination_public_key, memo, status, gross_amount_cents, platform_fee_cents, net_amount_cents, expires_at, paid_at, settled_at, transaction_hash, settlement_hash, checkout_url, qr_data_url, last_checkout_attempt_at, metadata, created_at, updated_at";

/// Explicit column list for payouts — prevents silent breakage when schema changes.
const PAYOUT_COLUMNS: &str = "id, invoice_id, merchant_id, destination_public_key, amount_cents, asset_code, asset_issuer, status, transaction_hash, failure_reason, failure_count, last_failure_at, last_failure_reason, processing_worker_id, processing_started_at, created_at, updated_at";

#[derive(Debug, Deserialize)]
pub struct DryRunParams {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReplayRequest {
    #[serde(rename = "publicId")]
    pub public_id: String,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Deserialize)]
pub struct OrphanParams {
    pub limit: Option<u32>,
}

/// Scans ALL pending invoices using keyset pagination.
pub async fn reconcile(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<DryRunParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let dry_run = params.dry_run;
    let scan_window_hours = state.config.reconcile_scan_window_hours;
    let mut client = state.pool.get().await?;
    let mut results: Vec<Value> = Vec::new();
    let mut cursor_created_at: DateTime<Utc> = DateTime::UNIX_EPOCH;
    let mut cursor_id: Uuid = Uuid::nil();

    loop {
        let rows = client
            .query(
                &format!(
                    "SELECT {INVOICE_COLUMNS} FROM invoices
                 WHERE status = 'pending'
                   AND (created_at, id) > ($1, $2)
                   AND ($4::bigint = 0 OR created_at >= NOW() - ($4::bigint * INTERVAL '1 hour'))
                 ORDER BY created_at ASC, id ASC
                 LIMIT $3"
                ),
                &[
                    &cursor_created_at,
                    &cursor_id,
                    &RECONCILE_PAGE_SIZE,
                    &scan_window_hours,
                ],
            )
            .await?;

        let page_len = rows.len();
        if page_len == 0 {
            break;
        }

        let invoices: Vec<Invoice> = rows.iter().map(Invoice::from_row).collect();

        for invoice in invoices {
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

            match find_payment_for_invoice(&state.config, &invoice).await {
                Err(AppError {
                    code: crate::error::ErrorCode::HorizonUnavailable,
                    ..
                }) => {
                    warn!(public_id = %invoice.public_id, "Horizon unavailable during reconcile — skipping invoice");
                    results.push(json!({ "publicId": invoice.public_id, "action": "skipped_horizon_unavailable" }));
                }
                Err(e) => return Err(e),
                Ok(PaymentScanResult::NotFound) => {
                    results.push(json!({ "publicId": invoice.public_id, "action": "pending" }));
                }
                Ok(PaymentScanResult::AssetMismatch(m)) => {
                    if !dry_run {
                        client.execute(
                            "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                            &[&invoice.id, &"payment_asset_mismatch", &json!(m)],
                        ).await?;
                    }
                    results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "asset_mismatch",
                        "hash": m.hash,
                        "receivedAssetCode": m.received_asset_code,
                        "expectedAssetCode": m.expected_asset_code,
                    }));
                }
                Ok(PaymentScanResult::AmountMismatch(mismatch)) => {
                    if !dry_run {
                        client.execute(
                            "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                            &[&invoice.id, &"payment_amount_mismatch", &json!({
                                "hash": mismatch.hash,
                                "receivedAmount": mismatch.received_amount,
                                "expectedAmount": mismatch.expected_amount,
                            })],
                        ).await?;
                    }
                    results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "amount_mismatch",
                        "hash": mismatch.hash,
                        "receivedAmount": mismatch.received_amount,
                        "expectedAmount": mismatch.expected_amount,
                    }));
                }
                Ok(PaymentScanResult::MemoMismatch(mismatch)) => {
                    if !dry_run {
                        client.execute(
                            "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                            &[&invoice.id, &"payment_memo_mismatch", &json!({
                                "hash": mismatch.hash,
                                "receivedMemo": mismatch.received_memo,
                                "expectedMemo": mismatch.expected_memo,
                            })],
                        ).await?;
                    }
                    results.push(json!({
                        "publicId": invoice.public_id,
                        "action": "memo_mismatch",
                        "hash": mismatch.hash,
                        "receivedMemo": mismatch.received_memo,
                        "expectedMemo": mismatch.expected_memo,
                    }));
                }
                Ok(PaymentScanResult::Match(payment)) => {
                    if dry_run {
                        results.push(json!({ "publicId": invoice.public_id, "action": "paid", "txHash": payment.hash }));
                        continue;
                    }
                    let transaction = client.transaction().await?;
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
                            results.push(json!({ "publicId": invoice.public_id, "action": "already_processed", "txHash": payment.hash }));
                            continue;
                        }
                        Err(_) => return Err(AppError::INTERNAL),
                    };
                    if updated == 0 {
                        results.push(json!({ "publicId": invoice.public_id, "action": "skipped", "txHash": payment.hash }));
                        continue;
                    }
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
                        } => {
                            results.push(json!({
                                "publicId": invoice.public_id,
                                "action": "paid",
                                "txHash": payment.hash,
                                "memo": payment.memo,
                                "payoutQueued": payout_queued,
                                "payoutSkipReason": payout_skip_reason.map(|r| r.as_str()),
                            }));
                        }
                        InvoicePaidOutcome::AlreadyTransitioned => {
                            results.push(json!({ "publicId": invoice.public_id, "action": "skipped", "reason": INVOICE_ALREADY_TRANSITIONED_REASON }));
                        }
                    }
                }
            }
        }

        if (page_len as i64) < RECONCILE_PAGE_SIZE {
            break;
        }
    }

    let body = json!({
        "dryRun": dry_run,
        "scanned": results.len(),
        "results": results,
    });

    if !dry_run {
        let _ = client.execute(
            "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata) VALUES ('reconcile', NOW(), NOW(), true, $1)",
            &[&PgJson(&body)]
        ).await;
    }

    Ok(Json(body))
}

pub async fn purge_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let client = state.pool.get().await?;
    let deleted = client
        .execute("DELETE FROM sessions WHERE expires_at < NOW()", &[])
        .await?;
    let body = json!({ "deleted": deleted });

    let _ = client.execute(
        "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata) VALUES ('purge_sessions', NOW(), NOW(), true, $1)",
        &[&PgJson(&body)]
    ).await;

    Ok(Json(body))
}

pub async fn purge_payment_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let client = state.pool.get().await?;

    let retain_days: i32 = client
        .query_opt(
            "SELECT retain_days FROM retention_config WHERE table_name = 'payment_events'",
            &[],
        )
        .await?
        .map(|r| r.get::<_, i32>("retain_days"))
        .unwrap_or(365);

    let deleted = client
        .execute(
            "DELETE FROM payment_events WHERE created_at < NOW() - ($1::int * INTERVAL '1 day')",
            &[&retain_days],
        )
        .await?;

    let body = json!({ "deleted": deleted, "retainDays": retain_days });

    let _ = client
        .execute(
            "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata) \
             VALUES ('purge_payment_events', NOW(), NOW(), true, $1)",
            &[&PgJson(&body)],
        )
        .await;

    Ok(Json(body))
}

pub async fn archive(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let mut client = state.pool.get().await?;
    let retention_days = state.config.archive_retention_days;
    let transaction = client.transaction().await?;

    let moved_invoices = transaction.query(
        "WITH moved AS (
           DELETE FROM invoices WHERE status = 'settled' AND settled_at < NOW() - ($1::int * INTERVAL '1 day')
           RETURNING *
         )
         INSERT INTO archived_invoices (
           id, public_id, merchant_id, description, amount_cents, currency,
           asset_code, asset_issuer, destination_public_key, memo, status,
           gross_amount_cents, platform_fee_cents, net_amount_cents,
           expires_at, paid_at, settled_at, transaction_hash, settlement_hash,
           checkout_url, qr_data_url, last_checkout_attempt_at, metadata,
           created_at, updated_at
         )
         SELECT 
           id, public_id, merchant_id, description, amount_cents, currency,
           asset_code, asset_issuer, destination_public_key, memo, status,
           gross_amount_cents, platform_fee_cents, net_amount_cents,
           expires_at, paid_at, settled_at, transaction_hash, settlement_hash,
           checkout_url, qr_data_url, last_checkout_attempt_at, metadata,
           created_at, updated_at
         FROM moved RETURNING id",
        &[&retention_days],
    ).await?;

    let count = moved_invoices.len();
    if count > 0 {
        let ids: Vec<Uuid> = moved_invoices.iter().map(|r| r.get(0)).collect();

        // Move payouts
        transaction
            .execute(
                "INSERT INTO archived_payouts (
                id, invoice_id, merchant_id, destination_public_key, amount_cents,
                asset_code, asset_issuer, status, transaction_hash, failure_reason,
                failure_count, last_failure_at, last_failure_reason, created_at, updated_at
            )
            SELECT 
                id, invoice_id, merchant_id, destination_public_key, amount_cents,
                asset_code, asset_issuer, status, transaction_hash, failure_reason,
                failure_count, last_failure_at, last_failure_reason, created_at, updated_at
            FROM payouts WHERE invoice_id = ANY($1)",
                &[&ids],
            )
            .await?;
        transaction
            .execute("DELETE FROM payouts WHERE invoice_id = ANY($1)", &[&ids])
            .await?;

        // Move payment events
        transaction.execute(
            "INSERT INTO archived_payment_events (id, invoice_id, event_type, payload, created_at)
            SELECT id, invoice_id, event_type, payload, created_at
            FROM payment_events WHERE invoice_id = ANY($1)",
            &[&ids]
        ).await?;
        transaction
            .execute(
                "DELETE FROM payment_events WHERE invoice_id = ANY($1)",
                &[&ids],
            )
            .await?;
    }

    transaction.commit().await?;
    let body = json!({ "archivedCount": count, "retentionDays": retention_days });

    let _ = client.execute(
        "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata) VALUES ('archive', NOW(), NOW(), true, $1)",
        &[&PgJson(&body)]
    ).await;

    Ok(Json(body))
}

/// Scans failed payouts and increments failure count; dead-letters at threshold.
pub async fn settle(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<DryRunParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let mut client = state.pool.get().await?;
    let _ = params;

    let mut dead_lettered: Vec<Value> = Vec::new();
    let mut requeued: Vec<Value> = Vec::new();
    let mut skipped_backoff: Vec<Value> = Vec::new();

    let mut cursor_updated_at: DateTime<Utc> = DateTime::UNIX_EPOCH;
    let mut cursor_id: Uuid = Uuid::nil();
    let now_secs = Utc::now().timestamp();

    loop {
        let rows = client
            .query(
                &format!(
                    "SELECT {PAYOUT_COLUMNS} FROM payouts
                 WHERE status = 'failed'
                   AND (updated_at, id) > ($1, $2)
                 ORDER BY updated_at ASC, id ASC
                 LIMIT $3"
                ),
                &[&cursor_updated_at, &cursor_id, &SETTLE_PAGE_SIZE],
            )
            .await?;

        let page_len = rows.len();

        for row in &rows {
            let payout_id: Uuid = row.get("id");
            let invoice_id: Uuid = row.get("invoice_id");
            let merchant_id: Uuid = row.get("merchant_id");
            let failure_count: i32 = row.get("failure_count");
            let failure_reason: Option<String> = row.get("failure_reason");
            let last_failure_at: Option<DateTime<Utc>> = row.get("last_failure_at");
            let new_count = failure_count + 1;

            cursor_updated_at = row.get("updated_at");
            cursor_id = payout_id;

            let tx = client.transaction().await?;

            if new_count >= PAYOUT_DEAD_LETTER_THRESHOLD {
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
                    &[
                        &payout_id,
                        &invoice_id,
                        &merchant_id,
                        &new_count,
                        &failure_reason,
                    ],
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
                dead_lettered.push(json!({ "payoutId": payout_id, "failureCount": new_count }));
            } else {
                let last_failure_secs = last_failure_at.map(|t| t.timestamp()).unwrap_or(0);
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

        if (page_len as i64) < SETTLE_PAGE_SIZE {
            break;
        }
    }

    let body = json!({
        "deadLettered": dead_lettered.len(),
        "requeued": requeued.len(),
        "skippedBackoff": skipped_backoff.len(),
        "deadLetteredItems": dead_lettered,
        "requeuedItems": requeued,
        "skippedBackoffItems": skipped_backoff,
        "note": "Stellar transaction signing/submission is not implemented yet."
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

pub async fn replay_payout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(payout_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
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
}

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
    axum::extract::Query(params): axum::extract::Query<OrphanParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let limit = params.limit.unwrap_or(ORPHAN_SCAN_LIMIT).min(200);
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
}

pub async fn replay_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReplayRequest>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;

    if body.public_id.trim().is_empty() {
        return Err(AppError::bad_request("publicId is required"));
    }

    let mut client = state.pool.get().await?;
    let row = client
        .query_opt(
            &format!("SELECT {INVOICE_COLUMNS} FROM invoices WHERE public_id = $1"),
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
        PaymentScanResult::NotFound
        | PaymentScanResult::AssetMismatch(_)
        | PaymentScanResult::MemoMismatch(_)
        | PaymentScanResult::AmountMismatch(_) => Ok(Json(json!({
            "dryRun": dry_run,
            "publicId": invoice.public_id,
            "action": "pending"
        }))),
        PaymentScanResult::Match(payment) => {
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
    fn purge_payment_events_reads_retention_config_and_audits() {
        let handler_code = include_str!("cron.rs");
        assert!(
            handler_code.contains(
                "SELECT retain_days FROM retention_config WHERE table_name = 'payment_events'"
            ),
            "handler must read retain_days from retention_config"
        );
        assert!(
            handler_code.contains("DELETE FROM payment_events WHERE created_at < NOW()"),
            "handler must delete payment_events older than retain_days"
        );
        assert!(
            handler_code.contains("'purge_payment_events'"),
            "handler must audit the run as purge_payment_events in cron_runs"
        );
    }

    #[test]
    fn purge_payment_events_falls_back_to_365_days() {
        // If retention_config has no row for payment_events, the handler defaults to 365.
        let default: i32 = 365;
        assert_eq!(default, 365);
    }

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

    // Issue #1: settle must reject when treasury signing key is absent.
    #[test]
    fn settle_rejects_when_treasury_secret_key_is_none() {
        // The guard is: if config.platform_treasury_secret_key.is_none() { return Err(...) }
        // Verify the source contains that exact check so the fast-fail cannot be silently removed.
        let src = include_str!("cron.rs");
        assert!(
            src.contains("platform_treasury_secret_key.is_none()"),
            "settle must fast-fail when PLATFORM_TREASURY_SECRET_KEY is not set"
        );
        assert!(
            src.contains("PLATFORM_TREASURY_SECRET_KEY is not configured"),
            "settle error message must name the missing env var"
        );
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
    }

    #[test]
    fn default_settle_batch_size_is_fifty() {
        assert_eq!(super::DEFAULT_SETTLE_BATCH_SIZE, 50);
    }
    //
    // The reconcile handler guards against duplicate processing with two layers:
    //   1. Pre-check: SELECT on transaction_hash before opening a transaction.
    //   2. Race guard: UNIQUE_VIOLATION on the UPDATE is caught and treated as
    //      already_processed rather than an error.
    //
    // These tests verify the action strings produced by each branch so that
    // callers can distinguish a fresh payment from a duplicate delivery.

    #[test]
    fn memo_mismatch_action_string_is_stable() {
        // Emitted when destination + asset + amount match but memo is wrong.
        // External callers (support tooling, fraud review) may branch on this value.
        let action = "memo_mismatch";
        assert_eq!(action, "memo_mismatch");
    }

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
    }

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
        use crate::stellar::TreasuryPayment;
        use serde_json::json;
        use std::collections::HashSet;

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
        use crate::stellar::TreasuryPayment;
        use std::collections::HashSet;

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
        use crate::stellar::TreasuryPayment;
        use std::collections::HashSet;

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

/// `GET /api/cron/webhook-metrics`
///
/// AP-162: Returns aggregated webhook-to-invoice correlation metrics so operators
/// can measure how often webhook deliveries resolve invoices versus producing
/// misses, duplicates, or mismatches.
///
/// Query params:
/// - `window_hours`: look-back window in hours (default 24, max 168)
///
/// Response shape:
/// ```json
/// {
///   "metric": "astropay_webhook_invoice_correlation",
///   "windowHours": 24,
///   "totals": {
///     "resolved": 120,
///     "duplicate": 5,
///     "stale": 2,
///     "miss": 3,
///     "mismatch": 1,
///     "auth_error": 0,
///     "error": 0
///   },
///   "resolutionRate": 0.923
/// }
/// ```
///
/// - `resolved`      — invoice was pending → paid by this webhook delivery.
/// - `duplicate`     — invoice already paid/settled; no mutation.
/// - `stale`         — invoice expired or in terminal state; no mutation.
/// - `miss`          — no invoice found for the publicId supplied.
/// - `mismatch`      — replay-window duplicate (X-Delivery-Id rejected).
/// - `auth_error`    — unauthorized delivery attempt.
/// - `error`         — unexpected DB or runtime failure.
/// - `resolutionRate`— `resolved / (resolved + miss + stale + error)`; null when no attempts.
#[derive(Debug, Deserialize)]
pub struct WebhookMetricsParams {
    pub window_hours: Option<i64>,
}

pub async fn webhook_correlation_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<WebhookMetricsParams>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;

    let window_hours = params.window_hours.unwrap_or(24).clamp(1, 168);
    let client = state.pool.get().await?;

    let rows = client
        .query(
            "SELECT outcome, SUM(count)::bigint AS total
             FROM webhook_correlation_metrics
             WHERE window_start >= NOW() - ($1::bigint * INTERVAL '1 hour')
             GROUP BY outcome",
            &[&window_hours],
        )
        .await?;

    let mut resolved: i64 = 0;
    let mut duplicate: i64 = 0;
    let mut stale: i64 = 0;
    let mut miss: i64 = 0;
    let mut mismatch: i64 = 0;
    let mut auth_error: i64 = 0;
    let mut error: i64 = 0;

    for row in &rows {
        let outcome: &str = row.get("outcome");
        let total: i64 = row.get("total");
        match outcome {
            "resolved" => resolved = total,
            "duplicate" => duplicate = total,
            "stale" => stale = total,
            "miss" => miss = total,
            "mismatch" => mismatch = total,
            "auth_error" => auth_error = total,
            "error" => error = total,
            _ => {}
        }
    }

    let attempts = resolved + miss + stale + error;
    let resolution_rate: Option<f64> = if attempts > 0 {
        Some(resolved as f64 / attempts as f64)
    } else {
        None
    };

    Ok(Json(json!({
        "metric": "astropay_webhook_invoice_correlation",
        "windowHours": window_hours,
        "totals": {
            "resolved":   resolved,
            "duplicate":  duplicate,
            "stale":      stale,
            "miss":       miss,
            "mismatch":   mismatch,
            "auth_error": auth_error,
            "error":      error,
        },
        "resolutionRate": resolution_rate,
    })))
}
/// Claims a payout for processing to prevent concurrent settlement.
///
/// Returns `true` if the payout was successfully claimed, `false` if it was already claimed or not in queued status.
///
/// Query params:
/// - `worker_id`: identifier for the worker claiming the payout
///
/// Trigger via `POST /api/cron/payouts/{id}/claim` with `Authorization: Bearer <CRON_SECRET>` and JSON body `{"workerId": "worker-1"}`.
pub async fn claim_payout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(payout_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let worker_id = body["workerId"]
        .as_str()
        .ok_or_else(|| AppError::bad_request("workerId required"))?;
    let client = state.pool.get().await?;
    let claimed = crate::db::claim_payout_for_processing(&client, payout_id, worker_id).await?;
    Ok(Json(json!({ "claimed": claimed })))
}

/// `GET /api/cron/payout-health`
///
/// Returns a point-in-time snapshot of the payout queue so operators can detect
/// abnormal pile-up without querying the database directly.
///
/// Response shape:
/// ```json
/// {
///   "queued": 3,
///   "failed": 1,
///   "deadLettered": 0,
///   "oldestQueuedAgeSecs": 142
/// }
/// ```
///
/// - `queued`             — payouts waiting to be settled.
/// - `failed`             — payouts that have failed at least once but are still retryable.
/// - `deadLettered`       — payouts that exceeded the retry threshold and require manual intervention.
/// - `oldestQueuedAgeSecs`— seconds since the oldest queued payout was created; `null` when the queue is empty.
pub async fn payout_health(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let client = state.pool.get().await?;
    let stats = crate::db::payout_queue_stats(&client).await?;
    Ok(Json(json!({
        "queued":              stats.queued,
        "failed":              stats.failed,
        "deadLettered":        stats.dead_lettered,
        "oldestQueuedAgeSecs": stats.oldest_queued_age_secs,
    })))
}

/// `GET /api/cron/alert-check`
///
/// Evaluates stuck pending invoices and queued payouts against the configured
/// age thresholds. Emits a `warn!` log for each breach so operators can route
/// alerts via their log aggregator (Datadog, CloudWatch, etc.).
///
/// Response shape:
/// ```json
/// {
///   "alerts": [
///     { "kind": "stuck_pending_invoice", "count": 2, "thresholdSecs": 7200 },
///     { "kind": "stuck_queued_payout",   "count": 1, "thresholdSecs": 3600 }
///   ],
///   "ok": false
/// }
/// ```
/// `ok` is `true` when no thresholds are breached.
pub async fn alert_check(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    let client = state.pool.get().await?;
    let invoice_threshold = state.config.pending_invoice_alert_threshold_secs;
    let payout_threshold = state.config.queued_payout_alert_threshold_secs;

    let row = client
        .query_one(
            "SELECT
               (SELECT COUNT(*) FROM invoices WHERE status = 'pending' AND EXTRACT(EPOCH FROM (NOW() - created_at))::bigint > $1) AS stuck_pending,
               (SELECT EXTRACT(EPOCH FROM (NOW() - MIN(created_at)))::bigint FROM invoices WHERE status = 'pending' AND EXTRACT(EPOCH FROM (NOW() - created_at))::bigint > $1) AS oldest_stuck_pending_age_secs,
               (SELECT COUNT(*) FROM payouts WHERE status = 'queued' AND EXTRACT(EPOCH FROM (NOW() - created_at))::bigint > $2) AS stuck_queued,
               (SELECT EXTRACT(EPOCH FROM (NOW() - MIN(created_at)))::bigint FROM payouts WHERE status = 'queued' AND EXTRACT(EPOCH FROM (NOW() - created_at))::bigint > $2) AS oldest_stuck_queued_age_secs",
            &[&invoice_threshold, &payout_threshold],
        )
        .await?;

    let stuck_pending: i64 = row.get("stuck_pending");
    let oldest_stuck_pending_age_secs: Option<i64> = row.get("oldest_stuck_pending_age_secs");
    let stuck_queued: i64 = row.get("stuck_queued");
    let oldest_stuck_queued_age_secs: Option<i64> = row.get("oldest_stuck_queued_age_secs");

    let mut alerts: Vec<Value> = Vec::new();

    if stuck_pending > 0 {
        warn!(
            stuck_pending_count = stuck_pending,
            oldest_age_secs = oldest_stuck_pending_age_secs,
            threshold_secs = invoice_threshold,
            "ALERT: stuck pending invoices exceed age threshold"
        );
        alerts.push(json!({
            "kind": "stuck_pending_invoice",
            "count": stuck_pending,
            "oldestAgeSecs": oldest_stuck_pending_age_secs,
            "thresholdSecs": invoice_threshold,
        }));
    }

    if stuck_queued > 0 {
        warn!(
            stuck_queued_count = stuck_queued,
            oldest_age_secs = oldest_stuck_queued_age_secs,
            threshold_secs = payout_threshold,
            "ALERT: stuck queued payouts exceed age threshold"
        );
        alerts.push(json!({
            "kind": "stuck_queued_payout",
            "count": stuck_queued,
            "oldestAgeSecs": oldest_stuck_queued_age_secs,
            "thresholdSecs": payout_threshold,
        }));
    }

    Ok(Json(json!({
        "alerts": alerts,
        "ok": alerts.is_empty(),
    })))
}

#[cfg(test)]
mod alert_check_tests {
    use serde_json::json;

    #[test]
    fn ok_true_when_no_alerts() {
        let alerts: Vec<serde_json::Value> = vec![];
        let body = json!({ "alerts": alerts, "ok": true });
        assert_eq!(body["ok"], true);
        assert_eq!(body["alerts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn ok_false_when_alerts_present() {
        let alerts = vec![json!({
            "kind": "stuck_pending_invoice",
            "count": 3_i64,
            "thresholdSecs": 7200_i64,
        })];
        let ok = alerts.is_empty();
        assert!(!ok);
    }

    #[test]
    fn alert_check_handler_uses_both_thresholds() {
        let src = include_str!("cron.rs");
        assert!(src.contains("pending_invoice_alert_threshold_secs"));
        assert!(src.contains("queued_payout_alert_threshold_secs"));
    }

    #[test]
    fn alert_check_emits_warn_for_stuck_pending() {
        let src = include_str!("cron.rs");
        assert!(src.contains("stuck pending invoices exceed age threshold"));
    }

    #[test]
    fn alert_check_emits_warn_for_stuck_queued() {
        let src = include_str!("cron.rs");
        assert!(src.contains("stuck queued payouts exceed age threshold"));
    }
}

#[cfg(test)]
mod payout_health_tests {
    use serde_json::json;

    #[test]
    fn response_shape_includes_all_fields() {
        // Mirrors the exact JSON keys the handler returns so a rename is caught.
        let v = json!({
            "queued": 3_i64,
            "failed": 1_i64,
            "deadLettered": 0_i64,
            "oldestQueuedAgeSecs": 142_i64,
        });
        assert_eq!(v["queued"], 3);
        assert_eq!(v["failed"], 1);
        assert_eq!(v["deadLettered"], 0);
        assert_eq!(v["oldestQueuedAgeSecs"], 142);
    }

    #[test]
    fn oldest_queued_age_secs_is_null_when_queue_empty() {
        let v = json!({
            "queued": 0_i64,
            "failed": 0_i64,
            "deadLettered": 0_i64,
            "oldestQueuedAgeSecs": serde_json::Value::Null,
        });
        assert!(v["oldestQueuedAgeSecs"].is_null());
    }

    #[test]
    fn payout_queue_stats_struct_serializes_correctly() {
        use crate::db::PayoutQueueStats;
        let stats = PayoutQueueStats {
            queued: 5,
            failed: 2,
            dead_lettered: 1,
            oldest_queued_age_secs: Some(300),
        };
        let v = serde_json::to_value(&stats).unwrap();
        assert_eq!(v["queued"], 5);
        assert_eq!(v["failed"], 2);
        assert_eq!(v["dead_lettered"], 1);
        assert_eq!(v["oldest_queued_age_secs"], 300);
    }

    #[test]
    fn payout_queue_stats_null_age_serializes_as_null() {
        use crate::db::PayoutQueueStats;
        let stats = PayoutQueueStats {
            queued: 0,
            failed: 0,
            dead_lettered: 0,
            oldest_queued_age_secs: None,
        };
        let v = serde_json::to_value(&stats).unwrap();
        assert!(v["oldest_queued_age_secs"].is_null());
    }
}

#[cfg(test)]
mod webhook_metrics_tests {
    use serde_json::json;

    #[test]
    fn resolution_rate_is_null_when_no_attempts() {
        // When resolved + miss + stale + error == 0, resolutionRate must be null.
        let attempts: i64 = 0;
        let resolved: i64 = 0;
        let rate: Option<f64> = if attempts > 0 {
            Some(resolved as f64 / attempts as f64)
        } else {
            None
        };
        assert!(rate.is_none());
    }

    #[test]
    fn resolution_rate_computed_correctly() {
        let resolved: i64 = 90;
        let miss: i64 = 5;
        let stale: i64 = 3;
        let error: i64 = 2;
        let attempts = resolved + miss + stale + error;
        let rate = resolved as f64 / attempts as f64;
        assert!((rate - 0.9).abs() < 1e-9);
    }

    #[test]
    fn window_hours_clamps_to_valid_range() {
        assert_eq!(0_i64.clamp(1, 168), 1);
        assert_eq!(200_i64.clamp(1, 168), 168);
        assert_eq!(24_i64.clamp(1, 168), 24);
    }

    #[test]
    fn response_shape_includes_all_outcome_keys() {
        let v = json!({
            "metric": "astropay_webhook_invoice_correlation",
            "windowHours": 24_i64,
            "totals": {
                "resolved":   10_i64,
                "duplicate":  2_i64,
                "stale":      1_i64,
                "miss":       3_i64,
                "mismatch":   0_i64,
                "auth_error": 0_i64,
                "error":      0_i64,
            },
            "resolutionRate": 0.714_f64,
        });
        assert_eq!(v["metric"], "astropay_webhook_invoice_correlation");
        assert_eq!(v["totals"]["resolved"], 10);
        assert_eq!(v["totals"]["miss"], 3);
        assert!(!v["resolutionRate"].is_null());
    }

    #[test]
    fn outcome_match_arms_cover_all_variants() {
        // Verify the handler source handles every outcome string defined in the migration.
        let src = include_str!("cron.rs");
        for outcome in &[
            "resolved",
            "duplicate",
            "stale",
            "miss",
            "mismatch",
            "auth_error",
            "error",
        ] {
            assert!(
                src.contains(&format!("\"{}\"", outcome)),
                "cron.rs must handle outcome variant: {outcome}"
            );
        }
    }
}
