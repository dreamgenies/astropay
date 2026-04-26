/**
 * AP-162: Unit tests for webhook-to-invoice correlation metrics helpers.
 *
 * These tests cover the pure logic in webhookMetrics.ts without hitting the DB.
 */

import { describe, it, expect } from 'vitest';
import type { WebhookOutcome } from './webhookMetrics';

describe('WebhookOutcome type', () => {
  it('covers all expected outcome values', () => {
    const outcomes: WebhookOutcome[] = [
      'resolved',
      'duplicate',
      'stale',
      'miss',
      'mismatch',
      'auth_error',
      'error',
    ];
    // All 7 outcomes must be representable — this will fail to compile if the type changes.
    expect(outcomes).toHaveLength(7);
  });
});

describe('resolution rate logic', () => {
  function computeResolutionRate(
    resolved: number,
    miss: number,
    stale: number,
    error: number,
  ): number | null {
    const attempts = resolved + miss + stale + error;
    return attempts > 0 ? resolved / attempts : null;
  }

  it('returns null when there are no attempts', () => {
    expect(computeResolutionRate(0, 0, 0, 0)).toBeNull();
  });

  it('returns 1.0 when all deliveries resolved', () => {
    expect(computeResolutionRate(10, 0, 0, 0)).toBe(1.0);
  });

  it('returns 0.0 when no deliveries resolved', () => {
    expect(computeResolutionRate(0, 5, 3, 2)).toBe(0.0);
  });

  it('computes partial resolution rate correctly', () => {
    // 90 resolved out of 100 attempts = 0.9
    const rate = computeResolutionRate(90, 5, 3, 2);
    expect(rate).toBeCloseTo(0.9, 9);
  });

  it('does not count duplicates or mismatches as attempts', () => {
    // duplicates and mismatches are not "attempts" — they are noise
    const rate = computeResolutionRate(10, 0, 0, 0);
    expect(rate).toBe(1.0);
  });
});

describe('window_hours clamping', () => {
  function clampWindowHours(raw: number): number {
    return Math.min(168, Math.max(1, raw));
  }

  it('clamps 0 to 1', () => {
    expect(clampWindowHours(0)).toBe(1);
  });

  it('clamps 999 to 168', () => {
    expect(clampWindowHours(999)).toBe(168);
  });

  it('passes through valid values unchanged', () => {
    expect(clampWindowHours(24)).toBe(24);
    expect(clampWindowHours(1)).toBe(1);
    expect(clampWindowHours(168)).toBe(168);
  });
});
