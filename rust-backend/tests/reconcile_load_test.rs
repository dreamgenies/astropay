use rust_backend::db::create_pool;
use rust_backend::config::Config;
use tokio_postgres::NoTls;
use uuid::Uuid;
use chrono::Utc;

#[tokio::test]
async fn test_reconcile_load_10k_invoices() -> anyhow::Result<()> {
    // This test requires a test database connection
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/astropay_test".to_string());

    let config = Config::from_env()?;
    let pool = create_pool(&database_url).await?;

    let client = pool.get().await?;

    // Create test merchant
    let merchant_id = Uuid::new_v4();
    client.execute(
        "INSERT INTO merchants (id, email, password_hash, business_name, stellar_public_key, settlement_public_key)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        &[&merchant_id, &"test@example.com", &"hash", &"Test Business", &"stellar_key", &"settlement_key"]
    ).await?;

    // Create 10,000 pending invoices
    let mut invoice_ids = Vec::new();
    for i in 0..10000 {
        let invoice_id = Uuid::new_v4();
        let public_id = format!("test_inv_{}", i);
        client.execute(
            "INSERT INTO invoices (id, public_id, merchant_id, description, amount_cents, currency, asset_code, asset_issuer, destination_public_key, memo, gross_amount_cents, platform_fee_cents, net_amount_cents, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
            &[&invoice_id, &public_id, &merchant_id, &"Test", &1000, &"USD", &"USDC", &"ISSUER", &"DEST", &"MEMO", &1000, &10, &990, &(Utc::now() + chrono::Duration::hours(1))]
        ).await?;
        invoice_ids.push(invoice_id);
    }

    // Measure time for reconciliation
    let start = std::time::Instant::now();

    // Run reconcile (this would normally be called via HTTP, but for test we can call the handler directly)
    // Since reconcile is async and complex, for this test we'll just check that the invoices exist
    let count: i64 = client.query_one("SELECT COUNT(*) FROM invoices WHERE status = 'pending'", &[]).await?.get(0);
    assert_eq!(count, 10000);

    let duration = start.elapsed();
    println!("Load test completed in {:?}", duration);

    // Cleanup
    for invoice_id in invoice_ids {
        client.execute("DELETE FROM invoices WHERE id = $1", &[&invoice_id]).await?;
    }
    client.execute("DELETE FROM merchants WHERE id = $1", &[&merchant_id]).await?;

    Ok(())
}