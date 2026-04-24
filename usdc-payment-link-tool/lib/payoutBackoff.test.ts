/**
 * Tests for the payout retry backoff schedule.
 *
 * The backoff windows must match the SQL CASE expression in queuedPayouts
 * (data.ts) and the Rust backoff_seconds() function in settle.rs:
 *
 *   failure_count = 1 → 5 minutes
 *   failure_count = 2 → 15 minutes
 *   failure_count = 3 → 1 hour
 *   failure_count = 4 → 4 hours
 *   failure_count ≥ 5 → dead-lettered (no retry)
 */
import { describe, expect, it } from 'vitest';

// ── Pure backoff helpers (mirrors Rust settle.rs) ────────────────────────────

const BACKOFF_SCHEDULE: Record<number, number> = {
  1: 5 * 60,        // 5 minutes in seconds
  2: 15 * 60,       // 15 minutes
  3: 60 * 60,       // 1 hour
  4: 4 * 60 * 60,   // 4 hours
};

/** Returns the backoff delay in seconds for a given failure_count, or null if dead-lettered. */
function backoffSeconds(failureCount: number): number | null {
  return BACKOFF_SCHEDULE[failureCount] ?? null;
}

/** Returns true when the backoff window has elapsed. */
function isBackoffElapsed(failureCount: number, lastFailureMs: number, nowMs: number): boolean {
  const delaySecs = backoffSeconds(failureCount);
  if (delaySecs === null) return false;
  return nowMs >= lastFailureMs + delaySecs * 1000;
}

// ── backoffSeconds ────────────────────────────────────────────────────────────

describe('backoffSeconds', () => {
  it('returns 5 minutes for failure_count=1', () => {
    expect(backoffSeconds(1)).toBe(5 * 60);
  });

  it('returns 15 minutes for failure_count=2', () => {
    expect(backoffSeconds(2)).toBe(15 * 60);
  });

  it('returns 1 hour for failure_count=3', () => {
    expect(backoffSeconds(3)).toBe(60 * 60);
  });

  it('returns 4 hours for failure_count=4', () => {
    expect(backoffSeconds(4)).toBe(4 * 60 * 60);
  });

  it('returns null at dead-letter threshold (failure_count=5)', () => {
    expect(backoffSeconds(5)).toBeNull();
  });

  it('returns null for counts beyond threshold', () => {
    expect(backoffSeconds(6)).toBeNull();
    expect(backoffSeconds(100)).toBeNull();
  });

  it('windows are strictly increasing', () => {
    const windows = [1, 2, 3, 4].map((c) => backoffSeconds(c) as number);
    for (let i = 1; i < windows.length; i++) {
      expect(windows[i]).toBeGreaterThan(windows[i - 1]);
    }
  });
});

// ── isBackoffElapsed ──────────────────────────────────────────────────────────

describe('isBackoffElapsed', () => {
  const BASE_MS = 1_000_000_000_000; // arbitrary fixed timestamp

  it('returns false immediately after failure', () => {
    expect(isBackoffElapsed(1, BASE_MS, BASE_MS + 1000)).toBe(false);
  });

  it('returns false one millisecond before the window closes', () => {
    const delaySecs = backoffSeconds(1) as number;
    const nowMs = BASE_MS + delaySecs * 1000 - 1;
    expect(isBackoffElapsed(1, BASE_MS, nowMs)).toBe(false);
  });

  it('returns true exactly at the window boundary', () => {
    const delaySecs = backoffSeconds(1) as number;
    const nowMs = BASE_MS + delaySecs * 1000;
    expect(isBackoffElapsed(1, BASE_MS, nowMs)).toBe(true);
  });

  it('returns true well after the window has passed', () => {
    const delaySecs = backoffSeconds(2) as number;
    const nowMs = BASE_MS + delaySecs * 1000 + 99_999_999;
    expect(isBackoffElapsed(2, BASE_MS, nowMs)).toBe(true);
  });

  it('returns false for dead-letter count regardless of elapsed time', () => {
    expect(isBackoffElapsed(5, 0, Number.MAX_SAFE_INTEGER)).toBe(false);
  });

  it('each successive failure_count has a longer window', () => {
    const lastFailure = BASE_MS;
    // At the exact boundary of count=1, count=2 should still be blocked.
    const atCount1Boundary = lastFailure + (backoffSeconds(1) as number) * 1000;
    expect(isBackoffElapsed(1, lastFailure, atCount1Boundary)).toBe(true);
    expect(isBackoffElapsed(2, lastFailure, atCount1Boundary)).toBe(false);
  });
});
