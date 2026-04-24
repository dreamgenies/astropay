use serde_json::{Value, json};

pub const DESTINATION_ACCOUNT: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
pub const BUYER_ACCOUNT: &str = "GBBD47IF6A3JQRYKRQJ3235GHKJ4GQV4QJV6T4QNVWJ6K4H2L6LJ5B6Q";
pub const ASSET_CODE: &str = "USDC";
pub const ASSET_ISSUER: &str = "GBBD47IF6A3JQRYKRQJ3235GHKJ4GQV4QJV6T4QNVWJ6K4H2L6LJ5B6Q";
pub const INVOICE_MEMO: &str = "astro_deadbeef";
pub const INVOICE_AMOUNT: &str = "12.50";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizonPaymentCaseKind {
    Good,
    Bad,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct HorizonPaymentCase {
    pub name: &'static str,
    pub kind: HorizonPaymentCaseKind,
    pub payment: Value,
    pub memo: &'static str,
    pub expected_match: bool,
}

pub fn horizon_payment_cases() -> Vec<HorizonPaymentCase> {
    vec![
        HorizonPaymentCase {
            name: "exact_usdc_payment",
            kind: HorizonPaymentCaseKind::Good,
            payment: credit_payment(
                "0000000000000000000000000000000000000000000000000000000000000001",
                Some(DESTINATION_ACCOUNT),
                None,
                ASSET_CODE,
                ASSET_ISSUER,
                INVOICE_AMOUNT,
            ),
            memo: INVOICE_MEMO,
            expected_match: true,
        },
        HorizonPaymentCase {
            name: "account_field_destination_fallback",
            kind: HorizonPaymentCaseKind::Good,
            payment: credit_payment(
                "0000000000000000000000000000000000000000000000000000000000000002",
                None,
                Some(DESTINATION_ACCOUNT),
                ASSET_CODE,
                ASSET_ISSUER,
                INVOICE_AMOUNT,
            ),
            memo: INVOICE_MEMO,
            expected_match: true,
        },
        HorizonPaymentCase {
            name: "wrong_asset_code",
            kind: HorizonPaymentCaseKind::Bad,
            payment: credit_payment(
                "0000000000000000000000000000000000000000000000000000000000000003",
                Some(DESTINATION_ACCOUNT),
                None,
                "EURC",
                ASSET_ISSUER,
                INVOICE_AMOUNT,
            ),
            memo: INVOICE_MEMO,
            expected_match: false,
        },
        HorizonPaymentCase {
            name: "wrong_destination",
            kind: HorizonPaymentCaseKind::Bad,
            payment: credit_payment(
                "0000000000000000000000000000000000000000000000000000000000000004",
                Some(BUYER_ACCOUNT),
                None,
                ASSET_CODE,
                ASSET_ISSUER,
                INVOICE_AMOUNT,
            ),
            memo: INVOICE_MEMO,
            expected_match: false,
        },
        HorizonPaymentCase {
            name: "wrong_amount",
            kind: HorizonPaymentCaseKind::Bad,
            payment: credit_payment(
                "0000000000000000000000000000000000000000000000000000000000000005",
                Some(DESTINATION_ACCOUNT),
                None,
                ASSET_CODE,
                ASSET_ISSUER,
                "12.49",
            ),
            memo: INVOICE_MEMO,
            expected_match: false,
        },
        HorizonPaymentCase {
            name: "native_asset_payment",
            kind: HorizonPaymentCaseKind::Bad,
            payment: native_payment(
                "0000000000000000000000000000000000000000000000000000000000000006",
                DESTINATION_ACCOUNT,
                INVOICE_AMOUNT,
            ),
            memo: INVOICE_MEMO,
            expected_match: false,
        },
        HorizonPaymentCase {
            name: "memo_disambiguates_other_invoice",
            kind: HorizonPaymentCaseKind::Ambiguous,
            payment: credit_payment(
                "0000000000000000000000000000000000000000000000000000000000000007",
                Some(DESTINATION_ACCOUNT),
                None,
                ASSET_CODE,
                ASSET_ISSUER,
                INVOICE_AMOUNT,
            ),
            memo: "astro_othermemo",
            expected_match: false,
        },
    ]
}

fn credit_payment(
    transaction_hash: &'static str,
    to: Option<&'static str>,
    account: Option<&'static str>,
    asset_code: &'static str,
    asset_issuer: &'static str,
    amount: &'static str,
) -> Value {
    let mut payment = json!({
        "id": format!("{transaction_hash}-0"),
        "paging_token": format!("{transaction_hash}-0"),
        "transaction_hash": transaction_hash,
        "source_account": BUYER_ACCOUNT,
        "from": BUYER_ACCOUNT,
        "type": "payment",
        "type_i": 1,
        "created_at": "2026-04-24T12:00:00Z",
        "asset_type": "credit_alphanum4",
        "asset_code": asset_code,
        "asset_issuer": asset_issuer,
        "amount": amount,
        "transaction_successful": true
    });
    if let Some(to) = to {
        payment["to"] = json!(to);
    }
    if let Some(account) = account {
        payment["account"] = json!(account);
    }
    payment
}

fn native_payment(transaction_hash: &'static str, to: &'static str, amount: &'static str) -> Value {
    json!({
        "id": format!("{transaction_hash}-0"),
        "paging_token": format!("{transaction_hash}-0"),
        "transaction_hash": transaction_hash,
        "source_account": BUYER_ACCOUNT,
        "from": BUYER_ACCOUNT,
        "to": to,
        "type": "payment",
        "type_i": 1,
        "created_at": "2026-04-24T12:00:00Z",
        "asset_type": "native",
        "amount": amount,
        "transaction_successful": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_library_covers_required_case_kinds() {
        let cases = horizon_payment_cases();
        assert!(
            cases
                .iter()
                .any(|case| case.kind == HorizonPaymentCaseKind::Good)
        );
        assert!(
            cases
                .iter()
                .any(|case| case.kind == HorizonPaymentCaseKind::Bad)
        );
        assert!(
            cases
                .iter()
                .any(|case| case.kind == HorizonPaymentCaseKind::Ambiguous)
        );
    }

    #[test]
    fn synthetic_payloads_do_not_use_live_transaction_hashes() {
        for case in horizon_payment_cases() {
            let hash = case
                .payment
                .get("transaction_hash")
                .and_then(|value| value.as_str())
                .expect("fixture transaction_hash");
            assert!(hash.chars().all(|ch| ch == '0' || ch.is_ascii_digit()));
            assert_eq!(hash.len(), 64);
        }
    }
}
