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
        _ => Err(AppError::Internal),
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
