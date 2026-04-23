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
  /** Max payouts processed per settle cron run. Defaults to 50. */
  settleBatchSize: Number(get('SETTLE_BATCH_SIZE', '50')),
  nextPublicStellarNetwork: get('NEXT_PUBLIC_STELLAR_NETWORK', get('STELLAR_NETWORK', 'TESTNET')),
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
