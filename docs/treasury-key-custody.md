# Treasury key custody and rotation

This document defines how `PLATFORM_TREASURY_SECRET_KEY` must be stored,
accessed, rotated, and kept out of frontend runtimes. Read it before
touching any code that signs or submits settlement transactions.

## What the treasury key is

`PLATFORM_TREASURY_SECRET_KEY` is the Ed25519 secret key (Stellar `S...`
strkey) for the platform treasury account. It is the only credential that
can sign outbound settlement transactions — the payout from the treasury to
each merchant's `settlement_public_key`.

The corresponding public key is `PLATFORM_TREASURY_PUBLIC_KEY` (a `G...`
strkey). The public key is safe to log, store in the DB, and include in
responses. **The secret key is not.**

## Where the secret key must and must not live

| Location | Allowed | Notes |
|---|---|---|
| Server-side environment variable (Rust process, Next.js Node runtime) | ✅ | Only place it should exist at runtime |
| CI/CD secret store (GitHub Actions, Railway, Vercel) | ✅ | Injected at deploy time, never committed |
| `.env.local` on a developer machine | ✅ testnet only | Must never hold a mainnet key |
| `.env`, `.env.example`, or any committed file | ❌ | `.env.example` uses a placeholder `S...` string |
| Browser bundle or `NEXT_PUBLIC_*` variable | ❌ | Would expose the key to every visitor |
| Application logs, error messages, or API responses | ❌ | Scrub before logging; never include in JSON |
| Database rows or `payment_events` payloads | ❌ | Only the public key is stored |

## How the secret key is loaded

**Rust backend** (`rust-backend/src/config.rs`):

```rust
platform_treasury_secret_key: env::var("PLATFORM_TREASURY_SECRET_KEY").ok(),
```

The field is `Option<String>`. A missing key is not a startup error — the
service boots without it. Settlement routes that require signing must call
`assertSettlementConfig()` (TypeScript) or check `config.platform_treasury_secret_key.is_some()`
(Rust) and return a clear error before attempting any transaction build.

**TypeScript settle cron** (`usdc-payment-link-tool/app/api/cron/settle/route.ts`):

```ts
assertSettlementConfig(); // throws if PLATFORM_TREASURY_SECRET_KEY is absent
```

`assertSettlementConfig` is defined in `lib/env.ts` and calls `required('PLATFORM_TREASURY_SECRET_KEY')`.
The settle route calls it at the top of the handler so the absence of the
key produces a `500` with a clear message rather than a signing failure
deep inside the Stellar SDK.

**Never** pass the secret key value into a function that logs its arguments,
serialises to JSON, or returns data to a client.

## Verifying the key is absent from the frontend bundle

The Next.js build will include any `process.env.X` reference that does not
use the `NEXT_PUBLIC_` prefix in a server component or route handler — but
it will **not** bake it into the browser bundle as long as it is only
referenced in server-only files.

Checks to run before every production deploy:

1. Confirm `PLATFORM_TREASURY_SECRET_KEY` does not appear in any file
   under `usdc-payment-link-tool/app/(auth)`, `usdc-payment-link-tool/app/(dashboard)`,
   `usdc-payment-link-tool/app/pay`, or any `*.tsx` / `*.ts` file that is
   imported by a client component.

2. Search the built output:
   ```bash
   grep -r "PLATFORM_TREASURY_SECRET_KEY" usdc-payment-link-tool/.next/static || echo "clean"
   ```
   This must print `clean`.

3. Confirm the key is not in `NEXT_PUBLIC_*` in any `.env*` file:
   ```bash
   grep "NEXT_PUBLIC_PLATFORM_TREASURY" usdc-payment-link-tool/.env* 2>/dev/null || echo "clean"
   ```

## Rotation procedure

Rotate the treasury key when any of the following occur:

- A team member with access to the secret leaves the organisation.
- The key is suspected to have been logged, leaked, or committed.
- Routine rotation policy (recommended: every 90 days for mainnet).
- A new treasury account is provisioned.

### Steps

1. **Generate a new keypair** on an air-gapped machine or using the
   Stellar Laboratory on a trusted, offline device. Never generate keys
   in a browser on a shared or networked machine.

2. **Fund the new treasury account** on the target network with enough
   XLM to meet the minimum balance and cover transaction fees.

3. **Drain the old treasury account** — wait for all in-flight settlement
   transactions to complete (check `payouts WHERE status = 'submitted'`
   are resolved) before switching keys.

4. **Update the secret in your secret store** (Railway, Vercel, or your
   CI/CD provider). Do not update `.env.local` on shared machines.

5. **Update `PLATFORM_TREASURY_PUBLIC_KEY`** to the new public key in
   the same deployment. Both variables must be updated atomically in the
   same deploy to avoid a window where the public key and secret key
   belong to different accounts.

6. **Redeploy** the Rust backend and the Next.js app in the same release.

7. **Verify** by running the settle cron with `?dry_run=true` and
   confirming the response does not error on key loading.

8. **Revoke or archive the old keypair** — remove it from all secret
   stores and record the rotation in your audit log.

### What not to do during rotation

- Do not update only one of `PLATFORM_TREASURY_PUBLIC_KEY` /
  `PLATFORM_TREASURY_SECRET_KEY`. A mismatch will cause every settlement
  transaction to fail with a signature error.
- Do not rotate while there are `submitted` payouts that have not yet
  been confirmed on-chain. The old key signed those transactions; the
  network will still accept them, but your service will no longer be
  able to resubmit them if they time out.
- Do not store the old key "just in case" in a `.env` file or comment.
  Delete it from every location once rotation is confirmed.

## What happens when the key is missing at runtime

**Rust backend:** `config.platform_treasury_secret_key` is `None`. Any
handler that needs to sign a transaction must check this and return an
error before calling into the Stellar SDK. The settle cron currently
returns a `note` field explaining that signing is not yet implemented;
once signing is wired up, a missing key must produce a `500` with a
message like `"treasury signing key not configured"` — never a panic.

**TypeScript settle cron:** `assertSettlementConfig()` throws
`"Missing required environment variable: PLATFORM_TREASURY_SECRET_KEY"`.
The route handler catches this and returns `500`. The `cron_runs` audit
row records `success = false` and the error detail.

In both cases the payout rows remain in `queued` status and will be
retried on the next settle run once the key is restored.

## Testnet vs mainnet keys

Keep testnet and mainnet keys in separate secret stores and separate
deployment environments. Never copy a mainnet key into a testnet
environment variable, even temporarily.

The `STELLAR_NETWORK` variable (`TESTNET` or `MAINNET`) controls which
Horizon endpoint and network passphrase are used. A testnet key used
against a mainnet Horizon endpoint will produce signature errors, not
fund loss — but the reverse (a mainnet key in a testnet environment)
risks accidental mainnet transactions during development.

**Verify:** `cargo test` in `rust-backend` includes a `sample_config`
fixture that sets `platform_treasury_secret_key: None` — confirming the
service can boot and be tested without a real key present.
