import { test, expect } from '@playwright/test';

const NEXTJS_BASE = 'http://localhost:3000';
const RUST_BASE = 'http://localhost:8080';

test('contract test between Next.js and Rust route responses', async ({ page, request }) => {
  // Login to Next.js via UI
  await page.goto('/login');
  await page.fill('input[name="email"]', 'alice@demo.astropay.test');
  await page.fill('input[name="password"]', 'demo1234');
  await page.click('button[type="submit"]');
  await expect(page).toHaveURL('/dashboard');

  // Get cookies for API
  const cookies = await page.context().cookies();
  const sessionCookie = cookies.find(c => c.name === 'astropay_session')?.value;

  // Test GET /api/invoices
  const nextjsInvoicesResponse = await request.get(`${NEXTJS_BASE}/api/invoices`, {
    headers: {
      Cookie: `session=${sessionCookie}`
    }
  });
  const rustInvoicesResponse = await request.get(`${RUST_BASE}/api/invoices`, {
    headers: {
      Cookie: `astropay_session=${sessionCookie}`
    }
  });

  expect(nextjsInvoicesResponse.status()).toBe(rustInvoicesResponse.status());
  const nextjsInvoices = await nextjsInvoicesResponse.json();
  const rustInvoices = await rustInvoicesResponse.json();

  // Compare structure (assuming same data since same DB)
  expect(nextjsInvoices).toHaveProperty('invoices');
  expect(rustInvoices).toHaveProperty('invoices');
  expect(nextjsInvoices.invoices.length).toBe(rustInvoices.invoices.length);

  // Test POST /api/invoices
  const invoiceData = {
    description: 'Contract test invoice',
    amountUsd: 5.00
  };

  const nextjsCreateResponse = await request.post(`${NEXTJS_BASE}/api/invoices`, {
    headers: {
      Cookie: `session=${sessionCookie}`,
      'Content-Type': 'application/json'
    },
    data: invoiceData
  });
  const rustCreateResponse = await request.post(`${RUST_BASE}/api/invoices`, {
    headers: {
      Cookie: `astropay_session=${sessionCookie}`,
      'Content-Type': 'application/json'
    },
    data: invoiceData
  });

  expect(nextjsCreateResponse.status()).toBe(rustCreateResponse.status());
  const nextjsCreate = await nextjsCreateResponse.json();
  const rustCreate = await rustCreateResponse.json();

  expect(nextjsCreate).toHaveProperty('invoice');
  expect(rustCreate).toHaveProperty('invoice');

  const invoiceId = nextjsCreate.invoice.id;

  // Test GET /api/invoices/{id}
  const nextjsGetResponse = await request.get(`${NEXTJS_BASE}/api/invoices/${invoiceId}`, {
    headers: {
      Cookie: `session=${sessionCookie}`
    }
  });
  const rustGetResponse = await request.get(`${RUST_BASE}/api/invoices/${invoiceId}`, {
    headers: {
      Cookie: `astropay_session=${sessionCookie}`
    }
  });

  expect(nextjsGetResponse.status()).toBe(rustGetResponse.status());
  const nextjsGet = await nextjsGetResponse.json();
  const rustGet = await rustGetResponse.json();

  expect(nextjsGet).toHaveProperty('invoice');
  expect(rustGet).toHaveProperty('invoice');

  // Test GET /api/invoices/{id}/status
  const nextjsStatusResponse = await request.get(`${NEXTJS_BASE}/api/invoices/${invoiceId}/status`, {
    headers: {
      Cookie: `session=${sessionCookie}`
    }
  });
  const rustStatusResponse = await request.get(`${RUST_BASE}/api/invoices/${invoiceId}/status`);

  expect(nextjsStatusResponse.status()).toBe(rustStatusResponse.status());
  const nextjsStatus = await nextjsStatusResponse.json();
  const rustStatus = await rustStatusResponse.json();

  expect(nextjsStatus).toHaveProperty('status');
  expect(rustStatus).toHaveProperty('status');
  expect(nextjsStatus.status).toBe(rustStatus.status);
});