import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('recordCheckoutAttempt', () => {
  let querySpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    querySpy = vi.fn().mockResolvedValue({ rows: [], rowCount: 1 });
    vi.doMock('@/db', () => ({
      query: querySpy,
      withTransaction: vi.fn(),
    }));
  });

  afterEach(() => {
    vi.resetModules();
    vi.restoreAllMocks();
  });

  it('stores a checkout attempt and refreshes the invoice attempt timestamp', async () => {
    const { recordCheckoutAttempt } = await import('@/lib/data');

    await recordCheckoutAttempt({
      invoiceId: 'invoice-1',
      action: 'build',
      status: 'succeeded',
      payload: { payer: 'GBPAYER' },
    });

    expect(querySpy).toHaveBeenCalledTimes(2);
    expect(querySpy.mock.calls[0][0]).toContain('INSERT INTO checkout_attempts');
    expect(querySpy.mock.calls[0][1]).toEqual([
      'invoice-1',
      'build',
      'succeeded',
      null,
      JSON.stringify({ payer: 'GBPAYER' }),
    ]);
    expect(querySpy.mock.calls[1][0]).toContain('last_checkout_attempt_at = NOW()');
    expect(querySpy.mock.calls[1][1]).toEqual(['invoice-1']);
  });

  it('does not throw when checkout audit storage fails', async () => {
    querySpy.mockRejectedValueOnce(new Error('missing checkout_attempts'));
    const { recordCheckoutAttempt } = await import('@/lib/data');

    await expect(
      recordCheckoutAttempt({
        invoiceId: 'invoice-1',
        action: 'submit',
        status: 'failed',
        errorDetail: 'Missing signed XDR',
      }),
    ).resolves.toBeUndefined();

    expect(querySpy).toHaveBeenCalledTimes(1);
  });
});
