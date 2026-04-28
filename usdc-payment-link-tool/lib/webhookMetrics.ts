/**
 * Webhook-to-invoice correlation metrics (AP-162).
 *
 * Tracks how often webhook deliveries resolve invoices versus producing
 * misses, duplicates, or mismatches. All counters are persisted in the
 * `webhook_correlation_metrics` table so operators can query them directly
 * or expose them via the metrics endpoint.
 *
 * Label vocabulary follows the spec in docs/observability/invoice-payout-lifecycle-metrics-spec.md.
 * Never include invoice_id, public_id, transaction hash, wallet address, or secrets.
 */

import { query } from '@/db';

export type WebhookOutcome =
  | 'resolved'      // invoice was pending → paid
  | 'duplicate'     // invoice already paid/settled; no mutation
  | 'stale'         // invoice expired or in terminal state; no mutation
  | 'miss'          // invoice not found for the given publicId
  | 'mismatch'      // delivery rejected due to replay-window duplicate (X-Delivery-Id)
  | 'auth_error'    // unauthorized delivery
  | 'error';        // unexpected DB or runtime failure

/**
 * Increments the webhook correlation counter for the given outcome.
 * Errors are swallowed so a metrics write never breaks the webhook path.
 */
export async function recordWebhookOutcome(outcome: WebhookOutcome): Promise<void> {
  try {
    await query(
      `INSERT INTO webhook_correlation_metrics (outcome, count, window_start)
       VALUES ($1, 1, date_trunc('hour', NOW()))
       ON CONFLICT (outcome, window_start)
       DO UPDATE SET count = webhook_correlation_metrics.count + 1,
                     updated_at = NOW()`,
      [outcome],
    );
  } catch {
    /* never let a metrics write break the webhook path */
  }
}

/**
 * Returns aggregated webhook outcome counts for the given look-back window.
 * Defaults to the last 24 hours.
 */
export async function getWebhookCorrelationSummary(windowHours = 24): Promise<{
  windowHours: number;
  totals: Record<WebhookOutcome, number>;
  resolutionRate: number | null;
}> {
  const result = await query<{ outcome: string; total: string }>(
    `SELECT outcome, SUM(count)::text AS total
     FROM webhook_correlation_metrics
     WHERE window_start >= NOW() - ($1 * INTERVAL '1 hour')
     GROUP BY outcome`,
    [windowHours],
  );

  const totals: Record<WebhookOutcome, number> = {
    resolved: 0,
    duplicate: 0,
    stale: 0,
    miss: 0,
    mismatch: 0,
    auth_error: 0,
    error: 0,
  };

  for (const row of result.rows) {
    const key = row.outcome as WebhookOutcome;
    if (key in totals) {
      totals[key] = parseInt(row.total, 10);
    }
  }

  const attempts = totals.resolved + totals.miss + totals.stale + totals.error;
  const resolutionRate = attempts > 0 ? totals.resolved / attempts : null;

  return { windowHours, totals, resolutionRate };
}
