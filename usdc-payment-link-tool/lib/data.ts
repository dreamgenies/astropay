import { query, withTransaction } from '@/db';
import { env } from '@/lib/env';
import { createQrDataUrl, buildCheckoutUrl } from '@/lib/stellar';
import { generateMemo, generatePublicId } from '@/lib/security';
import { isValidSettlementPublicKey } from '@/lib/stellarPublicKey';
import type { Invoice, Merchant } from '@/lib/types';
import type { AssetMismatch } from '@/lib/stellar';

export type MarkInvoicePaidPayoutResult = {
  payoutQueued: boolean;
  payoutSkipReason: 'invalid_settlement_public_key' | 'payout_already_queued' | null;
};

export const findMerchantByEmail = async (email: string) => {
  const result = await query<(Merchant & { password_hash: string })>(
    'SELECT id, email, business_name, stellar_public_key, settlement_public_key, password_hash, created_at FROM merchants WHERE email = $1',
    [email.toLowerCase()],
  );
  return result.rows[0] || null;
};

export const createMerchant = async ({ email, passwordHash, businessName, stellarPublicKey, settlementPublicKey }: {
  email: string;
  passwordHash: string;
  businessName: string;
  stellarPublicKey: string;
  settlementPublicKey: string;
}) => {
  const result = await query<Merchant>(
    `INSERT INTO merchants (email, password_hash, business_name, stellar_public_key, settlement_public_key)
     VALUES ($1, $2, $3, $4, $5)
     RETURNING id, email, business_name, stellar_public_key, settlement_public_key, created_at`,
    [email.toLowerCase(), passwordHash, businessName, stellarPublicKey, settlementPublicKey],
  );
  return result.rows[0];
};

export const createInvoice = async ({ merchantId, description, amountCents }: {
  merchantId: string;
  description: string;
  amountCents: number;
}) => {
  const fee = Math.max(1, Math.round((amountCents * env.platformFeeBps) / 10_000));
  const gross = amountCents;
  const net = gross - fee;
  const publicId = generatePublicId();
  const memo = generateMemo();
  const expiresAt = new Date(Date.now() + env.invoiceExpiryHours * 60 * 60 * 1000).toISOString();

  const result = await query<Invoice>(
    `INSERT INTO invoices (
      public_id, merchant_id, description, amount_cents, gross_amount_cents, platform_fee_cents, net_amount_cents,
      currency, asset_code, asset_issuer, destination_public_key, memo, expires_at, metadata
    ) VALUES ($1,$2,$3,$4,$5,$6,$7,'USD',$8,$9,$10,$11,$12,$13)
    RETURNING *`,
    [
      publicId,
      merchantId,
      description,
      amountCents,
      gross,
      fee,
      net,
      env.assetCode,
      env.assetIssuer,
      env.platformTreasuryPublicKey,
      memo,
      expiresAt,
      JSON.stringify({ product: 'ASTROpay' }),
    ],
  );

  const invoice = result.rows[0];
  const qrDataUrl = await createQrDataUrl(invoice);
  const checkoutUrl = buildCheckoutUrl(invoice.public_id);
  const updated = await query<Invoice>(
    'UPDATE invoices SET qr_data_url = $2, checkout_url = $3 WHERE id = $1 RETURNING *',
    [invoice.id, qrDataUrl, checkoutUrl],
  );
  return updated.rows[0];
};

export const listMerchantInvoices = async (merchantId: string) => {
  const result = await query<Invoice>('SELECT * FROM invoices WHERE merchant_id = $1 ORDER BY created_at DESC LIMIT 100', [merchantId]);
  return result.rows;
};

export const getMerchantInvoice = async (merchantId: string, id: string) => {
  const result = await query<Invoice>('SELECT * FROM invoices WHERE merchant_id = $1 AND id = $2', [merchantId, id]);
  return result.rows[0] || null;
};

export const getInvoiceByPublicId = async (publicId: string) => {
  const result = await query<(Invoice & { business_name: string })>(
    `SELECT invoices.*, merchants.business_name
     FROM invoices JOIN merchants ON merchants.id = invoices.merchant_id
     WHERE public_id = $1`,
    [publicId],
  );
  return result.rows[0] || null;
};

export const getInvoiceById = async (id: string) => {
  const result = await query<Invoice>('SELECT * FROM invoices WHERE id = $1', [id]);
  return result.rows[0] || null;
};

export const markInvoicePaid = async ({ invoiceId, transactionHash, payload }: {
  invoiceId: string;
  transactionHash: string;
  payload: Record<string, unknown>;
}): Promise<MarkInvoicePaidPayoutResult> => {
  return withTransaction(async (client) => {
    const updated = await client.query(
      `UPDATE invoices
       SET status = 'paid', paid_at = NOW(), transaction_hash = $2, updated_at = NOW()
       WHERE id = $1 AND status = 'pending'`,
      [invoiceId, transactionHash],
    );
    if (updated.rowCount === 0) {
      return { payoutQueued: false, payoutSkipReason: null };
    }

    await client.query('INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)', [invoiceId, 'payment_detected', payload]);

    const settlement = await client.query<{ settlement_public_key: string }>(
      `SELECT m.settlement_public_key
       FROM merchants m
       INNER JOIN invoices i ON i.merchant_id = m.id
       WHERE i.id = $1`,
      [invoiceId],
    );
    const settlementKey = settlement.rows[0]?.settlement_public_key ?? '';

    if (!isValidSettlementPublicKey(settlementKey)) {
      await client.query('INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)', [
        invoiceId,
        'payout_skipped_invalid_destination',
        { reason: 'invalid_settlement_public_key' },
      ]);
      return { payoutQueued: false, payoutSkipReason: 'invalid_settlement_public_key' };
    }

    const ins = await client.query(
      `INSERT INTO payouts (invoice_id, merchant_id, destination_public_key, amount_cents, asset_code, asset_issuer)
       SELECT id, merchant_id, (SELECT settlement_public_key FROM merchants WHERE merchants.id = invoices.merchant_id), net_amount_cents, asset_code, asset_issuer
       FROM invoices WHERE id = $1
       ON CONFLICT (invoice_id) DO NOTHING`,
      [invoiceId],
    );
    if ((ins.rowCount ?? 0) > 0) {
      return { payoutQueued: true, payoutSkipReason: null };
    }
    return { payoutQueued: false, payoutSkipReason: 'payout_already_queued' };
  });
};

export const markInvoiceExpired = async (invoiceId: string) => {
  await query(`UPDATE invoices SET status = 'expired', updated_at = NOW() WHERE id = $1 AND status = 'pending'`, [invoiceId]);
};

export const pendingInvoices = async () => {
  const result = await query<Invoice>(`SELECT * FROM invoices WHERE status = 'pending' ORDER BY created_at ASC LIMIT 100`);
  return result.rows;
};

export const queuedPayouts = async (limit = 50) => {
  const result = await query<any>(
    `SELECT payouts.*, invoices.public_id, invoices.net_amount_cents, invoices.asset_code, invoices.asset_issuer, invoices.id as invoice_id_ref
     FROM payouts JOIN invoices ON invoices.id = payouts.invoice_id
     WHERE payouts.status IN ('queued','failed') ORDER BY payouts.created_at ASC LIMIT $1`,
    [limit],
  );
  return result.rows;
};

export const markPayoutSubmitted = async (payoutId: string, txHash: string) => {
  await query(`UPDATE payouts SET status = 'submitted', transaction_hash = $2, updated_at = NOW() WHERE id = $1`, [payoutId, txHash]);
};

export const markPayoutSettled = async (payoutId: string, invoiceId: string, txHash: string) => {
  await withTransaction(async (client) => {
    await client.query(`UPDATE payouts SET status = 'settled', transaction_hash = $2, updated_at = NOW() WHERE id = $1`, [payoutId, txHash]);
    await client.query(`UPDATE invoices SET status = 'settled', settled_at = NOW(), settlement_hash = $2, updated_at = NOW() WHERE id = $1`, [invoiceId, txHash]);
    await client.query('INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)', [invoiceId, 'merchant_settled', { txHash }]);
  });
};

export const markPayoutFailed = async (payoutId: string, reason: string) => {
  await query(`UPDATE payouts SET status = 'failed', failure_reason = $2, updated_at = NOW() WHERE id = $1`, [payoutId, reason.slice(0, 500)]);
};

export const recordAssetMismatch = async (invoiceId: string, mismatch: AssetMismatch) => {
  await query('INSERT INTO payment_events (invoice_id, event_type, payload) VALUES ($1, $2, $3)', [
    invoiceId,
    'payment_asset_mismatch',
    JSON.stringify(mismatch),
  ]);
};

/** Persists a reconcile/settle cron run for ops and debugging. Swallows DB errors so cron HTTP behavior is unchanged. */
export const recordCronRun = async ({
  jobType,
  success,
  metadata,
  errorDetail,
}: {
  jobType: 'reconcile' | 'settle' | 'purge_sessions';
  success: boolean;
  metadata: Record<string, unknown>;
  errorDetail?: string | null;
}) => {
  try {
    await query(
      `INSERT INTO cron_runs (job_type, started_at, finished_at, success, metadata, error_detail)
       VALUES ($1, NOW(), NOW(), $2, $3::jsonb, $4)`,
      [jobType, success, JSON.stringify(metadata), errorDetail ?? null],
    );
  } catch {
    /* ignore audit failures */
  }
};
