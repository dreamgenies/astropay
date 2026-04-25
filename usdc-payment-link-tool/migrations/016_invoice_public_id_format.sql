-- Migration 016: enforce format constraint on invoices.public_id
--
-- All public IDs must match inv_[0-9a-f]{16} (the "inv_" prefix followed by
-- exactly 16 lowercase hex characters, e.g. inv_3f8a1b2c4d5e6f7a).
--
-- Both generators already produce this format:
--   Rust:    auth::generate_public_id()  → format!("inv_{}", hex::encode([0u8; 8]))
--   Next.js: generatePublicId()          → `inv_${crypto.randomBytes(8).toString('hex')}`
--
-- No existing rows need backfilling; the constraint is safe to apply immediately.

ALTER TABLE invoices
  ADD CONSTRAINT invoices_public_id_format
  CHECK (public_id ~ '^inv_[0-9a-f]{16}$');
