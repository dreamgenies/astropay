import { test, expect } from '@playwright/test';

test('paid invoice status after reconciliation e2e test', async ({ page, request }) => {
  // Mock Freighter wallet functions
  await page.addInitScript(() => {
    window.stellar = {
      freighter: {
        isConnected: async () => ({ isConnected: true }),
        requestAccess: async () => ({ address: 'GBTESTADDRESS1234567890123456789012345678901234567890' }),
        signTransaction: async (xdr: string) => ({ signedTxXdr: xdr + '_signed' }),
      },
    };
  });

  // Mock API responses
  await page.route('**/api/invoices/*/checkout', async (route) => {
    const request = route.request();
    const body = request.postDataJSON();
    if (body.mode === 'build-xdr') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          xdr: 'AAAA...mocked_xdr',
          networkPassphrase: 'Test SDF Network ; September 2015',
        }),
      });
    } else if (body.mode === 'submit-xdr') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ hash: 'mocked_tx_hash_123' }),
      });
    }
  });

  // Login as merchant
  await page.goto('/login');
  await page.fill('input[name="email"]', 'alice@demo.astropay.test');
  await page.fill('input[name="password"]', 'demo1234');
  await page.click('button[type="submit"]');
  await expect(page).toHaveURL('/dashboard');

  // Create invoice
  await page.goto('/dashboard/invoices/new');
  await page.fill('textarea[name="description"]', 'Test invoice for paid');
  await page.fill('input[name="amountUsd"]', '10.00');
  await page.click('button[type="submit"]');
  await expect(page).toHaveURL(/\/dashboard\/invoices\/[a-f0-9-]+/);

  // Get invoice id from URL
  const url = page.url();
  const invoiceId = url.split('/').pop();

  // Get public_id from DB
  const { Pool } = require('pg');
  const pool = new Pool({
    connectionString: process.env.DATABASE_URL || 'postgres://postgres:postgres@localhost:5432/astropay_test'
  });
  const client = await pool.connect();
  const result = await client.query('SELECT public_id FROM invoices WHERE id = $1', [invoiceId]);
  const publicId = result.rows[0].public_id;
  client.release();
  await pool.end();

  // Go to pay page
  await page.goto(`/pay/${publicId}`);

  // Pay with mocked wallet
  await page.click('button:has-text("Pay now")');
  await expect(page.locator('text=Submitted — confirming…')).toBeVisible();

  // Call reconcile API (assuming horizon is mocked to return the payment)
  const reconcileResponse = await request.get('/api/cron/reconcile', {
    headers: {
      'x-cron-secret': process.env.CRON_SECRET || 'cron'
    }
  });
  expect(reconcileResponse.ok()).toBe(true);
  const reconcileData = await reconcileResponse.json();
  expect(reconcileData).toContainEqual(expect.objectContaining({ action: 'paid', txHash: 'mocked_tx_hash_123' }));

  // Check invoice status
  const statusResponse = await request.get(`/api/invoices/${invoiceId}/status`);
  expect(statusResponse.ok()).toBe(true);
  const statusData = await statusResponse.json();
  expect(statusData.status).toBe('paid');
});