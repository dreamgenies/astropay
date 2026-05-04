import { test, expect } from '@playwright/test';

test('checkout smoke test with mocked wallet adapter', async ({ page }) => {
  // Mock Freighter wallet functions
  await page.addInitScript(() => {
    // Mock the @stellar/freighter-api functions
    (window as unknown as { stellar: unknown }).stellar = {
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
        body: JSON.stringify({ hash: 'mocked_tx_hash' }),
      });
    }
  });

  await page.route('**/api/invoices/*/status', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ status: 'paid' }),
    });
  });

  // Assume a test invoice exists with publicId 'test-invoice'
  // In a real setup, this would be created via API or seeded
  await page.goto('/pay/test-invoice');

  // Wait for the page to load
  await expect(page.locator('h1')).toBeVisible();

  // Click the pay button
  await page.click('button:has-text("Pay now")');

  // Wait for the payment flow to complete
  await expect(page.locator('text=Submitted — confirming…')).toBeVisible();

  // Eventually, the status should update to paid
  await expect(page.locator('text=paid')).toBeVisible();
});
