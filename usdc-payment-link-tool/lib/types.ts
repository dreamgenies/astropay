export type Merchant = {
  id: string;
  email: string;
  business_name: string;
  stellar_public_key: string;
  settlement_public_key: string;
  created_at: string;
};

export type InvoiceStatus = 'pending' | 'paid' | 'expired' | 'settled' | 'failed';

export type Invoice = {
  id: string;
  public_id: string;
  merchant_id: string;
  description: string;
  amount_cents: number;
  currency: string;
  asset_code: string;
  asset_issuer: string;
  destination_public_key: string;
  memo: string;
  status: InvoiceStatus;
  gross_amount_cents: number;
  platform_fee_cents: number;
  net_amount_cents: number;
  expires_at: string;
  paid_at: string | null;
  settled_at: string | null;
  transaction_hash: string | null;
  settlement_hash: string | null;
  checkout_url: string | null;
  qr_data_url: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type DashboardInvoice = Invoice & {
  business_name: string;
};

export type PayoutStatus = 'queued' | 'submitted' | 'settled' | 'failed' | 'dead_lettered';

export type Payout = {
  id: string;
  invoice_id: string;
  merchant_id: string;
  destination_public_key: string;
  amount_cents: number;
  asset_code: string;
  asset_issuer: string;
  status: PayoutStatus;
  transaction_hash: string | null;
  failure_reason: string | null;
  failure_count: number;
  last_failure_at: string | null;
  last_failure_reason: string | null;
  created_at: string;
  updated_at: string;
};
