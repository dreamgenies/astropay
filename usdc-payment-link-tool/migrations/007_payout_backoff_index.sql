-- Supports the backoff-aware payout query in queuedPayouts / settle cron.
-- The query filters on (status, last_failure_at) for failed payouts, so a
-- partial index on failed rows avoids a full table scan on every settle run.
--
-- Rollback:
--   DROP INDEX IF EXISTS payouts_failed_backoff_idx;
--   The settle cron will fall back to a full table scan for failed payouts.
--   No data is affected.

CREATE INDEX IF NOT EXISTS payouts_failed_backoff_idx
    ON payouts (last_failure_at ASC)
    WHERE status = 'failed';
