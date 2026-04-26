# Database Schema Updates

## Row-Locking Strategy for Concurrent Payout Workers (Issue #214)

### Overview
Added row-level locking mechanism to prevent concurrent payout workers from double-processing the same payout.

### Implementation
- **Migration**: `017_payout_row_locking.sql`
- **New columns**: `processing_worker_id`, `processing_started_at`
- **Functions**: `claim_payout_for_processing()`, `release_payout_from_processing()`

### Usage
```rust
// Claim a payout for processing
let claimed = claim_payout_for_processing(&client, payout_id, "worker-1").await?;
if claimed {
    // Process the payout
    // ...
    // Release when done
    release_payout_from_processing(&client, payout_id).await?;
}
```

## PGSSL Mode Validation (Issue #217)

### Overview
Added startup validation for PostgreSQL SSL configuration to catch invalid modes early.

### Implementation
- **Function**: `validate_ssl_mode()`
- **Valid modes**: disable, allow, prefer, require, verify-ca, verify-full
- **Integration**: Called during pool creation in `create_pool()`

### Error Handling
Invalid SSL modes will cause the application to fail at startup with a clear error message listing valid options.

## Business Name Constraint (Issue #220)

### Overview
Added database constraint to prevent empty or whitespace-only business names.

### Implementation
- **Migration**: `018_business_name_constraint.sql`
- **Constraint**: `merchants_business_name_not_empty`
- **Rule**: `LENGTH(TRIM(business_name)) > 0`

### Behavior
Any attempt to insert or update a merchant with an empty business name will be rejected by the database.

## Performance Test Fixtures (Issue #219)

### Overview
Added large-volume test dataset for performance testing and load simulation.

### Implementation
- **Migration**: `019_performance_test_fixtures.sql`
- **Data volume**: 10 merchants, 10,000 invoices, ~1,000 payouts, 50,000 events
- **Cleanup function**: `cleanup_performance_test_data()`

### Usage
```sql
-- View test data summary
SELECT * FROM performance_test_summary;

-- Clean up test data
SELECT cleanup_performance_test_data();
```

### Test Data Characteristics
- Realistic distribution of invoice statuses
- Varied payout states including failures
- Multiple event types per invoice
- Merchant data spread across multiple businesses


## Migration Rollback Notes (AP-250)

### Overview
Every migration file under `usdc-payment-link-tool/migrations/` now contains an inline rollback note in the SQL header comment. Migrations remain forward-only — no down-migration scripts are provided — but each file documents the exact SQL an operator needs to reverse the change if required.

### Rollback Note Format
Each note follows this pattern:

```sql
-- Rollback:
--   <SQL to reverse the change>
--   <Any warnings about data loss or ordering requirements>
```

### Key Rollback Warnings By Migration

| Migration | Risk Level | Notes |
|---|---|---|
| `001_init.sql` | Destructive | Drops all core tables; only safe on a fresh or test database |
| `005_payout_dead_letter.sql` | Data risk | Restoring the old status CHECK will fail if any `dead_lettered` rows exist |
| `007_cron_runs_purge_sessions.sql` | Data risk | Restoring the old CHECK will fail if `purge_sessions` rows exist in `cron_runs` |
| `007_invoice_transaction_hash_unique.sql` | Race risk | Removing the unique index re-exposes the concurrent-webhook race window |
| `011_webhook_deliveries.sql` | Functional risk | Dropping the table disables DB-level replay detection |
| `013_merchant_email_citext.sql` | Data risk | Rollback makes email uniqueness case-sensitive again; audit for case-variant duplicates first |
| `013_retention_policy.sql` | Data risk | Restoring the old CHECK will fail if `purge_payment_events` rows exist in `cron_runs` |
| `017_payout_row_locking.sql` | Race risk | Stop all settle workers before rolling back to avoid losing in-flight lock records |
| `019_performance_test_fixtures.sql` | Data risk | Use `cleanup_performance_test_data()` before dropping the function; do not use test data patterns for real records |

### Verification Steps
Before applying a rollback in production:
1. Confirm no application code depends on the schema change being reversed.
2. Check for rows that would violate a restored constraint (especially status CHECK columns).
3. Stop any cron workers that write to affected tables.
4. Run the rollback SQL in a transaction and verify row counts before committing.
5. Re-run the migration test suite (`cargo test`) to confirm the remaining schema is consistent.
