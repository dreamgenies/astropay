use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};

use crate::{
    AppState,
    auth::authorize_cron_request,
    error::AppError,
    models::StellarWebhookRequest,
    AppState, error::AppError, models::StellarWebhookRequest, stellar::is_valid_account_public_key,
};

pub async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "astropay-rust-backend" }))
}

pub async fn stellar_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<StellarWebhookRequest>,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    if payload.public_id.is_empty() || payload.transaction_hash.is_empty() {
        return Err(AppError::bad_request(
            "publicId and transactionHash are required",
        ));
    }

    let mut client = state.pool.get().await?;
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
    let status: String = row.get("status");
    let mut payout_queued: Option<bool> = None;
    let mut payout_skip_reason: Option<&'static str> = None;
    if status == "pending" {
        let transaction = client.transaction().await?;
        transaction
            .execute(
                "UPDATE invoices
                 SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
                 WHERE id = $1 AND status = 'pending'",
                &[&invoice_id, &payload.transaction_hash],
            )
            .await?;
        transaction
            .execute(
                "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                &[&invoice_id, &"payment_detected", &payload.rest],
            )
            .await?;
        let settlement_row = transaction
            .query_opt(
                "SELECT m.settlement_public_key
                 FROM merchants m
                 INNER JOIN invoices i ON i.merchant_id = m.id
                 WHERE i.id = $1",
                &[&invoice_id],
            )
            .await?;
        let settlement_key: Option<String> = settlement_row.map(|row| row.get(0));
        let settlement_key = settlement_key.unwrap_or_default();
        let (queued, skip) = if !is_valid_account_public_key(&settlement_key) {
            transaction
                .execute(
                    "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                    &[
                        &invoice_id,
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
                    &[&invoice_id],
                )
                .await?;
            if inserted > 0 {
                (true, None)
            } else {
                (false, Some("payout_already_queued"))
            }
        };
        payout_queued = Some(queued);
        payout_skip_reason = skip;
        transaction.commit().await?;
    }

    Ok(Json(json!({
        "received": true,
        "invoiceId": invoice_id,
        "status": status,
        "payoutQueued": payout_queued,
        "payoutSkipReason": payout_skip_reason
    })))
}

