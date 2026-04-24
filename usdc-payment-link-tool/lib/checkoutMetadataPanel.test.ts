import { describe, expect, it } from 'vitest';

describe('AP-003 collapsible metadata panel fields', () => {
  const alwaysVisibleFields = ['description', 'gross_amount_cents', 'expires_at'];
  const collapsibleFields = ['memo', 'destination_public_key'];

  it('keeps memo and destination out of the always-visible field set', () => {
    for (const field of collapsibleFields) {
      expect(alwaysVisibleFields).not.toContain(field);
    }
  });

  it('includes memo and destination in the collapsible field set', () => {
    expect(collapsibleFields).toContain('memo');
    expect(collapsibleFields).toContain('destination_public_key');
  });
});