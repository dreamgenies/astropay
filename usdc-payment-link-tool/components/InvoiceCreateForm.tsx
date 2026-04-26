'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';

export function InvoiceCreateForm() {
  const router = useRouter();
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  async function submit(formData: FormData) {
    setLoading(true);
    setError('');
    const res = await fetch('/api/invoices', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-correlation-id': crypto.randomUUID()
      },
      body: JSON.stringify(Object.fromEntries(formData.entries())),
    });
    const data = await res.json();
    setLoading(false);
    if (!res.ok) return setError(data.error || 'Failed to create invoice');
    router.push(`/dashboard/invoices/${data.invoice.id}`);
    router.refresh();
  }

  return (
    <form action={submit} className="card stack">
      <h1>New invoice</h1>
      <label><span>Description</span><textarea name="description" className="input" required /></label>
      <label><span>Amount in USD</span><input type="number" step="0.01" min="0.01" name="amountUsd" className="input" required /></label>
      <p className="muted small">A platform fee is deducted from this amount. The payer sends the full gross amount; you receive the net.</p>
      {error ? <p className="error">{error}</p> : null}
      <button className="button" disabled={loading}>{loading ? 'Creating...' : 'Create invoice'}</button>
    </form>
  );
}
