-- AP-189: Enforce payment-before-settlement ordering on invoices.
--
-- settled_at must not precede paid_at. Both columns are nullable; the
-- constraint only fires when both are non-NULL, so pending/paid-only rows
-- are unaffected.
--
-- Rollback:
--   ALTER TABLE invoices DROP CONSTRAINT IF EXISTS invoices_settled_after_paid_check;
--   No data is affected; the constraint is a guard only. Removing it allows
--   settled_at < paid_at to be written, which would indicate a data integrity issue.

ALTER TABLE invoices
    ADD CONSTRAINT invoices_settled_after_paid_check
    CHECK (settled_at IS NULL OR paid_at IS NULL OR settled_at >= paid_at);
