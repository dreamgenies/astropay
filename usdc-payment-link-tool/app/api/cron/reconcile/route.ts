import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import { findPaymentForInvoice } from '@/lib/stellar';
import { markInvoiceExpired, markInvoicePaid, pendingInvoices, recordAssetMismatch, recordMemoMismatch, recordCronRun } from '@/lib/data';

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
    const invoices = await pendingInvoices({ limit: scanLimit, windowHours: scanWindowHours });
    // pendingInvoices() uses keyset pagination internally and returns the full
    // backlog regardless of size — no arbitrary cap.
    const invoices = await pendingInvoices();
    scanned = invoices.length;

    for (const invoice of invoices) {
      if (Date.now() > new Date(invoice.expires_at).getTime()) {
        if (!dryRun) await markInvoiceExpired(invoice.id);
        results.push({ publicId: invoice.public_id, action: 'expired' });
        continue;
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
      const result = await findPaymentForInvoice(invoice);
      if (result && 'assetMismatch' in result) {
        if (!dryRun) await recordAssetMismatch(invoice.id, result.assetMismatch);
        results.push({ publicId: invoice.public_id, action: 'asset_mismatch', ...result.assetMismatch });
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
