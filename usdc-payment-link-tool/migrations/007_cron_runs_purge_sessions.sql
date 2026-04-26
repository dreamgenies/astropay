-- Extend cron_runs job_type to include purge_sessions so all cron jobs are audited uniformly.
--
-- Rollback:
--   ALTER TABLE cron_runs DROP CONSTRAINT IF EXISTS cron_runs_job_type_check;
--   ALTER TABLE cron_runs ADD CONSTRAINT cron_runs_job_type_check
--       CHECK (job_type = ANY (ARRAY['reconcile'::text, 'settle'::text]));
--   WARNING: any existing cron_runs rows with job_type = 'purge_sessions' will
--   violate the restored constraint. Delete or update those rows before rolling back.
ALTER TABLE cron_runs
    DROP CONSTRAINT IF EXISTS cron_runs_job_type_check;

ALTER TABLE cron_runs
    ADD CONSTRAINT cron_runs_job_type_check
        CHECK (job_type = ANY (ARRAY['reconcile'::text, 'settle'::text, 'purge_sessions'::text]));
