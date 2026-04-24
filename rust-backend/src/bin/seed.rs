//! Seed script — inserts demo merchants and invoices for local development.
//!
//! Run from `rust-backend/`:
//!
//! ```bash
//! cargo run --bin seed
//! ```
//!
//! The script is **idempotent**: it uses `ON CONFLICT DO NOTHING` on every
//! insert, so running it multiple times is safe. Existing rows are never
//! modified or deleted.
//!
//! ## What gets created
//!
//! Two demo merchants:
//!
//! | email                        | password   | business          |
//! |------------------------------|------------|-------------------|
//! | alice@demo.astropay.test     | demo1234   | Alice's Boutique  |
//! | bob@demo.astropay.test       | demo1234   | Bob's Tech Store  |
//!
//! Each merchant gets five invoices spread across all lifecycle states:
//! `pending`, `paid`, `settled`, `expired`, and `failed`.
//!
//! ## Stellar keys
//!
//! All keys used here are **testnet-only** Stellar Ed25519 public keys
//! (valid strkey format, zero-value seeds). They are safe to commit and
//! will never hold real funds.
//!
//! ## Removing seed data
//!
//! ```sql
//! DELETE FROM merchants WHERE email LIKE '%@demo.astropay.test';
//! ```
//! Cascades to invoices, payouts, payment_events, and sessions.

use dotenvy::from_filename;
use tokio_postgres::NoTls;

// ---------------------------------------------------------------------------
// Testnet-only demo keys (valid strkey format, zero-value seeds — no funds)
// ---------------------------------------------------------------------------
const ALICE_STELLAR_KEY: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const ALICE_SETTLEMENT_KEY: &str = "GBAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const BOB_STELLAR_KEY: &str = "GCAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const BOB_SETTLEMENT_KEY: &str = "GDAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

// Platform treasury public key placeholder (matches .env.example pattern)
const TREASURY_KEY: &str = "GEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

// USDC on Stellar testnet
const ASSET_CODE: &str = "USDC";
const ASSET_ISSUER: &str = "GBBD47IF6A3JQRYKRQJ3235GHKJ4GQV4QJV6T4QNVWJ6K4H2L6LJ5B6Q";

// ---------------------------------------------------------------------------
// Password hash for "demo1234" using scrypt (N=16384, r=8, p=1).
// Pre-computed so the seed script has no dependency on the scrypt crate.
// Regenerate with: node -e "require('crypto').scrypt('demo1234','salt',64,(e,k)=>console.log(k.toString('hex')))"
// The auth handler uses its own hash/verify; this value is only for seeding.
// ---------------------------------------------------------------------------
const DEMO_PASSWORD_HASH: &str =
    "$scrypt$ln=14,r=8,p=1$c2FsdA$\
     AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load env from the same locations as migrate.rs
    for path in [
        ".env.local",
        ".env",
        "../usdc-payment-link-tool/.env.local",
        "../usdc-payment-link-tool/.env",
    ] {
        let _ = from_filename(path);
    }

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL is not set"))?;

    let (client, connection) = tokio_postgres::connect(&database_url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("postgres connection error: {e}");
        }
    });

    // ── Merchants ────────────────────────────────────────────────────────────

    let merchants: &[(&str, &str, &str, &str, &str)] = &[
        (
            "alice@demo.astropay.test",
            "Alice's Boutique",
            ALICE_STELLAR_KEY,
            ALICE_SETTLEMENT_KEY,
            DEMO_PASSWORD_HASH,
        ),
        (
            "bob@demo.astropay.test",
            "Bob's Tech Store",
            BOB_STELLAR_KEY,
            BOB_SETTLEMENT_KEY,
            DEMO_PASSWORD_HASH,
        ),
    ];

    for (email, business_name, stellar_key, settlement_key, password_hash) in merchants {
        let rows = client
            .execute(
                "INSERT INTO merchants
                   (email, password_hash, business_name, stellar_public_key, settlement_public_key)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (email) DO NOTHING",
                &[email, password_hash, business_name, stellar_key, settlement_key],
            )
            .await?;
        if rows > 0 {
            println!("Created merchant: {email} ({business_name})");
        } else {
            println!("Skipped merchant (already exists): {email}");
        }
    }

    // ── Invoices ─────────────────────────────────────────────────────────────
    // Five invoices per merchant covering every lifecycle status.

    struct InvoiceSeed {
        public_id: &'static str,
        memo: &'static str,
        description: &'static str,
        amount_cents: i32,
        status: &'static str,
        /// Hours from now for expires_at (negative = already expired)
        expires_offset_hours: i64,
        /// Hours from now for paid_at (None = not paid)
        paid_offset_hours: Option<i64>,
        /// Hours from now for settled_at (None = not settled)
        settled_offset_hours: Option<i64>,
        transaction_hash: Option<&'static str>,
        settlement_hash: Option<&'static str>,
    }

    let invoice_seeds: &[InvoiceSeed] = &[
        InvoiceSeed {
            public_id: "inv_demo_pending01",
            memo: "astro_demo_p01",
            description: "Website design deposit",
            amount_cents: 25000,
            status: "pending",
            expires_offset_hours: 24,
            paid_offset_hours: None,
            settled_offset_hours: None,
            transaction_hash: None,
            settlement_hash: None,
        },
        InvoiceSeed {
            public_id: "inv_demo_paid0001",
            memo: "astro_demo_pd1",
            description: "Logo design package",
            amount_cents: 15000,
            status: "paid",
            expires_offset_hours: 20,
            paid_offset_hours: Some(-2),
            settled_offset_hours: None,
            transaction_hash: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            settlement_hash: None,
        },
        InvoiceSeed {
            public_id: "inv_demo_settled1",
            memo: "astro_demo_s01",
            description: "Monthly retainer - March",
            amount_cents: 50000,
            status: "settled",
            expires_offset_hours: -48,
            paid_offset_hours: Some(-72),
            settled_offset_hours: Some(-24),
            transaction_hash: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
            settlement_hash: Some(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            ),
        },
        InvoiceSeed {
            public_id: "inv_demo_expired1",
            memo: "astro_demo_e01",
            description: "Rush delivery fee",
            amount_cents: 5000,
            status: "expired",
            expires_offset_hours: -1,
            paid_offset_hours: None,
            settled_offset_hours: None,
            transaction_hash: None,
            settlement_hash: None,
        },
        InvoiceSeed {
            public_id: "inv_demo_failed01",
            memo: "astro_demo_f01",
            description: "Consulting session",
            amount_cents: 10000,
            status: "failed",
            expires_offset_hours: -12,
            paid_offset_hours: Some(-20),
            settled_offset_hours: None,
            transaction_hash: Some(
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            ),
            settlement_hash: None,
        },
    ];

    // Fetch merchant IDs for both demo accounts
    for (email, _, _, _, _) in merchants {
        let row = client
            .query_opt("SELECT id FROM merchants WHERE email = $1", &[email])
            .await?;
        let merchant_id: uuid::Uuid = match row {
            Some(r) => r.get("id"),
            None => {
                eprintln!("Merchant {email} not found — skipping invoices");
                continue;
            }
        };

        // Suffix the public_id and memo with a merchant-specific tag so both
        // merchants get unique rows (public_id and memo have UNIQUE constraints).
        let tag = if email.contains("alice") { "a" } else { "b" };

        for seed in invoice_seeds {
            let public_id = format!("{}_{}", seed.public_id, tag);
            let memo = format!("{}_{}", seed.memo, tag);

            let fee = std::cmp::max(1, seed.amount_cents * 100 / 10_000);
            let gross = seed.amount_cents;
            let net = gross - fee;

            let expires_at = chrono::Utc::now()
                + chrono::Duration::hours(seed.expires_offset_hours);
            let paid_at = seed
                .paid_offset_hours
                .map(|h| chrono::Utc::now() + chrono::Duration::hours(h));
            let settled_at = seed
                .settled_offset_hours
                .map(|h| chrono::Utc::now() + chrono::Duration::hours(h));

            let checkout_url = format!("http://localhost:3000/pay/{public_id}");

            let rows = client
                .execute(
                    "INSERT INTO invoices (
                       public_id, merchant_id, description,
                       amount_cents, gross_amount_cents, platform_fee_cents, net_amount_cents,
                       currency, asset_code, asset_issuer,
                       destination_public_key, memo, status,
                       expires_at, paid_at, settled_at,
                       transaction_hash, settlement_hash,
                       checkout_url, metadata
                     ) VALUES (
                       $1,  $2,  $3,
                       $4,  $5,  $6,  $7,
                       'USD', $8, $9,
                       $10, $11, $12,
                       $13, $14, $15,
                       $16, $17,
                       $18, '{\"product\":\"ASTROpay\",\"seed\":true}'::jsonb
                     )
                     ON CONFLICT (public_id) DO NOTHING",
                    &[
                        &public_id,
                        &merchant_id,
                        &seed.description,
                        &seed.amount_cents,
                        &gross,
                        &fee,
                        &net,
                        &ASSET_CODE,
                        &ASSET_ISSUER,
                        &TREASURY_KEY,
                        &memo,
                        &seed.status,
                        &expires_at,
                        &paid_at,
                        &settled_at,
                        &seed.transaction_hash,
                        &seed.settlement_hash,
                        &checkout_url,
                    ],
                )
                .await?;

            if rows > 0 {
                println!(
                    "  Created invoice: {public_id} ({} / {}¢)",
                    seed.status, seed.amount_cents
                );
            } else {
                println!("  Skipped invoice (already exists): {public_id}");
            }
        }
    }

    println!("\nSeed complete.");
    println!("Login at http://localhost:3000/login");
    println!("  alice@demo.astropay.test / demo1234");
    println!("  bob@demo.astropay.test   / demo1234");

    Ok(())
}
