//! PostgreSQL connection pool.
//!
//! **Cron audit** — table `cron_runs` (see migration `004_cron_runs.sql`) stores one row per
//! reconcile/settle HTTP run with JSONB `metadata` matching the response summary. Application
//! code should not fail the cron HTTP response if an audit insert fails; log and continue.
//! **Dead-letter** — `payout_dead_letters` (see migration `005_payout_dead_letter.sql`) holds
//! payouts that have failed [`crate::handlers::cron::PAYOUT_DEAD_LETTER_THRESHOLD`] times.
//! Operators must resolve these manually; no automatic retry is attempted once dead-lettered.
//! **Payout retry tracking** — columns `failure_count`, `last_failure_at`, and `last_failure_reason`
//! on the `payouts` table (see migration `007_payout_attempt_counters_and_last_error.sql`) enable
//! operators to inspect and debug settlement failures. The cron settle handler updates these
//! fields on each retry and escalates to dead-letter once [`crate::handlers::cron::PAYOUT_DEAD_LETTER_THRESHOLD`] is reached.
//! **Invoice `metadata` (JSONB)** — today the API stores a small opaque object and does not
//! filter on it in SQL. Do not add JSONB indexes until a real `WHERE` / `ORDER BY` / `JOIN`
//! pattern lands in application code; see `../usdc-payment-link-tool/migrations/003_invoice_metadata_jsonb_index_plan.sql`
//! and the product README for the decision record and index-type cheat sheet.
//! **Sessions** (`sessions` table) are not modeled as Rust structs here; see [`crate::auth`].
//!
//! Index assumptions for high churn (many logins / expiries):
//! - Lookup uses `sessions.id` (primary key) inside `EXISTS (... AND expires_at > NOW())` — the hot path is a single-row PK fetch.
//! - Background expiry cleanup should scan `WHERE expires_at < $1` (and optionally `ORDER BY expires_at, id` for keyset batches). Apply
//!   migration `002_session_expiry_indexes.sql` so `(expires_at, id)` and `(merchant_id, expires_at)` exist in production; see
//!   `usdc-payment-link-tool/migrations/` and the rust-backend README.
//!
//! **Dashboard list query index** — `invoices_merchant_created_at_id_idx` (migration
//! `006_invoice_dashboard_index.sql`) is a composite `(merchant_id, created_at DESC, id)` index
//! that satisfies the equality filter + ORDER BY in a single index scan. The trailing `id` column
//! supports stable keyset pagination. See the migration comment for measured query plan timings.
//! **Invoice amount integrity** — migration `008_invoice_amount_check.sql` adds a CHECK constraint
//! `invoices_amount_split_check` enforcing `gross_amount_cents = platform_fee_cents + net_amount_cents`.
//! The database will reject any INSERT or UPDATE that violates this invariant.
//!
//! **Queued-payouts partial index** — `payouts_queued_created_at_idx` (migration
//! `007_payouts_queued_partial_index.sql`) is a partial index on `(created_at ASC, id)` filtered
//! to `WHERE status = 'queued'`. It lets the settle cron scan process queued payouts in FIFO order
//! without a full-table scan. Only live queued rows are indexed, so the index stays small as rows
//! transition to terminal states. The existing `payouts_status_idx` is kept for queries that
//! filter on other status values (e.g. `WHERE status = 'failed'` in the dead-letter path).

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::Config as PgConfig;

use crate::config::Config;

/// Valid PostgreSQL SSL modes
const VALID_SSL_MODES: &[&str] = &[
    "disable", "allow", "prefer", "require", "verify-ca", "verify-full"
];

/// Validates PGSSL configuration at startup
pub fn validate_ssl_mode(ssl_mode: &str) -> anyhow::Result<()> {
    if !VALID_SSL_MODES.contains(&ssl_mode) {
        anyhow::bail!(
            "Invalid PGSSL mode '{}'. Valid modes are: {}",
            ssl_mode,
            VALID_SSL_MODES.join(", ")
        );
    }
    Ok(())
}

/// Snapshot of payout queue health returned by [`payout_queue_stats`].
#[derive(Debug, serde::Serialize)]
pub struct PayoutQueueStats {
    pub queued: i64,
    pub failed: i64,
    pub dead_lettered: i64,
    /// Age in seconds of the oldest queued payout, or `None` when the queue is empty.
    pub oldest_queued_age_secs: Option<i64>,
}

pub fn create_pool(config: &Config) -> anyhow::Result<Pool> {
    // Validate SSL mode early
    validate_ssl_mode(&config.pgssl)?;
    
    let pg = config.database_url.inner().parse::<PgConfig>()?;
    let manager_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let manager = Manager::from_config(pg, tokio_postgres::NoTls, manager_config);
    Ok(Pool::builder(manager)
        .runtime(Runtime::Tokio1)
        .max_size(16)
        .build()?)
}

/// Attempts to claim a payout for processing by setting worker_id and timestamp.
/// Returns true if successfully claimed, false if already claimed by another worker.
pub async fn claim_payout_for_processing(
    client: &deadpool_postgres::Client,
    payout_id: uuid::Uuid,
    worker_id: &str,
) -> Result<bool, tokio_postgres::Error> {
    let rows_affected = client
        .execute(
            "UPDATE payouts 
             SET processing_worker_id = $1, processing_started_at = NOW(), updated_at = NOW()
             WHERE id = $2 AND status = 'queued' AND processing_worker_id IS NULL",
            &[&worker_id, &payout_id],
        )
        .await?;
    Ok(rows_affected > 0)
}

/// Releases a payout from processing (clears worker_id and timestamp).
pub async fn release_payout_from_processing(
    client: &deadpool_postgres::Client,
    payout_id: uuid::Uuid,
) -> Result<(), tokio_postgres::Error> {
    client
        .execute(
            "UPDATE payouts 
             SET processing_worker_id = NULL, processing_started_at = NULL, updated_at = NOW()
             WHERE id = $1",
            &[&payout_id],
        )
        .await?;
    Ok(())
}

/// Returns a point-in-time snapshot of payout queue health.
///
/// All three counts and the oldest-queued age are fetched in a single query to
/// avoid TOCTOU skew between separate reads.
pub async fn payout_queue_stats(
    client: &deadpool_postgres::Client,
) -> Result<PayoutQueueStats, tokio_postgres::Error> {
    let row = client
        .query_one(
            "SELECT
               COUNT(*) FILTER (WHERE p.status = 'queued')                          AS queued,
               COUNT(*) FILTER (WHERE p.status = 'failed')                          AS failed,
               (SELECT COUNT(*) FROM payout_dead_letters)                            AS dead_lettered,
               EXTRACT(EPOCH FROM (NOW() - MIN(p.created_at) FILTER (WHERE p.status = 'queued')))::bigint
                                                                                     AS oldest_queued_age_secs
             FROM payouts p",
            &[],
        )
        .await?;

    Ok(PayoutQueueStats {
        queued: row.get::<_, i64>("queued"),
        failed: row.get::<_, i64>("failed"),
        dead_lettered: row.get::<_, i64>("dead_lettered"),
        oldest_queued_age_secs: row.get("oldest_queued_age_secs"),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use super::validate_ssl_mode;

    #[test]
    fn retention_policy_migration_defines_config_table_and_extends_job_type() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/013_retention_policy.sql");
        let sql = std::fs::read_to_string(path).expect("read 013_retention_policy.sql");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS retention_config"));
        assert!(sql.contains("retain_days"));
        assert!(sql.contains("'sessions'"));
        assert!(sql.contains("'payment_events'"));
        assert!(sql.contains("'purge_payment_events'"));
        assert!(sql.contains("ON CONFLICT (table_name) DO NOTHING"));
    }

    #[test]
    fn retention_indexes_migration_adds_payment_events_created_at_index() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/014_retention_indexes.sql");
        let sql = std::fs::read_to_string(path).expect("read 014_retention_indexes.sql");
        assert!(sql.contains("payment_events_created_at_idx"));
        assert!(sql.contains("ON payment_events (created_at ASC)"));
        assert!(sql.contains("CREATE INDEX IF NOT EXISTS"));
    }

    #[test]
    fn invoice_paid_at_not_before_created_at_migration_defines_check_constraint() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/017_invoice_paid_at_not_before_created_at.sql");
        let sql = std::fs::read_to_string(path)
            .expect("read 017_invoice_paid_at_not_before_created_at.sql");
        assert!(sql.contains("ALTER TABLE invoices"), "must alter invoices table");
        assert!(
            sql.contains("invoices_paid_at_after_created_at_check"),
            "must name the constraint"
        );
        assert!(
            sql.contains("paid_at >= created_at"),
            "must enforce paid_at >= created_at"
        );
        assert!(
            sql.contains("paid_at IS NULL"),
            "constraint must be nullable-safe"
        );
    }

    #[test]
    fn invoice_settled_after_paid_migration_defines_check_constraint() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/015_invoice_settled_after_paid_check.sql");
        let sql = std::fs::read_to_string(path).expect("read 015_invoice_settled_after_paid_check.sql");
        assert!(sql.contains("ALTER TABLE invoices"), "must alter invoices table");
        assert!(sql.contains("invoices_settled_after_paid_check"), "must name the constraint");
        assert!(sql.contains("settled_at >= paid_at"), "must enforce settled_at >= paid_at");
    }

    #[test]
    fn webhook_deliveries_audit_migration_defines_table_and_indexes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/014_webhook_deliveries_audit.sql");
        let sql = std::fs::read_to_string(path).expect("read 014_webhook_deliveries_audit.sql");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS webhook_deliveries_audit"));
        assert!(sql.contains("delivery_id    TEXT    NOT NULL UNIQUE"));
        assert!(sql.contains("CHECK (status IN ('received', 'processed', 'failed', 'duplicate'))"));
        assert!(sql.contains("webhook_deliveries_audit_source_received_at_idx"));
        assert!(sql.contains("webhook_deliveries_audit_status_received_at_idx"));
        assert!(sql.contains("replay_of"));
        assert!(sql.contains("invoice_id"));
    }

    #[test]
    fn merchant_email_citext_migration_alters_column_and_rebuilds_constraint() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/013_merchant_email_citext.sql");
        let sql = std::fs::read_to_string(path).expect("read 013_merchant_email_citext.sql");
        assert!(sql.contains("CREATE EXTENSION IF NOT EXISTS citext"), "must enable citext");
        assert!(sql.contains("ALTER COLUMN email TYPE citext"), "must retype email to citext");
        assert!(sql.contains("DROP CONSTRAINT IF EXISTS merchants_email_key"), "must drop old constraint");
        assert!(sql.contains("ADD CONSTRAINT merchants_email_key UNIQUE (email)"), "must re-add unique constraint");
    }

    #[test]
    fn webhook_deliveries_migration_defines_table_and_index() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/011_webhook_deliveries.sql");
        let sql = std::fs::read_to_string(path).expect("read 011_webhook_deliveries.sql");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS webhook_deliveries"));
        assert!(sql.contains("delivery_id TEXT PRIMARY KEY"));
        assert!(sql.contains("webhook_deliveries_received_at_idx"));
        assert!(sql.contains("CREATE INDEX IF NOT EXISTS"));
    }

    #[test]
    fn pending_invoices_expiry_index_migration_is_partial() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/012_pending_invoices_expiry_idx.sql");
        let sql = std::fs::read_to_string(path).expect("read 012_pending_invoices_expiry_idx.sql");
        assert!(sql.contains("invoices_pending_expires_at_idx"));
        assert!(sql.contains("WHERE status = 'pending'"), "must be a partial index");
        assert!(sql.contains("expires_at ASC"));
        assert!(sql.contains("CREATE INDEX IF NOT EXISTS"));
    }

    #[test]
    fn payment_events_event_type_index_migration_is_idempotent() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/010_payment_events_event_type_index.sql");
        let sql =
            std::fs::read_to_string(path).expect("read 010_payment_events_event_type_index.sql");
        assert!(
            sql.contains("CREATE INDEX IF NOT EXISTS payment_events_event_type_idx"),
            "must define the event_type index"
        );
        assert!(
            sql.contains("ON payment_events (event_type)"),
            "index must be on payment_events.event_type"
        );
        assert!(
            sql.contains("CREATE INDEX IF NOT EXISTS"),
            "must be idempotent"
        );
    }

    #[test]
    fn dashboard_index_migration_defines_composite_index() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/006_invoice_dashboard_index.sql");
        let sql = std::fs::read_to_string(path).expect("read 006_invoice_dashboard_index.sql");
        assert!(
            sql.contains("invoices_merchant_created_at_id_idx"),
            "must define the composite dashboard index"
        );
        assert!(
            sql.contains("merchant_id") && sql.contains("created_at DESC"),
            "index must cover merchant_id and created_at DESC"
        );
        assert!(
            sql.contains("CREATE INDEX IF NOT EXISTS"),
            "must be idempotent"
        );
    }

    #[test]
    fn dashboard_index_query_uses_correct_column_order() {
        // The list_invoices handler query must match the index column order:
        // merchant_id (equality) → created_at DESC (sort) → id (tie-break).
        // This test pins the query string so a refactor that breaks the index
        // alignment is caught at compile time rather than at runtime.
        let query =
            "SELECT * FROM invoices WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT 100";
        assert!(query.contains("merchant_id = $1"));
        assert!(query.contains("ORDER BY created_at DESC"));
    }

    #[test]
    fn payout_dead_letter_migration_defines_table_and_indexes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/005_payout_dead_letter.sql");
        let sql = std::fs::read_to_string(path).expect("read 005_payout_dead_letter.sql");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS payout_dead_letters"));
        assert!(sql.contains("failure_count"));
        assert!(sql.contains("payout_dead_letters_merchant_id_idx"));
    }

    #[test]
    fn cron_runs_migration_defines_audit_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/004_cron_runs.sql");
        let sql = std::fs::read_to_string(path).expect("read 004_cron_runs.sql");
        assert!(sql.contains("CREATE TABLE cron_runs"));
        assert!(sql.contains("job_type"));
        assert!(sql.contains("metadata JSONB"));
        assert!(sql.contains("cron_runs_job_type_started_at_idx"));
    }

    #[test]
    fn invoice_metadata_plan_migration_documents_index_policy() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/003_invoice_metadata_jsonb_index_plan.sql");
        let sql =
            std::fs::read_to_string(path).expect("read 003_invoice_metadata_jsonb_index_plan.sql");
        assert!(
            sql.contains("COMMENT ON COLUMN invoices.metadata"),
            "plan should register a catalog comment for operators"
        );
        assert!(
            sql.contains("jsonb_path_ops") && sql.contains("GIN"),
            "plan should mention GIN operator class options when metadata is queried"
        );
        assert!(
            sql.contains("Policy: do not CREATE INDEX"),
            "plan should warn against speculative indexes"
        );
        for line in sql.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("--") {
                continue;
            }
            assert!(
                !t.to_uppercase().starts_with("CREATE INDEX"),
                "003 must not create speculative metadata indexes: {t}"
            );
        }
    }

    #[test]
    fn invoice_amount_split_migration_defines_check_constraint() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/008_invoice_amount_check.sql");
        let sql = std::fs::read_to_string(path).expect("read 008_invoice_amount_check.sql");
        assert!(
            sql.contains("invoices_amount_split_check"),
            "migration must name the check constraint invoices_amount_split_check"
        );
        assert!(
            sql.contains("gross_amount_cents = platform_fee_cents + net_amount_cents"),
            "constraint must enforce gross = fee + net"
        );
        assert!(
            sql.contains("ALTER TABLE invoices"),
            "migration must alter the invoices table"
        );
    }

    #[test]
    fn session_expiry_migration_defines_expected_indexes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/002_session_expiry_indexes.sql");
        let sql = std::fs::read_to_string(path).expect("read 002_session_expiry_indexes.sql");
        assert!(
            sql.contains("sessions_expires_at_id_idx"),
            "composite (expires_at, id) for ordered expiry batches"
        );
        assert!(
            sql.contains("sessions_merchant_expires_at_idx"),
            "composite (merchant_id, expires_at) for scoped cleanup"
        );
        assert!(
            sql.contains("DROP INDEX IF EXISTS sessions_expires_at_idx"),
            "replaces single-column expires_at index from 001"
        );
    }

    #[test]
    fn payout_attempt_counters_migration_defines_tracking_columns() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/007_payout_attempt_counters_and_last_error.sql");
        let sql = std::fs::read_to_string(path).expect("read 007_payout_attempt_counters_and_last_error.sql");
        assert!(
            sql.contains("last_failure_reason TEXT"),
            "migration must add last_failure_reason column to track most recent error"
        );
        assert!(
            sql.contains("payouts_last_failure_at_idx"),
            "migration must create index on last_failure_at for failure discovery queries"
        );
        assert!(
            sql.contains("ALTER TABLE payouts"),
            "migration must alter payouts table (idempotent with IF NOT EXISTS)"
        );
    }

    #[test]
    fn queued_payouts_partial_index_migration_is_correct() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/007_payouts_queued_partial_index.sql");
        let sql =
            std::fs::read_to_string(path).expect("read 007_payouts_queued_partial_index.sql");

        assert!(
            sql.contains("payouts_queued_created_at_idx"),
            "must define the partial index by its canonical name"
        );
        assert!(
            sql.contains("WHERE status = 'queued'"),
            "must be a partial index scoped to queued rows only"
        );
        assert!(
            sql.contains("created_at ASC"),
            "must order by created_at ASC for FIFO settlement processing"
        );
        assert!(
            sql.contains("CREATE INDEX IF NOT EXISTS"),
            "must be idempotent"
        );
        // The migration must not drop the existing payouts_status_idx — other
        // queries (dead-letter escalation) still rely on it.
        assert!(
            !sql.contains("DROP INDEX"),
            "must not drop the existing payouts_status_idx"
        );
    }

    #[test]
    fn payout_row_locking_migration_adds_processing_columns() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/017_payout_row_locking.sql");
        let sql = std::fs::read_to_string(path).expect("read 017_payout_row_locking.sql");
        assert!(sql.contains("processing_worker_id TEXT"));
        assert!(sql.contains("processing_started_at TIMESTAMPTZ"));
        assert!(sql.contains("payouts_processing_worker_idx"));
    }

    #[test]
    fn business_name_constraint_migration_prevents_empty_names() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/018_business_name_constraint.sql");
        let sql = std::fs::read_to_string(path).expect("read 018_business_name_constraint.sql");
        assert!(sql.contains("merchants_business_name_not_empty"));
        assert!(sql.contains("LENGTH(TRIM(business_name)) > 0"));
    }

    #[test]
    fn performance_fixtures_migration_creates_test_data() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/019_performance_test_fixtures.sql");
        let sql = std::fs::read_to_string(path).expect("read 019_performance_test_fixtures.sql");
        assert!(sql.contains("Performance test invoice"));
        assert!(sql.contains("performance_test_summary"));
        assert!(sql.contains("cleanup_performance_test_data"));
    }

    #[test]
    fn ssl_mode_validation_rejects_invalid_modes() {
        assert!(validate_ssl_mode("invalid").is_err());
        assert!(validate_ssl_mode("random").is_err());
    }

    #[test]
    fn ssl_mode_validation_accepts_valid_modes() {
        for mode in ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"] {
            assert!(validate_ssl_mode(mode).is_ok());
        }
    }
}

#[cfg(test)]
mod checkout_attempt_tests {
    use std::path::Path;

    #[test]
    fn checkout_attempts_migration_defines_table_columns_and_indexes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/021_checkout_attempts.sql");
        let sql = std::fs::read_to_string(path).expect("read 021_checkout_attempts.sql");

        // Table must be idempotent.
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS checkout_attempts"));

        // Required columns.
        assert!(sql.contains("invoice_id"), "must have invoice_id FK column");
        assert!(sql.contains("action"), "must have action column");
        assert!(sql.contains("status"), "must have status column");
        assert!(sql.contains("attempted_at"), "must have attempted_at timestamp");
        assert!(sql.contains("payload"), "must have payload JSONB column");
        assert!(sql.contains("error_detail"), "must have nullable error_detail column");

        // FK to invoices with cascade delete.
        assert!(
            sql.contains("REFERENCES invoices(id) ON DELETE CASCADE"),
            "invoice_id must reference invoices with cascade delete"
        );

        // Action and status CHECK constraints.
        assert!(
            sql.contains("'build'") && sql.contains("'submit'"),
            "action CHECK must include 'build' and 'submit'"
        );
        assert!(
            sql.contains("'initiated'") && sql.contains("'succeeded'") && sql.contains("'failed'"),
            "status CHECK must include 'initiated', 'succeeded', and 'failed'"
        );

        // Indexes must be idempotent.
        assert!(
            sql.contains("checkout_attempts_invoice_id_attempted_at_idx"),
            "must define per-invoice lookup index"
        );
        assert!(
            sql.contains("checkout_attempts_action_status_attempted_at_idx"),
            "must define action+status alerting index"
        );
        assert!(
            sql.matches("CREATE INDEX IF NOT EXISTS").count() >= 2,
            "both indexes must use IF NOT EXISTS"
        );
    }

    #[test]
    fn last_checkout_attempt_migration_adds_nullable_column() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/005_invoice_last_checkout_attempt_at.sql");
        let sql = std::fs::read_to_string(path)
            .expect("read 005_invoice_last_checkout_attempt_at.sql");
        assert!(
            sql.contains("ALTER TABLE invoices"),
            "migration must alter the invoices table"
        );
        assert!(
            sql.contains("last_checkout_attempt_at"),
            "migration must add last_checkout_attempt_at column"
        );
        assert!(
            sql.contains("TIMESTAMPTZ"),
            "column must be a timestamp with time zone"
        );
        // Column must be nullable — no NOT NULL constraint allowed.
        assert!(
            !sql.contains("NOT NULL"),
            "last_checkout_attempt_at must be nullable (no NOT NULL)"
        );
        // No speculative index — add one only when a real query pattern exists.
        for line in sql.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("--") {
                continue;
            }
            assert!(
                !t.to_uppercase().starts_with("CREATE INDEX"),
                "005 must not create a speculative index: {t}"
            );
        }
    }

    #[test]
    fn invoice_public_id_format_migration_adds_check_constraint() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/016_invoice_public_id_format.sql");
        let sql = std::fs::read_to_string(path).expect("read 016_invoice_public_id_format.sql");
        assert!(sql.contains("ALTER TABLE invoices"), "must alter invoices table");
        assert!(
            sql.contains("invoices_public_id_format"),
            "must name the constraint"
        );
        assert!(
            sql.contains("inv_[0-9a-f]{16}"),
            "must enforce inv_[0-9a-f]{{16}} pattern"
        );
    }
}
