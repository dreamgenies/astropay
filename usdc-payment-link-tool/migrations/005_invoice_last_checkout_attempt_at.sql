-- Migration: 005_invoice_last_checkout_attempt_at.sql
--
-- Adds last_checkout_attempt_at to invoices so the checkout UI and
-- backend can record when a buyer last loaded the payment page.
--
-- The column is nullable: NULL means no checkout attempt has been
-- observed yet (invoices created before this migration, or invoices
-- that were never visited). Application code must treat NULL and a
-- timestamp in the past identically — both mean "not recently active".
--
-- No index is created here. The column is not used in WHERE filters
-- today. Add an index in a follow-up migration once a real query
-- pattern (e.g. "invoices with a checkout attempt in the last N
-- minutes") lands in application code.

ALTER TABLE invoices
  ADD COLUMN last_checkout_attempt_at TIMESTAMPTZ;

COMMENT ON COLUMN invoices.last_checkout_attempt_at IS
  'Timestamp of the most recent checkout page load for this invoice. '
  'NULL means no attempt has been recorded. Updated by the checkout '
  'handler on each visit; not set by the reconcile or settle cron jobs.';
