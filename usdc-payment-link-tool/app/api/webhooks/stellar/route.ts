import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import {
  getInvoiceByPublicId,
  isTransactionHashAlreadyProcessed,
  markInvoicePaid,
  recordWebhookDelivery,
  type MarkInvoicePaidPayoutResult,
} from '@/lib/data';
import { recordWebhookOutcome } from '@/lib/webhookMetrics';

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
  if (!authorized(request)) {
    // AP-162: count auth failures so operators can detect misconfigured callers.
    await recordWebhookOutcome('auth_error');
    return fail('Unauthorized', 401);
  }

  // Issue #162: Replay detection — reject duplicate deliveries within the window.
  const deliveryId = request.headers.get('x-delivery-id');
  if (deliveryId) {
    const isNew = await recordWebhookDelivery(deliveryId, env.webhookReplayWindowSecs);
    if (!isNew) {
      // AP-162: replay-window duplicate — count as mismatch (not a resolved invoice).
      await recordWebhookOutcome('mismatch');
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
    // AP-162: already-processed hash → duplicate delivery.
    await recordWebhookOutcome('duplicate');
    return ok({ received: true, alreadyProcessed: true, transactionHash });
  }

  const invoice = await getInvoiceByPublicId(publicId);
  if (!invoice) {
    // AP-162: no invoice found for the publicId supplied by the caller.
    await recordWebhookOutcome('miss');
    return fail('Invoice not found', 404);
  }

  let payout: MarkInvoicePaidPayoutResult | undefined;
  if (invoice.status === 'pending') {
    try {
      payout = await markInvoicePaid({ invoiceId: invoice.id, transactionHash, payload: body });
      // AP-162: invoice was pending and is now paid — this is a successful resolution.
      await recordWebhookOutcome('resolved');
    } catch {
      await recordWebhookOutcome('error');
      throw;
    }
  } else if (invoice.status === 'expired' || invoice.status === 'failed') {
    // AP-162: invoice is in a terminal non-paid state; payment cannot be applied.
    await recordWebhookOutcome('stale');
  } else {
    // AP-162: invoice already paid or settled — duplicate delivery.
    await recordWebhookOutcome('duplicate');
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
