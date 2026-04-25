// Test for deployment events and rollback functionality
// Run with: npm test -- deployment-events.test.js

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { Pool } from 'pg';

const pool = new Pool({
  connectionString: process.env.DATABASE_URL || 'postgres://postgres:postgres@localhost:5432/astropay_test'
});

describe('Deployment Events and Rollback', () => {
  beforeAll(async () => {
    // Ensure deployment_events table exists
    await pool.query(`
      CREATE TABLE IF NOT EXISTS deployment_events (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        event_type VARCHAR(50) NOT NULL,
        reason TEXT,
        metadata JSONB DEFAULT '{}',
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        created_by VARCHAR(100)
      );
    `);
  });

  afterAll(async () => {
    await pool.end();
  });

  it('should record deployment events', async () => {
    const result = await pool.query(`
      INSERT INTO deployment_events (event_type, reason, metadata, created_by)
      VALUES ($1, $2, $3, $4)
      RETURNING id, event_type, reason
    `, ['deployment', 'Test deployment', '{"stage": "test"}', 'test-suite']);

    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].event_type).toBe('deployment');
    expect(result.rows[0].reason).toBe('Test deployment');
  });

  it('should record rollback events', async () => {
    const result = await pool.query(`
      INSERT INTO deployment_events (event_type, reason, metadata, created_by)
      VALUES ($1, $2, $3, $4)
      RETURNING id, event_type, metadata
    `, ['emergency_rollback', 'Test rollback', '{"trigger": "error_rate", "threshold": "1%"}', 'test-suite']);

    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].event_type).toBe('emergency_rollback');
    expect(JSON.parse(result.rows[0].metadata).trigger).toBe('error_rate');
  });

  it('should query recent deployment events', async () => {
    // Insert a few test events
    await pool.query(`
      INSERT INTO deployment_events (event_type, reason, created_by)
      VALUES 
        ('deployment', 'Stage 1 cutover', 'ops-team'),
        ('rollback', 'Performance issues', 'ops-team'),
        ('deployment', 'Stage 2 cutover', 'ops-team')
    `);

    const result = await pool.query(`
      SELECT event_type, reason 
      FROM deployment_events 
      WHERE created_by = 'ops-team'
      ORDER BY created_at DESC 
      LIMIT 3
    `);

    expect(result.rows).toHaveLength(3);
    expect(result.rows[0].event_type).toBe('deployment'); // Most recent
    expect(result.rows[1].event_type).toBe('rollback');
    expect(result.rows[2].event_type).toBe('deployment');
  });

  it('should support deployment checkpoint creation', async () => {
    // Create test data
    await pool.query(`
      CREATE TEMP TABLE test_invoices AS
      SELECT 
        gen_random_uuid() as id,
        'pending' as status,
        '10.00' as amount,
        NOW() as created_at
      FROM generate_series(1, 5);
    `);

    // Create checkpoint
    await pool.query(`
      CREATE TEMP TABLE test_checkpoint AS
      SELECT 
        COUNT(*) as total_invoices,
        COUNT(*) FILTER (WHERE status = 'pending') as pending_invoices,
        NOW() as checkpoint_time
      FROM test_invoices;
    `);

    const result = await pool.query(`
      SELECT total_invoices, pending_invoices 
      FROM test_checkpoint
    `);

    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].total_invoices).toBe('5');
    expect(result.rows[0].pending_invoices).toBe('5');
  });
});