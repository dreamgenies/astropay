use dotenvy::from_filename;
use rust_backend::migrations::{apply_pending_migrations, default_migrations_dir};
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    for path in [
        ".env.local",
        ".env",
        "../usdc-payment-link-tool/.env.local",
        "../usdc-payment-link-tool/.env",
    ] {
        let _ = from_filename(path);
    }

    let database_url = std::env::var("DATABASE_URL")?;
    let (mut client, connection) = tokio_postgres::connect(&database_url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("postgres connection error: {error}");
        }
    });

    let migrations_dir = default_migrations_dir();
    for name in apply_pending_migrations(&mut client, &migrations_dir).await? {
        println!("Applied {name}");
    }

    Ok(())
}
