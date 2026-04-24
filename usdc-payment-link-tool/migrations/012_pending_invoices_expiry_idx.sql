-- Issue #191: Partial index for pending invoices by expiry.
-- Reconciliation queries that locate expiring or pending invoices scan only
-- the live pending rows, keeping the index small as invoices transition to
-- terminal states (paid, expired, settled, failed).
--
-- Covers the reconcile cron query:
--   SELECT * FROM invoices WHERE status = 'pending' ORDER BY expires_at ASC
-- and expiry-check queries:
--   SELECT * FROM invoices WHERE status = 'pending' AND expires_at <= NOW()

CREATE INDEX IF NOT EXISTS invoices_pending_expires_at_idx
  ON invoices (expires_at ASC)
  WHERE status = 'pending';
