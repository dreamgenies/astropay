-- Partial index for settlement scans over queued payouts.
--
-- The hot query in the settle cron path is:
--   SELECT * FROM payouts WHERE status = 'queued' ORDER BY created_at ASC LIMIT 100
--
-- The existing payouts_status_idx (001_init.sql) is a plain B-tree on the full
-- status column. At scale it still requires Postgres to fetch all matching rows
-- and sort them. Because 'queued' is the only actionable status for settlement
-- and rows leave this set quickly (they transition to submitted/settled/failed),
-- a partial index is the right tool:
--
--   - Index size stays small: only live queued rows are indexed.
--   - The planner can satisfy both the WHERE filter and ORDER BY created_at ASC
--     in a single index scan with no separate sort step.
--   - The trailing `id` column provides a stable tie-breaker for future
--     keyset-paginated settlement batches.
--   - Rows in terminal states (submitted, settled, failed, dead_lettered) are
--     never scanned by this index, keeping it tight.
--
-- The existing payouts_status_idx is kept: it is still used by queries that
-- filter on other status values (e.g. the dead-letter escalation path that
-- scans WHERE status = 'failed').

CREATE INDEX IF NOT EXISTS payouts_queued_created_at_idx
    ON payouts (created_at ASC, id)
    WHERE status = 'queued';
