use std::{env, net::SocketAddr};

use chrono::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub app_url: String,
    pub public_app_url: String,
    pub database_url: String,
    pub pgssl: String,
    pub session_secret: String,
    pub horizon_url: String,
    pub network_passphrase: String,
    pub stellar_network: String,
    pub asset_code: String,
    pub asset_issuer: String,
    pub platform_treasury_public_key: String,
    pub platform_treasury_secret_key: Option<String>,
    pub platform_fee_bps: i32,
    pub invoice_expiry_hours: i64,
    /// Shared secret for `Authorization: Bearer` on cron and Stellar webhook routes (see `auth::authorize_cron_request`).
    pub cron_secret: String,
    pub secure_cookies: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, env::VarError> {
        let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let bind_addr = format!("{host}:{port}")
            .parse()
            .expect("valid bind address");
        let app_url = env::var("APP_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
        Ok(Self {
            bind_addr,
            public_app_url: env::var("NEXT_PUBLIC_APP_URL").unwrap_or_else(|_| app_url.clone()),
            app_url: app_url.clone(),
            database_url: env::var("DATABASE_URL")?,
            pgssl: env::var("PGSSL").unwrap_or_else(|_| "disable".to_string()),
            session_secret: env::var("SESSION_SECRET")?,
            horizon_url: env::var("HORIZON_URL")
                .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".to_string()),
            network_passphrase: env::var("NETWORK_PASSPHRASE")
                .unwrap_or_else(|_| "Test SDF Network ; September 2015".to_string()),
            stellar_network: env::var("STELLAR_NETWORK").unwrap_or_else(|_| "TESTNET".to_string()),
            asset_code: env::var("ASSET_CODE").unwrap_or_else(|_| "USDC".to_string()),
            asset_issuer: env::var("ASSET_ISSUER")?,
            platform_treasury_public_key: env::var("PLATFORM_TREASURY_PUBLIC_KEY")?,
            platform_treasury_secret_key: env::var("PLATFORM_TREASURY_SECRET_KEY").ok(),
            platform_fee_bps: env::var("PLATFORM_FEE_BPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            invoice_expiry_hours: env::var("INVOICE_EXPIRY_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(24),
            cron_secret: env::var("CRON_SECRET").unwrap_or_default(),
            secure_cookies: app_url.starts_with("https://"),
        })
    }

    pub fn invoice_expiry(&self) -> Duration {
        Duration::hours(self.invoice_expiry_hours)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

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
    fn invoice_expiry_returns_hours_duration() {
        let config = sample_config();
        assert_eq!(config.invoice_expiry().num_hours(), 24);
    }

    #[test]
    fn config_preserves_url_network_and_fee_values() {
        let config = sample_config();
        assert_eq!(config.app_url, "http://localhost:3000");
        assert_eq!(config.public_app_url, "http://localhost:3000");
        assert_eq!(config.stellar_network, "TESTNET");
        assert_eq!(config.platform_fee_bps, 100);
    }

    #[test]
    fn config_keeps_ssl_and_secret_flags() {
        let config = sample_config();
        assert_eq!(config.pgssl, "disable");
        assert!(!config.secure_cookies);
        assert!(config.platform_treasury_secret_key.is_none());
    }
}
