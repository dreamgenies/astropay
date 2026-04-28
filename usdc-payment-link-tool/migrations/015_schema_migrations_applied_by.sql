-- Add applied_by to schema_migrations so each row records which runtime applied it.
-- Valid values: 'rust', 'nextjs'. DEFAULT 'unknown' covers rows written before this migration.
ALTER TABLE schema_migrations
  ADD COLUMN IF NOT EXISTS applied_by TEXT NOT NULL DEFAULT 'unknown';
