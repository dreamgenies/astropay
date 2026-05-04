pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod horizon_fixtures;
pub mod login_rate_limit;
pub mod migrations;
pub mod models;
pub mod money_state;
pub mod redact;
pub mod settle;
pub mod stellar;

use std::sync::Arc;

use config::Config;
use deadpool_postgres::Pool;
use login_rate_limit::LoginRateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: Pool,
    pub login_limiter: Arc<LoginRateLimiter>,
}
