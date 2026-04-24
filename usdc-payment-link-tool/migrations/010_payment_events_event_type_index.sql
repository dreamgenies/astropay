-- Avoid full-table scans when filtering payment_events by event_type.
--
-- Queries that filter on event_type (e.g. "show all late_payment_exception_created
-- events", "count payout_dead_lettered events for alerting") currently require a
-- sequential scan of the full table. At production event volumes this becomes
-- expensive.
--
-- A plain B-tree index on event_type is sufficient: the column has low cardinality
-- (handful of known values) but the table grows unboundedly, so index-only scans
-- for type-filtered aggregates and admin queries are worthwhile.
--
-- The existing payment_events_invoice_id_idx (from 001_init.sql) is kept; queries
-- that filter on both invoice_id AND event_type will use that index first (higher
-- selectivity on invoice_id).

CREATE INDEX IF NOT EXISTS payment_events_event_type_idx
    ON payment_events (event_type);
