import Link from 'next/link';
import { notFound, redirect } from 'next/navigation';
import { getCurrentMerchant } from '@/lib/auth';
import { getMerchantInvoice } from '@/lib/data';
import { centsToUsd, isoToLocal } from '@/lib/format';
import { PendingSettlementBanner } from '@/components/PendingSettlementBanner';
import { CopyButton } from '@/components/CopyButton';

export default async function InvoiceDetailPage({ params }: { params: Promise<{ id: string }> }) {
  const merchant = await getCurrentMerchant();
  if (!merchant) redirect('/login');
  const { id } = await params;
  const invoice = await getMerchantInvoice(merchant.id, id);
  if (!invoice) notFound();

  return (
    <div className="grid two">
      <div className="card stack">
        <div className="badge">Invoice</div>
        <h1 style={{ margin: 0 }}>{invoice.description}</h1>
        <p className="muted">Status: <strong>{invoice.status}</strong></p>
        <PendingSettlementBanner status={invoice.status} />
        <p>Gross: <strong>{centsToUsd(invoice.gross_amount_cents)}</strong></p>
        <p>Platform fee: <strong>{centsToUsd(invoice.platform_fee_cents)}</strong></p>
        <p>Merchant net: <strong>{centsToUsd(invoice.net_amount_cents)}</strong></p>
        <div className="copy-row"><span className="muted small">Public ID:</span><span className="mono muted small">{invoice.public_id}</span><CopyButton value={invoice.public_id} /></div>
        {/* AP-003: collapsible metadata panel for technical / debugging details */}
        <details>
          <summary className="muted small" style={{ cursor: 'pointer' }}>Technical details</summary>
          <div className="stack" style={{ marginTop: '0.5rem' }}>
            <div className="copy-row"><span className="muted small">ID:</span><span className="mono muted small">{invoice.id}</span><CopyButton value={invoice.id} /></div>
            <div className="copy-row"><span className="muted">Memo:</span><span className="mono muted">{invoice.memo}</span><CopyButton value={invoice.memo} /></div>
            {invoice.checkout_url ? <div className="copy-row"><span className="muted">Public link:</span><span className="mono muted small">{invoice.checkout_url}</span><CopyButton value={invoice.checkout_url} /></div> : null}
          </div>
        </details>
        <p className="muted">Created: {isoToLocal(invoice.created_at)}</p>
        <p className="muted">Expires: {isoToLocal(invoice.expires_at)}</p>
        <div className="row">
          <a className="button" href={invoice.checkout_url || '#'} target="_blank">Open checkout</a>
          <Link className="button secondary" href="/dashboard">Back</Link>
        </div>
      </div>
      <div className="card stack">
        <div className="badge">QR</div>
        {invoice.qr_data_url ? <img src={invoice.qr_data_url} alt="Invoice QR code" width={280} height={280} /> : null}
      </div>
    </div>
  );
}
