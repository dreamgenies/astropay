import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import {
  getInvoiceByPublicId,
  isTransactionHashAlreadyProcessed,
  markInvoicePaid,
  type MarkInvoicePaidPayoutResult,
} from '@/lib/data';

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function POST(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);
  const body = await request.json();
  const publicId = String(body.publicId || '');
  const transactionHash = String(body.transactionHash || '');
  if (!publicId || !transactionHash) return fail('publicId and transactionHash are required');

  // Idempotency guard: if this transaction hash is already recorded, the
  // payment was already processed. Return success without mutating state.
  if (await isTransactionHashAlreadyProcessed(transactionHash)) {
    return ok({ received: true, alreadyProcessed: true, transactionHash });
  }

  const invoice = await getInvoiceByPublicId(publicId);
  if (!invoice) return fail('Invoice not found', 404);

  let payout: MarkInvoicePaidPayoutResult | undefined;
  if (invoice.status === 'pending') {
    payout = await markInvoicePaid({ invoiceId: invoice.id, transactionHash, payload: body });
  }

  return ok({
    received: true,
    invoiceId: invoice.id,
    status: invoice.status,
    ...(payout && {
      payoutQueued: payout.payoutQueued,
      payoutSkipReason: payout.payoutSkipReason,
    }),
  });
}
