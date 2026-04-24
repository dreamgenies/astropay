import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import { findPaymentForInvoice } from '@/lib/stellar';
import { getInvoiceByPublicId, markInvoiceExpired, markInvoicePaid } from '@/lib/data';

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function POST(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);

  const body = await request.json();
  const publicId = String(body.publicId || '').trim();
  const dryRun = body.dry_run === true;

  if (!publicId) return fail('publicId is required', 400);

  const invoice = await getInvoiceByPublicId(publicId);
  if (!invoice) return fail(`Invoice '${publicId}' not found`, 404);

  if (invoice.status !== 'pending') {
    return ok({
      dryRun,
      publicId: invoice.public_id,
      action: 'skipped',
      reason: `invoice status is '${invoice.status}', only 'pending' invoices can be replayed`,
    });
  }

  if (Date.now() > new Date(invoice.expires_at).getTime()) {
    if (!dryRun) await markInvoiceExpired(invoice.id);
    return ok({ dryRun, publicId: invoice.public_id, action: 'expired' });
  }

  const payment = await findPaymentForInvoice(invoice);
  if (!payment) {
    return ok({ dryRun, publicId: invoice.public_id, action: 'pending' });
  }

  if (dryRun) {
    return ok({
      dryRun: true,
      publicId: invoice.public_id,
      action: 'paid',
      txHash: payment.hash,
    });
  }

  const payout = await markInvoicePaid({
    invoiceId: invoice.id,
    transactionHash: payment.hash,
    payload: payment.payment,
  });

  return ok({
    dryRun: false,
    publicId: invoice.public_id,
    action: 'paid',
    txHash: payment.hash,
    payoutQueued: payout.payoutQueued,
    payoutSkipReason: payout.payoutSkipReason,
  });
}
