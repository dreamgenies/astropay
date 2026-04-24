const get = (name: string, fallback?: string) => process.env[name] || fallback || '';
const required = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required environment variable: ${name}`);
  return value;
};

export const env = {
  appUrl: get('APP_URL', 'http://localhost:3000'),
  publicAppUrl: get('NEXT_PUBLIC_APP_URL', get('APP_URL', 'http://localhost:3000')),
  databaseUrl: get('DATABASE_URL'),
  sessionSecret: get('SESSION_SECRET', 'dev-session-secret-change-me'),
  horizonUrl: get('HORIZON_URL', 'https://horizon-testnet.stellar.org'),
  networkPassphrase: get('NETWORK_PASSPHRASE', 'Test SDF Network ; September 2015'),
  stellarNetwork: get('STELLAR_NETWORK', 'TESTNET'),
  assetCode: get('ASSET_CODE', 'USDC'),
  assetIssuer: get('ASSET_ISSUER', ''),
  platformTreasuryPublicKey: get('PLATFORM_TREASURY_PUBLIC_KEY', ''),
  platformTreasurySecretKey: get('PLATFORM_TREASURY_SECRET_KEY', ''),
  platformFeeBps: Number(get('PLATFORM_FEE_BPS', '100')),
  invoiceExpiryHours: Number(get('INVOICE_EXPIRY_HOURS', '24')),
  cronSecret: get('CRON_SECRET', ''),
  /** Secondary webhook secret for zero-downtime rotation (issue #159). When set, both primary and secondary are accepted. */
  webhookSecretSecondary: get('WEBHOOK_SECRET_SECONDARY', ''),
  /** Replay detection window in seconds (issue #162). Deliveries with the same X-Delivery-Id within this window are rejected. Defaults to 300 (5 min). */
  webhookReplayWindowSecs: Number(get('WEBHOOK_REPLAY_WINDOW_SECS', '300')),
  /** Max payouts processed per settle cron run. Defaults to 50. */
  settleBatchSize: Number(get('SETTLE_BATCH_SIZE', '50')),
  nextPublicStellarNetwork: get('NEXT_PUBLIC_STELLAR_NETWORK', get('STELLAR_NETWORK', 'TESTNET')),
  /** Maximum pending invoices scanned per reconcile run. Defaults to 100. */
  reconcileScanLimit: Number(get('RECONCILE_SCAN_LIMIT', '100')),
  /**
   * When > 0, reconcile only considers invoices created within this many hours.
   * Set to 0 (default) to scan all pending invoices regardless of age.
   */
  reconcileScanWindowHours: Number(get('RECONCILE_SCAN_WINDOW_HOURS', '0')),
};

export const assertCoreConfig = () => {
  required('DATABASE_URL');
  required('SESSION_SECRET');
  required('ASSET_CODE');
  required('ASSET_ISSUER');
  required('PLATFORM_TREASURY_PUBLIC_KEY');
};

export const assertSettlementConfig = () => {
  assertCoreConfig();
  required('PLATFORM_TREASURY_SECRET_KEY');
};

export const hasSettlementSigning = () => Boolean(env.platformTreasurySecretKey);
