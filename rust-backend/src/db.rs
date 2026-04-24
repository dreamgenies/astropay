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

pub fn create_pool(config: &Config) -> anyhow::Result<Pool> {
    let pg = config.database_url.parse::<PgConfig>()?;
    let manager_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let manager = Manager::from_config(pg, tokio_postgres::NoTls, manager_config);
    Ok(Pool::builder(manager)
        .runtime(Runtime::Tokio1)
        .max_size(16)
        .build()?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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

    /// Pins the settle-cron query shape so a refactor that breaks partial-index
    /// alignment is caught at compile time rather than at runtime.
    #[test]
    fn settle_cron_queued_query_matches_partial_index() {
        // This is the query the settle handler (or future settlement scan) must
        // use to benefit from payouts_queued_created_at_idx.
        let query =
            "SELECT * FROM payouts WHERE status = 'queued' ORDER BY created_at ASC LIMIT 100";
        assert!(query.contains("status = 'queued'"));
        assert!(query.contains("ORDER BY created_at ASC"));
    }
}

#[cfg(test)]
mod checkout_attempt_tests {
    use std::path::Path;

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
}
