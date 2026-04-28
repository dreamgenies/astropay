# Reconciliation Query Plan Analysis — AP-208

## Problem

The reconcile cron (`POST /api/cron/reconcile`) scans all pending invoices using
keyset pagination:

```sql
SELECT * FROM invoices
WHERE status = 'pending'
  AND (created_at, id) > ($1, $2)
  AND ($4::bigint = 0 OR created_at >= NOW() - ($4::bigint * INTERVAL '1 hour'))
ORDER BY created_at ASC, id ASC
LIMIT 100
```

Before migration `020`, the only relevant index was the single-column
`invoices_status_idx (status)`. The planner was forced to:

1. Bitmap-scan `invoices_status_idx` → collect all pending TIDs
2. Heap-fetch every pending row
3. Apply the keyset filter `(created_at, id) > cursor` in memory
4. Sort survivors by `(created_at ASC, id ASC)`
5. Return the first 100

At scale this is O(N) in the number of pending invoices per cron tick.

## Fix — migration `020_reconcile_pending_keyset_idx.sql`

```sql
CREATE INDEX IF NOT EXISTS invoices_pending_created_at_id_idx
  ON invoices (created_at ASC, id ASC)
  WHERE status = 'pending';
```

This partial composite index satisfies all three constraints in a single ordered
index scan:

| Constraint | How the index helps |
|---|---|
| `WHERE status = 'pending'` | Partial predicate — only pending rows are indexed |
| `ORDER BY created_at ASC, id ASC` | Leading columns match sort direction — no sort step |
| `(created_at, id) > ($1, $2)` | Planner pushes keyset cursor into the index scan start point |
| `created_at >= NOW() - interval` | Range on leading column — index range scan |

The index stays small automatically: rows that transition to `paid`, `expired`,
`settled`, or `failed` are removed from the partial index by the planner.

## Expected plan change

Load the fixture data from `019_performance_test_fixtures.sql`, then run:

```sql
EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT)
SELECT * FROM invoices
WHERE status = 'pending'
  AND (created_at, id) > ('1970-01-01 00:00:00+00'::timestamptz, '00000000-0000-0000-0000-000000000000'::uuid)
  AND (0::bigint = 0 OR created_at >= NOW() - (0::bigint * INTERVAL '1 hour'))
ORDER BY created_at ASC, id ASC
LIMIT 100;
```

**Before migration 020:**
```
Limit  (cost=... rows=100 ...)
  ->  Sort  (cost=... ...)
        Sort Key: created_at, id
        ->  Bitmap Heap Scan on invoices  (cost=... ...)
              Recheck Cond: (status = 'pending')
              ->  Bitmap Index Scan on invoices_status_idx
```

**After migration 020:**
```
Limit  (cost=... rows=100 ...)
  ->  Index Scan using invoices_pending_created_at_id_idx on invoices
        Index Cond: ((created_at, id) > ('1970-01-01 00:00:00+00', '00000000-...'))
        Filter: (0 = 0 OR created_at >= (now() - '0 hours'::interval))
```

The `Sort` and `Bitmap Heap Scan` nodes disappear. Each page fetch becomes a
bounded index range scan starting from the cursor position.

## Verification steps

### 1. Apply the migration

```bash
cd usdc-payment-link-tool
npm run db:migrate
# or via Rust runner:
cd rust-backend && cargo run --bin migrate
```

### 2. Confirm the index exists

```sql
SELECT indexname, indexdef
FROM pg_indexes
WHERE tablename = 'invoices'
  AND indexname = 'invoices_pending_created_at_id_idx';
```

Expected output:
```
indexname                          | indexdef
-----------------------------------+----------------------------------------------------------
invoices_pending_created_at_id_idx | CREATE INDEX invoices_pending_created_at_id_idx ON invoices
                                   | USING btree (created_at, id) WHERE status = 'pending'
```

### 3. Check index size vs full-table index

```sql
SELECT
  indexname,
  pg_size_pretty(pg_relation_size(indexrelid)) AS index_size
FROM pg_stat_user_indexes
WHERE relname = 'invoices'
  AND indexname IN (
    'invoices_status_idx',
    'invoices_pending_created_at_id_idx'
  );
```

The partial index should be significantly smaller than a full-table index on the
same columns because it excludes terminal-state rows.

### 4. Run EXPLAIN ANALYZE

Use the query in the "Expected plan change" section above. Confirm:
- No `Sort` node
- No `Bitmap Heap Scan` / `Bitmap Index Scan` on `invoices_status_idx`
- `Index Scan using invoices_pending_created_at_id_idx` is the access path

### 5. Run the unit test

```bash
cd rust-backend && cargo test reconcile_pending_keyset_index
```

## Edge cases and error handling

| Scenario | Behaviour |
|---|---|
| Empty pending backlog | Index scan returns 0 rows on first page; loop exits immediately |
| All pending invoices in window | Keyset cursor advances page-by-page; each page is a bounded index range scan |
| `scan_window_hours = 0` | The `$4::bigint = 0` branch short-circuits the window filter; full pending set is scanned in order |
| Invoice transitions mid-scan | `UPDATE … WHERE id = $1 AND status = 'pending'` is a no-op if already transitioned; `skipped` action is returned |
| Horizon unavailable | `skipped_horizon_unavailable` action is recorded; loop continues to next invoice |
| Concurrent reconcile runs | Keyset pagination is stateless; two concurrent runs may process the same page but the `AND status = 'pending'` guard on the UPDATE prevents double-marking |

## Index retention policy

The existing `invoices_status_idx (status)` is **not dropped**. It is still used
by queries that filter on non-pending statuses:

- Dashboard count queries (`WHERE status = 'paid'`, etc.)
- Dead-letter escalation paths
- Admin/operator queries

The new partial index is additive.
