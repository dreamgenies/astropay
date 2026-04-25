/**
 * Tests for log-redaction utilities (lib/redact.ts).
 *
 * Acceptance criteria (from issue backlog):
 * - Logs are useful for debugging but cannot leak secrets or high-risk identifiers.
 * - Error and edge-case handling is covered.
 */
import { describe, expect, it } from 'vitest';

import {
  redactCookieHeader,
  redactConnectionString,
  redactSecret,
  redactWalletKey,
} from '@/lib/redact';

// ── redactSecret (Authorization header) ──────────────────────────────────────

describe('redactSecret', () => {
  it('redacts the token portion of a Bearer header', () => {
    expect(redactSecret('Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig')).toBe('Bearer [REDACTED]');
  });

  it('fully redacts non-Bearer schemes', () => {
    expect(redactSecret('Token abc123')).toBe('[REDACTED]');
    expect(redactSecret('Basic dXNlcjpwYXNz')).toBe('[REDACTED]');
  });

  it('returns [REDACTED] for an empty string', () => {
    expect(redactSecret('')).toBe('[REDACTED]');
  });

  it('is case-sensitive — lowercase bearer is fully redacted', () => {
    // "bearer" (lowercase) is not the standard scheme; treat as opaque.
    expect(redactSecret('bearer secret')).toBe('[REDACTED]');
  });

  it('redacts a raw cron secret passed without a scheme', () => {
    expect(redactSecret('my-cron-secret-value')).toBe('[REDACTED]');
  });

  it('does not expose the token value in the output', () => {
    const token = 'super-secret-jwt-value';
    const result = redactSecret(`Bearer ${token}`);
    expect(result).not.toContain(token);
  });
});

// ── redactCookieHeader ────────────────────────────────────────────────────────

describe('redactCookieHeader', () => {
  it('redacts the session cookie value while preserving the name', () => {
    const input = 'astropay_session=eyJhbGciOiJIUzI1NiJ9.payload.sig';
    const output = redactCookieHeader(input);
    expect(output).toBe('astropay_session=[REDACTED]');
    expect(output).not.toContain('eyJ');
  });

  it('redacts all cookie values in a multi-cookie header', () => {
    const input = 'astropay_session=tok123; _ga=GA1.2.abc; other=val';
    const output = redactCookieHeader(input);
    expect(output).toContain('astropay_session=[REDACTED]');
    expect(output).toContain('_ga=[REDACTED]');
    expect(output).toContain('other=[REDACTED]');
    expect(output).not.toContain('tok123');
    expect(output).not.toContain('GA1.2.abc');
  });

  it('returns [REDACTED] for a bare flag cookie with no value', () => {
    expect(redactCookieHeader('bare-flag')).toBe('[REDACTED]');
  });

  it('returns [REDACTED] for an empty header', () => {
    expect(redactCookieHeader('')).toBe('[REDACTED]');
  });

  it('preserves cookie names so logs remain useful', () => {
    const output = redactCookieHeader('session=abc; csrf=xyz');
    expect(output).toContain('session=');
    expect(output).toContain('csrf=');
  });

  it('handles cookies with = signs in the value', () => {
    // Base64-encoded values contain '=' padding.
    const input = 'token=abc==; other=val';
    const output = redactCookieHeader(input);
    expect(output).toContain('token=[REDACTED]');
    expect(output).not.toContain('abc==');
  });
});

// ── redactConnectionString ────────────────────────────────────────────────────

describe('redactConnectionString', () => {
  it('redacts user:password from a postgres DSN', () => {
    const dsn = 'postgres://user:pass@localhost:5432/mydb';
    const out = redactConnectionString(dsn);
    expect(out).toBe('postgres://[REDACTED]@localhost:5432/mydb');
    expect(out).not.toContain('user');
    expect(out).not.toContain('pass');
  });

  it('leaves a DSN without credentials unchanged', () => {
    const dsn = 'postgres://localhost:5432/mydb';
    expect(redactConnectionString(dsn)).toBe(dsn);
  });

  it('returns [REDACTED] for a string without a scheme', () => {
    expect(redactConnectionString('not-a-dsn')).toBe('[REDACTED]');
    expect(redactConnectionString('')).toBe('[REDACTED]');
  });

  it('keeps host and database name visible after redaction', () => {
    const dsn = 'postgres://admin:hunter2@db.example.com:5432/astropay';
    const out = redactConnectionString(dsn);
    expect(out).toContain('db.example.com');
    expect(out).toContain('astropay');
    expect(out).not.toContain('hunter2');
    expect(out).not.toContain('admin');
  });

  it('handles DSNs with only a username (no password)', () => {
    const dsn = 'postgres://user@localhost/db';
    const out = redactConnectionString(dsn);
    expect(out).toBe('postgres://[REDACTED]@localhost/db');
    expect(out).not.toContain('user');
  });
});

// ── redactWalletKey ───────────────────────────────────────────────────────────

describe('redactWalletKey', () => {
  it('redacts a Stellar secret key (starts with S)', () => {
    const secretKey = 'SCZANGBA5RLBRQTV' + 'A'.repeat(40);
    expect(redactWalletKey(secretKey)).toBe('[REDACTED]');
  });

  it('does not redact a valid Stellar public key (starts with G, 56 chars)', () => {
    const publicKey = 'G' + 'A'.repeat(55);
    expect(redactWalletKey(publicKey)).toBe(publicKey);
  });

  it('redacts a short or malformed key', () => {
    expect(redactWalletKey('GABC')).toBe('[REDACTED]'); // too short
    expect(redactWalletKey('')).toBe('[REDACTED]');
  });

  it('redacts a key that starts with G but is not 56 chars', () => {
    expect(redactWalletKey('G' + 'A'.repeat(54))).toBe('[REDACTED]'); // 55 chars total
    expect(redactWalletKey('G' + 'A'.repeat(56))).toBe('[REDACTED]'); // 57 chars total
  });

  it('does not expose the secret key value in the output', () => {
    const key = 'SCZANGBA5RLBRQTV' + 'X'.repeat(40);
    expect(redactWalletKey(key)).not.toContain('SCZANGBA');
  });
});

// ── Integration: no secret leaks in realistic log scenarios ──────────────────

describe('redaction integration — realistic log scenarios', () => {
  it('a log object built from request headers does not leak secrets', () => {
    const authHeader = 'Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig';
    const cookieHeader = 'astropay_session=eyJhbGciOiJIUzI1NiJ9.payload.sig; _ga=GA1.2.abc';

    const safeLog = {
      auth: redactSecret(authHeader),
      cookie: redactCookieHeader(cookieHeader),
    };

    const serialized = JSON.stringify(safeLog);
    expect(serialized).not.toContain('eyJ');
    expect(serialized).toContain('[REDACTED]');
  });

  it('a startup log with DATABASE_URL does not leak credentials', () => {
    const databaseUrl = 'postgres://appuser:s3cr3t@prod-db.internal:5432/astropay';
    const safeLog = { database_url: redactConnectionString(databaseUrl) };
    const serialized = JSON.stringify(safeLog);
    expect(serialized).not.toContain('s3cr3t');
    expect(serialized).not.toContain('appuser');
    expect(serialized).toContain('prod-db.internal');
  });
});
