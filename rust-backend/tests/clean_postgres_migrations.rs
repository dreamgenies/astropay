use std::{env, str::FromStr};

use rust_backend::migrations::{apply_pending_migrations, default_migrations_dir, migration_files};
use tokio_postgres::{Config, NoTls};
use uuid::Uuid;

const ADMIN_URL_ENV: &str = "ASTROPAY_MIGRATION_TEST_ADMIN_DATABASE_URL";

#[tokio::test]
async fn migration_chain_applies_to_clean_postgres_database() -> anyhow::Result<()> {
    let Ok(admin_url) = env::var(ADMIN_URL_ENV) else {
        eprintln!("skipping clean Postgres migration test; set {ADMIN_URL_ENV} to opt in");
        return Ok(());
    };

    let admin_config = Config::from_str(&admin_url)?;
    let db_name = format!("astropay_migration_test_{}", Uuid::new_v4().simple());
    let (admin, admin_connection) = admin_config.connect(NoTls).await?;
    tokio::spawn(async move {
        if let Err(error) = admin_connection.await {
            eprintln!("postgres admin connection error: {error}");
        }
    });

    let quoted_db = quote_ident(&db_name)?;
    admin
        .batch_execute(&format!("CREATE DATABASE {quoted_db}"))
        .await?;

    let test_result = async {
        let mut test_config = admin_config.clone();
        test_config.dbname(&db_name);
        let (mut client, connection) = test_config.connect(NoTls).await?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                eprintln!("postgres test connection error: {error}");
            }
        });

        let migrations_dir = default_migrations_dir();
        let expected_migrations = migration_files(&migrations_dir)?
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        let applied = apply_pending_migrations(&mut client, &migrations_dir).await?;
        anyhow::ensure!(
            applied == expected_migrations,
            "applied migrations did not match migration directory order"
        );

        let applied_rows = client
            .query("SELECT id FROM schema_migrations ORDER BY id", &[])
            .await?
            .into_iter()
            .map(|row| row.get::<_, String>("id"))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            applied_rows == expected_migrations,
            "schema_migrations did not match applied migrations"
        );

        for table in [
            "merchants",
            "sessions",
            "invoices",
            "payment_events",
            "payouts",
            "cron_runs",
            "payout_dead_letters",
        ] {
            let expected_table = format!("public.{table}");
            let exists = client
                .query_one("SELECT to_regclass($1)::text", &[&expected_table])
                .await?
                .get::<_, Option<String>>(0);
            anyhow::ensure!(
                exists.as_deref() == Some(expected_table.as_str()),
                "expected table {expected_table} to exist"
            );
        }

        let second_run = apply_pending_migrations(&mut client, &migrations_dir).await?;
        anyhow::ensure!(
            second_run.is_empty(),
            "second migration pass must be idempotent"
        );

        Ok::<(), anyhow::Error>(())
    }
    .await;

    let _ = admin
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1",
            &[&db_name],
        )
        .await;
    let drop_result = admin
        .batch_execute(&format!("DROP DATABASE IF EXISTS {quoted_db}"))
        .await;

    test_result?;
    drop_result?;

    Ok(())
}

fn quote_ident(identifier: &str) -> anyhow::Result<String> {
    if identifier.is_empty() {
        anyhow::bail!("identifier cannot be empty");
    }
    if !identifier
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        anyhow::bail!("unsafe generated identifier: {identifier}");
    }
    Ok(format!("\"{identifier}\""))
}
