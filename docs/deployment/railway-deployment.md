# Railway Deployment Guidance

ASTROpay currently uses two major components:
- `usdc-payment-link-tool/`: Next.js frontend
- `rust-backend/`: Rust backend API

For deployment on Railway, these must be run as **separate services** to allow independent scaling, health checks, and lifecycle management. Attempting to run them in a single runtime container is strongly discouraged.

## 1. Rust Backend Service

1. Create a new service in Railway connecting to your GitHub repository.
2. In the service settings, set the **Root Directory** to `/rust-backend`.
3. Railway defaults to using the Nixpacks builder, which works out of the box with the `Cargo.toml`.
4. Define the following Environment Variables:
   - `DATABASE_URL` (Link to a Postgres instance)
   - `JWT_SECRET`
   - `STELLAR_NETWORK_PASSPHRASE`
   - `STELLAR_HORIZON_URL`
   - `PORT` (e.g., `8080`)
5. The start command will be automatically determined by Nixpacks (`cargo run --release`).

## 2. Next.js Frontend Service

1. Create a second service in Railway connecting to the same repository.
2. Set the **Root Directory** to `/usdc-payment-link-tool`.
3. Provide the frontend with the URL of the newly created Rust backend via environment variables:
   - `NEXT_PUBLIC_API_URL` (or internal Railway networking equivalent, e.g., `http://rust-backend.railway.internal:8080`)
   - Other Next.js specific variables as required by `.env.example`.
4. Ensure the build command is `npm run build` and the start command is `npm start`.

## Database Migrations

You can set up a separate transient worker or use the rust-backend release phase to run database migrations against the Railway Postgres service:
```bash
cargo run --bin migrate
```

## Summary

By keeping the Next.js App Router and the Axum API in separate Railway services, each service scales independently and correctly utilizes its native builder configurations. Do not use a unified `railway.json` attempt for both simultaneously; prefer infrastructure-as-code or Railway specific configurations per-directory.
