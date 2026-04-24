import { notFound } from 'next/navigation';
import { PayWithFreighter } from '@/components/PayWithFreighter';
import { CopyButton } from '@/components/CopyButton';
import { getInvoiceByPublicId } from '@/lib/data';
import { centsToUsd, isoToLocal } from '@/lib/format';

export default async function PayPage({ params }: { params: Promise<{ publicId: string }> }) {
  const { publicId } = await params;
  const invoice = await getInvoiceByPublicId(publicId);
  if (!invoice) notFound();

  // AP-010: show a purpose-built expired state instead of the generic checkout flow.
  if (invoice.status === 'expired') {
    return (
      <div className="grid two">
        <div className="card stack">
          <div className="merchant-brand">
            <span className="merchant-brand__name">{invoice.business_name}</span>
            <span className="badge">via ASTROpay</span>
          </div>
          <div className="badge">Invoice expired</div>
          <h1 style={{ margin: 0 }}>This invoice has expired</h1>
          <p className="muted">
            This payment link is no longer valid. Please contact{' '}
            <strong>{invoice.business_name}</strong> to request a new invoice.
          </p>
          <p className="muted small">Invoice: {invoice.description}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="grid two">
      <div className="card stack">
        <div className="merchant-brand">
          <span className="merchant-brand__name">{invoice.business_name}</span>
          <span className="badge">via ASTROpay</span>
        </div>
        <h1 style={{ margin: 0 }}>{invoice.description}</h1>
        <p>You pay: <strong>{centsToUsd(invoice.gross_amount_cents)} USDC</strong></p>
        {invoice.platform_fee_cents > 0 && (
          <p className="muted small">Includes {centsToUsd(invoice.platform_fee_cents)} platform fee — merchant receives {centsToUsd(invoice.net_amount_cents)}</p>
        )}
        <p className="muted small">Your payment goes to the ASTROpay treasury on Stellar and is settled to {invoice.business_name} after confirmation.</p>
        <p className="muted">Expires: {isoToLocal(invoice.expires_at)}</p>
        {/* AP-003: collapsible metadata panel — hidden by default to keep checkout clean */}
        <details>
          <summary className="muted small" style={{ cursor: 'pointer' }}>Payment details</summary>
          <div className="stack" style={{ marginTop: '0.5rem' }}>
            <div className="copy-row"><span className="muted">Memo:</span><span className="mono muted">{invoice.memo}</span><CopyButton value={invoice.memo} /></div>
            <div className="copy-row"><span className="muted">Destination:</span><span className="mono muted">{invoice.destination_public_key}</span><CopyButton value={invoice.destination_public_key} /></div>
          </div>
        </details>
      </div>
      <div className="stack">
        <div className="card stack">
          <div className="badge">QR checkout</div>
          {invoice.qr_data_url ? <img src={invoice.qr_data_url} alt="Invoice QR code" className="qr-img" /> : null}
        </div>
        <PayWithFreighter invoiceId={invoice.id} status={invoice.status} />
      </div>
    </div>
  );
}

