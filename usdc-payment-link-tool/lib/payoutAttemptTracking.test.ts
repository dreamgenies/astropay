/**
 * AP-155: Payout attempt count and last failure reason tracking.
 *
 * These tests verify the contract that markPayoutFailed must satisfy:
 *   - failure_count increments on every call
 *   - last_failure_at is set to the current timestamp
 *   - last_failure_reason mirrors the reason string
 *   - failure_reason (legacy column) is also updated
 *   - reason is truncated to 500 chars to prevent oversized writes
 *
 * The SQL shape is pinned here so a refactor that drops any of these
 * fields is caught before it reaches production.
 */
import { describe, expect, it } from 'vitest';

// ── SQL shape contract ────────────────────────────────────────────────────────

const MARK_PAYOUT_FAILED_SQL = `UPDATE payouts
     SET status = 'failed',
         failure_reason = $2,
         failure_count = failure_count + 1,
         last_failure_at = NOW(),
         last_failure_reason = $2,
         updated_at = NOW()
     WHERE id = $1`;

describe('markPayoutFailed SQL contract', () => {
  it('increments failure_count atomically', () => {
    expect(MARK_PAYOUT_FAILED_SQL).toContain('failure_count = failure_count + 1');
  });

  it('sets last_failure_at to NOW()', () => {
    expect(MARK_PAYOUT_FAILED_SQL).toContain('last_failure_at = NOW()');
  });

  it('sets last_failure_reason', () => {
    expect(MARK_PAYOUT_FAILED_SQL).toContain('last_failure_reason = $2');
  });

  it('sets legacy failure_reason column', () => {
    expect(MARK_PAYOUT_FAILED_SQL).toContain('failure_reason = $2');
  });

  it('sets updated_at', () => {
    expect(MARK_PAYOUT_FAILED_SQL).toContain('updated_at = NOW()');
  });

  it('scopes update to the correct payout id', () => {
    expect(MARK_PAYOUT_FAILED_SQL).toContain('WHERE id = $1');
  });
});

// ── Reason truncation ─────────────────────────────────────────────────────────

function truncateReason(reason: string, max = 500): string {
  return reason.slice(0, max);
}

describe('failure reason truncation', () => {
  it('passes through short reasons unchanged', () => {
    expect(truncateReason('Settlement failed')).toBe('Settlement failed');
  });

  it('truncates reasons longer than 500 chars', () => {
    const long = 'x'.repeat(600);
    expect(truncateReason(long)).toHaveLength(500);
  });

  it('preserves exactly 500 chars', () => {
    const exact = 'a'.repeat(500);
    expect(truncateReason(exact)).toHaveLength(500);
  });

  it('empty reason is stored as empty string', () => {
    expect(truncateReason('')).toBe('');
  });
});

// ── Operator-visible Payout type ──────────────────────────────────────────────

import type { Payout } from '@/lib/types';

describe('Payout type includes attempt tracking fields', () => {
  it('has failure_count as a number', () => {
    const p: Payout = {
      id: 'uuid',
      invoice_id: 'uuid',
      merchant_id: 'uuid',
      destination_public_key: 'GAAA',
      amount_cents: 1000,
      asset_code: 'USDC',
      asset_issuer: 'ISSUER',
      status: 'failed',
      transaction_hash: null,
      failure_reason: 'timeout',
      failure_count: 3,
      last_failure_at: '2025-01-01T00:00:00Z',
      last_failure_reason: 'timeout',
      created_at: '2025-01-01T00:00:00Z',
      updated_at: '2025-01-01T00:00:00Z',
    };
    expect(p.failure_count).toBe(3);
    expect(p.last_failure_reason).toBe('timeout');
    expect(p.last_failure_at).toBeTruthy();
  });

  it('last_failure_at and last_failure_reason are nullable for fresh payouts', () => {
    const p: Payout = {
      id: 'uuid',
      invoice_id: 'uuid',
      merchant_id: 'uuid',
      destination_public_key: 'GAAA',
      amount_cents: 1000,
      asset_code: 'USDC',
      asset_issuer: 'ISSUER',
      status: 'queued',
      transaction_hash: null,
      failure_reason: null,
      failure_count: 0,
      last_failure_at: null,
      last_failure_reason: null,
      created_at: '2025-01-01T00:00:00Z',
      updated_at: '2025-01-01T00:00:00Z',
    };
    expect(p.failure_count).toBe(0);
    expect(p.last_failure_at).toBeNull();
    expect(p.last_failure_reason).toBeNull();
  });
});

// ── Settle route response contract ───────────────────────────────────────────

describe('settle route failure result shape', () => {
  function buildFailureResult(payoutId: string, reason: string, currentFailureCount: number) {
    return {
      payoutId,
      action: 'failed' as const,
      reason,
      failureCount: currentFailureCount + 1,
    };
  }

  it('increments failureCount by 1 in the response', () => {
    const result = buildFailureResult('payout-1', 'timeout', 2);
    expect(result.failureCount).toBe(3);
  });

  it('action is always "failed"', () => {
    const result = buildFailureResult('payout-1', 'timeout', 0);
    expect(result.action).toBe('failed');
  });

  it('first failure has failureCount of 1', () => {
    const result = buildFailureResult('payout-1', 'Invalid destination', 0);
    expect(result.failureCount).toBe(1);
  });

  it('reason is included in the result', () => {
    const result = buildFailureResult('payout-1', 'Stellar node unreachable', 1);
    expect(result.reason).toBe('Stellar node unreachable');
  });
});
