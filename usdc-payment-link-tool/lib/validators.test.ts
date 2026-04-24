import { describe, expect, it } from 'vitest';

import { invoiceSchema, loginSchema, registerSchema } from '@/lib/validators';

const VALID_STELLAR_KEY = 'GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF';

describe('validators', () => {
  it('accepts a valid invoice payload and coerces amountUsd', () => {
    const parsed = invoiceSchema.parse({
      description: 'Consulting retainer',
      amountUsd: '12.50',
    });
    expect(parsed).toEqual({
      description: 'Consulting retainer',
      amountUsd: 12.5,
    });
  });

  it('rejects invalid invoice amounts and descriptions', () => {
    expect(() => invoiceSchema.parse({ description: 'A', amountUsd: 10 })).toThrow();
    expect(() => invoiceSchema.parse({ description: 'Valid description', amountUsd: 0 })).toThrow();
    expect(() => invoiceSchema.parse({ description: 'Valid description', amountUsd: 1_000_001 })).toThrow();
  });

  it('accepts login payloads with valid email and password shape', () => {
    expect(loginSchema.parse({ email: 'merchant@example.com', password: 'password123' })).toEqual({
      email: 'merchant@example.com',
      password: 'password123',
    });
  });

  it('rejects malformed login payloads', () => {
    expect(() => loginSchema.parse({ email: 'not-email', password: 'password123' })).toThrow();
    expect(() => loginSchema.parse({ email: 'merchant@example.com', password: 'short' })).toThrow();
  });

  it('validates Stellar keys during merchant registration', () => {
    const parsed = registerSchema.parse({
      email: 'merchant@example.com',
      password: 'password123',
      businessName: 'Merchant Studio',
      stellarPublicKey: VALID_STELLAR_KEY,
      settlementPublicKey: ` ${VALID_STELLAR_KEY} `,
    });

    expect(parsed.stellarPublicKey).toBe(VALID_STELLAR_KEY);
    expect(parsed.settlementPublicKey).toBe(VALID_STELLAR_KEY);
  });

  it('rejects invalid Stellar keys during merchant registration', () => {
    expect(() =>
      registerSchema.parse({
        email: 'merchant@example.com',
        password: 'password123',
        businessName: 'Merchant Studio',
        stellarPublicKey: 'not-a-stellar-key',
        settlementPublicKey: VALID_STELLAR_KEY,
      }),
    ).toThrow();
  });
});
