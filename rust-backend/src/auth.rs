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

use crate::{config::Config, error::AppError, models::Merchant};

pub const SESSION_COOKIE: &str = "astropay_session";

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
    use super::{
        generate_memo, generate_public_id, hash_password, session_cookie, verify_password,
    };
    use crate::config::Config;

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
}
