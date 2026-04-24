use axum::{
    Json,
    extract::{Path, State},
};
use axum_extra::extract::{CookieJar as ExtractedCookieJar, cookie::CookieJar};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use qrcode::{QrCode, render::svg};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{SESSION_COOKIE, current_merchant, generate_memo, generate_public_id},
    error::{AppError, AuthErrorCode},
    models::{Invoice, InvoiceRequest, Merchant},
    stellar::build_checkout_url,
};

pub async fn list_invoices(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
) -> Result<Json<Value>, AppError> {
    let client = state.pool.get().await?;
    let merchant = require_merchant(&state, &client, &jar).await?;
    // Uses invoices_merchant_created_at_id_idx (migration 006) — single index scan,
    // no separate sort step. Explicit column list avoids fetching qr_data_url
    // (large base64 blob) on the list view.
    let rows = client
        .query(
            "SELECT id, public_id, merchant_id, description, amount_cents, currency,
                    asset_code, asset_issuer, destination_public_key, memo, status,
                    gross_amount_cents, platform_fee_cents, net_amount_cents,
                    expires_at, paid_at, settled_at, transaction_hash, settlement_hash,
                    checkout_url, NULL::text AS qr_data_url, metadata, created_at, updated_at
             FROM invoices
             WHERE merchant_id = $1
             ORDER BY created_at DESC, id
             LIMIT 100",
            &[&merchant.id],
        )
        .await?;
    let invoices = rows.iter().map(Invoice::from_row).collect::<Vec<_>>();
    Ok(Json(json!({ "invoices": invoices })))
}

pub async fn create_invoice(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
    Json(payload): Json<InvoiceRequest>,
) -> Result<Json<Value>, AppError> {
    if payload.description.len() < 2 || payload.description.len() > 240 || payload.amount_usd <= 0.0
    {
        return Err(AppError::bad_request("Invalid payload"));
    }

    let client = state.pool.get().await?;
    let merchant = require_merchant(&state, &client, &jar).await?;

    let amount_cents = (payload.amount_usd * 100.0).round() as i32;
    let fee = std::cmp::max(1, (amount_cents * state.config.platform_fee_bps) / 10_000);
    let gross = amount_cents;
    let net = gross - fee;
    let public_id = generate_public_id();
    let memo = generate_memo();
    let expires_at = Utc::now() + state.config.invoice_expiry();
    let metadata = json!({ "product": "ASTROpay" });

    let row = client
        .query_one(
            "INSERT INTO invoices (
               public_id, merchant_id, description, amount_cents, gross_amount_cents, platform_fee_cents,
               net_amount_cents, currency, asset_code, asset_issuer, destination_public_key, memo, expires_at, metadata
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,'USD',$8,$9,$10,$11,$12,$13)
             RETURNING *",
            &[
                &public_id,
                &merchant.id,
                &payload.description,
                &amount_cents,
                &gross,
                &fee,
                &net,
                &state.config.asset_code,
                &state.config.asset_issuer,
                &state.config.platform_treasury_public_key,
                &memo,
                &expires_at,
                &metadata,
            ],
        )
        .await?;
    let invoice = Invoice::from_row(&row);
    let checkout_url = build_checkout_url(&state.config, &invoice.public_id);
    let qr_svg = QrCode::new(checkout_url.as_bytes())
        .map_err(|_| AppError::Internal)?
        .render::<svg::Color>()
        .min_dimensions(280, 280)
        .build();
    let qr_data_url = format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(qr_svg.as_bytes())
    );

    let updated = client
        .query_one(
            "UPDATE invoices SET qr_data_url = $2, checkout_url = $3 WHERE id = $1 RETURNING *",
            &[&invoice.id, &qr_data_url, &checkout_url],
        )
        .await?;

    Ok(Json(json!({ "invoice": Invoice::from_row(&updated) })))
}

pub async fn get_invoice(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let client = state.pool.get().await?;
    let merchant = require_merchant(&state, &client, &jar).await?;
    let row = client
        .query_opt(
            "SELECT * FROM invoices WHERE merchant_id = $1 AND id = $2",
            &[&merchant.id, &id],
        )
        .await?;
    let Some(row) = row else {
        return Err(AppError::not_found("Invoice not found"));
    };
    Ok(Json(json!({ "invoice": Invoice::from_row(&row) })))
}

pub async fn get_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let client = state.pool.get().await?;
    let row = client
        .query_opt(
            "SELECT status, paid_at, settled_at FROM invoices WHERE id = $1",
            &[&id],
        )
        .await?;
    let Some(row) = row else {
        return Err(AppError::not_found("Invoice not found"));
    };
    let status: String = row.get("status");
    let paid_at: Option<chrono::DateTime<Utc>> = row.get("paid_at");
    let settled_at: Option<chrono::DateTime<Utc>> = row.get("settled_at");
    Ok(Json(json!({
        "status": status,
        "paidAt": paid_at,
        "settledAt": settled_at
    })))
}

pub async fn unsupported_checkout() -> Result<Json<Value>, AppError> {
    Err(AppError::not_implemented(
        "Rust checkout XDR generation/submission is not implemented yet. Keep the Next.js payment routes for now or finish the Stellar port.",
    ))
}

async fn require_merchant(
    state: &AppState,
    client: &deadpool_postgres::Client,
    jar: &CookieJar,
) -> Result<Merchant, AppError> {
    current_merchant(
        client,
        &state.config,
        jar.get(SESSION_COOKIE).map(|cookie| cookie.value()),
    )
    .await?
    .ok_or_else(|| AppError::unauthorized_code(AuthErrorCode::SessionRequired))
}
