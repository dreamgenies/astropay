import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import { findPaymentForInvoice, checkPayoutTxConfirmed } from '@/lib/stellar';
import {
  markInvoiceExpired,
  markInvoicePaid,
  markPayoutFailed,
  markPayoutSettled,
  pendingInvoices,
  recordAssetMismatch,
  recordMemoMismatch,
  recordAmountMismatch,
  recordCronRun,
  submittedPayouts,
} from '@/lib/data';

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function GET(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);

  const dryRun = new URL(request.url).searchParams.get('dry_run') === 'true';
  const scanLimit = env.reconcileScanLimit;
  const scanWindowHours = env.reconcileScanWindowHours;
  let scanned = 0;
  const results: Array<Record<string, unknown>> = [];
  let success = true;
  let errorDetail: string | null = null;

  try {
    // Step 1: Confirm any payouts that were submitted to Stellar but not yet settled.
    const submitted = await submittedPayouts();
    for (const payout of submitted) {
      const txStatus = await checkPayoutTxConfirmed(payout.transaction_hash as string);
      if (txStatus === 'confirmed') {
        if (!dryRun) await markPayoutSettled(payout.id, payout.invoice_id_ref, payout.transaction_hash as string);
        results.push({ payoutId: payout.id, action: 'payout_settled', txHash: payout.transaction_hash });
      } else if (txStatus === 'failed') {
        const reason = 'Settlement transaction failed on Stellar';
        if (!dryRun) await markPayoutFailed(payout.id, reason);
        results.push({ payoutId: payout.id, action: 'payout_failed', reason });
      }
      // txStatus === 'pending': not yet in Horizon, leave as submitted and check next run
    }

    // Step 2: Scan pending invoices for inbound buyer payments.
    const invoices = await pendingInvoices({ limit: scanLimit, windowHours: scanWindowHours });
    scanned = invoices.length;

    for (const invoice of invoices) {
      if (Date.now() > new Date(invoice.expires_at).getTime()) {
        if (!dryRun) await markInvoiceExpired(invoice.id);
        results.push({ publicId: invoice.public_id, action: 'expired' });
        continue;
      }

      const result = await findPaymentForInvoice(invoice);
      if (result && 'assetMismatch' in result) {
        const mismatch = result.assetMismatch;
        if (!mismatch) continue;
        if (!dryRun) await recordAssetMismatch(invoice.id, mismatch);
        results.push({ publicId: invoice.public_id, action: 'asset_mismatch', ...mismatch });
        continue;
      }
      if (result && 'amountMismatch' in result) {
        const mismatch = result.amountMismatch;
        if (!mismatch) continue;
        if (!dryRun) await recordAmountMismatch(invoice.id, mismatch);
        results.push({ publicId: invoice.public_id, action: 'amount_mismatch', ...mismatch });
        continue;
      }
      // Issue #172: destination + asset + amount match but memo is wrong/missing.
      if (result && 'memoMismatch' in result) {
        if (!dryRun) await recordMemoMismatch(invoice.id, result.memoMismatch);
        results.push({ publicId: invoice.public_id, action: 'memo_mismatch', ...result.memoMismatch });
        continue;
      }
      const payment = result;
      if (payment) {
        if (dryRun) {
          results.push({ publicId: invoice.public_id, action: 'paid', txHash: payment.hash });
        } else {
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
        }
      } else {
        results.push({ publicId: invoice.public_id, action: 'pending' });
      }
    }

    return ok({
      dryRun,
      scanned,
      scanLimit,
      scanWindowHours: scanWindowHours > 0 ? scanWindowHours : null,
      results,
    });
  } catch (error) {
    success = false;
    errorDetail = error instanceof Error ? error.message : 'reconcile failed';
    return fail(errorDetail, 500);
  } finally {
    if (!dryRun) {
      await recordCronRun({
        jobType: 'reconcile',
        success,
        metadata: { scanned, scanLimit, scanWindowHours: scanWindowHours > 0 ? scanWindowHours : null, results },
        errorDetail,
      });
    }
  }
}

