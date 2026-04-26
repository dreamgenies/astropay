-- migration 013_retention_policy.sql
-- Defines retention policy for sessions and payment_events.
--
-- Retention rationale:
--   sessions         90 days  — beyond any realistic idle re-auth window; expired rows
--                               are already inert (expires_at < NOW()) but accumulate
--                               indefinitely without a purge policy.
--   payment_events  365 days  — one full year covers audit, dispute, and reconciliation
--                               windows for USDC payments on Stellar. Events older than
--                               one year have no operational value and inflate table scans.
--
-- The retention_config table is the single source of truth for these values.
-- The purge cron job reads them at runtime; changing a row here takes effect on the
-- next cron run without a code deploy.
--
-- Rollback:
--   DROP TABLE IF EXISTS retention_config;
--   ALTER TABLE cron_runs DROP CONSTRAINT IF EXISTS cron_runs_job_type_check;
--   ALTER TABLE cron_runs ADD CONSTRAINT cron_runs_job_type_check
--       CHECK (job_type = ANY (ARRAY[
--           'reconcile'::text, 'settle'::text, 'purge_sessions'::text
--       ]));
--   WARNING: any existing cron_runs rows with job_type = 'purge_payment_events'
--   will violate the restored constraint. Delete or update those rows first.
--   The purge_payment_events cron job must also be disabled before rolling back
--   to prevent it from writing rows that violate the constraint.

CREATE TABLE IF NOT EXISTS retention_config (
    table_name  TEXT PRIMARY KEY,
    retain_days INTEGER NOT NULL CHECK (retain_days > 0),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO retention_config (table_name, retain_days) VALUES
    ('sessions',        90),
    ('payment_events', 365)
ON CONFLICT (table_name) DO NOTHING;

-- Extend cron_runs job_type to include purge_payment_events so the retention
-- cron run is audited alongside reconcile, settle, and purge_sessions.
ALTER TABLE cron_runs
    DROP CONSTRAINT IF EXISTS cron_runs_job_type_check;

ALTER TABLE cron_runs
    ADD CONSTRAINT cron_runs_job_type_check
        CHECK (job_type = ANY (ARRAY[
            'reconcile'::text,
            'settle'::text,
            'purge_sessions'::text,
            'purge_payment_events'::text
        ]));
