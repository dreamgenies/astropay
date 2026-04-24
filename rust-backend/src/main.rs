mod auth;
mod config;
mod db;
mod error;
mod handlers;
mod login_rate_limit;
mod models;
mod settle;
mod stellar;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};
use deadpool_postgres::Pool;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::Config, db::create_pool, login_rate_limit::LoginRateLimiter};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: Pool,
    pub login_limiter: Arc<LoginRateLimiter>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_env_files();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env()?;
    let pool = create_pool(&config)?;
    let login_limiter = LoginRateLimiter::from_config(&config);
    let state = AppState {
        config: config.clone(),
        pool,
        login_limiter,
    };

    let app = Router::new()
        .route("/healthz", get(handlers::misc::health))
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/logout", post(handlers::auth::logout))
        .route("/api/auth/me", get(handlers::auth::me))
        .route("/api/auth/refresh", post(handlers::auth::refresh))
        .route(
            "/api/invoices",
            get(handlers::invoices::list_invoices).post(handlers::invoices::create_invoice),
        )
        .route("/api/invoices/{id}", get(handlers::invoices::get_invoice))
        .route(
            "/api/invoices/{id}/status",
            get(handlers::invoices::get_status),
        )
        .route(
            "/api/invoices/{id}/checkout",
            post(handlers::invoices::unsupported_checkout),
        )
        .route("/api/cron/reconcile", get(handlers::cron::reconcile))
        .route("/api/cron/settle", get(handlers::cron::settle))
        .route("/api/cron/purge-sessions", get(handlers::cron::purge_sessions))
        .route("/api/cron/payouts/:payout_id/replay", axum::routing::post(handlers::cron::replay_payout))
        .route("/api/cron/orphan-payments", get(handlers::cron::orphan_payments))
        .route(
            "/api/webhooks/stellar",
            post(handlers::misc::stellar_webhook),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!("rust backend listening on {}", config.bind_addr);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn load_env_files() {
    for path in [
        ".env.local",
        ".env",
        "../usdc-payment-link-tool/.env.local",
        "../usdc-payment-link-tool/.env",
    ] {
        let _ = dotenvy::from_filename(path);
    }
}
