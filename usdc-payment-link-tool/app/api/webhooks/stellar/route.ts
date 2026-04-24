import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import {
  getInvoiceByPublicId,
  isTransactionHashAlreadyProcessed,
  markInvoicePaid,
  recordWebhookDelivery,
  type MarkInvoicePaidPayoutResult,
} from '@/lib/data';

// Issue #159: Accept primary secret (CRON_SECRET) and optional secondary
// (WEBHOOK_SECRET_SECONDARY) so secrets can be rotated without downtime.
// Rotate by: set WEBHOOK_SECRET_SECONDARY=<new>, deploy, update callers to
// use new secret, then promote new secret to CRON_SECRET and clear secondary.
function authorized(request: Request): boolean {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '') ?? '';
  if (!bearer) return false;
  if (env.cronSecret && bearer === env.cronSecret) return true;
  if (env.webhookSecretSecondary && bearer === env.webhookSecretSecondary) return true;
  return false;
}

export async function POST(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);

  // Issue #162: Replay detection — reject duplicate deliveries within the window.
  const deliveryId = request.headers.get('x-delivery-id');
  if (deliveryId) {
    const isNew = await recordWebhookDelivery(deliveryId, env.webhookReplayWindowSecs);
    if (!isNew) {
      return ok({ received: true, duplicate: true, deliveryId });
    }
  }

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
