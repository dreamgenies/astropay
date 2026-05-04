use axum::http::{HeaderMap, header};
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{Duration, Utc};
use deadpool_postgres::GenericClient;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::{RngCore, rngs::OsRng};
use scrypt::{
    Params, Scrypt,
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
        return Err(AppError::unauthorized_code(
            AuthErrorCode::CronSecretMismatch,
        ));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if token == Some(cron_secret) {
        Ok(())
    } else {
        Err(AppError::unauthorized_code(
            AuthErrorCode::CronSecretMismatch,
        ))
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
    existing
        .iter()
        .any(|(es, et)| *es == stellar || *es == settlement || *et == stellar || *et == settlement)
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Claims {
    sid: Uuid,
    sub: Uuid,
    exp: usize,
}

pub fn hash_password(password: &str) -> Result<String, AppError> {
    hash_password_with_params(password, Params::recommended())
}

fn hash_password_with_params(password: &str, params: Params) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    Scrypt
        .hash_password_customized(password.as_bytes(), None, None, params, &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::INTERNAL)
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
        &EncodingKey::from_secret(config.session_secret.inner().as_bytes()),
    )?;
    Ok(session_cookie(config, token))
}

/// Extends an existing valid session by 30 days and returns a fresh cookie.
/// Requires the session row to still be unexpired; does not create a new session row.
pub async fn refresh_session<C>(
    client: &C,
    config: &Config,
    token: &str,
) -> Result<Option<Cookie<'static>>, AppError>
where
    C: GenericClient + Sync,
{
    let decoded = match decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.session_secret.inner().as_bytes()),
        &Validation::default(),
    ) {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };

    let updated = client
        .execute(
            "UPDATE sessions SET expires_at = NOW() + interval '30 days'
             WHERE id = $1 AND merchant_id = $2 AND expires_at > NOW()",
            &[&decoded.claims.sid, &decoded.claims.sub],
        )
        .await?;

    if updated == 0 {
        return Ok(None);
    }

    let new_exp = (Utc::now() + Duration::days(30)).timestamp() as usize;
    let new_claims = Claims {
        sid: decoded.claims.sid,
        sub: decoded.claims.sub,
        exp: new_exp,
    };
    let new_token = encode(
        &Header::default(),
        &new_claims,
        &EncodingKey::from_secret(config.session_secret.inner().as_bytes()),
    )?;
    Ok(Some(session_cookie(config, new_token)))
}

pub fn clear_session_cookie(config: &Config) -> Cookie<'static> {
    let mut cookie = session_cookie(config, String::new());
    cookie.make_removal();
    cookie
}

/// Resolves the merchant for a signed session cookie.
///
/// The nested `EXISTS` probes `sessions` by **`id` (JWT `sid`)** and `merchant_id` (`sub`).
/// `expires_at > NOW()` is evaluated on the single fetched row.
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
        &DecodingKey::from_secret(config.session_secret.inner().as_bytes()),
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
    use scrypt::Params;

    use super::{
        authorize_cron_request, generate_memo, generate_public_id, hash_password_with_params,
        session_cookie, verify_password, wallet_keys_conflict_with_existing,
    };
    use crate::config::Config;
    use crate::redact::Redacted;

    fn secure_config() -> Config {
        Config {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            app_url: "https://astropay.example.com".to_string(),
            public_app_url: "https://astropay.example.com".to_string(),
            database_url: Redacted::new("postgres://localhost/astropay".to_string()),
            pgssl: "require".to_string(),
            session_secret: Redacted::new("prod-secret".to_string()),
            horizon_url: "https://horizon.stellar.org".to_string(),
            network_passphrase: "Public Global Stellar Network ; September 2015".to_string(),
            stellar_network: "MAINNET".to_string(),
            asset_code: "USDC".to_string(),
            asset_issuer: "ISSUER".to_string(),
            platform_treasury_public_key: "TREASURY".to_string(),
            platform_treasury_secret_key: None,
            platform_fee_bps: 100,
            invoice_expiry_hours: 24,
            cron_secret: Redacted::new("cron".to_string()),
            secure_cookies: true,
            login_rate_ip_window_secs: 600,
            login_rate_ip_max: 80,
            login_rate_email_window_secs: 900,
            login_rate_email_fail_max: 12,
            reconcile_scan_limit: 100,
            reconcile_scan_window_hours: 0,
            log_format: crate::config::LogFormat::Json,
            archive_retention_days: 30,
            pending_invoice_alert_threshold_secs: 7200,
            queued_payout_alert_threshold_secs: 3600,
        }
    }

    fn insecure_config() -> Config {
        Config {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            app_url: "http://localhost:3000".to_string(),
            public_app_url: "http://localhost:3000".to_string(),
            database_url: Redacted::new("postgres://localhost/astropay".to_string()),
            pgssl: "disable".to_string(),
            session_secret: Redacted::new("secret".to_string()),
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            stellar_network: "TESTNET".to_string(),
            asset_code: "USDC".to_string(),
            asset_issuer: "ISSUER".to_string(),
            platform_treasury_public_key: "TREASURY".to_string(),
            platform_treasury_secret_key: None,
            platform_fee_bps: 100,
            invoice_expiry_hours: 24,
            cron_secret: Redacted::new("cron".to_string()),
            secure_cookies: false,
            login_rate_ip_window_secs: 600,
            login_rate_ip_max: 80,
            login_rate_email_window_secs: 900,
            login_rate_email_fail_max: 12,
            reconcile_scan_limit: 100,
            reconcile_scan_window_hours: 0,
            log_format: crate::config::LogFormat::Human,
            archive_retention_days: 30,
            pending_invoice_alert_threshold_secs: 7200,
            queued_payout_alert_threshold_secs: 3600,
        }
    }

    fn g_key(fill: char) -> String {
        format!("G{}", std::iter::repeat(fill).take(55).collect::<String>())
    }

    // --- secure cookie flag ---

    #[test]
    fn session_cookie_secure_flag_set_when_config_is_true() {
        let cookie = session_cookie(&secure_config(), "tok".to_string());
        assert!(
            cookie.secure().unwrap_or(false),
            "secure flag must be set for https config"
        );
    }

    #[test]
    fn session_cookie_secure_flag_unset_when_config_is_false() {
        let cookie = session_cookie(&insecure_config(), "tok".to_string());
        assert!(
            !cookie.secure().unwrap_or(true),
            "secure flag must be absent for http config"
        );
    }

    #[test]
    fn session_cookie_is_always_http_only() {
        assert!(
            session_cookie(&secure_config(), "t".to_string())
                .http_only()
                .unwrap_or(false)
        );
        assert!(
            session_cookie(&insecure_config(), "t".to_string())
                .http_only()
                .unwrap_or(false)
        );
    }

    #[test]
    fn session_cookie_name_and_path_are_stable() {
        let cookie = session_cookie(&insecure_config(), "token".to_string());
        assert_eq!(cookie.name(), "astropay_session");
        assert_eq!(cookie.path(), Some("/"));
    }

    // --- password hashing ---

    #[test]
    fn hash_and_verify_round_trip() {
        let params = Params::new(10, 1, 1, 32).unwrap();
        let hashed = hash_password_with_params("correct horse battery staple", params).unwrap();
        assert!(verify_password("correct horse battery staple", &hashed));
        assert!(!verify_password("wrong-password", &hashed));
    }

    // --- id generation ---

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
    fn generate_public_id_matches_db_constraint_pattern() {
        // Validates against the same pattern used in migration 016:
        // CHECK (public_id ~ '^inv_[0-9a-f]{16}$')
        for _ in 0..20 {
            let id = generate_public_id();
            assert!(id.starts_with("inv_"), "must start with inv_: {id}");
            assert_eq!(id.len(), 20, "must be 20 chars: {id}");
            let hex_part = &id[4..];
            assert!(
                hex_part
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
                "body must be lowercase hex: {id}"
            );
        }
    }

    // --- cron auth ---

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
    fn authorize_cron_rejects_missing_header() {
        assert!(authorize_cron_request("secret", &HeaderMap::new()).is_err());
    }

    // --- wallet key conflict ---

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

    #[test]
    fn wallet_conflict_no_conflict_with_empty_existing() {
        let s = g_key('x');
        let t = g_key('y');
        assert!(!wallet_keys_conflict_with_existing(
            &[],
            s.as_str(),
            t.as_str()
        ));
    }

    // --- cookie token value and SameSite ---

    #[test]
    fn session_cookie_stores_token_value() {
        let cookie = session_cookie(&insecure_config(), "my-jwt-token".to_string());
        assert_eq!(cookie.value(), "my-jwt-token");
    }

    #[test]
    fn session_cookie_same_site_is_lax() {
        use axum_extra::extract::cookie::SameSite;
        let cookie = session_cookie(&secure_config(), "tok".to_string());
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
    }

    #[test]
    fn session_cookie_constant_name_matches_cookie() {
        use super::SESSION_COOKIE;
        let cookie = session_cookie(&insecure_config(), "t".to_string());
        assert_eq!(cookie.name(), SESSION_COOKIE);
    }

    // --- id uniqueness ---

    #[test]
    fn generate_public_id_produces_unique_values() {
        let a = generate_public_id();
        let b = generate_public_id();
        assert_ne!(a, b, "two consecutive public_ids must differ");
    }

    #[test]
    fn generate_memo_produces_unique_values() {
        let a = generate_memo();
        let b = generate_memo();
        assert_ne!(a, b, "two consecutive memos must differ");
    }

    // --- password edge cases ---

    #[test]
    fn verify_password_returns_false_for_malformed_hash() {
        assert!(!verify_password("password", "not-a-valid-hash"));
    }

    #[test]
    fn verify_password_returns_false_for_empty_hash() {
        assert!(!verify_password("password", ""));
    }

    // --- cron auth edge cases ---

    #[test]
    fn authorize_cron_rejects_non_bearer_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic mysecret"),
        );
        assert!(authorize_cron_request("mysecret", &headers).is_err());
    }

    #[test]
    fn authorize_cron_rejects_bearer_with_extra_whitespace() {
        let mut headers = HeaderMap::new();
        // "Bearer  secret" — double space; strip_prefix("Bearer ") leaves " secret"
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer  mysecret"),
        );
        assert!(authorize_cron_request("mysecret", &headers).is_err());
    }
}
