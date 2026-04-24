import { describe, expect, it } from 'vitest';

import { amountToStellar, centsToUsd, isoToLocal } from '@/lib/format';

describe('format utilities', () => {
  it('formats cents as USD currency', () => {
    expect(centsToUsd(0)).toBe('$0.00');
    expect(centsToUsd(123456)).toBe('$1,234.56');
    expect(centsToUsd(-99)).toBe('-$0.99');
  });

  it('formats cents as Stellar fixed precision amounts', () => {
    expect(amountToStellar(1)).toBe('0.01');
    expect(amountToStellar(1234)).toBe('12.34');
    expect(amountToStellar(1200)).toBe('12.00');
  });

  it('uses an em dash placeholder for missing timestamps', () => {
    expect(isoToLocal(null)).toBe('—');
    expect(isoToLocal(undefined)).toBe('—');
    expect(isoToLocal('')).toBe('—');
  });

  it('renders ISO timestamps as local date strings', () => {
    const rendered = isoToLocal('2026-04-24T12:34:00Z');
    expect(rendered).toContain('2026');
    expect(rendered).not.toBe('—');
  });
});
