-- Tracks repeated settlement failures and surfaces chronic failures to operators.
-- A payout is moved to dead-letter after PAYOUT_DEAD_LETTER_THRESHOLD consecutive failures
-- (application constant, currently 5). The original payouts row is kept for audit; status
-- is set to 'dead_lettered' (added to the CHECK constraint below).
--
-- Rollback:
--   DROP TABLE IF EXISTS payout_dead_letters;
--   ALTER TABLE payouts DROP CONSTRAINT IF EXISTS payouts_status_check;
--   ALTER TABLE payouts ADD CONSTRAINT payouts_status_check
--       CHECK (status IN ('queued', 'submitted', 'settled', 'failed'));
--   ALTER TABLE payouts DROP COLUMN IF EXISTS failure_count;
--   ALTER TABLE payouts DROP COLUMN IF EXISTS last_failure_at;
--   WARNING: any payouts currently in 'dead_lettered' status will violate the
--   restored constraint. Resolve or re-queue those rows before rolling back.

-- Extend payouts status to include dead_lettered.
ALTER TABLE payouts
    DROP CONSTRAINT IF EXISTS payouts_status_check;

ALTER TABLE payouts
    ADD CONSTRAINT payouts_status_check
        CHECK (status IN ('queued', 'submitted', 'settled', 'failed', 'dead_lettered'));

-- Failure tracking columns on payouts.
ALTER TABLE payouts
    ADD COLUMN IF NOT EXISTS failure_count   INTEGER     NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS last_failure_at TIMESTAMPTZ;

-- Operator-visible dead-letter table: one row per payout that exceeded the threshold.
CREATE TABLE IF NOT EXISTS payout_dead_letters (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    payout_id   UUID        NOT NULL UNIQUE REFERENCES payouts(id) ON DELETE CASCADE,
    invoice_id  UUID        NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    merchant_id UUID        NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    failure_count INTEGER   NOT NULL,
    last_failure_reason TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS payout_dead_letters_merchant_id_idx ON payout_dead_letters (merchant_id);
CREATE INDEX IF NOT EXISTS payout_dead_letters_created_at_idx  ON payout_dead_letters (created_at DESC);
