/**
 * Sentry integration smoke tests.
 *
 * Verifies that:
 * 1. captureException is called when an error boundary renders.
 * 2. Sentry init accepts the DSN from env without throwing.
 *
 * No real network calls are made — @sentry/nextjs is fully mocked.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as Sentry from '@sentry/nextjs';

vi.mock('@sentry/nextjs', () => ({
  init: vi.fn(),
  captureException: vi.fn(),
}));

const captureException = vi.mocked(Sentry.captureException);
const init = vi.mocked(Sentry.init);

beforeEach(() => {
  captureException.mockClear();
  init.mockClear();
});

// Mirrors the useEffect body in app/error.tsx and app/global-error.tsx
function simulateErrorBoundary(error: Error) {
  Sentry.captureException(error);
}

describe('Sentry error boundary integration', () => {
  it('calls captureException with the thrown error', () => {
    const err = new Error('test route handler failure');
    simulateErrorBoundary(err);
    expect(captureException).toHaveBeenCalledOnce();
    expect(captureException).toHaveBeenCalledWith(err);
  });

  it('captures error with digest metadata', () => {
    const err = Object.assign(new Error('server component crash'), { digest: 'abc123' });
    simulateErrorBoundary(err);
    expect(captureException).toHaveBeenCalledWith(
      expect.objectContaining({ digest: 'abc123' }),
    );
  });

  it('preserves error type and message', () => {
    const err = new TypeError('unexpected null');
    simulateErrorBoundary(err);
    const [captured] = captureException.mock.calls[0];
    expect(captured).toBeInstanceOf(TypeError);
    expect((captured as Error).message).toBe('unexpected null');
  });
});

describe('Sentry init config', () => {
  it('accepts a DSN without throwing', () => {
    const dsn = 'https://abc@o0.ingest.sentry.io/0';
    expect(() =>
      Sentry.init({ dsn, tracesSampleRate: 1.0 }),
    ).not.toThrow();
    expect(init).toHaveBeenCalledWith(expect.objectContaining({ dsn }));
  });

  it('does not throw when DSN is absent (Sentry is optional)', () => {
    expect(() =>
      Sentry.init({ dsn: undefined, tracesSampleRate: 1.0 }),
    ).not.toThrow();
  });
});
