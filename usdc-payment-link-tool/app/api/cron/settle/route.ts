import { fail, ok } from '@/lib/http';
import { assertSettlementConfig, env } from '@/lib/env';
import { buildSettlementXdr, getServer } from '@/lib/stellar';
import { getInvoiceById, markPayoutFailed, markPayoutSettled, markPayoutSubmitted, queuedPayouts, recordCronRun } from '@/lib/data';
import { isValidSettlementPublicKey } from '@/lib/stellarPublicKey';

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function GET(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);
  const dryRun = new URL(request.url).searchParams.get('dry_run') === 'true';
  let processed = 0;
  const results: Array<Record<string, unknown>> = [];
  let success = true;
  let errorDetail: string | null = null;
  try {
    assertSettlementConfig();
    const payouts = await queuedPayouts();
    processed = payouts.length;

    for (const payout of payouts) {
      if (!isValidSettlementPublicKey(payout.destination_public_key)) {
        const reason = 'Invalid destination stellar public key';
        if (!dryRun) await markPayoutFailed(payout.id, reason);
        results.push({ payoutId: payout.id, action: 'failed', reason });
        continue;
      }
      try {
        const invoice = await getInvoiceById(payout.invoice_id_ref);
        if (!invoice || invoice.status !== 'paid') continue;
        if (dryRun) {
          results.push({ payoutId: payout.id, action: 'would_settle' });
          continue;
        }
        const tx = await buildSettlementXdr({ invoice, destination: payout.destination_public_key });
        const submission = await getServer().submitTransaction(tx);
        await markPayoutSubmitted(payout.id, submission.hash);
        await markPayoutSettled(payout.id, payout.invoice_id_ref, submission.hash);
        results.push({ payoutId: payout.id, action: 'settled', txHash: submission.hash });
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Settlement failed';
        if (!dryRun) await markPayoutFailed(payout.id, message);
        results.push({ payoutId: payout.id, action: 'failed', reason: message });
      }
    }

    return ok({ dryRun, processed, results });
  } catch (error) {
    success = false;
    errorDetail = error instanceof Error ? error.message : 'settle failed';
    return fail(errorDetail, 500);
  } finally {
    if (!dryRun) {
      await recordCronRun({
        jobType: 'settle',
        success,
        metadata: { processed, results },
        errorDetail,
      });
    }
  }
}
