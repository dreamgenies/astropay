/**
 * Log-redaction utilities for the Next.js application layer.
 *
 * ## Problem
 * Several values in this codebase must never appear in logs, error messages,
 * or Sentry breadcrumbs:
 *
 * - Wallet / Stellar secret keys (`PLATFORM_TREASURY_SECRET_KEY`, merchant
 *   `settlement_public_key` when used as a signing key)
 * - Session tokens — the raw JWT stored in the `astropay_session` cookie
 * - Cookie header strings — contain the session token verbatim
 * - Bearer tokens — `CRON_SECRET` / `WEBHOOK_SECRET_SECONDARY` values sent
 *   in `Authorization` headers
 * - Database connection strings — embed `user:password@host`
 *
 * ## Usage
 *
 * ```ts
 * import { redactSecret, redactCookieHeader, redactConnectionString } from '@/lib/redact';
 *
 * // Safe to log — shows "Bearer [REDACTED]"
 * logger.info({ auth: redactSecret(req.headers.get('authorization') ?? '') });
 *
 * // Safe to log — shows "astropay_session=[REDACTED]"
 * logger.info({ cookie: redactCookieHeader(req.headers.get('cookie') ?? '') });
 * ```
 *
 * ## What this does NOT cover
 * - Sentry `beforeSend` scrubbing — configure that separately in `sentry.server.config.ts`
 *   using `Sentry.init({ beforeSend })` if you need to strip fields from captured events.
 * - Structured log fields passed as plain objects — callers must not pass raw
 *   secret values as field values. Use these helpers or omit the field.
 */

const REDACTED = '[REDACTED]';

/**
 * Redact a bearer token from an `Authorization` header value.
 *
 * - `"Bearer <token>"` → `"Bearer [REDACTED]"`
 * - Anything else      → `"[REDACTED]"`
 *
 * @example
 * redactSecret('Bearer eyJ...')  // → 'Bearer [REDACTED]'
 * redactSecret('Token abc')      // → '[REDACTED]'
 * redactSecret('')               // → '[REDACTED]'
 */
export function redactSecret(value: string): string {
  if (!value) return REDACTED;
  if (value.startsWith('Bearer ')) return `Bearer ${REDACTED}`;
  return REDACTED;
}

/**
 * Redact all cookie values in a `Cookie` header string while preserving
 * cookie names so logs remain useful for debugging session issues.
 *
 * Input:  `"astropay_session=eyJ...; _ga=GA1.2.abc"`
 * Output: `"astropay_session=[REDACTED]; _ga=[REDACTED]"`
 *
 * @example
 * redactCookieHeader('astropay_session=tok; other=val')
 * // → 'astropay_session=[REDACTED]; other=[REDACTED]'
 */
export function redactCookieHeader(header: string): string {
  if (!header) return REDACTED;
  return header
    .split(';')
    .map((pair) => {
      const trimmed = pair.trim();
      const eqIdx = trimmed.indexOf('=');
      if (eqIdx === -1) return REDACTED;
      const name = trimmed.slice(0, eqIdx).trim();
      return `${name}=${REDACTED}`;
    })
    .join('; ');
}

/**
 * Redact the userinfo component (`user:password@`) from a database connection
 * string / DSN, leaving the host and database name visible for debugging.
 *
 * ```
 * postgres://user:pass@host:5432/db  →  postgres://[REDACTED]@host:5432/db
 * postgres://host/db                 →  postgres://host/db   (no credentials — unchanged)
 * not-a-dsn                          →  [REDACTED]
 * ```
 */
export function redactConnectionString(dsn: string): string {
  if (!dsn) return REDACTED;

  const schemeEnd = dsn.indexOf('://');
  if (schemeEnd === -1) return REDACTED;

  const afterScheme = dsn.slice(schemeEnd + 3);
  const slashPos = afterScheme.indexOf('/');
  const hostPart = slashPos === -1 ? afterScheme : afterScheme.slice(0, slashPos);
  const atPos = hostPart.lastIndexOf('@');

  if (atPos === -1) {
    // No credentials present — safe to return as-is.
    return dsn;
  }

  const scheme = dsn.slice(0, schemeEnd + 3);
  const hostAndDb = afterScheme.slice(atPos + 1);
  return `${scheme}${REDACTED}@${hostAndDb}`;
}

/**
 * Redact a Stellar secret key (starts with `S`) or any other high-entropy
 * wallet key that must never appear in logs.
 *
 * Public keys (start with `G`) are NOT redacted — they are safe to log.
 *
 * @example
 * redactWalletKey('SCZANGBA5RLBRQTV...')  // → '[REDACTED]'
 * redactWalletKey('GABC...')              // → 'GABC...'  (public key — safe)
 */
export function redactWalletKey(key: string): string {
  if (!key) return REDACTED;
  // Stellar secret keys start with 'S'; public keys start with 'G'.
  // Redact anything that is not a public key.
  if (key.startsWith('G') && key.length === 56) return key;
  return REDACTED;
}
