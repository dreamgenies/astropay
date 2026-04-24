-- Enforce webhook idempotency at the DB layer.
--
-- A transaction hash uniquely identifies a Stellar payment. Once an invoice is
-- marked paid with a given hash, any duplicate webhook or reconcile delivery
-- carrying the same hash must not produce a second state mutation.
--
-- The UNIQUE constraint makes the application-level guard race-safe: even if
-- two concurrent webhook deliveries pass the status == 'pending' check
-- simultaneously, only one INSERT/UPDATE will succeed; the other will receive
-- a unique-violation error that the handler maps to an idempotent 200 response.
--
-- NULL values are excluded from uniqueness (SQL standard), so invoices that
-- have not yet been paid (transaction_hash IS NULL) are unaffected.

CREATE UNIQUE INDEX IF NOT EXISTS invoices_transaction_hash_unique_idx
    ON invoices (transaction_hash)
    WHERE transaction_hash IS NOT NULL;
