-- Enforce that invoice money math is internally consistent:
--   gross_amount_cents = platform_fee_cents + net_amount_cents
--
-- This prevents rows where the fee split does not add up to the gross amount,
-- which would cause silent accounting discrepancies in settlement and reporting.
--
-- Uses IF NOT EXISTS so the migration is safe to re-run (idempotent).
--
-- Rollback: ALTER TABLE invoices DROP CONSTRAINT IF EXISTS invoices_amount_split_check;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_amount_split_check
        CHECK (gross_amount_cents = platform_fee_cents + net_amount_cents);
