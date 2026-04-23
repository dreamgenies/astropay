/// Pure domain types and validation for the payout → settled transition.
///
/// The DB-coupled execution lives in the cron handler. This module holds the
/// invariant checks so they can be exercised in tests without a live Postgres
/// connection.

#[derive(Debug, PartialEq, Clone)]
pub enum InvoiceStatus {
    Pending,
    Paid,
    Settled,
    Expired,
    Failed,
}

impl InvoiceStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "paid" => Some(Self::Paid),
            "settled" => Some(Self::Settled),
            "expired" => Some(Self::Expired),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Paid => "paid",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum PayoutStatus {
    Queued,
    Submitted,
    Settled,
    Failed,
    DeadLettered,
}

impl PayoutStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "submitted" => Some(Self::Submitted),
            "settled" => Some(Self::Settled),
            "failed" => Some(Self::Failed),
            "dead_lettered" => Some(Self::DeadLettered),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SettleError {
    /// Invoice is not in `paid` state — settlement must not proceed.
    InvoiceNotPaid { actual: String },
    /// Payout is already settled or failed — idempotency guard.
    PayoutAlreadyTerminal { actual: String },
    /// tx_hash is empty — Stellar submission must have returned a hash.
    MissingTxHash,
}

/// Validate that a settle transition is legal before touching the DB.
///
/// Returns `Ok(())` when both records are in the correct pre-transition state
/// and a non-empty tx_hash is present. Returns `Err(SettleError)` otherwise.
pub fn validate_settle_transition(
    invoice_status: &str,
    payout_status: &str,
    tx_hash: &str,
) -> Result<(), SettleError> {
    if tx_hash.is_empty() {
        return Err(SettleError::MissingTxHash);
    }

    match InvoiceStatus::from_str(invoice_status) {
        Some(InvoiceStatus::Paid) => {}
        _ => {
            return Err(SettleError::InvoiceNotPaid {
                actual: invoice_status.to_string(),
            });
        }
    }

    match PayoutStatus::from_str(payout_status) {
        Some(PayoutStatus::Settled) | Some(PayoutStatus::Failed) | Some(PayoutStatus::DeadLettered) => {
            return Err(SettleError::PayoutAlreadyTerminal {
                actual: payout_status.to_string(),
            });
        }
        _ => {}
    }

    Ok(())
}

/// Describe the DB writes that must happen atomically for a settle transition.
///
/// This is the authoritative list of mutations. The cron handler executes them;
/// tests assert that all three are present and coherent.
#[derive(Debug, PartialEq)]
pub struct SettleMutations {
    pub payout_status: &'static str,
    pub invoice_status: &'static str,
    pub event_type: &'static str,
}

pub const SETTLE_MUTATIONS: SettleMutations = SettleMutations {
    payout_status: "settled",
    invoice_status: "settled",
    event_type: "merchant_settled",
};

#[cfg(test)]
mod tests {
    use super::*;

    // ── InvoiceStatus ────────────────────────────────────────────────────────

    #[test]
    fn invoice_status_round_trips_all_variants() {
        for s in ["pending", "paid", "settled", "expired", "failed"] {
            let status = InvoiceStatus::from_str(s).unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn invoice_status_rejects_unknown_string() {
        assert!(InvoiceStatus::from_str("processing").is_none());
        assert!(InvoiceStatus::from_str("").is_none());
    }

    // ── PayoutStatus ─────────────────────────────────────────────────────────

    #[test]
    fn payout_status_round_trips_all_variants() {
        for s in ["queued", "submitted", "settled", "failed", "dead_lettered"] {
            assert!(PayoutStatus::from_str(s).is_some());
        }
    }

    #[test]
    fn rejects_dead_lettered_payout() {
        assert_eq!(
            validate_settle_transition("paid", "dead_lettered", "abc123"),
            Err(SettleError::PayoutAlreadyTerminal {
                actual: "dead_lettered".to_string()
            })
        );
    }

    // ── validate_settle_transition ───────────────────────────────────────────

    #[test]
    fn accepts_paid_invoice_with_queued_payout_and_hash() {
        assert!(validate_settle_transition("paid", "queued", "abc123").is_ok());
    }

    #[test]
    fn accepts_paid_invoice_with_submitted_payout() {
        assert!(validate_settle_transition("paid", "submitted", "abc123").is_ok());
    }

    #[test]
    fn rejects_pending_invoice() {
        assert_eq!(
            validate_settle_transition("pending", "queued", "abc123"),
            Err(SettleError::InvoiceNotPaid {
                actual: "pending".to_string()
            })
        );
    }

    #[test]
    fn rejects_already_settled_invoice() {
        assert_eq!(
            validate_settle_transition("settled", "queued", "abc123"),
            Err(SettleError::InvoiceNotPaid {
                actual: "settled".to_string()
            })
        );
    }

    #[test]
    fn rejects_expired_invoice() {
        assert_eq!(
            validate_settle_transition("expired", "queued", "abc123"),
            Err(SettleError::InvoiceNotPaid {
                actual: "expired".to_string()
            })
        );
    }

    #[test]
    fn rejects_already_settled_payout() {
        assert_eq!(
            validate_settle_transition("paid", "settled", "abc123"),
            Err(SettleError::PayoutAlreadyTerminal {
                actual: "settled".to_string()
            })
        );
    }

    #[test]
    fn rejects_failed_payout() {
        assert_eq!(
            validate_settle_transition("paid", "failed", "abc123"),
            Err(SettleError::PayoutAlreadyTerminal {
                actual: "failed".to_string()
            })
        );
    }

    #[test]
    fn rejects_empty_tx_hash() {
        assert_eq!(
            validate_settle_transition("paid", "queued", ""),
            Err(SettleError::MissingTxHash)
        );
    }

    #[test]
    fn empty_tx_hash_is_checked_before_invoice_status() {
        // MissingTxHash takes priority so callers get the most actionable error.
        assert_eq!(
            validate_settle_transition("pending", "queued", ""),
            Err(SettleError::MissingTxHash)
        );
    }

    // ── SETTLE_MUTATIONS coherence ───────────────────────────────────────────

    #[test]
    fn settle_mutations_target_settled_status_on_both_records() {
        assert_eq!(SETTLE_MUTATIONS.payout_status, "settled");
        assert_eq!(SETTLE_MUTATIONS.invoice_status, "settled");
    }

    #[test]
    fn settle_mutations_emit_merchant_settled_event() {
        assert_eq!(SETTLE_MUTATIONS.event_type, "merchant_settled");
    }

    #[test]
    fn settle_mutations_payout_and_invoice_reach_same_terminal_state() {
        // Both records must land on the same status string so dashboard queries
        // that join payouts ↔ invoices on status remain coherent.
        assert_eq!(
            SETTLE_MUTATIONS.payout_status,
            SETTLE_MUTATIONS.invoice_status
        );
    }
}
