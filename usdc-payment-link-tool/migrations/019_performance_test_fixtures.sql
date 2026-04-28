-- Large-volume fixture dataset for performance tests
-- Issue #219: Generates high-volume test data for invoice, payout, and event workloads
--
-- Rollback:
--   SELECT cleanup_performance_test_data();
--   DROP FUNCTION IF EXISTS cleanup_performance_test_data();
--   DROP VIEW IF EXISTS performance_test_summary;
--   The cleanup function (defined at the bottom of this migration) removes all
--   test merchants, invoices, payouts, and payment_events inserted here.
--   Run it before dropping the function and view.
--   WARNING: this migration inserts data identified by email pattern
--   'merchant%@test.com' and description pattern 'Performance test invoice%'.
--   Do not use these patterns for real data or the cleanup function will delete it.

-- Insert test merchants (10 merchants)
INSERT INTO merchants (id, email, password_hash, business_name, stellar_public_key, settlement_public_key)
SELECT 
    gen_random_uuid(),
    'merchant' || i || '@test.com',
    '$2b$12$test_hash_' || i,
    'Test Business ' || i,
    'STELLAR_PK_' || LPAD(i::text, 10, '0'),
    'SETTLEMENT_PK_' || LPAD(i::text, 10, '0')
FROM generate_series(1, 10) AS i
ON CONFLICT (email) DO NOTHING;

-- Insert large volume of invoices (10,000 invoices across merchants)
WITH merchant_ids AS (
    SELECT id, ROW_NUMBER() OVER () as rn 
    FROM merchants 
    WHERE email LIKE 'merchant%@test.com'
    LIMIT 10
)
INSERT INTO invoices (
    id, public_id, merchant_id, description, amount_cents, currency,
    asset_code, asset_issuer, destination_public_key, memo, status,
    gross_amount_cents, platform_fee_cents, net_amount_cents,
    expires_at, paid_at, settled_at, metadata
)
SELECT 
    gen_random_uuid(),
    'inv_' || LPAD((i % 10000)::text, 16, '0'),
    m.id,
    'Performance test invoice ' || i,
    (1000 + (i % 50000))::integer,
    'USD',
    'USDC',
    'TEST_ISSUER',
    'DEST_PK_' || LPAD(i::text, 10, '0'),
    'MEMO_' || LPAD(i::text, 20, '0'),
    CASE 
        WHEN i % 10 = 0 THEN 'settled'
        WHEN i % 5 = 0 THEN 'paid'
        WHEN i % 20 = 0 THEN 'expired'
        ELSE 'pending'
    END,
    (1000 + (i % 50000))::integer,
    ((1000 + (i % 50000)) * 0.01)::integer,
    ((1000 + (i % 50000)) * 0.99)::integer,
    NOW() + INTERVAL '24 hours',
    CASE WHEN i % 5 = 0 THEN NOW() - INTERVAL '1 hour' ELSE NULL END,
    CASE WHEN i % 10 = 0 THEN NOW() - INTERVAL '30 minutes' ELSE NULL END,
    jsonb_build_object('test_data', true, 'batch', i / 1000)
FROM generate_series(1, 10000) AS i
CROSS JOIN merchant_ids m
WHERE i % 10 = (m.rn - 1)
ON CONFLICT (public_id) DO NOTHING;

-- Insert payouts for settled invoices (approximately 1,000 payouts)
INSERT INTO payouts (
    id, invoice_id, merchant_id, destination_public_key, amount_cents,
    asset_code, asset_issuer, status, failure_count, last_failure_at
)
SELECT 
    gen_random_uuid(),
    i.id,
    i.merchant_id,
    i.destination_public_key,
    i.net_amount_cents,
    i.asset_code,
    i.asset_issuer,
    CASE 
        WHEN random() < 0.1 THEN 'failed'
        WHEN random() < 0.05 THEN 'queued'
        ELSE 'settled'
    END,
    CASE WHEN random() < 0.1 THEN (random() * 3)::integer ELSE 0 END,
    CASE WHEN random() < 0.1 THEN NOW() - INTERVAL '1 hour' ELSE NULL END
FROM invoices i
WHERE i.status IN ('paid', 'settled')
AND i.description LIKE 'Performance test invoice%'
ON CONFLICT (invoice_id) DO NOTHING;

-- Insert payment events (50,000 events across invoices)
INSERT INTO payment_events (id, invoice_id, event_type, payload)
SELECT 
    gen_random_uuid(),
    i.id,
    CASE (random() * 4)::integer
        WHEN 0 THEN 'checkout_started'
        WHEN 1 THEN 'payment_submitted'
        WHEN 2 THEN 'payment_confirmed'
        ELSE 'settlement_completed'
    END,
    jsonb_build_object(
        'timestamp', NOW() - (random() * INTERVAL '7 days'),
        'test_event', true,
        'sequence', generate_series
    )
FROM invoices i
CROSS JOIN generate_series(1, 5)
WHERE i.description LIKE 'Performance test invoice%'
LIMIT 50000;

-- Create summary view for performance test verification
CREATE OR REPLACE VIEW performance_test_summary AS
SELECT 
    'merchants' as table_name,
    COUNT(*) as test_records
FROM merchants 
WHERE email LIKE 'merchant%@test.com'
UNION ALL
SELECT 
    'invoices' as table_name,
    COUNT(*) as test_records
FROM invoices 
WHERE description LIKE 'Performance test invoice%'
UNION ALL
SELECT 
    'payouts' as table_name,
    COUNT(*) as test_records
FROM payouts p
JOIN invoices i ON p.invoice_id = i.id
WHERE i.description LIKE 'Performance test invoice%'
UNION ALL
SELECT 
    'payment_events' as table_name,
    COUNT(*) as test_records
FROM payment_events pe
JOIN invoices i ON pe.invoice_id = i.id
WHERE i.description LIKE 'Performance test invoice%';

-- Performance test cleanup function
CREATE OR REPLACE FUNCTION cleanup_performance_test_data() 
RETURNS void AS $$
BEGIN
    DELETE FROM payment_events 
    WHERE invoice_id IN (
        SELECT id FROM invoices 
        WHERE description LIKE 'Performance test invoice%'
    );
    
    DELETE FROM payouts 
    WHERE invoice_id IN (
        SELECT id FROM invoices 
        WHERE description LIKE 'Performance test invoice%'
    );
    
    DELETE FROM invoices 
    WHERE description LIKE 'Performance test invoice%';
    
    DELETE FROM merchants 
    WHERE email LIKE 'merchant%@test.com';
    
    DROP VIEW IF EXISTS performance_test_summary;
END;
$$ LANGUAGE plpgsql;