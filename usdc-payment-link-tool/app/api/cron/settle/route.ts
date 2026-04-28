import { fail, ok } from '@/lib/http';
import { assertSettlementConfig, env } from '@/lib/env';
import { buildSettlementXdr, getServer } from '@/lib/stellar';
import {
  claimPayout,
  getInvoiceById,
  markPayoutFailed,
  markPayoutSubmitted,
  queuedPayouts,
  recordCronRun,
} from '@/lib/data';
import { isValidSettlementPublicKey } from '@/lib/stellarPublicKey';

/** Default number of queued payouts processed per settle run. Override with SETTLE_BATCH_SIZE env var. */
const DEFAULT_SETTLE_BATCH_SIZE = 50;

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function GET(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);

  const dryRun = new URL(request.url).searchParams.get('dry_run') === 'true';
  const batchSize = Math.max(1, Number(env.settleBatchSize) || DEFAULT_SETTLE_BATCH_SIZE);
  let processed = 0;
  const results: Array<Record<string, unknown>> = [];
  let success = true;
  let errorDetail: string | null = null;

  try {
    assertSettlementConfig();
    const payouts = await queuedPayouts(batchSize);
    processed = payouts.length;

    for (const payout of payouts) {
      // Claim the payout to prevent concurrent processing
      const claimed = await claimPayout(payout.id, 'settle-worker');
      if (!claimed) {
        results.push({
          payoutId: payout.id,
          action: 'skipped',
          reason: 'Payout already being processed by another worker',
        });
        continue;
      }

      if (!isValidSettlementPublicKey(payout.destination_public_key)) {
        const reason = 'Invalid destination stellar public key';
        if (!dryRun) await markPayoutFailed(payout.id, reason);
        results.push({
          payoutId: payout.id,
          action: 'failed',
          reason,
          failureCount: (payout.failure_count ?? 0) + 1,
        });
        continue;
      }

      try {
        const invoice = await getInvoiceById(payout.invoice_id_ref);
        if (!invoice || invoice.status !== 'paid') continue;

        if (dryRun) {
          results.push({ payoutId: payout.id, action: 'would_submit' });
          continue;
        }

        const tx = await buildSettlementXdr({ invoice, destination: payout.destination_public_key });
        const submission = await getServer().submitTransaction(tx);
        // Mark as submitted only — the reconcile cron confirms settlement once
        // the transaction is seen on-chain (AP-149).
        await markPayoutSubmitted(payout.id, submission.hash);
        results.push({ payoutId: payout.id, action: 'submitted', txHash: submission.hash });
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Settlement failed';
        if (!dryRun) await markPayoutFailed(payout.id, message);
        results.push({
          payoutId: payout.id,
          action: 'failed',
          reason: message,
          failureCount: (payout.failure_count ?? 0) + 1,
        });
      }
    }

    return ok({ dryRun, batchSize, processed, results });
  } catch (error) {
    success = false;
    errorDetail = error instanceof Error ? error.message : 'settle failed';
    return fail(errorDetail, 500);
  } finally {
    if (!dryRun) {
      await recordCronRun({
        jobType: 'settle',
        success,
        metadata: { batchSize, processed, results },
        errorDetail,
      });
    }
  }
}
