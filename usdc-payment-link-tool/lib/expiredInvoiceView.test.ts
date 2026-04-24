import { describe, expect, it } from 'vitest';

type InvoiceStatus = 'pending' | 'paid' | 'expired' | 'settled' | 'failed';

function resolveCheckoutRoute(status: InvoiceStatus): 'expired' | 'checkout' {
  if (status === 'expired') return 'expired';
  return 'checkout';
}

describe('AP-010 expired invoice routing', () => {
  it('routes expired invoices to the dedicated expired view', () => {
    expect(resolveCheckoutRoute('expired')).toBe('expired');
  });

  it('keeps active invoices on the checkout flow', () => {
    expect(resolveCheckoutRoute('pending')).toBe('checkout');
    expect(resolveCheckoutRoute('paid')).toBe('checkout');
    expect(resolveCheckoutRoute('settled')).toBe('checkout');
    expect(resolveCheckoutRoute('failed')).toBe('checkout');
  });
});