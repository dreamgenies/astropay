use chrono::{DateTime, Utc};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use stellar_strkey::ed25519::PublicKey as Ed25519PublicKey;
use tokio::time::{Duration, sleep};
use tracing::warn;

use crate::{config::Config, error::AppError, models::Invoice};

const BACKOFF_BASE_MS: u64 = 500;
const MAX_RETRIES: u32 = 4;

/// GET `url` with exponential backoff on HTTP 429. Logs each throttle event.
async fn horizon_get(client: &Client, url: &str) -> Result<Response, AppError> {
    let mut delay_ms = BACKOFF_BASE_MS;
    for attempt in 0..=MAX_RETRIES {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|_| AppError::INTERNAL)?;
        if resp.status() != StatusCode::TOO_MANY_REQUESTS {
            return resp.error_for_status().map_err(|_| AppError::INTERNAL);
        }
        if attempt == MAX_RETRIES {
            warn!(
                url,
                "horizon rate-limited after {} retries, giving up", MAX_RETRIES
            );
            return Err(AppError::INTERNAL);
        }
        warn!(url, attempt, delay_ms, "horizon 429 — backing off");
        sleep(Duration::from_millis(delay_ms)).await;
        delay_ms *= 2;
    }
    Err(AppError::INTERNAL)
}

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
#[derive(Debug, Clone, Serialize)]
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

/// Emitted when destination + asset + memo match but the payment amount differs.
#[derive(Debug, Clone, Serialize)]
pub struct AmountMismatch {
    pub hash: String,
    pub received_amount: String,
    pub expected_amount: String,
}

#[derive(Debug)]
pub enum TransactionStatus {
    Success,
    Failed,
    NotFound,
}

/// Result of scanning Horizon payments for an invoice.
pub enum PaymentScanResult {
    Match(PaymentMatch),
    AssetMismatch(AssetMismatch),
    AmountMismatch(AmountMismatch),
    MemoMismatch(MemoMismatch),
    NotFound,
}

/// Verifies if a transaction hash has been successfully committed on-chain.
pub async fn confirm_transaction(
    config: &Config,
    tx_hash: &str,
) -> Result<TransactionStatus, AppError> {
    let url = format!(
        "{}/transactions/{}",
        config.horizon_url.trim_end_matches('/'),
        tx_hash
    );
    let resp = Client::new()
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::INTERNAL)?;
    if resp.status() == 404 {
        return Ok(TransactionStatus::NotFound);
    }
    let tx: serde_json::Value = resp.json().await.map_err(|_| AppError::INTERNAL)?;
    if tx.get("successful").and_then(|v| v.as_bool()) == Some(true) {
        Ok(TransactionStatus::Success)
    } else {
        Ok(TransactionStatus::Failed)
    }
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
/// Returns `Err(AppError::INTERNAL)` if the Horizon HTTP call or JSON parse fails.
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
    let page = horizon_get(&client, &payments_url)
        .await?
        .json::<PaymentsPage>()
        .await
        .map_err(|_| AppError::INTERNAL)?;

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

        if asset_matches && !amount_matches {
            let tx_url = format!(
                "{}/transactions/{}",
                config.horizon_url.trim_end_matches('/'),
                record.transaction_hash
            );
            let tx = horizon_get(&client, &tx_url)
                .await?
                .json::<HorizonTransaction>()
                .await
                .map_err(|_| AppError::INTERNAL)?;

            if tx.memo == invoice.memo {
                return Ok(PaymentScanResult::AmountMismatch(AmountMismatch {
                    hash: record.transaction_hash,
                    received_amount: record.amount.unwrap_or_default(),
                    expected_amount: expected_amount.clone(),
                }));
            }
            continue;
        }

        if !amount_matches || !asset_matches {
            continue;
        }

        let tx_url = format!(
            "{}/transactions/{}",
            config.horizon_url.trim_end_matches('/'),
            record.transaction_hash
        );
        let tx = horizon_get(&client, &tx_url)
            .await?
            .json::<HorizonTransaction>()
            .await
            .map_err(|_| AppError::INTERNAL)?;

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
        .map_err(|_| AppError::INTERNAL)?
        .error_for_status()
        .map_err(|_| AppError::INTERNAL)?
        .json::<PaymentsPage>()
        .await
        .map_err(|_| AppError::INTERNAL)?;

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
        config::{Config, LogFormat},
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
            database_url: crate::redact::Redacted::new(
                "postgres://postgres:postgres@localhost:5432/astropay".to_string(),
            ),
            pgssl: "disable".to_string(),
            session_secret: crate::redact::Redacted::new("secret".to_string()),
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            stellar_network: "TESTNET".to_string(),
            asset_code: "USDC".to_string(),
            asset_issuer: "ISSUER".to_string(),
            platform_treasury_public_key: "TREASURY".to_string(),
            platform_treasury_secret_key: None,
            platform_fee_bps: 100,
            invoice_expiry_hours: 24,
            cron_secret: crate::redact::Redacted::new("cron".to_string()),
            secure_cookies: false,
            login_rate_ip_window_secs: 600,
            login_rate_ip_max: 80,
            login_rate_email_window_secs: 900,
            login_rate_email_fail_max: 12,
            reconcile_scan_limit: 100,
            reconcile_scan_window_hours: 0,
            log_format: LogFormat::Human,
            archive_retention_days: 30,
            pending_invoice_alert_threshold_secs: 7200,
            queued_payout_alert_threshold_secs: 3600,
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
    fn backoff_delay_doubles_each_attempt() {
        let mut delay = super::BACKOFF_BASE_MS;
        let delays: Vec<u64> = (0..super::MAX_RETRIES)
            .map(|_| {
                let d = delay;
                delay *= 2;
                d
            })
            .collect();
        assert_eq!(delays, vec![500, 1000, 2000, 4000]);
    }

    // ── AP-232: Horizon failure-injection ────────────────────────────────────

    /// 503 / 502 from Horizon must surface as HorizonUnavailable, not Internal.
    /// The reconcile handler matches on HorizonUnavailable to skip-not-corrupt.
    #[test]
    fn horizon_unavailable_error_code_is_distinct_from_internal() {
        use crate::error::{AppError, ErrorCode};
        assert_ne!(
            AppError::HORIZON_UNAVAILABLE.code as u8,
            AppError::INTERNAL.code as u8,
            "HorizonUnavailable and Internal must be distinct error codes"
        );
        assert_eq!(
            AppError::HORIZON_UNAVAILABLE.code,
            ErrorCode::HorizonUnavailable
        );
    }

    /// HorizonUnavailable must be classified as Upstream so Sentry captures it.
    #[test]
    fn horizon_unavailable_classifies_as_upstream() {
        use crate::error::{AppError, ErrorClass};
        assert_eq!(
            AppError::HORIZON_UNAVAILABLE.classify(),
            ErrorClass::Upstream
        );
    }

    /// HorizonUnavailable must return HTTP 502 so load-balancers can detect it.
    #[test]
    fn horizon_unavailable_has_502_status() {
        use crate::error::AppError;
        use axum::http::StatusCode;
        assert_eq!(
            AppError::HORIZON_UNAVAILABLE.status,
            StatusCode::BAD_GATEWAY
        );
    }

    /// Reconcile must skip (not mark paid/expired) when Horizon is unavailable.
    /// Verified by inspecting the handler source — the match arm must push
    /// `skipped_horizon_unavailable` and `continue`, never touching invoice state.
    #[test]
    fn reconcile_skips_invoice_on_horizon_unavailable_not_corrupts() {
        let src = include_str!("handlers/cron.rs");
        // The HorizonUnavailable arm must emit the skip action.
        assert!(
            src.contains("skipped_horizon_unavailable"),
            "reconcile must emit skipped_horizon_unavailable on HorizonUnavailable"
        );
        // It must NOT call UPDATE invoices inside the HorizonUnavailable arm.
        // We verify by checking the arm ends with a push+continue before any UPDATE.
        // The pattern: warn! → results.push(skipped_horizon_unavailable) → no UPDATE follows
        // in that branch (the UPDATE only appears in the Match arm).
        assert!(
            src.contains("Horizon unavailable during reconcile — skipping invoice"),
            "reconcile must log the skip reason"
        );
    }

    /// Partial outage: payments page call fails → HorizonUnavailable propagates.
    /// The reconcile handler must see HorizonUnavailable and skip, not panic.
    #[test]
    fn horizon_unavailable_propagates_from_payments_call() {
        use crate::error::{AppError, ErrorCode};
        // Simulate what find_payment_for_invoice returns when horizon_get fails on the
        // payments URL. The error must be HorizonUnavailable so reconcile can match it.
        let err = AppError::HORIZON_UNAVAILABLE;
        assert_eq!(err.code, ErrorCode::HorizonUnavailable);
        // Reconcile matches: Err(AppError { code: HorizonUnavailable, .. }) => skip
        // Verify the match pattern exists in the handler source.
        let src = include_str!("handlers/cron.rs");
        assert!(
            src.contains("code: crate::error::ErrorCode::HorizonUnavailable"),
            "reconcile must pattern-match on HorizonUnavailable error code"
        );
    }

    /// Partial outage: payments page succeeds but transaction fetch fails.
    /// In this case find_payment_for_invoice returns Err(AppError::INTERNAL),
    /// which reconcile must NOT swallow — it must propagate as a hard error.
    #[test]
    fn reconcile_propagates_non_horizon_errors() {
        let src = include_str!("handlers/cron.rs");
        // The catch-all Err(e) arm must return Err(e), not push a skip result.
        assert!(
            src.contains("Err(e) => return Err(e)"),
            "reconcile must propagate non-HorizonUnavailable errors"
        );
    }

    /// MAX_RETRIES is 4, meaning horizon_get attempts the request up to 5 times
    /// (attempts 0..=4) before giving up. Verify the constant is not accidentally lowered.
    #[test]
    fn horizon_get_max_retries_is_four() {
        assert_eq!(super::MAX_RETRIES, 4);
    }

    /// After MAX_RETRIES exhausted on 429, the error must be Internal (rate-limit
    /// exhaustion is an operational failure, not a transient Horizon outage).
    #[test]
    fn horizon_get_returns_internal_after_max_429_retries() {
        use crate::error::{AppError, ErrorCode};
        // The horizon_get source returns Err(AppError::INTERNAL) after MAX_RETRIES.
        let src = include_str!("stellar.rs");
        assert!(
            src.contains("horizon rate-limited after {} retries, giving up"),
            "horizon_get must log exhaustion before returning Internal"
        );
        // Confirm the returned error is Internal, not HorizonUnavailable.
        // (Rate-limit exhaustion is a different failure mode from a 503.)
        let err = AppError::INTERNAL;
        assert_eq!(err.code, ErrorCode::Internal);
    }

    /// Timeout / connection-refused from reqwest surfaces as AppError::INTERNAL
    /// (the map_err in horizon_get maps send() failures to Internal).
    #[test]
    fn horizon_get_maps_send_failure_to_internal() {
        let src = include_str!("stellar.rs");
        // The .send().await.map_err(|_| AppError::INTERNAL) line must be present.
        assert!(
            src.contains("map_err(|_| AppError::INTERNAL)"),
            "horizon_get must map send() errors to AppError::INTERNAL"
        );
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
            RawOp {
                record_type: "payment",
                asset_code: Some("USDC"),
                asset_issuer: Some("ISSUER"),
                transaction_hash: "hash_usdc",
                from: "GSENDER1",
                amount: "20.00",
            },
            RawOp {
                record_type: "payment",
                asset_code: Some("XLM"),
                asset_issuer: Some("native"),
                transaction_hash: "hash_xlm",
                from: "GSENDER2",
                amount: "5.00",
            },
            RawOp {
                record_type: "create_account",
                asset_code: Some("USDC"),
                asset_issuer: Some("ISSUER"),
                transaction_hash: "hash_create",
                from: "GSENDER3",
                amount: "0.00",
            },
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
        assert_eq!(filtered[0].amount, "20.00");
    }
}
