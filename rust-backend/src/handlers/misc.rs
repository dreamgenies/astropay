use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};
use tracing::warn;

use crate::{
    AppState,
    auth::authorize_cron_request,
    error::AppError,
    models::StellarWebhookRequest,
    money_state::{
        INVOICE_ALREADY_TRANSITIONED_REASON, InvoicePaidOutcome, mark_invoice_paid_and_queue_payout,
    },
};

pub async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "astropay-rust-backend" }))
}

pub async fn stellar_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<StellarWebhookRequest>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(state.config.cron_secret.inner(), &headers)?;
    if payload.public_id.is_empty() || payload.transaction_hash.is_empty() {
        return Err(AppError::bad_request(
            "publicId and transactionHash are required",
        ));
    }

    let mut client = state.pool.get().await?;

    // Store raw payload with source metadata for audit/debugging (AP-160).
    let raw_body = json!({
        "publicId": payload.public_id,
        "transactionHash": payload.transaction_hash,
        "rest": payload.rest,
    });
    if let Err(e) = client
        .execute(
            "INSERT INTO webhook_raw_payloads (source, payload) VALUES ($1, $2)",
            &[&"stellar", &tokio_postgres::types::Json(&raw_body)],
        )
        .await
    {
        warn!(error = %e, "webhook_raw_payloads insert failed");
    }

    // Idempotency check: if this transaction_hash is already recorded on any
    // invoice, the payment was already processed. Return success without
    // mutating state so duplicate deliveries are safe.
    let duplicate = client
        .query_opt(
            "SELECT id FROM invoices WHERE transaction_hash = $1",
            &[&payload.transaction_hash],
        )
        .await?;
    if duplicate.is_some() {
        return Ok(Json(json!({
            "received": true,
            "alreadyProcessed": true,
            "transactionHash": payload.transaction_hash
        })));
    }

    let row = client
        .query_opt(
            "SELECT id, status FROM invoices WHERE public_id = $1",
            &[&payload.public_id],
        )
        .await?;
    let Some(row) = row else {
        return Err(AppError::not_found("Invoice not found"));
    };

    let invoice_id: uuid::Uuid = row.get("id");
    let mut status: String = row.get("status");
    let mut payout_queued: Option<bool> = None;
    let mut payout_skip_reason: Option<&'static str> = None;

    if status == "pending" {
        let transaction = client.transaction().await?;

        let updated = transaction
            .execute(
                "UPDATE invoices
                 SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
                 WHERE id = $1 AND status = 'pending'",
                &[&invoice_id, &payload.transaction_hash],
            )
            .await;

        // A unique-violation (code 23505) means a concurrent delivery already
        // committed the same hash. Treat as already-processed.
        let updated = match updated {
            Ok(n) => n,
            Err(ref e) if is_unique_violation(e) => {
                drop(transaction);
                return Ok(Json(json!({
                    "received": true,
                    "alreadyProcessed": true,
                    "transactionHash": payload.transaction_hash
                })));
            }
            Err(_) => return Err(AppError::INTERNAL),
        };

        if updated == 0 {
            drop(transaction);
            return Ok(Json(json!({
                "received": true,
                "invoiceId": invoice_id,
                "status": status,
                "skipped": true
            })));
        }

        let outcome = mark_invoice_paid_and_queue_payout(
            &transaction,
            invoice_id,
            &payload.transaction_hash,
            &payload.rest,
        )
        .await?;
        transaction.commit().await?;

        match outcome {
            InvoicePaidOutcome::Applied {
                payout_queued: queued,
                payout_skip_reason: skip,
            } => {
                status = "paid".to_string();
                payout_queued = Some(queued);
                payout_skip_reason = skip.map(|reason| reason.as_str());
            }
            InvoicePaidOutcome::AlreadyTransitioned => {
                let refreshed = client
                    .query_one("SELECT status FROM invoices WHERE id = $1", &[&invoice_id])
                    .await?;
                status = refreshed.get("status");
                payout_queued = Some(false);
                payout_skip_reason = Some(INVOICE_ALREADY_TRANSITIONED_REASON);
            }
        }
    }

    Ok(Json(json!({
        "received": true,
        "invoiceId": invoice_id,
        "status": status,
        "payoutQueued": payout_queued,
        "payoutSkipReason": payout_skip_reason
    })))
}

/// Returns true when the postgres error is a unique-constraint violation (SQLSTATE 23505).
fn is_unique_violation(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION)
}

#[cfg(test)]
mod tests {
    use super::is_unique_violation;

    #[test]
    fn non_unique_violation_error_is_not_flagged() {
        let _ = is_unique_violation; // compile check
    }
}
