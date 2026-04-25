/// Log-redaction utilities for sensitive configuration values.
///
/// # Problem
/// Rust's `{:?}` (Debug) and `{}` (Display) formatting will happily print any
/// value that derives or implements those traits. A `Config` struct that derives
/// `Debug` would expose `session_secret`, `platform_treasury_secret_key`,
/// `cron_secret`, and the `database_url` (which embeds credentials) in every
/// log line that formats the config.
///
/// # Solution
/// `Redacted<T>` is a newtype wrapper that:
/// - Stores the inner value normally so it can be accessed via `.inner()`.
/// - Implements `Debug` and `Display` as the literal string `[REDACTED]`.
/// - Implements `Clone` when `T: Clone`.
///
/// Use `redact_log_value` for one-off string sanitisation (e.g. stripping a
/// bearer token from an ad-hoc log message).
///
/// # Patterns that are NOT covered
/// - Structured tracing fields passed as `field = value` — callers must not
///   pass raw secret strings as field values. Use `field = "[REDACTED]"` or
///   omit the field entirely.
/// - Sentry breadcrumbs / event data — see `sentry::configure_scope` if you
///   need to scrub data before it leaves the process.

use std::fmt;

/// A wrapper that hides its inner value from `Debug` and `Display` output.
///
/// ```rust
/// use rust_backend::redact::Redacted;
///
/// let secret = Redacted::new("super-secret-key".to_string());
/// assert_eq!(format!("{secret:?}"), "[REDACTED]");
/// assert_eq!(format!("{secret}"),   "[REDACTED]");
/// assert_eq!(secret.inner(), "super-secret-key");
/// ```
#[derive(Clone)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wrap a value so it is hidden from log output.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Access the inner value.
    pub fn inner(&self) -> &T {
        &self.0
    }

    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

// Allow `Redacted<String>` to be used wherever `&str` is expected.
impl<T: AsRef<str>> AsRef<str> for Redacted<T> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

/// Redact a bearer token from a raw `Authorization` header value.
///
/// Returns `"Bearer [REDACTED]"` when the value starts with `"Bearer "`,
/// otherwise returns `"[REDACTED]"` so the header is never logged verbatim.
///
/// ```rust
/// use rust_backend::redact::redact_authorization_header;
///
/// assert_eq!(redact_authorization_header("Bearer mysecret"), "Bearer [REDACTED]");
/// assert_eq!(redact_authorization_header("Token abc"),       "[REDACTED]");
/// assert_eq!(redact_authorization_header(""),                "[REDACTED]");
/// ```
pub fn redact_authorization_header(value: &str) -> &'static str {
    if value.starts_with("Bearer ") {
        "Bearer [REDACTED]"
    } else {
        "[REDACTED]"
    }
}

/// Redact a `Cookie` header string, replacing every cookie value with
/// `[REDACTED]` while preserving cookie names so logs remain useful.
///
/// Input:  `"astropay_session=eyJ...; other=val"`
/// Output: `"astropay_session=[REDACTED]; other=[REDACTED]"`
pub fn redact_cookie_header(header: &str) -> String {
    header
        .split(';')
        .map(|pair| {
            let pair = pair.trim();
            match pair.split_once('=') {
                Some((name, _value)) => format!("{}=[REDACTED]", name.trim()),
                None => "[REDACTED]".to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Redact a connection string / DSN, replacing the userinfo component
/// (`user:password@`) with `[REDACTED]@` so the host and database name
/// remain visible for debugging while credentials are hidden.
///
/// ```text
/// postgres://user:pass@host:5432/db  →  postgres://[REDACTED]@host:5432/db
/// postgres://host/db                 →  postgres://host/db   (unchanged — no credentials)
/// ```
pub fn redact_connection_string(dsn: &str) -> String {
    // Find the scheme separator "://"
    let Some(after_scheme) = dsn.find("://").map(|i| i + 3) else {
        return "[REDACTED]".to_string();
    };
    let rest = &dsn[after_scheme..];

    // If there is an '@' before the first '/', userinfo is present.
    let slash_pos = rest.find('/').unwrap_or(rest.len());
    let host_part = &rest[..slash_pos];

    if let Some(at_pos) = host_part.rfind('@') {
        let scheme = &dsn[..after_scheme];
        let host_and_db = &rest[at_pos + 1..];
        format!("{scheme}[REDACTED]@{host_and_db}")
    } else {
        // No credentials in the DSN — return as-is.
        dsn.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Redacted<T> ──────────────────────────────────────────────────────────

    #[test]
    fn redacted_debug_emits_placeholder() {
        let r = Redacted::new("super-secret".to_string());
        assert_eq!(format!("{r:?}"), "[REDACTED]");
    }

    #[test]
    fn redacted_display_emits_placeholder() {
        let r = Redacted::new("super-secret".to_string());
        assert_eq!(format!("{r}"), "[REDACTED]");
    }

    #[test]
    fn redacted_inner_returns_original_value() {
        let r = Redacted::new("my-secret".to_string());
        assert_eq!(r.inner(), "my-secret");
    }

    #[test]
    fn redacted_into_inner_consumes_wrapper() {
        let r = Redacted::new("key".to_string());
        assert_eq!(r.into_inner(), "key");
    }

    #[test]
    fn redacted_clone_preserves_inner_value() {
        let r = Redacted::new("cloneable".to_string());
        let r2 = r.clone();
        assert_eq!(r2.inner(), "cloneable");
    }

    #[test]
    fn redacted_as_ref_str_works() {
        let r = Redacted::new("hello".to_string());
        let s: &str = r.as_ref();
        assert_eq!(s, "hello");
    }

    #[test]
    fn redacted_struct_debug_hides_secret_field() {
        #[derive(Debug)]
        struct Config {
            name: String,
            secret: Redacted<String>,
        }
        let cfg = Config {
            name: "app".to_string(),
            secret: Redacted::new("s3cr3t".to_string()),
        };
        let output = format!("{cfg:?}");
        assert!(output.contains("name: \"app\""), "name should be visible");
        assert!(output.contains("[REDACTED]"), "secret must be redacted");
        assert!(!output.contains("s3cr3t"), "raw secret must not appear");
    }

    // ── redact_authorization_header ──────────────────────────────────────────

    #[test]
    fn bearer_token_is_redacted() {
        assert_eq!(
            redact_authorization_header("Bearer mysecret"),
            "Bearer [REDACTED]"
        );
    }

    #[test]
    fn non_bearer_scheme_is_fully_redacted() {
        assert_eq!(redact_authorization_header("Token abc"), "[REDACTED]");
    }

    #[test]
    fn empty_authorization_header_is_redacted() {
        assert_eq!(redact_authorization_header(""), "[REDACTED]");
    }

    #[test]
    fn bearer_prefix_case_sensitive() {
        // "bearer" (lowercase) is NOT the standard scheme — treat as opaque.
        assert_eq!(redact_authorization_header("bearer secret"), "[REDACTED]");
    }

    // ── redact_cookie_header ─────────────────────────────────────────────────

    #[test]
    fn session_cookie_value_is_redacted() {
        let input = "astropay_session=eyJhbGciOiJIUzI1NiJ9.payload.sig";
        let output = redact_cookie_header(input);
        assert_eq!(output, "astropay_session=[REDACTED]");
        assert!(!output.contains("eyJ"), "JWT must not appear in output");
    }

    #[test]
    fn multiple_cookies_all_values_redacted() {
        let input = "astropay_session=tok123; _ga=GA1.2.abc; other=val";
        let output = redact_cookie_header(input);
        assert!(output.contains("astropay_session=[REDACTED]"));
        assert!(output.contains("_ga=[REDACTED]"));
        assert!(output.contains("other=[REDACTED]"));
        assert!(!output.contains("tok123"));
        assert!(!output.contains("GA1.2.abc"));
    }

    #[test]
    fn cookie_without_value_is_redacted() {
        let output = redact_cookie_header("bare-flag");
        assert_eq!(output, "[REDACTED]");
    }

    #[test]
    fn empty_cookie_header_returns_redacted() {
        let output = redact_cookie_header("");
        assert_eq!(output, "[REDACTED]");
    }

    // ── redact_connection_string ─────────────────────────────────────────────

    #[test]
    fn postgres_dsn_with_credentials_is_redacted() {
        let dsn = "postgres://user:pass@localhost:5432/mydb";
        let out = redact_connection_string(dsn);
        assert_eq!(out, "postgres://[REDACTED]@localhost:5432/mydb");
        assert!(!out.contains("user"), "username must not appear");
        assert!(!out.contains("pass"), "password must not appear");
    }

    #[test]
    fn postgres_dsn_without_credentials_is_unchanged() {
        let dsn = "postgres://localhost:5432/mydb";
        assert_eq!(redact_connection_string(dsn), dsn);
    }

    #[test]
    fn dsn_without_scheme_returns_redacted_placeholder() {
        assert_eq!(redact_connection_string("not-a-dsn"), "[REDACTED]");
    }

    #[test]
    fn dsn_host_and_db_remain_visible_after_redaction() {
        let dsn = "postgres://admin:hunter2@db.example.com:5432/astropay";
        let out = redact_connection_string(dsn);
        assert!(out.contains("db.example.com"), "host must remain visible");
        assert!(out.contains("astropay"), "db name must remain visible");
        assert!(!out.contains("hunter2"), "password must be hidden");
        assert!(!out.contains("admin"), "username must be hidden");
    }
}
