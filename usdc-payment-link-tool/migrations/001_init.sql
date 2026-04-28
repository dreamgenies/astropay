-- Rollback notes:
--   This is the baseline migration. A full rollback means dropping all tables
--   created here in reverse dependency order:
--     DROP TABLE IF EXISTS payouts;
--     DROP TABLE IF EXISTS payment_events;
--     DROP TABLE IF EXISTS invoices;
--     DROP TABLE IF EXISTS sessions;
--     DROP TABLE IF EXISTS merchants;
--   The pgcrypto extension can be left in place; removing it is rarely necessary
--   and requires no active dependents:
--     DROP EXTENSION IF EXISTS pgcrypto;
--   WARNING: dropping these tables is destructive and irreversible in production.
--   Only proceed after confirming all dependent migrations have been rolled back first.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE merchants (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  business_name TEXT NOT NULL,
  stellar_public_key TEXT NOT NULL,
  settlement_public_key TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE sessions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  expires_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
-- Initial indexes; migration 002_session_expiry_indexes.sql refines these for expiry batching and merchant+expiry filters.
CREATE INDEX sessions_merchant_id_idx ON sessions (merchant_id);
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);

CREATE TABLE invoices (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  public_id TEXT NOT NULL UNIQUE,
  merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  description TEXT NOT NULL,
  amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
  currency TEXT NOT NULL DEFAULT 'USD',
  asset_code TEXT NOT NULL,
  asset_issuer TEXT NOT NULL,
  destination_public_key TEXT NOT NULL,
  memo TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','paid','expired','settled','failed')),
  gross_amount_cents INTEGER NOT NULL,
  platform_fee_cents INTEGER NOT NULL,
  net_amount_cents INTEGER NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  paid_at TIMESTAMPTZ,
  settled_at TIMESTAMPTZ,
  transaction_hash TEXT,
  settlement_hash TEXT,
  checkout_url TEXT,
  qr_data_url TEXT,
  -- JSONB: indexing strategy is documented in 003_invoice_metadata_jsonb_index_plan.sql (no automatic GIN).
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX invoices_merchant_id_idx ON invoices (merchant_id);
CREATE INDEX invoices_status_idx ON invoices (status);
CREATE INDEX invoices_expires_at_idx ON invoices (expires_at);

CREATE TABLE payment_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX payment_events_invoice_id_idx ON payment_events (invoice_id);

CREATE TABLE payouts (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  invoice_id UUID NOT NULL UNIQUE REFERENCES invoices(id) ON DELETE CASCADE,
  merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
  destination_public_key TEXT NOT NULL,
  amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
  asset_code TEXT NOT NULL,
  asset_issuer TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','submitted','settled','failed')),
  transaction_hash TEXT,
  failure_reason TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX payouts_status_idx ON payouts (status);
