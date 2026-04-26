use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
    Router,
};
use deadpool_postgres::Pool;
use rust_backend::{
    config::Config,
    db::create_pool,
    handlers,
};
use std::{env, str::FromStr};
use tokio_postgres::NoTls;
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN_URL_ENV: &str = "ASTROPAY_MIGRATION_TEST_ADMIN_DATABASE_URL";

async fn setup_ephemeral_db() -> anyhow::Result<(String, String)> {
    let admin_url = env::var(ADMIN_URL_ENV)
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_string());
    
    let admin_config = tokio_postgres::Config::from_str(&admin_url)?;
    let db_name = format!("astropay_cron_test_{}", Uuid::new_v4().simple());
    let (admin, admin_connection) = admin_config.connect(NoTls).await?;
    tokio::spawn(async move {
        if let Err(error) = admin_connection.await {
            eprintln!("postgres admin connection error: {error}");
        }
    });

    let quoted_db = format!("\"{}\"", db_name);
    admin.batch_execute(&format!("CREATE DATABASE {}", quoted_db)).await?;

    let mut test_url = admin_url.parse::<url::Url>()?;
    test_url.set_path(&db_name);
    
    // We must run migrations manually for the new ephemeral DB
    let mut db_client = admin_config.clone();
    db_client.dbname(&db_name);
    let (mut client, connection) = db_client.connect(NoTls).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    rust_backend::migrations::apply_pending_migrations(&mut client, &rust_backend::migrations::default_migrations_dir()).await?;
    
    Ok((db_name, test_url.to_string()))
}

async fn teardown_ephemeral_db(db_name: String) -> anyhow::Result<()> {
    let admin_url = env::var(ADMIN_URL_ENV)
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_string());
    let admin_config = tokio_postgres::Config::from_str(&admin_url)?;
    let (admin, connection) = admin_config.connect(NoTls).await?;
    tokio::spawn(async move { let _ = connection.await; });

    let quoted_db = format!("\"{}\"", db_name);
    let _ = admin.execute("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1", &[&db_name]).await;
    admin.batch_execute(&format!("DROP DATABASE IF EXISTS {}", quoted_db)).await?;
    Ok(())
}

#[tokio::test]
async fn test_cron_rejection_paths() -> anyhow::Result<()> {
    if env::var(ADMIN_URL_ENV).is_err() && env::var("CI").is_err() {
        // Just stub for local if it fails, but let's try to proceed
    }

    let (db_name, database_url) = setup_ephemeral_db().await?;

    let result = async {
        env::set_var("DATABASE_URL", &database_url);
        env::set_var("CRON_SECRET", "supersecret");
        env::set_var("JWT_SECRET", "jwtsecret_must_be_at_least_32_bytes_long!");
        env::set_var("STELLAR_NETWORK_PASSPHRASE", "Test SDF Network ; September 2015");
        
        // Build minimal app
        let config = Config::from_env().unwrap();
        let pool = create_pool(&config).unwrap();
        // Since LoginRateLimiter needs Config, we would need to replicate AppState
        // The handlers::cron routes just need state with config & pool.
        
        let state = rust_backend::AppState {
            config: config.clone(),
            pool,
            login_limiter: std::sync::Arc::new(rust_backend::login_rate_limit::LoginRateLimiter::from_config(&config)),
        };

        let app = Router::new()
            .route("/api/cron/reconcile", get(handlers::cron::reconcile))
            .with_state(state);

        // Test missing token
        let req = Request::builder()
            .uri("/api/cron/reconcile")
            .body(Body::empty())?;
        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Test malformed token
        let req = Request::builder()
            .uri("/api/cron/reconcile")
            .header(header::AUTHORIZATION, "InvalidFormat")
            .body(Body::empty())?;
        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Test wrong bearer token
        let req = Request::builder()
            .uri("/api/cron/reconcile")
            .header(header::AUTHORIZATION, "Bearer wrong_secret")
            .body(Body::empty())?;
        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Test correct token
        let req = Request::builder()
            .uri("/api/cron/reconcile")
            .header(header::AUTHORIZATION, "Bearer supersecret")
            .body(Body::empty())?;
        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);

        Ok::<(), anyhow::Error>(())
    }.await;

    teardown_ephemeral_db(db_name).await?;

    result?;
    Ok(())
}
