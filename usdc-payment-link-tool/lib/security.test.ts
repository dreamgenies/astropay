import { describe, it, expect } from 'vitest';
import { generatePublicId, generateMemo } from './security';

const PUBLIC_ID_RE = /^inv_[0-9a-f]{16}$/;

describe('generatePublicId', () => {
  it('matches the DB constraint pattern inv_[0-9a-f]{16}', () => {
    for (let i = 0; i < 20; i++) {
      expect(generatePublicId()).toMatch(PUBLIC_ID_RE);
    }
  });

  it('is exactly 20 characters', () => {
    expect(generatePublicId()).toHaveLength(20);
  });

  it('produces unique values', () => {
    const ids = new Set(Array.from({ length: 50 }, generatePublicId));
    expect(ids.size).toBe(50);
  });
});

describe('generateMemo', () => {
  it('starts with astro_ and has 12 hex chars', () => {
    expect(generateMemo()).toMatch(/^astro_[0-9a-f]{12}$/);
  });
});
