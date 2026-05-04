/// Unit tests for payment payload matching edge cases.
///
/// Covers `payment_matches_invoice` and `invoice_amount_to_asset` across all
/// five matching criteria: destination, asset code, asset issuer, amount, and
/// memo. Also exercises the `build_settlement_memo` helper and the
/// `PaymentScanResult` mismatch variants.
///
/// No live database or Horizon connection is required.
use chrono::{Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use rust_backend::{
    horizon_fixtures::{
        ASSET_CODE, ASSET_ISSUER, BUYER_ACCOUNT, DESTINATION_ACCOUNT, INVOICE_AMOUNT, INVOICE_MEMO,
    },
    models::Invoice,
    stellar::{
        MemoMismatch, PaymentScanResult, SETTLEMENT_MEMO_MAX_BYTES, build_settlement_memo,
        invoice_amount_to_asset, payment_matches_invoice,
    },
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_invoice() -> Invoice {
    Invoice {
        id: Uuid::new_v4(),
        public_id: "inv_test01".to_string(),
        merchant_id: Uuid::new_v4(),
        description: "Edge-case test invoice".to_string(),
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

/// Build a minimal credit-payment JSON record.
fn credit_payment(
    to: Option<&str>,
    account: Option<&str>,
    asset_code: &str,
    asset_issuer: &str,
    amount: &str,
) -> serde_json::Value {
    let mut record = json!({
        "type": "payment",
        "asset_code": asset_code,
        "asset_issuer": asset_issuer,
        "amount": amount,
        "transaction_hash": "0000000000000000000000000000000000000000000000000000000000000099"
    });
    if let Some(to) = to {
        record["to"] = json!(to);
    }
    if let Some(account) = account {
        record["account"] = json!(account);
    }
    record
}

// ── Destination edge cases ────────────────────────────────────────────────────

#[test]
fn matches_when_destination_in_to_field() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn matches_when_destination_in_account_field_only() {
    let invoice = make_invoice();
    let record = credit_payment(
        None,
        Some(DESTINATION_ACCOUNT),
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn to_field_takes_precedence_over_account_field() {
    // `to` = correct destination, `account` = wrong account → should match.
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        Some(BUYER_ACCOUNT),
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_when_destination_is_wrong_account() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(BUYER_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_when_both_to_and_account_are_absent() {
    let invoice = make_invoice();
    let record = json!({
        "type": "payment",
        "asset_code": ASSET_CODE,
        "asset_issuer": ASSET_ISSUER,
        "amount": INVOICE_AMOUNT,
        "transaction_hash": "0".repeat(64)
    });
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_destination_with_extra_whitespace() {
    let invoice = make_invoice();
    let padded = format!(" {DESTINATION_ACCOUNT}");
    let record = credit_payment(
        Some(&padded),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

// ── Asset code edge cases ─────────────────────────────────────────────────────

#[test]
fn rejects_wrong_asset_code() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        "EURC",
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_lowercase_asset_code() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        "usdc",
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_empty_asset_code() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        "",
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_native_xlm_payment_missing_asset_fields() {
    let invoice = make_invoice();
    // Native payments have no asset_code / asset_issuer fields.
    let record = json!({
        "type": "payment",
        "to": DESTINATION_ACCOUNT,
        "asset_type": "native",
        "amount": INVOICE_AMOUNT,
        "transaction_hash": "0".repeat(64)
    });
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

// ── Asset issuer edge cases ───────────────────────────────────────────────────

#[test]
fn rejects_wrong_asset_issuer() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF", // different issuer
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_empty_asset_issuer() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        "",
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_correct_code_but_wrong_issuer() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGZS4BPQC4SPIN1ZF3QDLZ", // plausible but wrong
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

// ── Amount edge cases ─────────────────────────────────────────────────────────

#[test]
fn rejects_amount_off_by_one_cent() {
    let invoice = make_invoice();
    // 12.49 instead of 12.50
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        "12.49",
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_amount_overpayment() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        "12.51",
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_amount_without_decimal_places() {
    // "12" vs "12.50" — string comparison must be exact.
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        "12",
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_amount_with_extra_decimal_precision() {
    // "12.500" vs "12.50"
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        "12.500",
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_zero_amount() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        "0.00",
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn rejects_empty_amount_field() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        "",
    );
    assert!(!payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}

#[test]
fn invoice_amount_to_asset_formats_to_two_decimal_places() {
    let mut invoice = make_invoice();
    invoice.gross_amount_cents = 100;
    assert_eq!(invoice_amount_to_asset(&invoice), "1.00");

    invoice.gross_amount_cents = 1;
    assert_eq!(invoice_amount_to_asset(&invoice), "0.01");

    invoice.gross_amount_cents = 100_000;
    assert_eq!(invoice_amount_to_asset(&invoice), "1000.00");
}

#[test]
fn invoice_amount_to_asset_rounds_correctly_for_large_amounts() {
    let mut invoice = make_invoice();
    invoice.gross_amount_cents = 99_999_999;
    assert_eq!(invoice_amount_to_asset(&invoice), "999999.99");
}

// ── Memo edge cases ───────────────────────────────────────────────────────────

#[test]
fn rejects_empty_memo() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(&record, "", &invoice));
}

#[test]
fn rejects_memo_for_different_invoice() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(!payment_matches_invoice(
        &record,
        "astro_othermemo",
        &invoice
    ));
}

#[test]
fn rejects_memo_with_wrong_prefix() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    // Settlement memo prefix instead of buyer memo prefix.
    assert!(!payment_matches_invoice(&record, "s:inv_test01", &invoice));
}

#[test]
fn rejects_memo_with_extra_trailing_whitespace() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    let padded = format!("{INVOICE_MEMO} ");
    assert!(!payment_matches_invoice(&record, &padded, &invoice));
}

#[test]
fn rejects_memo_case_variation() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    let upper = INVOICE_MEMO.to_uppercase();
    assert!(!payment_matches_invoice(&record, &upper, &invoice));
}

// ── Settlement memo helpers ───────────────────────────────────────────────────

#[test]
fn settlement_memo_has_s_colon_prefix() {
    let memo = build_settlement_memo("inv_abc");
    assert!(memo.starts_with("s:"), "expected 's:' prefix, got: {memo}");
}

#[test]
fn settlement_memo_exact_for_short_id() {
    assert_eq!(build_settlement_memo("inv_abc"), "s:inv_abc");
}

#[test]
fn settlement_memo_truncates_at_28_bytes() {
    let long_id = "x".repeat(50);
    let memo = build_settlement_memo(&long_id);
    assert_eq!(
        memo.len(),
        SETTLEMENT_MEMO_MAX_BYTES,
        "memo must be exactly 28 bytes when id is long"
    );
}

#[test]
fn settlement_memo_does_not_truncate_short_id() {
    let memo = build_settlement_memo("short");
    assert_eq!(memo, "s:short");
    assert!(memo.len() < SETTLEMENT_MEMO_MAX_BYTES);
}

#[test]
fn settlement_memo_at_exact_boundary() {
    // "s:" is 2 chars; fill remaining 26 chars exactly.
    let id = "a".repeat(26);
    let memo = build_settlement_memo(&id);
    assert_eq!(memo.len(), SETTLEMENT_MEMO_MAX_BYTES);
    assert_eq!(memo, format!("s:{id}"));
}

#[test]
fn settlement_memo_is_deterministic() {
    let id = "inv_deterministic";
    assert_eq!(build_settlement_memo(id), build_settlement_memo(id));
}

#[test]
fn settlement_memo_differs_from_buyer_memo_prefix() {
    let memo = build_settlement_memo("inv_abc");
    assert!(
        !memo.starts_with("astro_"),
        "settlement memo must not use buyer prefix"
    );
}

// ── PaymentScanResult mismatch variants ──────────────────────────────────────

#[test]
fn memo_mismatch_variant_holds_correct_fields() {
    let mm = MemoMismatch {
        hash: "txhash01".to_string(),
        received_memo: "astro_wrong".to_string(),
        expected_memo: INVOICE_MEMO.to_string(),
    };
    assert_eq!(mm.hash, "txhash01");
    assert_eq!(mm.received_memo, "astro_wrong");
    assert_eq!(mm.expected_memo, INVOICE_MEMO);
}

#[test]
fn payment_scan_result_memo_mismatch_is_matchable() {
    let result = PaymentScanResult::MemoMismatch(MemoMismatch {
        hash: "h".to_string(),
        received_memo: "wrong".to_string(),
        expected_memo: "right".to_string(),
    });
    assert!(matches!(result, PaymentScanResult::MemoMismatch(_)));
}

#[test]
fn payment_scan_result_not_found_is_matchable() {
    let result = PaymentScanResult::NotFound;
    assert!(matches!(result, PaymentScanResult::NotFound));
}

// ── All-criteria combined ─────────────────────────────────────────────────────

#[test]
fn all_five_criteria_must_match_simultaneously() {
    let invoice = make_invoice();

    // Correct on 4 of 5 — each single-field deviation must reject.
    let cases: &[(&str, Option<&str>, Option<&str>, &str, &str, &str, &str)] = &[
        // (name, to, account, asset_code, asset_issuer, amount, memo)
        (
            "wrong_destination",
            Some(BUYER_ACCOUNT),
            None,
            ASSET_CODE,
            ASSET_ISSUER,
            INVOICE_AMOUNT,
            INVOICE_MEMO,
        ),
        (
            "wrong_asset_code",
            Some(DESTINATION_ACCOUNT),
            None,
            "EURC",
            ASSET_ISSUER,
            INVOICE_AMOUNT,
            INVOICE_MEMO,
        ),
        (
            "wrong_asset_issuer",
            Some(DESTINATION_ACCOUNT),
            None,
            ASSET_CODE,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            INVOICE_AMOUNT,
            INVOICE_MEMO,
        ),
        (
            "wrong_amount",
            Some(DESTINATION_ACCOUNT),
            None,
            ASSET_CODE,
            ASSET_ISSUER,
            "12.49",
            INVOICE_MEMO,
        ),
        (
            "wrong_memo",
            Some(DESTINATION_ACCOUNT),
            None,
            ASSET_CODE,
            ASSET_ISSUER,
            INVOICE_AMOUNT,
            "astro_other",
        ),
    ];

    for (name, to, account, asset_code, asset_issuer, amount, memo) in cases {
        let record = credit_payment(*to, *account, asset_code, asset_issuer, amount);
        assert!(
            !payment_matches_invoice(&record, memo, &invoice),
            "case '{name}' should not match but did"
        );
    }
}

#[test]
fn all_five_criteria_correct_produces_match() {
    let invoice = make_invoice();
    let record = credit_payment(
        Some(DESTINATION_ACCOUNT),
        None,
        ASSET_CODE,
        ASSET_ISSUER,
        INVOICE_AMOUNT,
    );
    assert!(payment_matches_invoice(&record, INVOICE_MEMO, &invoice));
}
