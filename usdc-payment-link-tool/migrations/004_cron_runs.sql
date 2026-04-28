-- Append-only audit of reconcile / settle cron invocations (HTTP handlers).
-- metadata holds the response-shaped summary (counts, per-invoice or per-payout outcomes).
-- error_detail is set when success is false (handler error, not implemented, or future partial-failure modes).
--
-- Rollback:
--   DROP TABLE IF EXISTS cron_runs;
--   No application data is lost; this is an audit/observability table only.
--   Dependent migrations that extend the job_type CHECK constraint
--   (007_cron_runs_purge_sessions.sql, 013_retention_policy.sql) must be rolled
--   back first, or the DROP will fail due to constraint dependencies.

CREATE TABLE cron_runs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  job_type TEXT NOT NULL CHECK (job_type = ANY (ARRAY['reconcile'::text, 'settle'::text])),
  started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  finished_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  success BOOLEAN NOT NULL,
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  error_detail TEXT
);

CREATE INDEX cron_runs_job_type_started_at_idx ON cron_runs (job_type, started_at DESC);
