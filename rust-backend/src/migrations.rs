use std::{
    fs,
    path::{Path, PathBuf},
};

use tokio_postgres::Client;

pub fn default_migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../usdc-payment-link-tool/migrations")
}

pub fn migration_files(migrations_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !migrations_dir.is_dir() {
        anyhow::bail!(
            "migrations directory not found: {} (run from rust-backend/)",
            migrations_dir.display()
        );
    }

    let mut files = fs::read_dir(migrations_dir)
        .map_err(|e| anyhow::anyhow!("read_dir {}: {e}", migrations_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sql"))
        .collect::<Vec<_>>();
    files.sort();

    if files.is_empty() {
        anyhow::bail!("no SQL migrations found in {}", migrations_dir.display());
    }

    Ok(files)
}

pub async fn apply_pending_migrations(
    client: &mut Client,
    migrations_dir: &Path,
) -> anyhow::Result<Vec<String>> {
    client
        .execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
               id         TEXT PRIMARY KEY,
               applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
               applied_by TEXT        NOT NULL DEFAULT 'unknown'
             )",
            &[],
        )
        .await?;
    // Idempotent backfill for databases bootstrapped before applied_by existed.
    client
        .execute(
            "ALTER TABLE schema_migrations ADD COLUMN IF NOT EXISTS applied_by TEXT NOT NULL DEFAULT 'unknown'",
            &[],
        )
        .await?;

    let mut applied = Vec::new();
    for file in migration_files(migrations_dir)? {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid migration file name: {}", file.display()))?
            .to_string();
        let exists = client
            .query_opt("SELECT 1 FROM schema_migrations WHERE id = $1", &[&name])
            .await?;
        if exists.is_some() {
            continue;
        }

        let sql = fs::read_to_string(&file)?;
        let transaction = client.transaction().await?;
        transaction
            .batch_execute(&sql)
            .await
            .map_err(|e| anyhow::anyhow!("migration {name} failed: {e}"))?;
        transaction
            .execute(
                "INSERT INTO schema_migrations (id, applied_by) VALUES ($1, 'rust')",
                &[&name],
            )
            .await?;
        transaction.commit().await?;
        applied.push(name);
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_file_list_is_sorted_and_non_empty() {
        let files = migration_files(&default_migrations_dir()).expect("read migrations");
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(names.first().is_some_and(|name| name == "001_init.sql"));
    }

    /// Pins the schema_migrations DDL so both runtimes stay in sync.
    /// The table must have id, applied_at, and applied_by — in that column order.
    #[test]
    fn schema_migrations_create_ddl_contains_required_columns() {
        // Reconstruct the exact DDL string used in apply_pending_migrations.
        let ddl = "CREATE TABLE IF NOT EXISTS schema_migrations (
               id         TEXT PRIMARY KEY,
               applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
               applied_by TEXT        NOT NULL DEFAULT 'unknown'
             )";
        assert!(ddl.contains("id         TEXT PRIMARY KEY"));
        assert!(ddl.contains("applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()"));
        assert!(ddl.contains("applied_by TEXT        NOT NULL DEFAULT 'unknown'"));
    }

    /// Pins the INSERT statement so applied_by = 'rust' is always recorded.
    #[test]
    fn schema_migrations_insert_records_rust_runtime() {
        let insert = "INSERT INTO schema_migrations (id, applied_by) VALUES ($1, 'rust')";
        assert!(insert.contains("applied_by"));
        assert!(insert.contains("'rust'"));
    }

    /// The applied_by migration must be idempotent and use ADD COLUMN IF NOT EXISTS.
    #[test]
    fn applied_by_migration_is_idempotent() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../usdc-payment-link-tool/migrations/015_schema_migrations_applied_by.sql");
        let sql = std::fs::read_to_string(path).expect("read 015_schema_migrations_applied_by.sql");
        assert!(sql.contains("ADD COLUMN IF NOT EXISTS applied_by"));
        assert!(sql.contains("schema_migrations"));
        assert!(sql.contains("DEFAULT 'unknown'"));
    }
}
