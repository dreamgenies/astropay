-- AP-198: Normalize merchant email uniqueness using citext.
--
-- citext stores the value as-is but performs all comparisons case-insensitively,
-- so "Alice@Example.com" and "alice@example.com" are treated as the same address
-- at the database level without requiring application-side lowercasing.
--
-- Steps:
--   1. Enable the citext extension (idempotent).
--   2. Drop the existing TEXT unique constraint.
--   3. Re-type the column to citext.
--   4. Re-add the unique constraint (now case-insensitive by virtue of the type).
--
-- Rollback:
--   ALTER TABLE merchants DROP CONSTRAINT IF EXISTS merchants_email_key;
--   ALTER TABLE merchants ALTER COLUMN email TYPE TEXT;
--   ALTER TABLE merchants ADD CONSTRAINT merchants_email_key UNIQUE (email);
--   The citext extension can be left in place (it is harmless if unused).
--   WARNING: after rollback, email uniqueness becomes case-sensitive again.
--   Any merchants registered with case-variant emails during the citext window
--   will now appear as distinct accounts. Audit for duplicates before rolling back.

CREATE EXTENSION IF NOT EXISTS citext;

ALTER TABLE merchants
    DROP CONSTRAINT IF EXISTS merchants_email_key;

ALTER TABLE merchants
    ALTER COLUMN email TYPE citext;

ALTER TABLE merchants
    ADD CONSTRAINT merchants_email_key UNIQUE (email);
