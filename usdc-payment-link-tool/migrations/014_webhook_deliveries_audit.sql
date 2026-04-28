-- AP-203: Full audit table for webhook deliveries.
--
-- Extends the replay-detection approach in 011_webhook_deliveries.sql with
-- source identification, processing status, and replay metadata so operators
-- can inspect, debug, and replay any inbound webhook call.
--
-- Columns:
--   id            — surrogate PK for foreign-key and pagination use.
--   delivery_id   — caller-supplied idempotency key (unique; used for replay detection).
--   source        — identifies the caller, e.g. 'stellar', 'internal'.
--   status        — processing outcome: 'received' | 'processed' | 'failed' | 'duplicate'.
--   payload       — raw request body as JSONB.
--   headers       — subset of HTTP headers (nullable).
--   error_detail  — failure message when status = 'failed' (nullable).
--   invoice_id    — linked invoice when resolvable (nullable).
--   replay_of     — delivery_id of the original delivery this replays (nullable).
--   received_at   — wall-clock arrival time.
--   processed_at  — when processing completed (nullable).
--
-- Rollback:
--   DROP TABLE IF EXISTS webhook_deliveries_audit;
--   This table is an audit/observability store. Dropping it loses delivery history
--   but does not affect live webhook processing (replay detection uses
--   webhook_deliveries from 011, not this table).

CREATE TABLE IF NOT EXISTS webhook_deliveries_audit (
  id             UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
  delivery_id    TEXT    NOT NULL UNIQUE,
  source         TEXT    NOT NULL,
  status         TEXT    NOT NULL DEFAULT 'received'
                         CHECK (status IN ('received', 'processed', 'failed', 'duplicate')),
  payload        JSONB   NOT NULL DEFAULT '{}'::jsonb,
  headers        JSONB,
  error_detail   TEXT,
  invoice_id     UUID    REFERENCES invoices(id) ON DELETE SET NULL,
  replay_of      TEXT,
  received_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  processed_at   TIMESTAMPTZ
);

-- Hot path: look up recent deliveries by source for monitoring dashboards.
CREATE INDEX IF NOT EXISTS webhook_deliveries_audit_source_received_at_idx
  ON webhook_deliveries_audit (source, received_at DESC);

-- Operator query: find all failed deliveries for retry/replay.
CREATE INDEX IF NOT EXISTS webhook_deliveries_audit_status_received_at_idx
  ON webhook_deliveries_audit (status, received_at DESC);
