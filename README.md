# ASTROpay

ASTROpay is a hosted USDC payment-link and invoicing platform on Stellar.

This repository currently contains two major codebases:

- `usdc-payment-link-tool/`: the Next.js application, UI, and the original route-handler backend
- `rust-backend/`: the Rust API migration, where backend responsibilities are being moved deliberately instead of being left inside the frontend runtime forever

## Current architecture

The product still serves the web experience from Next.js, but the backend is being pulled into Rust for the parts that matter operationally:

- merchant auth and session handling
- invoice creation and retrieval
- webhook-driven payment marking
- SQL-backed migration execution
- reconciliation logic against Horizon

The Rust backend is not at full feature parity yet. It still needs the remaining Stellar-heavy pieces completed properly:

- checkout XDR generation/submission
- merchant settlement execution
- full cron settlement flow

That split is intentional. A fake “all-Rust now” claim would be dishonest.

## Repository layout

### `usdc-payment-link-tool`

- Next.js App Router frontend
- current checkout UI
- current deployment configs for Vercel, Railway, and Docker
- existing TypeScript implementation of backend behavior

### `rust-backend`

- Axum-based API service
- Postgres connection pool
- cookie-backed JWT sessions
- Rust migration runner
- Rust reconciliation path and backend service foundation

## Local development

### Next.js app

```bash
cd usdc-payment-link-tool
cp .env.example .env.local
npm install
npm run db:migrate
npm run dev
```

### Rust backend

```bash
cd rust-backend
cp .env.example .env.local
cargo check
cargo run --bin migrate
cargo run
```

## CI checks

GitHub Actions runs the repo as two independent jobs so one runtime cannot hide the other runtime's failure:

- Next.js: `cd usdc-payment-link-tool && npm ci && npm test && npm run typecheck`
- Rust: `cd rust-backend && cargo fmt --check && cargo test`

## Deployment reality

If you are evaluating the repo for production-readiness, read:

- [usdc-payment-link-tool/DEPLOY_CHECKLIST.md](usdc-payment-link-tool/DEPLOY_CHECKLIST.md)
- [rust-backend/README.md](rust-backend/README.md)
- [docs/deployment/staged-release-rollback-playbook.md](docs/deployment/staged-release-rollback-playbook.md)
- [docs/deployment/railway-deployment.md](docs/deployment/railway-deployment.md)

The right reading is not “Rust solved everything.”

The right reading is:

- the frontend exists
- the backend extraction has started
- the Rust service is real
- the remaining parity work is still explicit

## Issue backlog

Contributor planning now lives in:

- [docs/issue-backlog/astropay-250-issues.md](docs/issue-backlog/astropay-250-issues.md)
- [docs/observability/invoice-payout-lifecycle-metrics-spec.md](docs/observability/invoice-payout-lifecycle-metrics-spec.md)
- [docs/database/schema-ownership.md](docs/database/schema-ownership.md)
- [.github/ISSUE_TEMPLATE/backlog-item.md](.github/ISSUE_TEMPLATE/backlog-item.md)
- [scripts/publish_issue_backlog.py](scripts/publish_issue_backlog.py)

That backlog is intentionally grounded in the current repo state:

- Next.js still owns checkout XDR build and settlement execution today
- Rust owns only part of the operational backend
- contributor issues should move the architecture forward without pretending parity already exists

To publish the markdown backlog as real GitHub issues after re-authenticating the CLI:

```bash
gh auth login -h github.com
python3 scripts/publish_issue_backlog.py --dry-run --limit 5
python3 scripts/publish_issue_backlog.py --repo dreamgenies/astropay --limit 25
python3 scripts/publish_issue_backlog.py --repo dreamgenies/astropay --start AP-026 --end AP-050
```
