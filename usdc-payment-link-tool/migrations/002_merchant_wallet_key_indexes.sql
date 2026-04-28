-- Speeds up registration checks that reject reuse of stellar or settlement keys
-- across either column (see rust-backend register handler).
--
-- Rollback:
--   DROP INDEX IF EXISTS merchants_stellar_public_key_idx;
--   DROP INDEX IF EXISTS merchants_settlement_public_key_idx;
--   No data is affected; registration uniqueness checks will fall back to sequential scans.
CREATE INDEX IF NOT EXISTS merchants_stellar_public_key_idx ON merchants (stellar_public_key);
CREATE INDEX IF NOT EXISTS merchants_settlement_public_key_idx ON merchants (settlement_public_key);
