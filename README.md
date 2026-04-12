# ASTROpay

Stripe for USDC on Stellar.

ASTROpay helps merchants create USDC payment links, serve hosted checkout pages, manage invoices, reconcile payments, and split platform fees on Stellar.

## Features

- Payment links
- Hosted checkout
- Invoices
- Reconciliation
- Fee splitting

## Tech Stack

- Next.js
- PostgreSQL
- Stellar SDK
- Freighter wallet

## Project Structure

- `usdc-payment-link-tool/` — Next.js app and current product UI
- `rust-backend/` — Rust backend migration and service foundation

## Local Setup

### Next.js app

```bash
cd usdc-payment-link-tool
cp .env.example .env.local
npm install
npm run db:migrate
npm run dev
```

### Rust backend

```bash
cd rust-backend
cp .env.example .env.local
cargo check
cargo run --bin migrate
cargo run
```

## Deployment

### Vercel

1. Import the repo into Vercel.
2. Set the required environment variables.
3. Attach PostgreSQL or connect an external Postgres instance.
4. Deploy the Next.js app from `usdc-payment-link-tool/`.

### Railway

1. Create a new Railway project.
2. Add PostgreSQL.
3. Deploy the app service from `usdc-payment-link-tool/`.
4. Set the required environment variables.
5. Run migrations before production traffic.

## Environment Variables

- `APP_URL`
- `NEXT_PUBLIC_APP_URL`
- `DATABASE_URL`
- `PGSSL`
- `SESSION_SECRET`
- `CRON_SECRET`
- `STELLAR_NETWORK`
- `NEXT_PUBLIC_STELLAR_NETWORK`
- `HORIZON_URL`
- `NETWORK_PASSPHRASE`
- `ASSET_CODE`
- `ASSET_ISSUER`
- `PLATFORM_TREASURY_PUBLIC_KEY`
- `PLATFORM_TREASURY_SECRET_KEY`
- `PLATFORM_FEE_BPS`
- `INVOICE_EXPIRY_HOURS`

## Screenshots

Add product screenshots here:

- Dashboard
- Hosted checkout
- Invoice detail
- Payment flow

## License

MIT
