-- Add tracking for payout retry attempts and most recent failure reason.
--
-- This migration ensures the payouts table can record:
-- 1. failure_count: number of failed attempts (added in 005_payout_dead_letter.sql)
-- 2. last_failure_at: timestamp of the most recent failure (added in 005_payout_dead_letter.sql)
-- 3. last_failure_reason: most recent failure error message (new in this migration)
--
-- These fields enable operators to debug settlement failures and understand
-- which payouts have failed repeatedly (before escalating to dead-letter).
--
-- Rollback: DROP COLUMN IF EXISTS last_failure_reason FROM payouts.
-- Note: This does NOT undo the additions from 005_payout_dead_letter.sql;
-- those columns (failure_count, last_failure_at) are independent and
-- remain even if this migration is rolled back.

-- Add column to track the most recent failure reason.
ALTER TABLE payouts
    ADD COLUMN IF NOT EXISTS last_failure_reason TEXT;

-- Add index to support queries filtering or sorting by failure presence and recency.
-- This allows quick discovery of payouts with recent failures for retry/debug workflows.
CREATE INDEX IF NOT EXISTS payouts_last_failure_at_idx ON payouts (last_failure_at DESC NULLS LAST);

-- Enforce data coherence: if last_failure_at is set, last_failure_reason should be present.
-- (PostgreSQL check constraints cannot enforce this conditionally, so this is a documentation
-- comment for application code and migrations: when updating last_failure_at, also set
-- last_failure_reason.)
