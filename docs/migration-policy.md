# Route handler migration policy

This document defines when logic stays in Next.js and when it moves to the Rust/Axum backend.

## Current split

| Layer | Owns |
|---|---|
| Next.js (`usdc-payment-link-tool`) | Checkout XDR build and submission, cron reconcile/settle, all UI pages |
| Rust (`rust-backend`) | Merchant auth and sessions, invoice CRUD, webhook payment marking, SQL migrations |

## Decision criteria

Move a route handler to Rust when **all** of the following are true:

1. The route has no dependency on Next.js-only APIs (cookies via `next/headers`, `NextResponse`, `revalidatePath`, etc.).
2. The route is called by server-to-server traffic (cron, webhooks, mobile clients) or is on the auth/session critical path.
3. The route's correctness can be verified with `cargo test` without a running Next.js process.
4. The Rust service already owns the relevant DB tables for that domain.

Keep a route in Next.js when:

- It renders or redirects to a page (App Router server components, `redirect()`).
- It depends on `NEXT_PUBLIC_*` env vars that are baked at build time.
- It is part of the Stellar checkout XDR flow until that flow is ported (see backlog items for checkout and settlement).

## Port sequence

Port in dependency order. Do not port a route before its upstream dependencies are in Rust.

1. Auth and sessions — **done**
2. Invoice CRUD — **done**
3. Webhook payment marking — **done**
4. Checkout XDR build and submission — **not started** (blocked on Stellar SDK port)
5. Cron reconcile — **not started** (blocked on checkout)
6. Cron settle — **not started** (blocked on checkout)

## How to port a route

1. Implement the handler in `rust-backend/src/handlers/<domain>.rs`.
2. Register the route in `rust-backend/src/main.rs`.
3. Add unit tests in the same file under `#[cfg(test)]`.
4. Remove the corresponding `app/api/.../route.ts` file from the Next.js app.
5. Update `usdc-payment-link-tool/README.md` — move the route from "current Rust gaps" to "current Rust coverage".
6. Update this document's table above.

## What stays in `lib/http.ts` and `lib/data.ts`

`lib/http.ts` (`ok` / `fail` helpers) and `lib/data.ts` (DB query functions) are used exclusively by Next.js route handlers that have not been ported yet. Once all callers of a function in `data.ts` are removed, delete that function. Do not add new functions to `data.ts` for logic that belongs in Rust.

## Verification

After each port:

```bash
# Rust side
cd rust-backend && cargo test

# Next.js side — confirm no broken imports remain
cd usdc-payment-link-tool && npm run typecheck
```
