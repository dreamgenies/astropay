-- Add constraint preventing empty business_name
-- Issue #220: Ensures merchants cannot persist blank business names
--
-- Rollback:
--   ALTER TABLE merchants DROP CONSTRAINT IF EXISTS merchants_business_name_not_empty;
--   No data is affected; the constraint is a guard only. Removing it allows
--   empty or whitespace-only business names to be written.

-- Add check constraint to prevent empty or whitespace-only business names
ALTER TABLE merchants 
    ADD CONSTRAINT IF NOT EXISTS merchants_business_name_not_empty 
    CHECK (LENGTH(TRIM(business_name)) > 0);