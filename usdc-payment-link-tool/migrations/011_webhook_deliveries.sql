-- Issue #162: Replay detection window for webhook deliveries.
-- Stores a delivery_id per webhook call; duplicates within the replay window
-- are rejected before any invoice mutation occurs.
--
-- Issue #159: Webhook secret rotation support.
-- The application layer handles dual-secret validation; no schema change needed.
-- This table is used only for replay detection.

CREATE TABLE IF NOT EXISTS webhook_deliveries (
  delivery_id TEXT PRIMARY KEY,
  received_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Partial index: only rows within the replay window matter for duplicate checks.
-- Rows older than the window are dead weight; a periodic purge job can DELETE
-- WHERE received_at < NOW() - INTERVAL '5 minutes'.
CREATE INDEX IF NOT EXISTS webhook_deliveries_received_at_idx
  ON webhook_deliveries (received_at);
