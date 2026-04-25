#!/bin/bash
# Smoke test script for staged deployments
# Usage: ./smoke-test.sh [backend_url] [test_email] [test_password]

set -e

BACKEND_URL=${1:-"http://localhost:8080"}
TEST_EMAIL=${2:-"test@example.com"}
TEST_PASSWORD=${3:-"test123"}

echo "🧪 Running smoke tests against $BACKEND_URL"

# Test 1: Health check
echo "1. Testing health endpoint..."
curl -s -f "$BACKEND_URL/health" > /dev/null || {
    echo "❌ Health check failed"
    exit 1
}
echo "✅ Health check passed"

# Test 2: Authentication
echo "2. Testing authentication..."
AUTH_RESPONSE=$(curl -s -X POST "$BACKEND_URL/api/auth/login" \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\"}" \
    -c cookies.txt)

if echo "$AUTH_RESPONSE" | grep -q "error"; then
    echo "❌ Authentication failed: $AUTH_RESPONSE"
    exit 1
fi
echo "✅ Authentication passed"

# Test 3: Invoice creation
echo "3. Testing invoice creation..."
INVOICE_RESPONSE=$(curl -s -X POST "$BACKEND_URL/api/invoices" \
    -H "Content-Type: application/json" \
    -b cookies.txt \
    -d '{"amount":"10.00","description":"Smoke test invoice"}')

INVOICE_ID=$(echo "$INVOICE_RESPONSE" | jq -r '.id // empty')
if [[ -z "$INVOICE_ID" ]]; then
    echo "❌ Invoice creation failed: $INVOICE_RESPONSE"
    exit 1
fi
echo "✅ Invoice creation passed (ID: $INVOICE_ID)"

# Test 4: Invoice retrieval
echo "4. Testing invoice retrieval..."
curl -s -f "$BACKEND_URL/api/invoices/$INVOICE_ID" \
    -b cookies.txt > /dev/null || {
    echo "❌ Invoice retrieval failed"
    exit 1
}
echo "✅ Invoice retrieval passed"

# Test 5: Cron endpoint (if CRON_SECRET is available)
if [[ -n "$CRON_SECRET" ]]; then
    echo "5. Testing reconciliation endpoint..."
    curl -s -f "$BACKEND_URL/api/cron/reconcile" \
        -H "Authorization: Bearer $CRON_SECRET" > /dev/null || {
        echo "❌ Reconciliation endpoint failed"
        exit 1
    }
    echo "✅ Reconciliation endpoint passed"
fi

# Cleanup
rm -f cookies.txt

echo "🎉 All smoke tests passed!"