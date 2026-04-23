-- Raw webhook payloads stored on arrival for audit and debugging.
-- source: identifies the caller (e.g. 'stellar', 'internal').
-- headers: subset of HTTP headers forwarded by the caller (optional, nullable).

CREATE TABLE webhook_raw_payloads (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  source TEXT NOT NULL,
  payload JSONB NOT NULL,
  headers JSONB,
  received_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX webhook_raw_payloads_source_received_at_idx
  ON webhook_raw_payloads (source, received_at DESC);
