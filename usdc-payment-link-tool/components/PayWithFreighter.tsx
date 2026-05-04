'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { isConnected, requestAccess, signTransaction } from '@stellar/freighter-api';
import { PendingSettlementBanner } from './PendingSettlementBanner';
import { PaymentFailurePanel } from '@/components/PaymentFailurePanel';
import { resolveCheckoutFailure, type CheckoutFailureStage } from '@/lib/paymentFailure';

type Props = { invoiceId: string; status: string };

type FailureState = { message: string; stage: CheckoutFailureStage };

/** Distinct progress states for the Freighter checkout flow. */
type TxStep =
  | 'idle'
  | 'connecting'    // requesting Freighter access
  | 'building'      // POST build-xdr in flight
  | 'signing'       // waiting for Freighter signature
  | 'submitting'    // POST submit-xdr in flight
  | 'confirmed'     // submission accepted; polling for on-chain confirmation
  | 'failed';       // terminal error

const TX_STEP_LABEL: Record<TxStep, string> = {
  idle: 'Pay now',
  connecting: 'Connecting…',
  building: 'Building transaction…',
  signing: 'Waiting for signature…',
  submitting: 'Submitting…',
  confirmed: 'Submitted — confirming…',
  failed: 'Failed',
};

async function readJsonBody(res: Response): Promise<Record<string, unknown>> {
  const text = await res.text();
  if (!text) return {};
  try {
    return JSON.parse(text) as Record<string, unknown>;
  } catch {
    return { error: text };
  }
}

export function PayWithFreighter({ invoiceId, status: initialStatus }: Props) {
  const [address, setAddress] = useState('');
  const [status, setStatus] = useState(initialStatus);
  const [failure, setFailure] = useState<FailureState | null>(null);
  const [loading, setLoading] = useState(false);
  const [txStep, setTxStep] = useState<TxStep>('idle');
  const inFlight = useRef(false);

  const failureView = useMemo(
    () => (failure ? resolveCheckoutFailure(failure.message, failure.stage) : null),
    [failure],
  );

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    async function poll() {
      const res = await fetch(`/api/invoices/${invoiceId}/status`, {
        cache: 'no-store',
        headers: {
          'x-correlation-id': crypto.randomUUID()
        }
      });
      const data = (await readJsonBody(res)) as { status?: string };
      if (res.ok && data.status) {
        setStatus(data.status);
        if (['pending', 'paid', 'processing'].includes(data.status)) timer = setTimeout(poll, 5000);
      }
    }
    poll();
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [invoiceId]);

  async function connect(): Promise<string> {
    setFailure(null);
    setTxStep('connecting');
    try {
      const connected = await isConnected();
      if (!connected.isConnected) {
        setTxStep('failed');
        setFailure({ message: 'Freighter is not connected in this browser.', stage: 'wallet' });
        return '';
      }
      const res = await requestAccess();
      if ('address' in res && res.address) {
        setAddress(res.address);
        setFailure(null);
        setTxStep('idle');
        return res.address;
      }
      const message =
        'error' in res && res.error
          ? String((res as { error?: { message?: string } }).error?.message || res.error)
          : 'Unable to access Freighter';
      setTxStep('failed');
      setFailure({ message, stage: 'wallet' });
      return '';
    } catch (e) {
      setTxStep('failed');
      setFailure({
        message: e instanceof Error ? e.message : 'Unable to reach Freighter',
        stage: 'wallet',
      });
      return '';
    }
  }

  async function pay() {
    // Synchronous ref guard prevents duplicate submissions from rapid/repeated clicks.
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setFailure(null);
    setTxStep('idle');
    try {
      const payer = address || (await connect());
      if (!payer) {
        return;
      }

      setTxStep('building');
      const buildRes = await fetch(`/api/invoices/${invoiceId}/checkout`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'x-correlation-id': crypto.randomUUID()
        },
        body: JSON.stringify({ mode: 'build-xdr', payer }),
      });
      const buildData = await readJsonBody(buildRes);
      if (!buildRes.ok) {
        setTxStep('failed');
        setFailure({
          message: typeof buildData.error === 'string' ? buildData.error : 'Failed to build transaction',
          stage: 'build',
        });
        return;
      }

      const xdr = typeof buildData.xdr === 'string' ? buildData.xdr : '';
      const passphrase =
        typeof buildData.networkPassphrase === 'string' ? buildData.networkPassphrase : '';
      if (!xdr || !passphrase) {
        setTxStep('failed');
        setFailure({ message: 'Checkout response was missing transaction data.', stage: 'build' });
        return;
      }

      setTxStep('signing');
      let signedXdr = '';
      try {
        const signed = await signTransaction(xdr, { networkPassphrase: passphrase });
        signedXdr = 'signedTxXdr' in signed ? signed.signedTxXdr : '';
      } catch (e) {
        setTxStep('failed');
        setFailure({
          message: e instanceof Error ? e.message : 'Freighter could not sign the transaction',
          stage: 'wallet',
        });
        return;
      }

      if (!signedXdr) {
        setTxStep('failed');
        setFailure({ message: 'Freighter did not return a signed transaction', stage: 'wallet' });
        return;
      }

      setTxStep('submitting');
      const submitRes = await fetch(`/api/invoices/${invoiceId}/checkout`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'x-correlation-id': crypto.randomUUID()
        },
        body: JSON.stringify({ mode: 'submit-xdr', signedXdr }),
      });
      const submitData = await readJsonBody(submitRes);
      if (!submitRes.ok) {
        setTxStep('failed');
        setFailure({
          message: typeof submitData.error === 'string' ? submitData.error : 'Failed to submit transaction',
          stage: 'submit',
        });
        return;
      }

      setTxStep('confirmed');
      setStatus('processing');
    } catch (e) {
      setTxStep('failed');
      setFailure({
        message: e instanceof Error ? e.message : 'Payment failed',
        stage: 'build',
      });
    } finally {
      inFlight.current = false;
      setLoading(false);
    }
  }

  return (
    <div className="card stack">
      <div className="badge">Freighter checkout</div>
      <p className="muted">
        Status: <strong>{status}</strong>
      </p>
      {txStep !== 'idle' && txStep !== 'failed' && (
        <p className="muted small">{TX_STEP_LABEL[txStep]}</p>
      )}
      <div className="row">
        <button type="button" className="button secondary" onClick={() => void connect()}>
          Connect Freighter
        </button>
        <button
          type="button"
          className="button"
          onClick={() => void pay()}
          disabled={loading || ['paid', 'settled', 'expired'].includes(status)}
        >
          {loading ? TX_STEP_LABEL[txStep] : 'Pay now'}
        </button>
      </div>
      {address ? (
        <p className="muted">
          Payer: <span className="mono">{address}</span>
        </p>
      ) : null}
      <PendingSettlementBanner status={status} />
      {failureView ? (
        <PaymentFailurePanel
          view={failureView}
          onDismiss={() => setFailure(null)}
          onRetry={failure?.stage === 'wallet' ? () => void connect() : () => void pay()}
        />
      ) : null}
    </div>
  );
}
