use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    routing::{get, post},
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
use axum_extra::extract::cookie::Cookie;

const ADMIN_URL_ENV: &str = "ASTROPAY_MIGRATION_TEST_ADMIN_DATABASE_URL";

async fn setup_ephemeral_db() -> anyhow::Result<(String, String)> {
    let admin_url = env::var(ADMIN_URL_ENV)
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/postgres".to_string());
    
    let admin_config = tokio_postgres::Config::from_str(&admin_url)?;
    let db_name = format!("astropay_invcrud_test_{}", Uuid::new_v4().simple());
    let (admin, admin_connection) = admin_config.connect(NoTls).await?;
    tokio::spawn(async move { let _ = admin_connection.await; });

    let quoted_db = format!("\"{}\"", db_name);
    admin.batch_execute(&format!("CREATE DATABASE {}", quoted_db)).await?;

    let mut test_url = admin_url.parse::<url::Url>()?;
    test_url.set_path(&db_name);
    
    let mut db_client = admin_config.clone();
    db_client.dbname(&db_name);
    let (mut client, connection) = db_client.connect(NoTls).await?;
    tokio::spawn(async move { let _ = connection.await; });
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
async fn test_invoice_crud_paths() -> anyhow::Result<()> {
    let (db_name, database_url) = setup_ephemeral_db().await?;

    let result = async {
        env::set_var("DATABASE_URL", &database_url);
        env::set_var("CRON_SECRET", "cron");
        env::set_var("JWT_SECRET", "jwtsecret_must_be_at_least_32_bytes_long!");
        env::set_var("STELLAR_NETWORK_PASSPHRASE", "Test SDF Network ; September 2015");
        
        let config = Config::from_env().unwrap();
        let pool = create_pool(&config).unwrap();
        
        let state = rust_backend::AppState {
            config: config.clone(),
            pool: pool.clone(),
            login_limiter: std::sync::Arc::new(rust_backend::login_rate_limit::LoginRateLimiter::from_config(&config)),
        };

        let app = Router::new()
            .route("/api/invoices", get(handlers::invoices::list_invoices).post(handlers::invoices::create_invoice))
            .route("/api/invoices/:id", get(handlers::invoices::get_invoice))
            .with_state(state.clone());

        // We need an authenticated merchant session. So we insert a merchant and create a token.
        let client = pool.get().await?;
        let merchant_id = Uuid::new_v4();
        client.execute(
            "INSERT INTO merchants (id, email, password_hash, business_name, stellar_public_key, settlement_public_key) VALUES ($1,$2,$3,$4,$5,$6)",
            &[&merchant_id, &"test@example.com", &"hash", &"Biz", &"GB...", &"GB..."]
        ).await?;

        // Mint session
        let session_id = Uuid::new_v4();
        client.execute(
            "INSERT INTO sessions (id, merchant_id, expires_at) VALUES ($1, $2, NOW() + INTERVAL '1 day')",
            &[&session_id, &merchant_id]
        ).await?;
        let jwt = rust_backend::auth::mint_jwt(&config, session_id, merchant_id).unwrap();

        // 1. Create Invoice
        let payload = r#"{"description": "Test Inv", "amountUsd": 12.5}"#;
        let req = Request::builder()
            .uri("/api/invoices")
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, format!("astropay_session={}", jwt))
            .body(Body::from(payload))?;

        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(res.into_body(), 1024*1024).await?;
        let json: serde_json::Value = serde_json::from_slice(&body_bytes)?;
        let invoice_id = json["invoice"]["id"].as_str().unwrap().to_string();

        assert_eq!(json["invoice"]["amountUsd"], 12.5);
        assert_eq!(json["invoice"]["description"], "Test Inv");

        // 2. Get Invoice by ID
        let req = Request::builder()
            .uri(&format!("/api/invoices/{}", invoice_id))
            .header(header::COOKIE, format!("astropay_session={}", jwt))
            .body(Body::empty())?;
        
        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(res.into_body(), 1024*1024).await?;
        let json: serde_json::Value = serde_json::from_slice(&body_bytes)?;
        assert_eq!(json["invoice"]["id"], invoice_id);

        // 3. List Invoices
        let req = Request::builder()
            .uri("/api/invoices")
            .header(header::COOKIE, format!("astropay_session={}", jwt))
            .body(Body::empty())?;
        
        let res = app.clone().oneshot(req).await?;
        assert_eq!(res.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(res.into_body(), 1024*1024).await?;
        let json: serde_json::Value = serde_json::from_slice(&body_bytes)?;
        assert!(json["invoices"].as_array().unwrap().len() >= 1);

        Ok::<(), anyhow::Error>(())
    }.await;

    teardown_ephemeral_db(db_name).await?;

    result?;
    Ok(())
}
