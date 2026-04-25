-- Migration: Add deployment events tracking table
-- This supports the rollback playbook by providing audit trail

CREATE TABLE IF NOT EXISTS deployment_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR(50) NOT NULL, -- 'deployment', 'rollback', 'emergency_rollback', 'cutover'
    reason TEXT,
    metadata JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by VARCHAR(100) -- operator/system identifier
);

-- Index for querying recent events
CREATE INDEX IF NOT EXISTS idx_deployment_events_created_at 
ON deployment_events(created_at DESC);

-- Index for querying by event type
CREATE INDEX IF NOT EXISTS idx_deployment_events_type 
ON deployment_events(event_type, created_at DESC);

-- Add comment for documentation
COMMENT ON TABLE deployment_events IS 'Audit trail for deployments, rollbacks, and architecture changes';
COMMENT ON COLUMN deployment_events.event_type IS 'Type of deployment event: deployment, rollback, emergency_rollback, cutover';
COMMENT ON COLUMN deployment_events.metadata IS 'Additional context like feature flags, affected services, etc.';

-- Example usage:
-- INSERT INTO deployment_events (event_type, reason, metadata, created_by)
-- VALUES ('deployment', 'Rust backend cutover stage 1', '{"rust_auth_enabled": true, "traffic_percentage": 10}', 'ops-team');