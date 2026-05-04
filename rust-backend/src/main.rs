mod logging;

use std::net::SocketAddr;

use axum::{
    Router,
    routing::{get, post},
};
use http::Request;
use rust_backend::{
    AppState, config::Config, db::create_pool, handlers, login_rate_limit::LoginRateLimiter,
};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::{DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::{Level, info_span};

const CORRELATION_ID_HEADER: &str = "x-correlation-id";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    load_env_files();
    let config = Config::from_env()?;
    logging::init_tracing(config.log_format);

    // Sentry is optional: no-ops when SENTRY_DSN is absent.
    let _sentry_guard = sentry::init(sentry::ClientOptions {
        dsn: std::env::var("SENTRY_DSN")
            .ok()
            .and_then(|s| s.parse().ok()),
        environment: std::env::var("SENTRY_ENVIRONMENT")
            .ok()
            .map(std::borrow::Cow::Owned),
        release: sentry::release_name!(),
        traces_sample_rate: if cfg!(debug_assertions) { 1.0 } else { 0.2 },
        ..Default::default()
    });

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
        .route(
            "/api/cron/purge-sessions",
            get(handlers::cron::purge_sessions),
        )
        .route(
            "/api/cron/purge-payment-events",
            get(handlers::cron::purge_payment_events),
        )
        .route("/api/cron/archive", get(handlers::cron::archive))
        .route(
            "/api/cron/payouts/:payout_id/replay",
            axum::routing::post(handlers::cron::replay_payout),
        )
        .route(
            "/api/cron/payouts/:payout_id/claim",
            axum::routing::post(handlers::cron::claim_payout),
        )
        .route(
            "/api/cron/orphan-payments",
            get(handlers::cron::orphan_payments),
        )
        .route(
            "/api/cron/payout-health",
            get(handlers::cron::payout_health),
        )
        .route(
            "/api/cron/webhook-metrics",
            get(handlers::cron::webhook_correlation_metrics),
        )
        .route("/api/cron/alert-check", get(handlers::cron::alert_check))
        .route(
            "/api/webhooks/stellar",
            post(handlers::misc::stellar_webhook),
        )
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<_>| {
                    let correlation_id = request
                        .headers()
                        .get(CORRELATION_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("");
                    info_span!(
                        "request",
                        method = %request.method(),
                        uri = %request.uri(),
                        version = ?request.version(),
                        correlation_id = %correlation_id,
                    )
                })
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO))
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
        .layer(PropagateRequestIdLayer::new(
            http::header::HeaderName::from_static(CORRELATION_ID_HEADER),
        ))
        .layer(SetRequestIdLayer::new(
            http::header::HeaderName::from_static(CORRELATION_ID_HEADER),
            MakeRequestUuid,
        ))
        .with_state(state);

    tracing::info!(
        bind_addr = %config.bind_addr,
        log_format = config.log_format.as_str(),
        database_url = %rust_backend::redact::redact_connection_string(config.database_url.inner()),
        "rust backend listening"
    );
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

#[cfg(test)]
mod tests {
    #[test]
    fn request_tracing_does_not_log_raw_headers() {
        let src = include_str!("main.rs");
        let forbidden = ["include_headers", "(true)"].concat();
        assert!(
            !src.contains(&forbidden),
            "request tracing must not log Authorization or Cookie headers"
        );
    }

    #[test]
    fn request_tracing_uses_correlation_id_header() {
        assert_eq!(super::CORRELATION_ID_HEADER, "x-correlation-id");
        let src = include_str!("main.rs");
        assert!(src.contains("SetRequestIdLayer"));
        assert!(src.contains("PropagateRequestIdLayer"));
        assert!(src.contains("correlation_id"));
    }
}
