-- AP-162: Webhook-to-invoice correlation metrics table.
--
-- Stores hourly bucketed counts of webhook delivery outcomes so operators
-- can measure how often deliveries resolve invoices versus producing misses,
-- duplicates, or mismatches without scanning raw event logs.
--
-- Outcome vocabulary (matches lib/webhookMetrics.ts WebhookOutcome type):
--   resolved   — invoice was pending → paid
--   duplicate  — invoice already paid/settled; no mutation
--   stale      — invoice expired or in terminal state; no mutation
--   miss       — no invoice found for the publicId
--   mismatch   — replay-window duplicate (X-Delivery-Id rejected)
--   auth_error — unauthorized delivery
--   error      — unexpected DB or runtime failure
--
-- Rollback: DROP TABLE IF EXISTS webhook_correlation_metrics;

CREATE TABLE IF NOT EXISTS webhook_correlation_metrics (
  outcome      TEXT        NOT NULL,
  window_start TIMESTAMPTZ NOT NULL,
  count        BIGINT      NOT NULL DEFAULT 0,
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),

  CONSTRAINT webhook_correlation_metrics_pkey PRIMARY KEY (outcome, window_start),
  CONSTRAINT webhook_correlation_metrics_outcome_check
    CHECK (outcome IN ('resolved','duplicate','stale','miss','mismatch','auth_error','error'))
);

-- Index to support time-range queries used by getWebhookCorrelationSummary.
CREATE INDEX IF NOT EXISTS webhook_correlation_metrics_window_start_idx
  ON webhook_correlation_metrics (window_start DESC);

COMMENT ON TABLE webhook_correlation_metrics IS
  'AP-162: Hourly bucketed webhook delivery outcome counters for operator observability.';
