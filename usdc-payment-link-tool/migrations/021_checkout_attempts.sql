-- Migration: 021_checkout_attempts.sql
--
-- Creates a dedicated `checkout_attempts` table so that checkout build and
-- submit actions can be tracked separately from generic payment_events.
--
-- Each row records one attempt by a buyer to initiate or complete a checkout
-- for an invoice. The `action` column distinguishes the two lifecycle steps:
--   'build'  — the payment transaction was constructed and presented to the wallet
--   'submit' — the signed transaction was submitted to the Stellar network
--
-- `status` captures the outcome of that action:
--   'initiated' — action started, no outcome yet
--   'succeeded' — action completed successfully
--   'failed'    — action completed with an error
--
-- `error_detail` is nullable and populated only on failure.
-- `payload` is JSONB for extensible per-attempt metadata (e.g. fee, sequence number).
-- No GIN index is added speculatively; add one when a real WHERE/ORDER BY pattern lands.

CREATE TABLE IF NOT EXISTS checkout_attempts (
  id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  invoice_id   UUID        NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
  action       TEXT        NOT NULL CHECK (action IN ('build', 'submit')),
  status       TEXT        NOT NULL CHECK (status IN ('initiated', 'succeeded', 'failed')),
  error_detail TEXT,
  payload      JSONB       NOT NULL DEFAULT '{}'::jsonb,
  attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Supports queries like "all attempts for invoice X ordered by time".
CREATE INDEX IF NOT EXISTS checkout_attempts_invoice_id_attempted_at_idx
  ON checkout_attempts (invoice_id, attempted_at DESC);

-- Supports queries like "all failed submits in the last hour" for alerting.
CREATE INDEX IF NOT EXISTS checkout_attempts_action_status_attempted_at_idx
  ON checkout_attempts (action, status, attempted_at DESC);
