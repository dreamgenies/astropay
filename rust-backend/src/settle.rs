/// Pure domain types and validation for the payout → settled transition.
///
/// The DB-coupled execution lives in the cron handler. This module holds the
/// invariant checks so they can be exercised in tests without a live Postgres
/// connection.
///
/// # Retry backoff policy
///
/// Failed payouts are not immediately requeued. Each failure increments
/// `failure_count` and sets `last_failure_at`. A payout is only eligible for
/// retry once the backoff window for its current failure count has elapsed:
///
/// | failure_count | backoff window |
/// |---------------|----------------|
/// | 1             | 5 minutes      |
/// | 2             | 15 minutes     |
/// | 3             | 1 hour         |
/// | 4             | 4 hours        |
/// | ≥ 5           | dead-lettered  |
///
/// Use [`backoff_seconds`] to compute the required delay for a given count.

#[derive(Debug, PartialEq, Clone)]
pub enum InvoiceStatus {
    Pending,
    Paid,
    Settled,
    Expired,
    Failed,
}

impl InvoiceStatus {
    pub fn parse_status(s: &str) -> Option<Self> {
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
    pub fn parse_status(s: &str) -> Option<Self> {
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

    match InvoiceStatus::parse_status(invoice_status) {
        Some(InvoiceStatus::Paid) => {}
        _ => {
            return Err(SettleError::InvoiceNotPaid {
                actual: invoice_status.to_string(),
            });
        }
    }

    match PayoutStatus::parse_status(payout_status) {
        Some(PayoutStatus::Settled)
        | Some(PayoutStatus::Failed)
        | Some(PayoutStatus::DeadLettered) => {
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

// ── Retry backoff ─────────────────────────────────────────────────────────────

/// Returns the required backoff delay in seconds before a payout with the given
/// `failure_count` is eligible for retry.
///
/// `failure_count` is the value *after* the most recent failure has been
/// recorded (i.e. the count that will be stored in the DB row).
///
/// Returns `None` when the count has reached or exceeded the dead-letter
/// threshold — callers should escalate rather than schedule a retry.
pub fn backoff_seconds(failure_count: i32) -> Option<i64> {
    match failure_count {
        1 => Some(5 * 60),      // 5 minutes
        2 => Some(15 * 60),     // 15 minutes
        3 => Some(60 * 60),     // 1 hour
        4 => Some(4 * 60 * 60), // 4 hours
        _ => None,              // dead-letter threshold reached
    }
}

/// Returns `true` when enough time has elapsed since `last_failure_at` for the
/// payout to be retried, given its current `failure_count`.
///
/// `now_secs` and `last_failure_secs` are Unix timestamps (seconds).
/// Returns `false` if the payout should be dead-lettered (no backoff window).
pub fn is_backoff_elapsed(failure_count: i32, last_failure_secs: i64, now_secs: i64) -> bool {
    match backoff_seconds(failure_count) {
        Some(delay) => now_secs >= last_failure_secs + delay,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── InvoiceStatus ────────────────────────────────────────────────────────

    #[test]
    fn invoice_status_round_trips_all_variants() {
        for s in ["pending", "paid", "settled", "expired", "failed"] {
            let status = InvoiceStatus::parse_status(s).unwrap();
            assert_eq!(status.as_str(), s);
        }
    }

    #[test]
    fn invoice_status_rejects_unknown_string() {
        assert!(InvoiceStatus::parse_status("processing").is_none());
        assert!(InvoiceStatus::parse_status("").is_none());
    }

    // ── PayoutStatus ─────────────────────────────────────────────────────────

    #[test]
    fn payout_status_round_trips_all_variants() {
        for s in ["queued", "submitted", "settled", "failed", "dead_lettered"] {
            assert!(PayoutStatus::parse_status(s).is_some());
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

    // ── backoff_seconds ──────────────────────────────────────────────────────

    #[test]
    fn backoff_seconds_returns_expected_delays() {
        assert_eq!(backoff_seconds(1), Some(5 * 60));
        assert_eq!(backoff_seconds(2), Some(15 * 60));
        assert_eq!(backoff_seconds(3), Some(60 * 60));
        assert_eq!(backoff_seconds(4), Some(4 * 60 * 60));
    }

    #[test]
    fn backoff_seconds_returns_none_at_dead_letter_threshold() {
        assert_eq!(backoff_seconds(5), None);
        assert_eq!(backoff_seconds(6), None);
        assert_eq!(backoff_seconds(100), None);
    }

    #[test]
    fn backoff_seconds_returns_none_for_zero() {
        // failure_count=0 means no failure has been recorded yet; treat as dead-letter guard.
        assert_eq!(backoff_seconds(0), None);
    }

    // ── is_backoff_elapsed ───────────────────────────────────────────────────

    #[test]
    fn backoff_not_elapsed_immediately_after_failure() {
        let last_failure = 1_000_000_i64;
        let now = last_failure + 1; // 1 second later — well within any window
        assert!(!is_backoff_elapsed(1, last_failure, now));
    }

    #[test]
    fn backoff_elapsed_after_full_window_passes() {
        let last_failure = 1_000_000_i64;
        let delay = backoff_seconds(1).unwrap();
        let now = last_failure + delay;
        assert!(is_backoff_elapsed(1, last_failure, now));
    }

    #[test]
    fn backoff_not_elapsed_one_second_before_window() {
        let last_failure = 1_000_000_i64;
        let delay = backoff_seconds(2).unwrap();
        let now = last_failure + delay - 1;
        assert!(!is_backoff_elapsed(2, last_failure, now));
    }

    #[test]
    fn backoff_elapsed_exactly_at_window_boundary() {
        let last_failure = 1_000_000_i64;
        let delay = backoff_seconds(3).unwrap();
        let now = last_failure + delay;
        assert!(is_backoff_elapsed(3, last_failure, now));
    }

    #[test]
    fn backoff_returns_false_for_dead_letter_count() {
        // failure_count >= 5 has no retry window; is_backoff_elapsed must return false.
        let last_failure = 0_i64;
        let now = i64::MAX;
        assert!(!is_backoff_elapsed(5, last_failure, now));
    }

    #[test]
    fn backoff_windows_are_strictly_increasing() {
        let windows: Vec<i64> = (1..=4).map(|c| backoff_seconds(c).unwrap()).collect();
        for pair in windows.windows(2) {
            assert!(
                pair[1] > pair[0],
                "backoff windows must be strictly increasing"
            );
        }
    }
}
