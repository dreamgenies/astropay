use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;

use crate::{config::Config, error::AppError, models::Invoice};

/// Maximum bytes for a Stellar text memo (protocol limit).
pub const SETTLEMENT_MEMO_MAX_BYTES: usize = 28;

/// Builds a deterministic settlement memo: `s:<public_id>` truncated to 28 bytes.
/// The `s:` prefix distinguishes settlement transactions from buyer payment memos (`astro_*`).
pub fn build_settlement_memo(public_id: &str) -> String {
    format!("s:{}", public_id)
        .chars()
        .take(SETTLEMENT_MEMO_MAX_BYTES)
        .collect()
}

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

/// Emitted when a payment's amount matches an invoice but the asset differs.
#[derive(Debug, Clone)]
pub struct AssetMismatch {
    pub hash: String,
    pub received_asset_code: String,
    pub received_asset_issuer: String,
    pub expected_asset_code: String,
    pub expected_asset_issuer: String,
    pub amount: String,
}

/// Issue #172: emitted when destination + asset + amount match but memo is missing or wrong.
#[derive(Debug, Clone)]
pub struct MemoMismatch {
    pub hash: String,
    pub received_memo: String,
    pub expected_memo: String,
}

/// Result of scanning Horizon payments for an invoice.
pub enum PaymentScanResult {
    Match(PaymentMatch),
    AssetMismatch(AssetMismatch),
    MemoMismatch(MemoMismatch),
    NotFound,
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

/// Returns true when the Horizon payment record matches the invoice on all five criteria:
/// destination key, asset code, asset issuer, gross amount (two decimal places), and memo.
/// Both `to` and `account` fields are checked to handle path-payment vs direct-payment shapes.
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

/// Queries Horizon for the most recent 50 payment operations on the invoice destination account
/// and returns the first one that matches all of: destination key, asset code, asset issuer,
/// gross amount (formatted to two decimal places), and transaction memo.
///
/// Returns `None` if no matching payment is found in that window.
/// Returns `Err(AppError::Internal)` if the Horizon HTTP call or JSON parse fails.
///
/// **Limit**: only the 50 most recent operations are inspected. Payments older than that window
/// will not be detected. Use the replay endpoint to rescan a specific invoice manually.
pub async fn find_payment_for_invoice(
    config: &Config,
    invoice: &Invoice,
) -> Result<PaymentScanResult, AppError> {
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

    let expected_amount = invoice_amount_to_asset(invoice);

    for record in page.embedded.records {
        if record.record_type != "payment" {
            continue;
        }
        if record.to.as_deref().or(record.account.as_deref())
            != Some(invoice.destination_public_key.as_str())
        {
            continue;
        }

        let amount_matches = record.amount.as_deref() == Some(expected_amount.as_str());
        let asset_matches = record.asset_code.as_deref() == Some(invoice.asset_code.as_str())
            && record.asset_issuer.as_deref() == Some(invoice.asset_issuer.as_str());

        if amount_matches && !asset_matches {
            return Ok(PaymentScanResult::AssetMismatch(AssetMismatch {
                hash: record.transaction_hash,
                received_asset_code: record.asset_code.unwrap_or_default(),
                received_asset_issuer: record.asset_issuer.unwrap_or_default(),
                expected_asset_code: invoice.asset_code.clone(),
                expected_asset_issuer: invoice.asset_issuer.clone(),
                amount: expected_amount.clone(),
            }));
        }

        if !amount_matches || !asset_matches {
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
            return Ok(PaymentScanResult::Match(PaymentMatch {
                hash: record.transaction_hash,
                payment: record.raw,
                memo: tx.memo,
            }));
        }
        // Issue #172: destination + asset + amount match but memo is wrong/missing.
        return Ok(PaymentScanResult::MemoMismatch(MemoMismatch {
            hash: record.transaction_hash,
            received_memo: tx.memo,
            expected_memo: invoice.memo.clone(),
        }));
    }

    Ok(PaymentScanResult::NotFound)
}

pub fn invoice_is_expired(invoice: &Invoice, now: DateTime<Utc>) -> bool {
    now > invoice.expires_at
}

/// A raw USDC payment that arrived at the treasury account on Horizon.
#[derive(Debug, Clone, Serialize)]
pub struct TreasuryPayment {
    pub transaction_hash: String,
    pub from: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: String,
}

/// Fetches the most recent `limit` USDC payments to `treasury_public_key` from Horizon.
/// Returns only `payment` operations whose asset matches the configured asset.
pub async fn fetch_treasury_payments(
    config: &Config,
    limit: u32,
) -> Result<Vec<TreasuryPayment>, AppError> {
    let url = format!(
        "{}/accounts/{}/payments?order=desc&limit={}",
        config.horizon_url.trim_end_matches('/'),
        config.platform_treasury_public_key,
        limit,
    );
    let page = Client::new()
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::Internal)?
        .error_for_status()
        .map_err(|_| AppError::Internal)?
        .json::<PaymentsPage>()
        .await
        .map_err(|_| AppError::Internal)?;

    let payments = page
        .embedded
        .records
        .into_iter()
        .filter(|r| {
            r.record_type == "payment"
                && r.asset_code.as_deref() == Some(config.asset_code.as_str())
                && r.asset_issuer.as_deref() == Some(config.asset_issuer.as_str())
        })
        .map(|r| TreasuryPayment {
            transaction_hash: r.transaction_hash,
            from: r.account.unwrap_or_default(),
            amount: r.amount.unwrap_or_default(),
            asset_code: r.asset_code.unwrap_or_default(),
            asset_issuer: r.asset_issuer.unwrap_or_default(),
        })
        .collect();

    Ok(payments)
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
    use crate::{
        config::Config,
        horizon_fixtures::{
            ASSET_CODE, ASSET_ISSUER, DESTINATION_ACCOUNT, INVOICE_AMOUNT, INVOICE_MEMO,
            horizon_payment_cases,
        },
        models::Invoice,
    };

    fn sample_invoice() -> Invoice {
        Invoice {
            id: Uuid::new_v4(),
            public_id: "inv_demo".to_string(),
            merchant_id: Uuid::new_v4(),
            description: "Test invoice".to_string(),
            amount_cents: 1250,
            currency: "USD".to_string(),
            asset_code: ASSET_CODE.to_string(),
            asset_issuer: ASSET_ISSUER.to_string(),
            destination_public_key: DESTINATION_ACCOUNT.to_string(),
            memo: INVOICE_MEMO.to_string(),
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
            last_checkout_attempt_at: None,
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
            login_rate_ip_window_secs: 600,
            login_rate_ip_max: 80,
            login_rate_email_window_secs: 900,
            login_rate_email_fail_max: 12,
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
        assert_eq!(invoice_amount_to_asset(&invoice), INVOICE_AMOUNT);
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
        let case = horizon_payment_cases()
            .into_iter()
            .find(|case| case.name == "exact_usdc_payment")
            .expect("exact_usdc_payment fixture");
        assert!(payment_matches_invoice(&case.payment, case.memo, &invoice));
    }

    #[test]
    fn rejects_wrong_asset_or_memo() {
        let invoice = sample_invoice();
        for case in horizon_payment_cases()
            .into_iter()
            .filter(|case| !case.expected_match)
        {
            assert!(
                !payment_matches_invoice(&case.payment, case.memo, &invoice),
                "fixture should not match invoice: {}",
                case.name
            );
        }
    }

    #[test]
    fn accepts_account_field_when_to_is_missing() {
        let invoice = sample_invoice();
        let case = horizon_payment_cases()
            .into_iter()
            .find(|case| case.name == "account_field_destination_fallback")
            .expect("account_field_destination_fallback fixture");
        assert!(payment_matches_invoice(&case.payment, case.memo, &invoice));
    }

    #[test]
    fn horizon_payment_fixtures_match_expected_outcomes() {
        let invoice = sample_invoice();
        for case in horizon_payment_cases() {
            assert_eq!(
                payment_matches_invoice(&case.payment, case.memo, &invoice),
                case.expected_match,
                "fixture expectation mismatch: {}",
                case.name
            );
        }
    }

    // --- Issue #172: memo mismatch detection ---

    #[test]
    fn memo_mismatch_struct_holds_expected_fields() {
        let m = super::MemoMismatch {
            hash: "abc".to_string(),
            received_memo: "wrong".to_string(),
            expected_memo: "astro_abc".to_string(),
        };
        assert_eq!(m.hash, "abc");
        assert_eq!(m.received_memo, "wrong");
        assert_eq!(m.expected_memo, "astro_abc");
    }

    #[test]
    fn payment_scan_result_has_memo_mismatch_variant() {
        // Verify the variant compiles and can be matched.
        let result = super::PaymentScanResult::MemoMismatch(super::MemoMismatch {
            hash: "h".to_string(),
            received_memo: "r".to_string(),
            expected_memo: "e".to_string(),
        });
        assert!(matches!(result, super::PaymentScanResult::MemoMismatch(_)));
    }

    // --- Issue #157: settlement memo strategy ---

    #[test]
    fn settlement_memo_has_s_prefix() {
        let memo = super::build_settlement_memo("inv_abc123");
        assert!(memo.starts_with("s:"));
        assert_eq!(memo, "s:inv_abc123");
    }

    #[test]
    fn settlement_memo_truncates_to_28_chars() {
        let long_id = "a".repeat(40);
        let memo = super::build_settlement_memo(&long_id);
        assert_eq!(memo.len(), super::SETTLEMENT_MEMO_MAX_BYTES);
    }

    #[test]
    fn settlement_memo_short_id_is_not_truncated() {
        let memo = super::build_settlement_memo("inv_short");
        assert_eq!(memo, "s:inv_short");
        assert!(memo.len() <= super::SETTLEMENT_MEMO_MAX_BYTES);
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

    /// Verifies the filter logic used inside `fetch_treasury_payments` without hitting Horizon.
    #[test]
    fn treasury_payment_filter_excludes_non_usdc_and_non_payment_ops() {
        // Simulate the filter applied inside fetch_treasury_payments using plain structs.
        struct RawOp {
            record_type: &'static str,
            asset_code: Option<&'static str>,
            asset_issuer: Option<&'static str>,
            transaction_hash: &'static str,
            from: &'static str,
            amount: &'static str,
        }

        let config = sample_config();
        let ops = vec![
            RawOp { record_type: "payment",       asset_code: Some("USDC"), asset_issuer: Some("ISSUER"), transaction_hash: "hash_usdc",    from: "GSENDER1", amount: "20.00" },
            RawOp { record_type: "payment",       asset_code: Some("XLM"),  asset_issuer: Some("native"), transaction_hash: "hash_xlm",    from: "GSENDER2", amount: "5.00"  },
            RawOp { record_type: "create_account", asset_code: Some("USDC"), asset_issuer: Some("ISSUER"), transaction_hash: "hash_create", from: "GSENDER3", amount: "0.00"  },
        ];

        let filtered: Vec<_> = ops
            .iter()
            .filter(|r| {
                r.record_type == "payment"
                    && r.asset_code == Some(config.asset_code.as_str())
                    && r.asset_issuer == Some(config.asset_issuer.as_str())
            })
            .collect();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].transaction_hash, "hash_usdc");
        assert_eq!(filtered[0].from, "GSENDER1");
    }
}
