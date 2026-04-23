-- Session index layout for expiry-heavy workloads and auth lookups.
--
-- Assumptions (see rust-backend README and src/db.rs):
-- 1) current_merchant / getCurrentMerchant resolve sessions by primary key (id) from the JWT sid claim — no extra index beyond PK.
-- 2) Global expiry sweeps use WHERE expires_at < cutoff (optionally ORDER BY expires_at, id for stable pagination). A composite on
--    (expires_at, id) supports range scans and ordered batches without a separate sort on large tables.
-- 3) Merchant-scoped deletes or audits use WHERE merchant_id = ? AND expires_at < ?. A composite (merchant_id, expires_at) replaces a
--    standalone merchant_id index: the left prefix still accelerates WHERE merchant_id = ? alone.

DROP INDEX IF EXISTS sessions_expires_at_idx;
DROP INDEX IF EXISTS sessions_merchant_id_idx;

CREATE INDEX sessions_expires_at_id_idx ON sessions (expires_at, id);
CREATE INDEX sessions_merchant_expires_at_idx ON sessions (merchant_id, expires_at);
