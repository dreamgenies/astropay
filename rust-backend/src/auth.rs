use axum::http::{HeaderMap, header};
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{Duration, Utc};
use deadpool_postgres::GenericClient;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::{RngCore, rngs::OsRng};
use scrypt::{
    Scrypt,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use uuid::Uuid;

use crate::{
    config::Config,
    error::{AppError, AuthErrorCode},
    models::Merchant,
};

/// Validates `Authorization: Bearer <token>` for cron and webhook routes.
pub fn authorize_cron_request(cron_secret: &str, headers: &HeaderMap) -> Result<(), AppError> {
    if cron_secret.is_empty() {
        return Err(AppError::unauthorized_code(AuthErrorCode::CronSecretMismatch));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if token == Some(cron_secret) {
        Ok(())
    } else {
        Err(AppError::unauthorized_code(AuthErrorCode::CronSecretMismatch))
    }
}

pub const SESSION_COOKIE: &str = "astropay_session";

/// Same rule as registration SQL: neither incoming key may appear in any existing
/// merchant row as either `stellar_public_key` or `settlement_public_key`.
pub fn wallet_keys_conflict_with_existing(
    existing: &[(&str, &str)],
    stellar: &str,
    settlement: &str,
) -> bool {
    existing.iter().any(|(es, et)| {
        *es == stellar || *es == settlement || *et == stellar || *et == settlement
    })
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Claims {
    sid: Uuid,
    sub: Uuid,
    exp: usize,
}

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    Scrypt
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::Internal)
}

pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    PasswordHash::new(stored_hash)
        .ok()
        .and_then(|parsed| Scrypt.verify_password(password.as_bytes(), &parsed).ok())
        .is_some()
}

pub fn generate_public_id() -> String {
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    format!("inv_{}", hex::encode(bytes))
}

pub fn generate_memo() -> String {
    let mut bytes = [0u8; 6];
    OsRng.fill_bytes(&mut bytes);
    format!("astro_{}", hex::encode(bytes))
}

pub async fn create_session<C>(
    client: &C,
    config: &Config,
    merchant_id: Uuid,
) -> Result<Cookie<'static>, AppError>
where
    C: GenericClient + Sync,
{
    let row = client
        .query_one(
            "INSERT INTO sessions (merchant_id, expires_at) VALUES ($1, NOW() + interval '30 days') RETURNING id",
            &[&merchant_id],
        )
        .await?;
    let session_id: Uuid = row.get("id");
    let claims = Claims {
        sid: session_id,
        sub: merchant_id,
        exp: (Utc::now() + Duration::days(30)).timestamp() as usize,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.session_secret.as_bytes()),
    )?;
    Ok(session_cookie(config, token))
}

pub fn clear_session_cookie(config: &Config) -> Cookie<'static> {
    let mut cookie = session_cookie(config, String::new());
    cookie.make_removal();
    cookie
}

/// Resolves the merchant for a signed session cookie.
///
/// The nested `EXISTS` probes `sessions` by **`id` (JWT `sid`)** and `merchant_id` (`sub`). PostgreSQL uses the session **primary key**
/// for that probe; `expires_at > NOW()` is evaluated on the single fetched row. Bulk expiry deletes are a separate workload and rely on
/// btree indexes on `expires_at` (see migrations `002_session_expiry_indexes.sql`).
pub async fn current_merchant<C>(
    client: &C,
    config: &Config,
    token: Option<&str>,
) -> Result<Option<Merchant>, AppError>
where
    C: GenericClient + Sync,
{
    let Some(token) = token else {
        return Ok(None);
    };

    let decoded = match decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.session_secret.as_bytes()),
        &Validation::default(),
    ) {
        Ok(decoded) => decoded,
        Err(_) => return Ok(None),
    };

    let row = client
        .query_opt(
            "SELECT id, email, business_name, stellar_public_key, settlement_public_key, created_at
             FROM merchants
             WHERE id = $1
               AND EXISTS (
                 SELECT 1
                 FROM sessions
                 WHERE id = $2 AND merchant_id = $1 AND expires_at > NOW()
               )",
            &[&decoded.claims.sub, &decoded.claims.sid],
        )
        .await?;

    Ok(row.map(|row| Merchant::from_row(&row)))
}

fn session_cookie(config: &Config, token: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .secure(config.secure_cookies)
        .same_site(SameSite::Lax)
        .build()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{
        authorize_cron_request, generate_memo, generate_public_id, hash_password, session_cookie,
        verify_password, wallet_keys_conflict_with_existing,
    };
    use crate::config::Config;

    fn g_key(fill: char) -> String {
        format!("G{}", std::iter::repeat(fill).take(55).collect::<String>())
    }

    fn sample_config() -> Config {
        Config {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            app_url: "http://localhost:3000".to_string(),
            public_app_url: "http://localhost:3000".to_string(),
            database_url: "postgres://postgres:postgres@localhost:5432/astropay".to_string(),
            pgssl: "disable".to_string(),
            session_secret: "secret".to_string(),
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            stellar_network: "TESTNET".to_string(),
            asset_code: "USDC".to_string(),
            asset_issuer: "ISSUER".to_string(),
            platform_treasury_public_key: "TREASURY".to_string(),
            platform_treasury_secret_key: None,
            platform_fee_bps: 100,
            invoice_expiry_hours: 24,
            cron_secret: "cron".to_string(),
            secure_cookies: false,
            login_rate_ip_window_secs: 600,
            login_rate_ip_max: 80,
            login_rate_email_window_secs: 900,
            login_rate_email_fail_max: 12,
        }
    }

    #[test]
    fn hash_and_verify_round_trip() {
        let hashed = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hashed));
        assert!(!verify_password("wrong-password", &hashed));
    }

    #[test]
    fn generated_ids_have_expected_prefixes_and_lengths() {
        let public_id = generate_public_id();
        let memo = generate_memo();
        assert!(public_id.starts_with("inv_"));
        assert!(memo.starts_with("astro_"));
        assert_eq!(public_id.len(), 20);
        assert_eq!(memo.len(), 18);
    }

    #[test]
    fn session_cookie_is_http_only() {
        let cookie = session_cookie(&sample_config(), "token".to_string());
        assert_eq!(cookie.name(), "astropay_session");
        assert_eq!(cookie.value(), "token");
        assert!(cookie.http_only().unwrap_or(false));
    }

    #[test]
    fn authorize_cron_accepts_matching_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer mysecret"),
        );
        assert!(authorize_cron_request("mysecret", &headers).is_ok());
    }

    #[test]
    fn authorize_cron_rejects_empty_configured_secret() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer x"),
        );
        assert!(authorize_cron_request("", &headers).is_err());
    }

    #[test]
    fn authorize_cron_rejects_wrong_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong"),
        );
        assert!(authorize_cron_request("cron_secret", &headers).is_err());
    }

    #[test]
    fn authorize_cron_rejects_when_secret_not_configured() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer anything"),
        );
        assert!(authorize_cron_request("", &headers).is_err());
    }

    #[test]
    fn wallet_conflict_detects_stellar_reuse() {
        let s1 = g_key('1');
        let t1 = g_key('2');
        let s2 = g_key('3');
        let t2 = g_key('4');
        assert!(wallet_keys_conflict_with_existing(
            &[(s1.as_str(), t1.as_str())],
            s1.as_str(),
            t2.as_str()
        ));
        assert!(!wallet_keys_conflict_with_existing(
            &[(s1.as_str(), t1.as_str())],
            s2.as_str(),
            t2.as_str()
        ));
    }

    #[test]
    fn wallet_conflict_detects_cross_column_reuse() {
        let s1 = g_key('a');
        let t1 = g_key('b');
        let s2 = g_key('c');
        assert!(wallet_keys_conflict_with_existing(
            &[(s1.as_str(), t1.as_str())],
            s2.as_str(),
            s1.as_str(),
        ));
        assert!(wallet_keys_conflict_with_existing(
            &[(s1.as_str(), t1.as_str())],
            t1.as_str(),
            s2.as_str(),
        ));
    }
}
