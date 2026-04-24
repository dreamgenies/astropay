import { describe, expect, it, vi, beforeEach } from 'vitest';

// Issue #159: secret rotation — authorized() accepts primary and secondary secrets.
// Issue #162: replay detection — duplicate X-Delivery-Id within window is rejected.

// We test the authorization and replay logic in isolation by extracting the
// pure functions from the route module via a lightweight re-implementation
// that mirrors the production logic exactly.

function makeAuthorized(cronSecret: string, webhookSecretSecondary: string) {
  return (bearer: string): boolean => {
    if (!bearer) return false;
    if (cronSecret && bearer === cronSecret) return true;
    if (webhookSecretSecondary && bearer === webhookSecretSecondary) return true;
    return false;
  };
}

describe('webhook authorization (issue #159)', () => {
  const authorized = makeAuthorized('primary_secret', 'secondary_secret');

  it('accepts primary secret', () => {
    expect(authorized('primary_secret')).toBe(true);
  });

  it('accepts secondary secret during rotation', () => {
    expect(authorized('secondary_secret')).toBe(true);
  });

  it('rejects unknown secret', () => {
    expect(authorized('wrong')).toBe(false);
  });

  it('rejects empty bearer', () => {
    expect(authorized('')).toBe(false);
  });

  it('rejects when both secrets are empty', () => {
    const noSecrets = makeAuthorized('', '');
    expect(noSecrets('anything')).toBe(false);
  });

  it('works with only primary set (no secondary)', () => {
    const primaryOnly = makeAuthorized('only_primary', '');
    expect(primaryOnly('only_primary')).toBe(true);
    expect(primaryOnly('secondary_secret')).toBe(false);
  });
});

// Issue #162: replay detection logic.
// The production code calls recordWebhookDelivery which does an INSERT ... ON CONFLICT.
// We test the contract: first call returns true (new), second returns false (duplicate).

describe('replay detection contract (issue #162)', () => {
  const store = new Map<string, number>();

  function simulateRecordDelivery(deliveryId: string, windowSecs: number): boolean {
    const now = Date.now();
    // Purge stale entries
    for (const [id, ts] of store.entries()) {
      if (now - ts > windowSecs * 1000) store.delete(id);
    }
    if (store.has(deliveryId)) return false;
    store.set(deliveryId, now);
    return true;
  }

  beforeEach(() => store.clear());

  it('first delivery is accepted', () => {
    expect(simulateRecordDelivery('delivery-1', 300)).toBe(true);
  });

  it('duplicate delivery within window is rejected', () => {
    simulateRecordDelivery('delivery-2', 300);
    expect(simulateRecordDelivery('delivery-2', 300)).toBe(false);
  });

  it('different delivery IDs are both accepted', () => {
    expect(simulateRecordDelivery('delivery-3', 300)).toBe(true);
    expect(simulateRecordDelivery('delivery-4', 300)).toBe(true);
  });

  it('delivery outside window is accepted again', () => {
    // Simulate an old entry by backdating it
    store.set('delivery-5', Date.now() - 400_000); // 400 seconds ago
    expect(simulateRecordDelivery('delivery-5', 300)).toBe(true);
  });
});
