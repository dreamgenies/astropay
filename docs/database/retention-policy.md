# Retention policy — sessions and payment_events

## Policy values

| Table            | Retain for | Source of truth                          |
|------------------|-----------|------------------------------------------|
| `sessions`       | 90 days   | `retention_config` (migration 013)       |
| `payment_events` | 365 days  | `retention_config` (migration 013)       |

## Rationale

**sessions — 90 days**
Expired sessions (`expires_at < NOW()`) are already inert; the auth path rejects them.
They accumulate indefinitely without a purge. 90 days covers any realistic idle
re-authentication window and keeps the table lean for the expiry-index scans used by
`purge_sessions`.

**payment_events — 365 days**
One full year covers audit, dispute, and reconciliation windows for USDC payments on
Stellar. Events older than one year have no operational value and inflate sequential
scans on the `payment_events` table.

## How the policy is applied

The `retention_config` table (created in `013_retention_policy.sql`) is the single
source of truth. The purge cron job reads `retain_days` at runtime:

```sql
-- sessions (existing purge_sessions cron)
DELETE FROM sessions WHERE expires_at < NOW() - INTERVAL '90 days';

-- payment_events (purge_payment_events cron)
DELETE FROM payment_events
WHERE created_at < NOW() - (
    SELECT retain_days || ' days' FROM retention_config WHERE table_name = 'payment_events'
)::INTERVAL;
```

Changing a `retain_days` value in `retention_config` takes effect on the next cron
run without a code deploy.

## Indexes

- `sessions` — `sessions_expires_at_id_idx` (migration `002`) already supports the
  purge scan; no new index needed.
- `payment_events` — `payment_events_created_at_idx` (migration `014`) is added to
  avoid a full sequential scan when deleting by age.

## Cron audit

Both purge jobs write a row to `cron_runs` with `job_type` of `purge_sessions` or
`purge_payment_events` respectively (constraint extended in migration `013`).

## Edge cases

- If `retention_config` has no row for a table the cron job must skip that table and
  log a warning rather than defaulting silently.
- Deletes should be batched (e.g. `LIMIT 1000` per tick) to avoid long-held locks on
  high-volume deployments.
- `payment_events` rows are cascade-deleted when their parent `invoice` is deleted, so
  the retention purge only needs to handle orphan-free rows that outlive the retention
  window while their invoice still exists.

## Changing the policy

Update the relevant row in `retention_config` directly:

```sql
UPDATE retention_config SET retain_days = 180, updated_at = NOW()
WHERE table_name = 'sessions';
```

No migration is required for a value change. A migration is required only if the
table structure itself changes.
