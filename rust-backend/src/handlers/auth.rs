use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, State},
};
use axum_extra::extract::{CookieJar as ExtractedCookieJar, cookie::CookieJar};
use serde_json::json;

use crate::{
    AppState,
    auth::{
        SESSION_COOKIE, clear_session_cookie, create_session, current_merchant, hash_password,
        refresh_session, verify_password,
    },
    error::{AppError, AuthErrorCode},
    models::{LoginRequest, RegisterRequest},
    stellar::is_valid_account_public_key,
};

pub async fn register(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
    Json(payload): Json<RegisterRequest>,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    validate_register(&payload)?;
    let stellar = payload.stellar_public_key.trim();
    let settlement = payload.settlement_public_key.trim();
    let client = state.pool.get().await?;

    let existing = client
        .query_opt(
            "SELECT 1 FROM merchants WHERE email = $1",
            &[&payload.email.trim()],
        )
        .await?;
    if existing.is_some() {
        return Err(AppError::conflict(
            "A merchant with that email already exists",
        ));
    }

    let keys_taken = client
        .query_opt(
            "SELECT 1 FROM merchants
             WHERE stellar_public_key = $1
                OR settlement_public_key = $1
                OR stellar_public_key = $2
                OR settlement_public_key = $2",
            &[&stellar, &settlement],
        )
        .await?;
    if keys_taken.is_some() {
        return Err(AppError::conflict(
            "One or both Stellar public keys are already registered on another merchant account. Each business and settlement key may only be linked once.",
        ));
    }

    let password_hash = hash_password(&payload.password)?;
    let row = client
        .query_one(
            "INSERT INTO merchants (email, password_hash, business_name, stellar_public_key, settlement_public_key)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, email, business_name, stellar_public_key, settlement_public_key, created_at",
            &[
                &payload.email.trim(),
                &password_hash,
                &payload.business_name,
                &stellar,
                &settlement,
            ],
        )
        .await?;

    let merchant = crate::models::Merchant::from_row(&row);
    let cookie = create_session(&client, &state.config, merchant.id).await?;
    Ok((jar.add(cookie), Json(json!({ "merchant": merchant }))))
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    jar: ExtractedCookieJar,
    Json(payload): Json<LoginRequest>,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    let email_key = payload.email.trim().to_lowercase();
    if payload.password.len() < 8 || !email_key.contains('@') {
        return Err(AppError::bad_request("Invalid payload"));
    }

    state.login_limiter.check_ip(&addr.ip().to_string()).await?;

    let client = state.pool.get().await?;
    let row = client
        .query_opt(
            "SELECT id, email, business_name, stellar_public_key, settlement_public_key, created_at, password_hash
             FROM merchants
             WHERE email = $1",
            &[&email_key],
        )
        .await?;
    let Some(row) = row else {
        state.login_limiter.record_email_failure(&email_key).await?;
        return Err(AppError::unauthorized_code(
            AuthErrorCode::InvalidCredentials,
        ));
    };
    let merchant = crate::models::Merchant::from_row(&row);
    let password_hash: String = row.get("password_hash");
    if !verify_password(&payload.password, &password_hash) {
        state.login_limiter.record_email_failure(&email_key).await?;
        return Err(AppError::unauthorized_code(
            AuthErrorCode::InvalidCredentials,
        ));
    }
    let cookie = create_session(&client, &state.config, merchant.id).await?;
    state.login_limiter.clear_email_failures(&email_key).await;
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
        None => Err(AppError::unauthorized_code(AuthErrorCode::SessionRequired)),
    }
}

pub async fn refresh(
    State(state): State<AppState>,
    jar: ExtractedCookieJar,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_owned());
    let Some(token) = token else {
        return Err(AppError::unauthorized_code(AuthErrorCode::SessionRequired));
    };
    let client = state.pool.get().await?;
    match refresh_session(&client, &state.config, &token).await? {
        Some(cookie) => Ok((jar.add(cookie), Json(serde_json::json!({ "ok": true })))),
        None => Err(AppError::unauthorized_code(AuthErrorCode::SessionRequired)),
    }
}

fn validate_register(payload: &RegisterRequest) -> Result<(), AppError> {
    let stellar = payload.stellar_public_key.trim();
    let settlement = payload.settlement_public_key.trim();
    if !payload.email.contains('@')
        || payload.password.len() < 8
        || payload.business_name.len() < 2
        || payload.business_name.len() > 120
        || !is_valid_account_public_key(stellar)
        || !is_valid_account_public_key(settlement)
    {
        return Err(AppError::bad_request("Invalid payload"));
    }
    Ok(())
}
