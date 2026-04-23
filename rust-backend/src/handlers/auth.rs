use axum::{Json, extract::State};
use axum_extra::extract::{CookieJar as ExtractedCookieJar, cookie::CookieJar};
use serde_json::json;

use crate::{
    AppState,
    auth::{
        SESSION_COOKIE, clear_session_cookie, create_session, current_merchant, hash_password,
        verify_password,
    },
    error::AppError,
    models::{LoginRequest, RegisterRequest},
    stellar::is_valid_account_public_key,
};

pub async fn register(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
    Json(payload): Json<RegisterRequest>,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    validate_register(&payload)?;
    let client = state.pool.get().await?;

    let existing = client
        .query_opt(
            "SELECT 1 FROM merchants WHERE email = $1",
            &[&payload.email.to_lowercase()],
        )
        .await?;
    if existing.is_some() {
        return Err(AppError::conflict(
            "A merchant with that email already exists",
        ));
    }

    let password_hash = hash_password(&payload.password)?;
    let row = client
        .query_one(
            "INSERT INTO merchants (email, password_hash, business_name, stellar_public_key, settlement_public_key)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, email, business_name, stellar_public_key, settlement_public_key, created_at",
            &[
                &payload.email.to_lowercase(),
                &password_hash,
                &payload.business_name,
                &payload.stellar_public_key,
                &payload.settlement_public_key,
            ],
        )
        .await?;

    let merchant = crate::models::Merchant::from_row(&row);
    let cookie = create_session(&client, &state.config, merchant.id).await?;
    Ok((jar.add(cookie), Json(json!({ "merchant": merchant }))))
}

pub async fn login(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
    Json(payload): Json<LoginRequest>,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    if payload.password.len() < 8 || !payload.email.contains('@') {
        return Err(AppError::bad_request("Invalid payload"));
    }

    let client = state.pool.get().await?;
    let row = client
        .query_opt(
            "SELECT id, email, business_name, stellar_public_key, settlement_public_key, created_at, password_hash
             FROM merchants
             WHERE email = $1",
            &[&payload.email.to_lowercase()],
        )
        .await?;
    let Some(row) = row else {
        return Err(AppError::unauthorized("Invalid credentials"));
    };
    let merchant = crate::models::Merchant::from_row(&row);
    let password_hash: String = row.get("password_hash");
    if !verify_password(&payload.password, &password_hash) {
        return Err(AppError::unauthorized("Invalid credentials"));
    }
    let cookie = create_session(&client, &state.config, merchant.id).await?;
    Ok((
        jar.add(cookie),
        Json(json!({ "merchant": merchant.as_login() })),
    ))
}

pub async fn logout(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    Ok((
        jar.add(clear_session_cookie(&state.config)),
        Json(json!({ "ok": true })),
    ))
}

pub async fn me(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
) -> Result<Json<serde_json::Value>, AppError> {
    let client = state.pool.get().await?;
    let merchant = current_merchant(
        &client,
        &state.config,
        jar.get(SESSION_COOKIE).map(|c| c.value()),
    )
    .await?;
    match merchant {
        Some(merchant) => Ok(Json(json!({ "merchant": merchant }))),
        None => Err(AppError::unauthorized("Unauthorized")),
    }
}

fn validate_register(payload: &RegisterRequest) -> Result<(), AppError> {
    if !payload.email.contains('@')
        || payload.password.len() < 8
        || payload.business_name.len() < 2
        || payload.business_name.len() > 120
        || !is_valid_account_public_key(&payload.stellar_public_key)
        || !is_valid_account_public_key(&payload.settlement_public_key)
    {
        return Err(AppError::bad_request("Invalid payload"));
    }
    Ok(())
}
