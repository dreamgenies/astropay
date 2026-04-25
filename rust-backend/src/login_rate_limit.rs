//! In-memory sliding-window limits for `POST /api/auth/login`.
//! For multi-instance production deployments, replace with a shared store (e.g. Redis).

use std::collections::HashMap;
use std::sync::Arc;

use time::{Duration, OffsetDateTime};
use tokio::sync::Mutex;

use crate::{config::Config, error::AppError};

#[derive(Clone, Debug)]
pub struct LoginRateLimiterSettings {
    pub ip_window: Duration,
    pub ip_max: u32,
    pub email_window: Duration,
    pub email_max_fail: u32,
}

impl From<&Config> for LoginRateLimiterSettings {
    fn from(c: &Config) -> Self {
        let ip_secs = (c.login_rate_ip_window_secs.min(i64::MAX as u64)) as i64;
        let email_secs = (c.login_rate_email_window_secs.min(i64::MAX as u64)) as i64;
        Self {
            ip_window: Duration::seconds(ip_secs.max(1)),
            ip_max: c.login_rate_ip_max,
            email_window: Duration::seconds(email_secs.max(1)),
            email_max_fail: c.login_rate_email_fail_max,
        }
    }
}

struct Inner {
    ip_hits: HashMap<String, Vec<OffsetDateTime>>,
    email_fails: HashMap<String, Vec<OffsetDateTime>>,
}

pub struct LoginRateLimiter {
    inner: Mutex<Inner>,
    settings: LoginRateLimiterSettings,
}

impl LoginRateLimiter {
    pub fn new(settings: LoginRateLimiterSettings) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                ip_hits: HashMap::new(),
                email_fails: HashMap::new(),
            }),
            settings,
        })
    }

    pub fn from_config(config: &Config) -> Arc<Self> {
        Self::new(LoginRateLimiterSettings::from(config))
    }

    /// All limits disabled (for tests or explicit opt-out).
    pub fn disabled() -> Arc<Self> {
        Self::new(LoginRateLimiterSettings {
            ip_window: Duration::seconds(1),
            ip_max: 0,
            email_window: Duration::seconds(1),
            email_max_fail: 0,
        })
    }

    /// Counts one login attempt for this client IP. Call only after basic payload validation.
    pub async fn check_ip(&self, ip: &str) -> Result<(), AppError> {
        if self.settings.ip_max == 0 {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let now = OffsetDateTime::now_utc();
        let v = inner.ip_hits.entry(ip.to_string()).or_default();
        prune_old(v, self.settings.ip_window, now);
        if (v.len() as u32) >= self.settings.ip_max {
            let retry = retry_after_seconds(v, self.settings.ip_window, now);
            return Err(AppError::rate_limited(retry));
        }
        v.push(now);
        Ok(())
    }

    /// Records a failed credential check (unknown email or bad password). Cleared on successful login.
    pub async fn record_email_failure(&self, email: &str) -> Result<(), AppError> {
        if self.settings.email_max_fail == 0 {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let now = OffsetDateTime::now_utc();
        let v = inner.email_fails.entry(email.to_string()).or_default();
        prune_old(v, self.settings.email_window, now);
        if (v.len() as u32) >= self.settings.email_max_fail {
            let retry = retry_after_seconds(v, self.settings.email_window, now);
            return Err(AppError::rate_limited(retry));
        }
        v.push(now);
        Ok(())
    }

    pub async fn clear_email_failures(&self, email: &str) {
        let mut inner = self.inner.lock().await;
        inner.email_fails.remove(email);
    }
}

fn prune_old(events: &mut Vec<OffsetDateTime>, window: Duration, now: OffsetDateTime) {
    let cutoff = now - window;
    events.retain(|t| *t >= cutoff);
}

fn retry_after_seconds(events: &[OffsetDateTime], window: Duration, now: OffsetDateTime) -> u64 {
    let Some(oldest) = events.iter().copied().min() else {
        return 1;
    };
    let until = oldest + window - now;
    let secs = until.whole_seconds().max(1).min(86_400);
    secs as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tight() -> Arc<LoginRateLimiter> {
        LoginRateLimiter::new(LoginRateLimiterSettings {
            ip_window: Duration::seconds(3600),
            ip_max: 3,
            email_window: Duration::seconds(3600),
            email_max_fail: 3,
        })
    }

    #[tokio::test]
    async fn ip_limit_blocks_after_max() {
        let lim = tight();
        lim.check_ip("1.2.3.4").await.unwrap();
        lim.check_ip("1.2.3.4").await.unwrap();
        lim.check_ip("1.2.3.4").await.unwrap();
        let result = lim.check_ip("1.2.3.4").await;
        assert!(result.is_err(), "4th attempt must be rate-limited");
        assert_eq!(
            result.unwrap_err().status,
            axum::http::StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test]
    async fn email_failures_reset_on_clear() {
        let lim = tight();
        lim.record_email_failure("m@example.com").await.unwrap();
        lim.record_email_failure("m@example.com").await.unwrap();
        lim.clear_email_failures("m@example.com").await;
        lim.record_email_failure("m@example.com").await.unwrap();
        lim.record_email_failure("m@example.com").await.unwrap();
        lim.record_email_failure("m@example.com").await.unwrap();
        assert!(lim.record_email_failure("m@example.com").await.is_err());
    }

    #[tokio::test]
    async fn disabled_never_blocks() {
        let lim = LoginRateLimiter::disabled();
        for _ in 0..50 {
            lim.check_ip("9.9.9.9").await.unwrap();
            lim.record_email_failure("x@y.z").await.unwrap();
        }
    }
}
