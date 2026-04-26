import { describe, expect, it, vi, beforeEach } from 'vitest';
import { buildSettlementMemo, SETTLEMENT_MEMO_MAX_BYTES } from '@/lib/stellar';

// --- Issue #157: settlement memo strategy ---

describe('buildSettlementMemo', () => {
  it('prefixes memo with s:', () => {
    expect(buildSettlementMemo('inv_abc123')).toBe('s:inv_abc123');
  });

  it('truncates to 28 bytes', () => {
    const longId = 'a'.repeat(40);
    const memo = buildSettlementMemo(longId);
    expect(memo.length).toBe(SETTLEMENT_MEMO_MAX_BYTES);
  });

  it('does not truncate short ids', () => {
    const memo = buildSettlementMemo('inv_short');
    expect(memo).toBe('s:inv_short');
    expect(memo.length).toBeLessThanOrEqual(SETTLEMENT_MEMO_MAX_BYTES);
  });

  it('SETTLEMENT_MEMO_MAX_BYTES is 28', () => {
    expect(SETTLEMENT_MEMO_MAX_BYTES).toBe(28);
  });
});

// --- Issue #171: asset mismatch detection ---

const BASE_INVOICE = {
  id: 'inv-id',
  public_id: 'inv_abc',
  merchant_id: 'merch-id',
  description: 'Test',
  amount_cents: 1000,
  currency: 'USD',
  asset_code: 'USDC',
  asset_issuer: 'ISSUER_A',
  destination_public_key: 'DEST_KEY',
  memo: 'astro_deadbeef',
  status: 'pending' as const,
  gross_amount_cents: 1000,
  platform_fee_cents: 10,
  net_amount_cents: 990,
  expires_at: new Date(Date.now() + 86400000).toISOString(),
  paid_at: null,
  settled_at: null,
  transaction_hash: null,
  settlement_hash: null,
  checkout_url: null,
  qr_data_url: null,
  metadata: {},
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
};

// Mock stellar-sdk server and env before importing findPaymentForInvoice
vi.mock('@/lib/env', () => ({
  env: {
    horizonUrl: 'https://horizon-testnet.stellar.org',
    networkPassphrase: 'Test SDF Network ; September 2015',
    assetCode: 'USDC',
    assetIssuer: 'ISSUER_A',
    platformTreasuryPublicKey: 'TREASURY',
    platformTreasurySecretKey: '',
    settleBatchSize: 50,
  },
}));

const mockTransactionCall = vi.fn();
const mockPaymentsCall = vi.fn();

vi.mock('stellar-sdk', async (importOriginal) => {
  const actual = await importOriginal<typeof import('stellar-sdk')>();
  return {
    ...actual,
    Horizon: {
      Server: vi.fn().mockImplementation(() => ({
        payments: () => ({
          forAccount: () => ({
            order: () => ({ limit: () => ({ call: mockPaymentsCall }) }),
          }),
        }),
        transactions: () => ({
          transaction: () => ({ call: mockTransactionCall }),
        }),
      })),
    },
  };
});

describe('findPaymentForInvoice', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('returns assetMismatch when amount matches but asset differs', async () => {
    mockPaymentsCall.mockResolvedValue({
      records: [
        {
          type: 'payment',
          to: 'DEST_KEY',
          asset_code: 'XLM',           // wrong asset
          asset_issuer: 'native',
          amount: '10.00',             // correct amount
          transaction_hash: 'tx_hash_1',
        },
      ],
    });

    const { findPaymentForInvoice } = await import('@/lib/stellar');
    const result = await findPaymentForInvoice(BASE_INVOICE);

    expect(result).not.toBeNull();
    expect(result).toHaveProperty('assetMismatch');
    if (result && 'assetMismatch' in result) {
      expect(result.assetMismatch.receivedAssetCode).toBe('XLM');
      expect(result.assetMismatch.expectedAssetCode).toBe('USDC');
      expect(result.assetMismatch.hash).toBe('tx_hash_1');
    }
  });

  it('returns amountMismatch when destination+asset+memo match but amount differs', async () => {
    mockPaymentsCall.mockResolvedValue({
      records: [
        {
          type: 'payment',
          to: 'DEST_KEY',
          asset_code: 'USDC',
          asset_issuer: 'ISSUER_A',
          amount: '11.00',           // wrong amount
          transaction_hash: 'tx_hash_5',
        },
      ],
    });
    mockTransactionCall.mockResolvedValue({ memo: 'astro_deadbeef' });

    const { findPaymentForInvoice } = await import('@/lib/stellar');
    const result = await findPaymentForInvoice(BASE_INVOICE);

    expect(result).not.toBeNull();
    expect(result).toHaveProperty('amountMismatch');
    if (result && 'amountMismatch' in result) {
      expect(result.amountMismatch.receivedAmount).toBe('11.00');
      expect(result.amountMismatch.expectedAmount).toBe('10.00');
      expect(result.amountMismatch.hash).toBe('tx_hash_5');
    }
  });

  it('returns memoMismatch when destination+asset+amount match but memo is wrong', async () => {
    mockPaymentsCall.mockResolvedValue({
      records: [
        {
          type: 'payment',
          to: 'DEST_KEY',
          asset_code: 'USDC',
          asset_issuer: 'ISSUER_A',
          amount: '10.00',
          transaction_hash: 'tx_hash_3',
        },
      ],
    });
    mockTransactionCall.mockResolvedValue({ memo: 'wrong_memo' });

    const { findPaymentForInvoice } = await import('@/lib/stellar');
    const result = await findPaymentForInvoice(BASE_INVOICE);

    expect(result).not.toBeNull();
    expect(result).toHaveProperty('memoMismatch');
    if (result && 'memoMismatch' in result) {
      expect(result.memoMismatch.receivedMemo).toBe('wrong_memo');
      expect(result.memoMismatch.expectedMemo).toBe('astro_deadbeef');
      expect(result.memoMismatch.hash).toBe('tx_hash_3');
    }
  });

  it('returns memoMismatch when memo is null/missing', async () => {
    mockPaymentsCall.mockResolvedValue({
      records: [
        {
          type: 'payment',
          to: 'DEST_KEY',
          asset_code: 'USDC',
          asset_issuer: 'ISSUER_A',
          amount: '10.00',
          transaction_hash: 'tx_hash_4',
        },
      ],
    });
    mockTransactionCall.mockResolvedValue({ memo: undefined });

    const { findPaymentForInvoice } = await import('@/lib/stellar');
    const result = await findPaymentForInvoice(BASE_INVOICE);

    expect(result).toHaveProperty('memoMismatch');
  });

  it('returns null when no matching payment found', async () => {
    mockPaymentsCall.mockResolvedValue({ records: [] });
    const { findPaymentForInvoice } = await import('@/lib/stellar');
    const result = await findPaymentForInvoice(BASE_INVOICE);
    expect(result).toBeNull();
  });

  it('returns payment match when asset and amount both match', async () => {
    mockPaymentsCall.mockResolvedValue({
      records: [
        {
          type: 'payment',
          to: 'DEST_KEY',
          asset_code: 'USDC',
          asset_issuer: 'ISSUER_A',
          amount: '10.00',
          transaction_hash: 'tx_hash_2',
        },
      ],
    });
    mockTransactionCall.mockResolvedValue({ memo: 'astro_deadbeef' });

    const { findPaymentForInvoice } = await import('@/lib/stellar');
    const result = await findPaymentForInvoice(BASE_INVOICE);

    expect(result).not.toBeNull();
    expect(result).not.toHaveProperty('assetMismatch');
    if (result && !('assetMismatch' in result)) {
      expect(result.hash).toBe('tx_hash_2');
    }
  });
});

// --- AP-149: checkPayoutTxConfirmed ---

describe('checkPayoutTxConfirmed', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('returns "confirmed" when Horizon reports a successful transaction', async () => {
    mockTransactionCall.mockResolvedValue({ successful: true });
    const { checkPayoutTxConfirmed } = await import('@/lib/stellar');
    expect(await checkPayoutTxConfirmed('hash_abc')).toBe('confirmed');
  });

  it('returns "failed" when Horizon reports an unsuccessful transaction', async () => {
    mockTransactionCall.mockResolvedValue({ successful: false });
    const { checkPayoutTxConfirmed } = await import('@/lib/stellar');
    expect(await checkPayoutTxConfirmed('hash_abc')).toBe('failed');
  });

  it('returns "pending" when Horizon returns 404', async () => {
    mockTransactionCall.mockRejectedValue({ response: { status: 404 } });
    const { checkPayoutTxConfirmed } = await import('@/lib/stellar');
    expect(await checkPayoutTxConfirmed('hash_abc')).toBe('pending');
  });

  it('re-throws unexpected network errors', async () => {
    mockTransactionCall.mockRejectedValue(new Error('Network timeout'));
    const { checkPayoutTxConfirmed } = await import('@/lib/stellar');
    await expect(checkPayoutTxConfirmed('hash_abc')).rejects.toThrow('Network timeout');
  });
});

