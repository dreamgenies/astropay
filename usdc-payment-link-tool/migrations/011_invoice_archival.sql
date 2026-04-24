-- migration 011_invoice_archival.sql
-- Create archive tables to keep the main tables lean and performant.

CREATE TABLE IF NOT EXISTS archived_invoices (
    id UUID PRIMARY KEY,
    public_id TEXT NOT NULL,
    merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    description TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    currency TEXT NOT NULL,
    asset_code TEXT NOT NULL,
    asset_issuer TEXT NOT NULL,
    destination_public_key TEXT NOT NULL,
    memo TEXT NOT NULL,
    status TEXT NOT NULL,
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
    last_checkout_attempt_at TIMESTAMPTZ,
    metadata JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS archived_invoices_merchant_id_idx ON archived_invoices (merchant_id);
CREATE INDEX IF NOT EXISTS archived_invoices_archived_at_idx ON archived_invoices (archived_at);

-- Archiving related payouts
CREATE TABLE IF NOT EXISTS archived_payouts (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES archived_invoices(id) ON DELETE CASCADE,
    merchant_id UUID NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    destination_public_key TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    asset_code TEXT NOT NULL,
    asset_issuer TEXT NOT NULL,
    status TEXT NOT NULL,
    transaction_hash TEXT,
    failure_reason TEXT,
    failure_count INTEGER,
    last_failure_at TIMESTAMPTZ,
    last_failure_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Archiving related payment events
CREATE TABLE IF NOT EXISTS archived_payment_events (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES archived_invoices(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS archived_payment_events_invoice_id_idx ON archived_payment_events (invoice_id);
