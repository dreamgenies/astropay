import QRCode from 'qrcode';
import { Asset, BASE_FEE, Horizon, Keypair, Memo, Operation, TransactionBuilder } from 'stellar-sdk';
import { env } from '@/lib/env';
import { amountToStellar } from '@/lib/format';
import type { Invoice } from '@/lib/types';

export const getServer = () => new Horizon.Server(env.horizonUrl);
export const getAsset = () => new Asset(env.assetCode, env.assetIssuer);
export const invoiceAmountToAsset = (invoice: Pick<Invoice, 'gross_amount_cents'>) => amountToStellar(invoice.gross_amount_cents);

export const buildCheckoutUrl = (publicId: string) => `${env.publicAppUrl}/pay/${publicId}`;

export const buildPaymentUri = (invoice: Invoice) => {
  const amount = encodeURIComponent(invoiceAmountToAsset(invoice));
  const destination = encodeURIComponent(invoice.destination_public_key);
  const assetCode = encodeURIComponent(invoice.asset_code);
  const issuer = encodeURIComponent(invoice.asset_issuer);
  const memo = encodeURIComponent(invoice.memo);
  return `web+stellar:pay?destination=${destination}&amount=${amount}&asset_code=${assetCode}&asset_issuer=${issuer}&memo=${memo}&memo_type=text`;
};

export const createQrDataUrl = async (invoice: Invoice) => QRCode.toDataURL(buildCheckoutUrl(invoice.public_id), { width: 280, margin: 1 });

export const buildBuyerPaymentXdr = async (payerPublicKey: string, invoice: Invoice) => {
  const server = getServer();
  const source = await server.loadAccount(payerPublicKey);
  const tx = new TransactionBuilder(source, {
    fee: String(Number(BASE_FEE) * 10),
    networkPassphrase: env.networkPassphrase,
  })
    .addOperation(Operation.payment({
      destination: invoice.destination_public_key,
      asset: getAsset(),
      amount: invoiceAmountToAsset(invoice),
    }))
    .addMemo(Memo.text(invoice.memo))
    .setTimeout(180)
    .build();

  return tx.toXDR();
};

export const submitSignedXdr = async (signedXdr: string) => {
  const tx = TransactionBuilder.fromXDR(signedXdr, env.networkPassphrase);
  return getServer().submitTransaction(tx);
};

export type AssetMismatch = {
  hash: string;
  receivedAssetCode: string;
  receivedAssetIssuer: string;
  expectedAssetCode: string;
  expectedAssetIssuer: string;
  amount: string;
};

export const findPaymentForInvoice = async (
  invoice: Invoice,
): Promise<{ hash: string; payment: any; memo: string; assetMismatch?: never } | { assetMismatch: AssetMismatch } | null> => {
  const expectedAmount = invoiceAmountToAsset(invoice);
  const page = await getServer().payments().forAccount(invoice.destination_public_key).order('desc').limit(50).call();
  for (const record of page.records as any[]) {
    if (record.type !== 'payment') continue;
    if ((record.to || record.account) !== invoice.destination_public_key) continue;
    const amountMatches = Number(record.amount).toFixed(2) === expectedAmount;
    const assetMatches = record.asset_code === invoice.asset_code && record.asset_issuer === invoice.asset_issuer;
    if (amountMatches && !assetMatches) {
      return {
        assetMismatch: {
          hash: record.transaction_hash,
          receivedAssetCode: record.asset_code ?? '',
          receivedAssetIssuer: record.asset_issuer ?? '',
          expectedAssetCode: invoice.asset_code,
          expectedAssetIssuer: invoice.asset_issuer,
          amount: expectedAmount,
        },
      };
    }
    if (!amountMatches || !assetMatches) continue;
    const tx = await getServer().transactions().transaction(record.transaction_hash).call();
    if (tx.memo === invoice.memo) {
      return { hash: record.transaction_hash, payment: record, memo: tx.memo };
    }
  }
  return null;
};

/** Maximum bytes for a Stellar text memo (protocol limit). */
export const SETTLEMENT_MEMO_MAX_BYTES = 28;

/**
 * Builds a deterministic settlement memo: `s:<publicId>` truncated to 28 bytes.
 * The `s:` prefix distinguishes settlement transactions from buyer payment memos (`astro_*`).
 * Stellar text memos are limited to 28 bytes; excess characters are silently truncated.
 */
export const buildSettlementMemo = (publicId: string): string =>
  `s:${publicId}`.slice(0, SETTLEMENT_MEMO_MAX_BYTES);

/**
 * Checks whether a submitted Stellar transaction has been confirmed on-chain.
 *
 * Returns:
 *   'confirmed' — transaction succeeded on Stellar
 *   'failed'    — transaction was included in a ledger but failed
 *   'pending'   — transaction not yet found in Horizon (still propagating)
 *
 * Throws on unexpected network errors so callers can surface them.
 */
export const checkPayoutTxConfirmed = async (txHash: string): Promise<'confirmed' | 'failed' | 'pending'> => {
  try {
    const tx = await getServer().transactions().transaction(txHash).call();
    return tx.successful ? 'confirmed' : 'failed';
  } catch (err: any) {
    if (err?.response?.status === 404) return 'pending';
    throw err;
  }
};

export const buildSettlementXdr = async ({ invoice, destination }: { invoice: Invoice; destination: string }) => {
  if (!env.platformTreasurySecretKey) throw new Error('Settlement signing key is missing');
  const server = getServer();
  const treasury = Keypair.fromSecret(env.platformTreasurySecretKey);
  const source = await server.loadAccount(treasury.publicKey());
  const memo = buildSettlementMemo(invoice.public_id);
  const tx = new TransactionBuilder(source, {
    fee: String(Number(BASE_FEE) * 10),
    networkPassphrase: env.networkPassphrase,
  })
    .addOperation(Operation.payment({
      destination,
      asset: getAsset(),
      amount: (invoice.net_amount_cents / 100).toFixed(2),
    }))
    .addMemo(Memo.text(memo))
    .setTimeout(180)
    .build();
  tx.sign(treasury);
  return tx;
};
