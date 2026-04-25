'use client';

import * as Sentry from '@sentry/nextjs';
import { useEffect } from 'react';

export default function GlobalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    Sentry.captureException(error);
  }, [error]);

  return (
    <html lang="en">
      <body>
        <main style={{ padding: '2rem', textAlign: 'center' }}>
          <h2>Something went wrong</h2>
          <p className="muted small">
            {error.digest ? `Error ID: ${error.digest}` : 'An unexpected error occurred.'}
          </p>
          <button className="button" onClick={reset}>
            Try again
          </button>
        </main>
      </body>
    </html>
  );
}
