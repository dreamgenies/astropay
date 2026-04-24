'use client';

import type { CheckoutFailureView } from '@/lib/paymentFailure';

type Props = {
  view: CheckoutFailureView;
  onDismiss: () => void;
  onRetry?: () => void;
};

export function PaymentFailurePanel({ view, onDismiss, onRetry }: Props) {
  return (
    <div className="callout callout--danger stack" role="alert">
      <div className="row" style={{ justifyContent: 'space-between', alignItems: 'flex-start' }}>
        <h2 className="callout__title">{view.title}</h2>
        <button type="button" className="button secondary button--compact" onClick={onDismiss}>
          Dismiss
        </button>
      </div>
      <p className="callout__lead">{view.description}</p>
      {view.hints.length ? (
        <div>
          <p className="muted small" style={{ margin: '0 0 8px' }}>
            What you can try
          </p>
          <ul className="hint-list">
            {view.hints.map((h) => (
              <li key={h}>{h}</li>
            ))}
          </ul>
        </div>
      ) : null}
      {view.technical ? (
        <details className="technical-details">
          <summary className="muted small">Technical detail</summary>
          <pre className="mono small technical-details__pre">{view.technical}</pre>
        </details>
      ) : null}
      {view.retryLabel && onRetry ? (
        <button type="button" className="button" onClick={onRetry}>
          {view.retryLabel}
        </button>
      ) : null}
    </div>
  );
}
