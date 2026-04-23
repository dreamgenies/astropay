//! PostgreSQL connection pool.
//!
//! **Cron audit** — table `cron_runs` (see migration `004_cron_runs.sql`) stores one row per
//! reconcile/settle HTTP run with JSONB `metadata` matching the response summary. Application
//! code should not fail the cron HTTP response if an audit insert fails; log and continue.
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
    fn cron_runs_migration_defines_audit_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/004_cron_runs.sql");
        let sql = std::fs::read_to_string(path).expect("read 004_cron_runs.sql");
        assert!(sql.contains("CREATE TABLE cron_runs"));
        assert!(sql.contains("job_type"));
        assert!(sql.contains("metadata JSONB"));
        assert!(sql.contains("cron_runs_job_type_started_at_idx"));
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
}
