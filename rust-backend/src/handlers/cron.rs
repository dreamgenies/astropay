use axum::{Json, extract::State, http::HeaderMap};
use chrono::Utc;
use serde_json::{Value, json};
use tokio_postgres::types::Json as PgJson;
use tracing::warn;

use crate::{
    AppState,
    auth::authorize_cron_request,
    error::AppError,
    models::Invoice,
    stellar::{find_payment_for_invoice, invoice_is_expired, is_valid_account_public_key},
};

pub async fn reconcile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    let mut client = state.pool.get().await?;
    let rows = client
        .query(
            "SELECT * FROM invoices WHERE status = 'pending' ORDER BY created_at ASC LIMIT 100",
            &[],
        )
        .await?;
    let invoices = rows.iter().map(Invoice::from_row).collect::<Vec<_>>();
    let mut results = Vec::with_capacity(invoices.len());

    for invoice in invoices {
        if invoice_is_expired(&invoice, Utc::now()) {
            client
                .execute(
                    "UPDATE invoices SET status = 'expired', updated_at = NOW() WHERE id = $1 AND status = 'pending'",
                    &[&invoice.id],
                )
                .await?;
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
                results.push(json!({ "publicId": invoice.public_id, "action": "pending" }));
            }
        }
    }

    let body = json!({
        "scanned": results.len(),
        "results": results
    });
    if let Err(e) = client
        .execute(
            "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata, error_detail)
             VALUES ('reconcile', NOW(), NOW(), true, $1, NULL)",
            &[&PgJson(&body)],
        )
        .await
    {
        warn!(error = %e, "cron_runs audit insert failed for reconcile");
    }

    Ok(Json(body))
}

pub async fn settle(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize_cron_request(&state.config.cron_secret, &headers)?;
    Err(AppError::not_implemented(
        "Rust settlement execution is not implemented yet. Port the Stellar transaction signing/submission path before claiming payout parity.",
    ))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use crate::auth::authorize_cron_request;
    authorize_cron(&state, &headers)?;
    let msg = "Rust settlement execution is not implemented yet. Port the Stellar transaction signing/submission path before claiming payout parity.";
    let client = state.pool.get().await?;
    if let Err(e) = client
        .execute(
            "INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata, error_detail)
             VALUES ('settle', NOW(), NOW(), false, '{}'::jsonb, $1)",
            &[&msg],
        )
        .await
    {
        warn!(error = %e, "cron_runs audit insert failed for settle");
    }
    Err(AppError::not_implemented(msg))
}

fn authorize_cron_secret(cron_secret: &str, headers: &HeaderMap) -> Result<(), AppError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if token == Some(cron_secret) {
        Ok(())
    } else {
        Err(AppError::unauthorized("Unauthorized"))
    }
}

fn authorize_cron(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    authorize_cron_secret(state.config.cron_secret.as_str(), headers)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::authorize_cron_secret;

    #[test]
    fn authorizes_valid_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer cron_secret"),
        );
        assert!(authorize_cron_request("cron_secret", &headers).is_ok());
        assert!(authorize_cron_secret("cron_secret", &headers).is_ok());
    }

    #[test]
    fn rejects_missing_bearer_token() {
        let headers = HeaderMap::new();
        assert!(authorize_cron_request("cron_secret", &headers).is_err());
        assert!(authorize_cron_secret("cron_secret", &headers).is_err());
    }
}
