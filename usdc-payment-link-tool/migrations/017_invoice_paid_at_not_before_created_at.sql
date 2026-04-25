-- AP-188: Prevent time-travel invoice data by blocking paid_at < created_at.
--
-- paid_at must not precede the invoice's own creation timestamp. The column is
-- nullable; the constraint only fires when paid_at is non-NULL, so pending
-- invoices are unaffected.
--
-- Rollback: ALTER TABLE invoices DROP CONSTRAINT IF EXISTS invoices_paid_at_after_created_at_check;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_paid_at_after_created_at_check
    CHECK (paid_at IS NULL OR paid_at >= created_at);
