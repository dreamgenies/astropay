import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import { findPaymentForInvoice } from '@/lib/stellar';
import { markInvoiceExpired, markInvoicePaid, pendingInvoices, recordCronRun } from '@/lib/data';

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function GET(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);
  let scanned = 0;
  const results: Array<Record<string, unknown>> = [];
  let success = true;
  let errorDetail: string | null = null;
  try {
    const invoices = await pendingInvoices();
    scanned = invoices.length;

    for (const invoice of invoices) {
      if (Date.now() > new Date(invoice.expires_at).getTime()) {
        await markInvoiceExpired(invoice.id);
        results.push({ publicId: invoice.public_id, action: 'expired' });
        continue;
      }
      const payment = await findPaymentForInvoice(invoice);
      if (payment) {
        await markInvoicePaid({ invoiceId: invoice.id, transactionHash: payment.hash, payload: payment.payment });
        results.push({ publicId: invoice.public_id, action: 'paid', txHash: payment.hash });
      } else {
        results.push({ publicId: invoice.public_id, action: 'pending' });
      }
    }
    const payment = await findPaymentForInvoice(invoice);
    if (payment) {
      const payout = await markInvoicePaid({
        invoiceId: invoice.id,
        transactionHash: payment.hash,
        payload: payment.payment,
      });
      results.push({
        publicId: invoice.public_id,
        action: 'paid',
        txHash: payment.hash,
        payoutQueued: payout.payoutQueued,
        payoutSkipReason: payout.payoutSkipReason,
      });
    } else {
      results.push({ publicId: invoice.public_id, action: 'pending' });
    }
  }

    return ok({ scanned, results });
  } catch (error) {
    success = false;
    errorDetail = error instanceof Error ? error.message : 'reconcile failed';
    return fail(errorDetail, 500);
  } finally {
    await recordCronRun({
      jobType: 'reconcile',
      success,
      metadata: { scanned, results },
      errorDetail,
    });
  }
}
