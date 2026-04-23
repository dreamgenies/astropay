use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;

use crate::{config::Config, error::AppError, models::Invoice};

/// Returns true when `value` is a well-formed Stellar Ed25519 account strkey (checksum-valid `G...`).
pub fn is_valid_account_public_key(value: &str) -> bool {
    Ed25519PublicKey::from_string(value).is_ok()
}

#[derive(Debug, Clone)]
pub struct PaymentMatch {
    pub hash: String,
    pub payment: serde_json::Value,
    pub memo: String,
}

#[derive(Debug, Deserialize)]
struct PaymentsPage {
    #[serde(rename = "_embedded")]
    embedded: EmbeddedPayments,
}

#[derive(Debug, Deserialize)]
struct EmbeddedPayments {
    records: Vec<HorizonPayment>,
}

#[derive(Debug, Deserialize, Clone)]
struct HorizonPayment {
    #[serde(rename = "type")]
    record_type: String,
    to: Option<String>,
    account: Option<String>,
    asset_code: Option<String>,
    asset_issuer: Option<String>,
    amount: Option<String>,
    transaction_hash: String,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct HorizonTransaction {
    memo: String,
}

pub fn build_checkout_url(config: &Config, public_id: &str) -> String {
    format!(
        "{}/pay/{}",
        config.public_app_url.trim_end_matches('/'),
        public_id
    )
}

pub fn invoice_amount_to_asset(invoice: &Invoice) -> String {
    format!("{:.2}", invoice.gross_amount_cents as f64 / 100.0)
}

pub fn payment_matches_invoice(record: &serde_json::Value, memo: &str, invoice: &Invoice) -> bool {
    let destination = record
        .get("to")
        .or_else(|| record.get("account"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let asset_code = record
        .get("asset_code")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let asset_issuer = record
        .get("asset_issuer")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let amount = record
        .get("amount")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    destination == invoice.destination_public_key
        && asset_code == invoice.asset_code
        && asset_issuer == invoice.asset_issuer
        && amount == invoice_amount_to_asset(invoice)
        && memo == invoice.memo
}

pub async fn find_payment_for_invoice(
    config: &Config,
    invoice: &Invoice,
) -> Result<Option<PaymentMatch>, AppError> {
    let payments_url = format!(
        "{}/accounts/{}/payments?order=desc&limit=50",
        config.horizon_url.trim_end_matches('/'),
        invoice.destination_public_key
    );
    let client = Client::new();
    let page = client
        .get(payments_url)
        .send()
        .await
        .map_err(|_| AppError::Internal)?
        .error_for_status()
        .map_err(|_| AppError::Internal)?
        .json::<PaymentsPage>()
        .await
        .map_err(|_| AppError::Internal)?;

    for record in page.embedded.records {
        if record.record_type != "payment" {
            continue;
        }
        if record.to.as_deref().or(record.account.as_deref())
            != Some(invoice.destination_public_key.as_str())
        {
            continue;
        }
        if record.asset_code.as_deref() != Some(invoice.asset_code.as_str()) {
            continue;
        }
        if record.asset_issuer.as_deref() != Some(invoice.asset_issuer.as_str()) {
            continue;
        }
        if record.amount.as_deref() != Some(invoice_amount_to_asset(invoice).as_str()) {
            continue;
        }

        let tx_url = format!(
            "{}/transactions/{}",
            config.horizon_url.trim_end_matches('/'),
            record.transaction_hash
        );
        let tx = client
            .get(tx_url)
            .send()
            .await
            .map_err(|_| AppError::Internal)?
            .error_for_status()
            .map_err(|_| AppError::Internal)?
            .json::<HorizonTransaction>()
            .await
            .map_err(|_| AppError::Internal)?;

        if payment_matches_invoice(&record.raw, &tx.memo, invoice) {
            return Ok(Some(PaymentMatch {
                hash: record.transaction_hash,
                payment: record.raw,
                memo: tx.memo,
            }));
        }
    }

    Ok(None)
}

pub fn invoice_is_expired(invoice: &Invoice, now: DateTime<Utc>) -> bool {
    now > invoice.expires_at
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        build_checkout_url, invoice_amount_to_asset, invoice_is_expired,
        is_valid_account_public_key, payment_matches_invoice,
    };
    use crate::{config::Config, models::Invoice};

    fn sample_invoice() -> Invoice {
        Invoice {
            id: Uuid::new_v4(),
            public_id: "inv_demo".to_string(),
            merchant_id: Uuid::new_v4(),
            description: "Test invoice".to_string(),
            amount_cents: 1250,
            currency: "USD".to_string(),
            asset_code: "USDC".to_string(),
            asset_issuer: "ISSUER".to_string(),
            destination_public_key: "DESTINATION".to_string(),
            memo: "astro_deadbeef".to_string(),
            status: "pending".to_string(),
            gross_amount_cents: 1250,
            platform_fee_cents: 13,
            net_amount_cents: 1237,
            expires_at: Utc::now() + Duration::hours(2),
            paid_at: None,
            settled_at: None,
            transaction_hash: None,
            settlement_hash: None,
            checkout_url: None,
            qr_data_url: None,
            metadata: json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
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
        }
    }

    #[test]
    fn builds_checkout_url_from_public_id() {
        let config = sample_config();
        assert_eq!(
            build_checkout_url(&config, "inv_123"),
            "http://localhost:3000/pay/inv_123"
        );
    }

    #[test]
    fn converts_invoice_amount_to_stellar_precision() {
        let invoice = sample_invoice();
        assert_eq!(invoice_amount_to_asset(&invoice), "12.50");
    }

    #[test]
    fn detects_expired_invoice() {
        let mut invoice = sample_invoice();
        invoice.expires_at = Utc::now() - Duration::minutes(1);
        assert!(invoice_is_expired(&invoice, Utc::now()));
    }

    #[test]
    fn matches_horizon_payment_payload_to_invoice() {
        let invoice = sample_invoice();
        let record = json!({
            "to": "DESTINATION",
            "asset_code": "USDC",
            "asset_issuer": "ISSUER",
            "amount": "12.50"
        });
        assert!(payment_matches_invoice(&record, "astro_deadbeef", &invoice));
    }

    #[test]
    fn rejects_wrong_asset_or_memo() {
        let invoice = sample_invoice();
        let wrong_asset = json!({
            "to": "DESTINATION",
            "asset_code": "XLM",
            "asset_issuer": "ISSUER",
            "amount": "12.50"
        });
        let wrong_memo = json!({
            "to": "DESTINATION",
            "asset_code": "USDC",
            "asset_issuer": "ISSUER",
            "amount": "12.50"
        });
        assert!(!payment_matches_invoice(
            &wrong_asset,
            "astro_deadbeef",
            &invoice
        ));
        assert!(!payment_matches_invoice(
            &wrong_memo,
            "astro_other",
            &invoice
        ));
    }

    #[test]
    fn accepts_account_field_when_to_is_missing() {
        let invoice = sample_invoice();
        let record = json!({
            "account": "DESTINATION",
            "asset_code": "USDC",
            "asset_issuer": "ISSUER",
            "amount": "12.50"
        });
        assert!(payment_matches_invoice(&record, "astro_deadbeef", &invoice));
    }

    #[test]
    fn accepts_valid_ed25519_account_strkey() {
        assert!(is_valid_account_public_key(
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        ));
    }

    #[test]
    fn rejects_invalid_account_strkeys() {
        assert!(!is_valid_account_public_key(""));
        assert!(!is_valid_account_public_key("   "));
        assert!(!is_valid_account_public_key("not-a-key"));
        assert!(!is_valid_account_public_key(
            "MCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCAAAAAAM"
        ));
        assert!(!is_valid_account_public_key(
            "GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG"
        ));
    }
}
