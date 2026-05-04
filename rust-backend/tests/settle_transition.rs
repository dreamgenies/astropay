/// Integration tests: settle-to-invoice-settled transition
///
/// These tests exercise the full state-machine contract for payout settlement:
///
///   paid invoice + queued/submitted payout + tx_hash
///     → payout.status = 'settled'
///     → invoice.status = 'settled'
///     → payment_events row with event_type = 'merchant_settled'
///
/// No live database is required. The pure validation and mutation-descriptor
/// functions in `settle.rs` are the contract under test.
use rust_backend::settle::{
    InvoiceStatus, PayoutStatus, SETTLE_MUTATIONS, SettleError, validate_settle_transition,
};

// ── Happy path ───────────────────────────────────────────────────────────────

#[test]
fn paid_invoice_queued_payout_transitions_to_settled() {
    let result = validate_settle_transition("paid", "queued", "tx_abc123");
    assert!(
        result.is_ok(),
        "expected Ok for paid/queued/hash, got {result:?}"
    );
}

#[test]
fn paid_invoice_submitted_payout_transitions_to_settled() {
    // A payout that was submitted but not yet confirmed should still be
    // settleable (idempotent re-submission scenario).
    let result = validate_settle_transition("paid", "submitted", "tx_def456");
    assert!(result.is_ok());
}

#[test]
fn settle_produces_settled_status_on_both_records() {
    // The atomic DB write must land both records on 'settled'.
    assert_eq!(SETTLE_MUTATIONS.invoice_status, "settled");
    assert_eq!(SETTLE_MUTATIONS.payout_status, "settled");
}

#[test]
fn settle_emits_merchant_settled_payment_event() {
    assert_eq!(SETTLE_MUTATIONS.event_type, "merchant_settled");
}

// ── Invoice pre-condition guards ─────────────────────────────────────────────

#[test]
fn pending_invoice_blocks_settlement() {
    let err = validate_settle_transition("pending", "queued", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::InvoiceNotPaid {
            actual: "pending".to_string()
        }
    );
}

#[test]
fn already_settled_invoice_blocks_settlement() {
    let err = validate_settle_transition("settled", "queued", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::InvoiceNotPaid {
            actual: "settled".to_string()
        }
    );
}

#[test]
fn expired_invoice_blocks_settlement() {
    let err = validate_settle_transition("expired", "queued", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::InvoiceNotPaid {
            actual: "expired".to_string()
        }
    );
}

#[test]
fn failed_invoice_blocks_settlement() {
    let err = validate_settle_transition("failed", "queued", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::InvoiceNotPaid {
            actual: "failed".to_string()
        }
    );
}

// ── Payout pre-condition guards ──────────────────────────────────────────────

#[test]
fn already_settled_payout_is_rejected() {
    let err = validate_settle_transition("paid", "settled", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::PayoutAlreadyTerminal {
            actual: "settled".to_string()
        }
    );
}

#[test]
fn failed_payout_is_rejected() {
    // A failed payout must be re-queued explicitly before settlement can retry.
    let err = validate_settle_transition("paid", "failed", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::PayoutAlreadyTerminal {
            actual: "failed".to_string()
        }
    );
}

// ── tx_hash guard ────────────────────────────────────────────────────────────

#[test]
fn empty_tx_hash_blocks_settlement() {
    let err = validate_settle_transition("paid", "queued", "").unwrap_err();
    assert_eq!(err, SettleError::MissingTxHash);
}

#[test]
fn missing_hash_is_checked_before_invoice_status() {
    // Callers should get MissingTxHash even when the invoice is also wrong,
    // so the most actionable error surfaces first.
    let err = validate_settle_transition("pending", "queued", "").unwrap_err();
    assert_eq!(err, SettleError::MissingTxHash);
}

// ── Coherence invariants ─────────────────────────────────────────────────────

#[test]
fn payout_and_invoice_reach_identical_terminal_status() {
    // Dashboard queries join payouts ↔ invoices on status. Both must be
    // 'settled' after the atomic write — never one without the other.
    assert_eq!(
        SETTLE_MUTATIONS.payout_status, SETTLE_MUTATIONS.invoice_status,
        "payout and invoice must land on the same status string"
    );
}

#[test]
fn invoice_status_enum_covers_all_db_values() {
    for s in ["pending", "paid", "settled", "expired", "failed"] {
        assert!(
            InvoiceStatus::parse_status(s).is_some(),
            "InvoiceStatus missing variant for '{s}'"
        );
    }
}

#[test]
fn payout_status_enum_covers_all_db_values() {
    for s in ["queued", "submitted", "settled", "failed", "dead_lettered"] {
        assert!(
            PayoutStatus::parse_status(s).is_some(),
            "PayoutStatus missing variant for '{s}'"
        );
    }
}

#[test]
fn only_paid_invoice_status_allows_settlement() {
    let all_statuses = ["pending", "paid", "settled", "expired", "failed"];
    for status in all_statuses {
        let result = validate_settle_transition(status, "queued", "tx_abc");
        if status == "paid" {
            assert!(result.is_ok(), "expected Ok for 'paid', got {result:?}");
        } else {
            assert!(result.is_err(), "expected Err for '{status}', got Ok");
        }
    }
}

#[test]
fn only_non_terminal_payout_statuses_allow_settlement() {
    let terminal = ["settled", "failed", "dead_lettered"];
    let non_terminal = ["queued", "submitted"];

    for status in terminal {
        assert!(
            validate_settle_transition("paid", status, "tx_abc").is_err(),
            "expected Err for terminal payout status '{status}'"
        );
    }
    for status in non_terminal {
        assert!(
            validate_settle_transition("paid", status, "tx_abc").is_ok(),
            "expected Ok for non-terminal payout status '{status}'"
        );
    }
}

#[test]
fn dead_lettered_payout_is_rejected() {
    // A dead-lettered payout requires manual operator intervention before
    // settlement can be retried — it must never be auto-settled.
    let err = validate_settle_transition("paid", "dead_lettered", "tx_abc").unwrap_err();
    assert_eq!(
        err,
        SettleError::PayoutAlreadyTerminal {
            actual: "dead_lettered".to_string()
        }
    );
}
