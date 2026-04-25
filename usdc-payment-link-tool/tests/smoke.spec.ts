import { test, expect } from '@playwright/test';

test('merchant login and invoice creation smoke test', async ({ page }) => {
  // Go to login page
  await page.goto('/login');

  // Fill login form
  await page.fill('input[name="email"]', 'alice@demo.astropay.test');
  await page.fill('input[name="password"]', 'demo1234');

  // Submit form
  await page.click('button[type="submit"]');

  // Should redirect to dashboard
  await expect(page).toHaveURL('/dashboard');

  // Go to new invoice page
  await page.goto('/dashboard/invoices/new');

  // Fill invoice form
  await page.fill('textarea[name="description"]', 'Test invoice for smoke test');
  await page.fill('input[name="amountUsd"]', '100.00');

  // Submit form
  await page.click('button[type="submit"]');

  // Should redirect to invoice page
  await expect(page).toHaveURL(/\/dashboard\/invoices\/[a-f0-9-]+/);

  // Check that invoice details are displayed
  await expect(page.locator('h1')).toContainText('Invoice');
});