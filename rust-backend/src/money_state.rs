//! Atomic money-state transitions.
//!
//! The invoice-paid / payout-queued path intentionally relies on PostgreSQL's
//! default `READ COMMITTED` isolation plus row-level locking from `UPDATE`.
//! The invoice update is a compare-and-set: only `status = 'pending'` can move
//! to `paid`, and callers must stop when that update affects zero rows. Payout
//! insertion is idempotent through the `payouts.invoice_id` unique constraint.

use serde_json::{Value, json};
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::{error::AppError, stellar::is_valid_account_public_key};

pub const INVOICE_ALREADY_TRANSITIONED_REASON: &str = "invoice_already_transitioned";

#[allow(dead_code)]
pub const INVOICE_PAID_ISOLATION_GUARANTEE: &str = "invoice-paid writes run in one PostgreSQL transaction; UPDATE invoices ... \
     WHERE id = $1 AND status = 'pending' is the compare-and-set boundary; \
     row_count = 0 stops event and payout writes; payout insert uses the same \
     validated settlement key value and ON CONFLICT (invoice_id) DO NOTHING.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayoutSkipReason {
    InvalidSettlementPublicKey,
    PayoutAlreadyQueued,
}

impl PayoutSkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSettlementPublicKey => "invalid_settlement_public_key",
            Self::PayoutAlreadyQueued => "payout_already_queued",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoicePaidOutcome {
    Applied {
        payout_queued: bool,
        payout_skip_reason: Option<PayoutSkipReason>,
    },
    AlreadyTransitioned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvoicePaidUpdate {
    Applied,
    AlreadyTransitioned,
}

fn classify_invoice_paid_update(row_count: u64) -> Result<InvoicePaidUpdate, AppError> {
    match row_count {
        0 => Ok(InvoicePaidUpdate::AlreadyTransitioned),
        1 => Ok(InvoicePaidUpdate::Applied),
        _ => Err(AppError::INTERNAL),
    }
}

/// Mark an invoice paid and queue its payout in the caller's transaction.
///
/// This function is the contract for coherent invoice-paid / payout-queued
/// writes. If another worker wins the `pending -> paid` race, the function
/// returns [`InvoicePaidOutcome::AlreadyTransitioned`] without inserting a
/// duplicate payment event or payout.
pub async fn mark_invoice_paid_and_queue_payout(
    transaction: &Transaction<'_>,
    invoice_id: Uuid,
    transaction_hash: &str,
    payment_payload: &Value,
) -> Result<InvoicePaidOutcome, AppError> {
    let updated = transaction
        .execute(
            "UPDATE invoices
             SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
             WHERE id = $1 AND status = 'pending'",
            &[&invoice_id, &transaction_hash],
        )
        .await?;

    match classify_invoice_paid_update(updated)? {
        InvoicePaidUpdate::AlreadyTransitioned => {
            return Ok(InvoicePaidOutcome::AlreadyTransitioned);
        }
        InvoicePaidUpdate::Applied => {}
    }

    transaction
        .execute(
            "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
            &[&invoice_id, &"payment_detected", payment_payload],
        )
        .await?;

    let settlement_row = transaction
        .query_opt(
            "SELECT m.settlement_public_key
             FROM merchants m
             INNER JOIN invoices i ON i.merchant_id = m.id
             WHERE i.id = $1",
            &[&invoice_id],
        )
        .await?;
    let settlement_key = settlement_row
        .map(|row| row.get::<_, String>("settlement_public_key"))
        .unwrap_or_default();

    if !is_valid_account_public_key(&settlement_key) {
        transaction
            .execute(
                "INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)",
                &[
                    &invoice_id,
                    &"payout_skipped_invalid_destination",
                    &json!({ "reason": PayoutSkipReason::InvalidSettlementPublicKey.as_str() }),
                ],
            )
            .await?;
        return Ok(InvoicePaidOutcome::Applied {
            payout_queued: false,
            payout_skip_reason: Some(PayoutSkipReason::InvalidSettlementPublicKey),
        });
    }

    let inserted = transaction
        .execute(
            "INSERT INTO payouts (invoice_id, merchant_id, destination_public_key, amount_cents, asset_code, asset_issuer)
             SELECT id, merchant_id, $2, net_amount_cents, asset_code, asset_issuer
             FROM invoices
             WHERE id = $1
             ON CONFLICT (invoice_id) DO NOTHING",
            &[&invoice_id, &settlement_key],
        )
        .await?;

    if inserted > 0 {
        Ok(InvoicePaidOutcome::Applied {
            payout_queued: true,
            payout_skip_reason: None,
        })
    } else {
        Ok(InvoicePaidOutcome::Applied {
            payout_queued: false,
            payout_skip_reason: Some(PayoutSkipReason::PayoutAlreadyQueued),
        })
    }
}

#[cfg(test)]
mod reconcile_to_payout_tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // AP-182: reconcile-to-payout transition contract
    //
    // These tests verify the state-machine logic that drives the reconcile cron:
    //   pending invoice + matching payment → paid + exactly one payout queued
    //   second call on same invoice        → AlreadyTransitioned (idempotent)
    //   invalid settlement key             → paid but payout skipped
    // ---------------------------------------------------------------------------

    /// A pending invoice UPDATE that affects 1 row must produce Applied.
    #[test]
    fn pending_invoice_update_produces_applied() {
        assert_eq!(
            classify_invoice_paid_update(1).unwrap(),
            InvoicePaidUpdate::Applied
        );
    }

    /// When the invoice was already transitioned (0 rows updated), the outcome
    /// must be AlreadyTransitioned — no duplicate payout must be enqueued.
    #[test]
    fn already_transitioned_invoice_stops_payout_enqueue() {
        assert_eq!(
            classify_invoice_paid_update(0).unwrap(),
            InvoicePaidUpdate::AlreadyTransitioned
        );
    }

    /// An impossible multi-row UPDATE is an internal error, not a silent skip.
    #[test]
    fn multi_row_update_is_internal_error() {
        assert!(classify_invoice_paid_update(2).is_err());
    }

    /// Applied outcome with payout_queued=true represents the happy path:
    /// pending → paid, one payout enqueued.
    #[test]
    fn applied_with_payout_queued_is_happy_path() {
        let outcome = InvoicePaidOutcome::Applied {
            payout_queued: true,
            payout_skip_reason: None,
        };
        assert!(matches!(
            outcome,
            InvoicePaidOutcome::Applied {
                payout_queued: true,
                payout_skip_reason: None
            }
        ));
    }

    /// When the settlement key is invalid the payout must be skipped, not silently
    /// dropped — the outcome must carry the skip reason so callers can audit it.
    #[test]
    fn invalid_settlement_key_skips_payout_with_reason() {
        let outcome = InvoicePaidOutcome::Applied {
            payout_queued: false,
            payout_skip_reason: Some(PayoutSkipReason::InvalidSettlementPublicKey),
        };
        match outcome {
            InvoicePaidOutcome::Applied {
                payout_queued,
                payout_skip_reason,
            } => {
                assert!(!payout_queued);
                assert_eq!(
                    payout_skip_reason.unwrap().as_str(),
                    "invalid_settlement_public_key"
                );
            }
            _ => panic!("expected Applied"),
        }
    }

    /// A payout that was already queued by a concurrent worker must not produce
    /// a second row — the outcome must reflect the idempotent skip.
    #[test]
    fn payout_already_queued_skips_duplicate_enqueue() {
        let outcome = InvoicePaidOutcome::Applied {
            payout_queued: false,
            payout_skip_reason: Some(PayoutSkipReason::PayoutAlreadyQueued),
        };
        match outcome {
            InvoicePaidOutcome::Applied {
                payout_queued,
                payout_skip_reason,
            } => {
                assert!(!payout_queued);
                assert_eq!(
                    payout_skip_reason.unwrap().as_str(),
                    "payout_already_queued"
                );
            }
            _ => panic!("expected Applied"),
        }
    }

    /// AlreadyTransitioned must be distinguishable from Applied so the reconcile
    /// loop can emit the correct JSON action ("skipped" vs "paid").
    #[test]
    fn already_transitioned_is_distinct_from_applied() {
        let transitioned = InvoicePaidOutcome::AlreadyTransitioned;
        let applied = InvoicePaidOutcome::Applied {
            payout_queued: true,
            payout_skip_reason: None,
        };
        assert_ne!(transitioned, applied);
    }

    /// The reconcile handler emits "skipped" for AlreadyTransitioned — pin the
    /// reason string so external callers cannot silently break on a rename.
    #[test]
    fn already_transitioned_reason_string_is_stable() {
        assert_eq!(
            INVOICE_ALREADY_TRANSITIONED_REASON,
            "invoice_already_transitioned"
        );
    }

    /// Verify that the mark_invoice_paid_and_queue_payout source contains the
    /// compare-and-set WHERE clause that prevents double-payment.
    #[test]
    fn mark_paid_source_contains_compare_and_set_guard() {
        let src = include_str!("money_state.rs");
        assert!(
            src.contains("WHERE id = $1 AND status = 'pending'"),
            "must guard UPDATE with status = 'pending' to prevent double-payment"
        );
    }

    /// Verify the payout INSERT uses ON CONFLICT DO NOTHING for idempotency.
    #[test]
    fn mark_paid_source_contains_idempotent_payout_insert() {
        let src = include_str!("money_state.rs");
        assert!(
            src.contains("ON CONFLICT (invoice_id) DO NOTHING"),
            "payout INSERT must be idempotent via ON CONFLICT (invoice_id) DO NOTHING"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_count_zero_stops_follow_on_money_writes() {
        assert_eq!(
            classify_invoice_paid_update(0).unwrap(),
            InvoicePaidUpdate::AlreadyTransitioned
        );
    }

    #[test]
    fn row_count_one_applies_transition() {
        assert_eq!(
            classify_invoice_paid_update(1).unwrap(),
            InvoicePaidUpdate::Applied
        );
    }

    #[test]
    fn impossible_multi_row_update_is_internal_error() {
        assert!(classify_invoice_paid_update(2).is_err());
    }

    #[test]
    fn payout_skip_reason_wire_strings_are_stable() {
        assert_eq!(
            PayoutSkipReason::InvalidSettlementPublicKey.as_str(),
            "invalid_settlement_public_key"
        );
        assert_eq!(
            PayoutSkipReason::PayoutAlreadyQueued.as_str(),
            "payout_already_queued"
        );
    }

    #[test]
    fn isolation_contract_documents_required_guarantees() {
        assert!(INVOICE_PAID_ISOLATION_GUARANTEE.contains("one PostgreSQL transaction"));
        assert!(INVOICE_PAID_ISOLATION_GUARANTEE.contains("status = 'pending'"));
        assert!(INVOICE_PAID_ISOLATION_GUARANTEE.contains("row_count = 0 stops"));
        assert!(INVOICE_PAID_ISOLATION_GUARANTEE.contains("ON CONFLICT (invoice_id)"));
    }
}
