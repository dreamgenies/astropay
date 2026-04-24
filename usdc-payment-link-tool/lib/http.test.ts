import { describe, expect, it } from 'vitest';

import { fail, ok } from '@/lib/http';

describe('HTTP response helpers', () => {
  it('serializes successful JSON responses', async () => {
    const response = ok({ ready: true }, { status: 201 });

    expect(response.status).toBe(201);
    expect(await response.json()).toEqual({ ready: true });
  });

  it('serializes error JSON responses with default status', async () => {
    const response = fail('Invalid payload');

    expect(response.status).toBe(400);
    expect(await response.json()).toEqual({ error: 'Invalid payload' });
  });

  it('serializes error JSON responses with custom status', async () => {
    const response = fail('Not found', 404);

    expect(response.status).toBe(404);
    expect(await response.json()).toEqual({ error: 'Not found' });
  });
});
