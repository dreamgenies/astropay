'use client';

import { useEffect, useMemo, useState } from 'react';
import { isConnected, requestAccess, signTransaction } from '@stellar/freighter-api';
import { PendingSettlementBanner } from './PendingSettlementBanner';
import { PaymentFailurePanel } from '@/components/PaymentFailurePanel';
import { resolveCheckoutFailure, type CheckoutFailureStage } from '@/lib/paymentFailure';

type Props = { invoiceId: string; status: string };

type FailureState = { message: string; stage: CheckoutFailureStage };

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

  const failureView = useMemo(
    () => (failure ? resolveCheckoutFailure(failure.message, failure.stage) : null),
    [failure],
  );

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    async function poll() {
      const res = await fetch(`/api/invoices/${invoiceId}/status`, { cache: 'no-store' });
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
    try {
      const connected = await isConnected();
      if (!connected.isConnected) {
        setFailure({ message: 'Freighter is not connected in this browser.', stage: 'wallet' });
        return '';
      }
      const res = await requestAccess();
      if ('address' in res && res.address) {
        setAddress(res.address);
        setFailure(null);
        return res.address;
      }
      const message =
        'error' in res && res.error
          ? String((res as { error?: { message?: string } }).error?.message || res.error)
          : 'Unable to access Freighter';
      setFailure({ message, stage: 'wallet' });
      return '';
    } catch (e) {
      setFailure({
        message: e instanceof Error ? e.message : 'Unable to reach Freighter',
        stage: 'wallet',
      });
      return '';
    }
  }

  async function pay() {
    setLoading(true);
    setFailure(null);
    try {
      const payer = address || (await connect());
      if (!payer) {
        setLoading(false);
        return;
      }

      const buildRes = await fetch(`/api/invoices/${invoiceId}/checkout`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mode: 'build-xdr', payer }),
      });
      const buildData = await readJsonBody(buildRes);
      if (!buildRes.ok) {
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
        setFailure({ message: 'Checkout response was missing transaction data.', stage: 'build' });
        return;
      }

      let signedXdr = '';
      try {
        const signed = await signTransaction(xdr, { networkPassphrase: passphrase });
        signedXdr = 'signedTxXdr' in signed ? signed.signedTxXdr : '';
      } catch (e) {
        setFailure({
          message: e instanceof Error ? e.message : 'Freighter could not sign the transaction',
          stage: 'wallet',
        });
        return;
      }

      if (!signedXdr) {
        setFailure({ message: 'Freighter did not return a signed transaction', stage: 'wallet' });
        return;
      }

      const submitRes = await fetch(`/api/invoices/${invoiceId}/checkout`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mode: 'submit-xdr', signedXdr }),
      });
      const submitData = await readJsonBody(submitRes);
      if (!submitRes.ok) {
        setFailure({
          message: typeof submitData.error === 'string' ? submitData.error : 'Failed to submit transaction',
          stage: 'submit',
        });
        return;
      }

      setStatus('processing');
    } catch (e) {
      setFailure({
        message: e instanceof Error ? e.message : 'Payment failed',
        stage: 'build',
      });
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="card stack">
      <div className="badge">Freighter checkout</div>
      <p className="muted">
        Status: <strong>{status}</strong>
      </p>
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
          {loading ? 'Processing...' : 'Pay now'}
        </button>
      </div>
      <PendingSettlementBanner status={status} />
      {address ? (
        <p className="muted">
          Payer: <span className="mono">{address}</span>
        </p>
      ) : null}
      {failureView ? <PaymentFailurePanel view={failureView} onDismiss={() => setFailure(null)} /> : null}
    </div>
  );
}
