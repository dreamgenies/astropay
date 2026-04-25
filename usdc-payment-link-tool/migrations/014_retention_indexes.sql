-- migration 014_retention_indexes.sql
-- Supports efficient retention deletes on payment_events.
--
-- The purge query is:
--   DELETE FROM payment_events WHERE created_at < NOW() - INTERVAL '365 days'
-- Without this index that is a full sequential scan. The index keeps the delete
-- scoped to only the rows that qualify.
--
-- sessions already has sessions_expires_at_id_idx (002_session_expiry_indexes.sql)
-- which the existing purge_sessions cron uses; no new index is needed there.

CREATE INDEX IF NOT EXISTS payment_events_created_at_idx
    ON payment_events (created_at ASC);
