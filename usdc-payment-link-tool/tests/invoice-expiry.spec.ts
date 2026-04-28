import { test, expect } from '@playwright/test';
import { Pool } from 'pg';

const pool = new Pool({
  connectionString: process.env.DATABASE_URL || 'postgres://postgres:postgres@localhost:5432/astropay_test'
});

test('invoice expiry transition e2e test', async ({ page, request }) => {
  // Login as merchant
  await page.goto('/login');
  await page.fill('input[name="email"]', 'alice@demo.astropay.test');
  await page.fill('input[name="password"]', 'demo1234');
  await page.click('button[type="submit"]');
  await expect(page).toHaveURL('/dashboard');

  // Create invoice
  await page.goto('/dashboard/invoices/new');
  await page.fill('textarea[name="description"]', 'Test invoice for expiry');
  await page.fill('input[name="amountUsd"]', '10.00');
  await page.click('button[type="submit"]');
  await expect(page).toHaveURL(/\/dashboard\/invoices\/[a-f0-9-]+/);

  // Get invoice id from URL
  const url = page.url();
  const invoiceId = url.split('/').pop();

  // Update expires_at to past
  const client = await pool.connect();
  await client.query("UPDATE invoices SET expires_at = NOW() - INTERVAL '1 hour' WHERE id = $1", [invoiceId]);
  client.release();

  // Call reconcile API
  const reconcileResponse = await request.get('/api/cron/reconcile', {
    headers: {
      'x-cron-secret': process.env.CRON_SECRET || 'cron'
    }
  });
  expect(reconcileResponse.ok()).toBe(true);
  const reconcileData = await reconcileResponse.json();
  expect(reconcileData).toContainEqual(expect.objectContaining({ action: 'expired' }));

  // Check invoice status
  const statusResponse = await request.get(`/api/invoices/${invoiceId}/status`);
  expect(statusResponse.ok()).toBe(true);
  const statusData = await statusResponse.json();
  expect(statusData.status).toBe('expired');

  // Get public_id from DB
  const client2 = await pool.connect();
  const result = await client2.query('SELECT public_id FROM invoices WHERE id = $1', [invoiceId]);
  const publicId = result.rows[0].public_id;
  client2.release();

  await page.goto(`/pay/${publicId}`);
  await expect(page.locator('text=Invoice expired')).toBeVisible();
});