use rust_backend::{
    config::Config,
    db::{claim_payout_for_processing, create_pool, release_payout_from_processing},
};
use uuid::Uuid;

#[tokio::test]
async fn test_payout_row_locking() -> anyhow::Result<()> {
    let database_url = match std::env::var("TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => return Ok(()),
    };

    set_test_env("DATABASE_URL", &database_url);
    set_test_env("SESSION_SECRET", "test-session-secret");
    set_test_env("ASSET_ISSUER", "ISSUER");
    set_test_env("PLATFORM_TREASURY_PUBLIC_KEY", "TREASURY");

    let config = Config::from_env()?;
    let pool = create_pool(&config)?;
    let client = pool.get().await?;

    let payout_id = Uuid::new_v4();
    let merchant_id = Uuid::new_v4();
    let invoice_id = Uuid::new_v4();

    client
        .execute(
            "INSERT INTO merchants (id, email, password_hash, business_name, stellar_public_key, settlement_public_key)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            &[
                &merchant_id,
                &"test@example.com",
                &"hash",
                &"Test Business",
                &"stellar_key",
                &"settlement_key",
            ],
        )
        .await?;

    client
        .execute(
            "INSERT INTO invoices (id, public_id, merchant_id, description, amount_cents, currency, asset_code, asset_issuer, destination_public_key, memo, gross_amount_cents, platform_fee_cents, net_amount_cents, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT DO NOTHING",
            &[
                &invoice_id,
                &"test_inv",
                &merchant_id,
                &"Test",
                &1000,
                &"USD",
                &"USDC",
                &"ISSUER",
                &"DEST",
                &"MEMO",
                &1000,
                &10,
                &990,
                &chrono::Utc::now(),
            ],
        )
        .await?;

    client
        .execute(
            "INSERT INTO payouts (id, invoice_id, merchant_id, destination_public_key, amount_cents, asset_code, asset_issuer, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING",
            &[
                &payout_id,
                &invoice_id,
                &merchant_id,
                &"DEST",
                &990,
                &"USDC",
                &"ISSUER",
                &"queued",
            ],
        )
        .await?;

    let claimed = claim_payout_for_processing(&client, payout_id, "worker-1").await?;
    assert!(claimed, "Should successfully claim unclaimed payout");

    let claimed_again = claim_payout_for_processing(&client, payout_id, "worker-2").await?;
    assert!(
        !claimed_again,
        "Should not be able to claim already claimed payout"
    );

    release_payout_from_processing(&client, payout_id).await?;

    let claimed_after_release = claim_payout_for_processing(&client, payout_id, "worker-3").await?;
    assert!(
        claimed_after_release,
        "Should be able to claim payout after release"
    );

    client
        .execute("DELETE FROM payouts WHERE id = $1", &[&payout_id])
        .await?;
    client
        .execute("DELETE FROM invoices WHERE id = $1", &[&invoice_id])
        .await?;
    client
        .execute("DELETE FROM merchants WHERE id = $1", &[&merchant_id])
        .await?;

    Ok(())
}

fn set_test_env(key: &str, value: &str) {
    // SAFETY: this opt-in integration test sets process env before constructing
    // config and does not spawn threads that concurrently mutate env.
    unsafe { std::env::set_var(key, value) }
}
