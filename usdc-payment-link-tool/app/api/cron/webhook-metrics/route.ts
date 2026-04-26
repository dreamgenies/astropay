/**
 * GET /api/cron/webhook-metrics
 *
 * AP-162: Returns aggregated webhook-to-invoice correlation metrics so operators
 * can measure how often webhook deliveries resolve invoices versus producing
 * misses, duplicates, or mismatches.
 *
 * Query params:
 *   window_hours  — look-back window in hours (default 24, max 168)
 *
 * Requires Authorization: Bearer <CRON_SECRET>.
 */
import { fail, ok } from '@/lib/http';
import { env } from '@/lib/env';
import { getWebhookCorrelationSummary } from '@/lib/webhookMetrics';

function authorized(request: Request) {
  const auth = request.headers.get('authorization');
  const bearer = auth?.replace('Bearer ', '');
  return bearer && bearer === env.cronSecret;
}

export async function GET(request: Request) {
  if (!authorized(request)) return fail('Unauthorized', 401);

  const raw = new URL(request.url).searchParams.get('window_hours');
  const windowHours = Math.min(168, Math.max(1, Number(raw) || 24));

  const summary = await getWebhookCorrelationSummary(windowHours);

  return ok({
    metric: 'astropay_webhook_invoice_correlation',
    ...summary,
  });
}
