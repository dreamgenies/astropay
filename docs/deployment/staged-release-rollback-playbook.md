# Staged Release and Rollback Playbook

## Overview

This playbook covers staged deployment strategies for ASTROpay's dual-architecture system (Next.js + Rust backend) with safe rollback procedures that preserve invoice and payout state integrity.

## Architecture Change Categories

### 1. Backend Route Migration (Next.js → Rust)
- **Risk Level**: High
- **State Impact**: Invoice/payout data integrity critical
- **Rollback Complexity**: Medium

### 2. Database Schema Changes
- **Risk Level**: Critical
- **State Impact**: Direct data structure changes
- **Rollback Complexity**: High

### 3. Payment Flow Changes
- **Risk Level**: Critical
- **State Impact**: Money movement and reconciliation
- **Rollback Complexity**: High

## Staged Release Strategies

### Strategy 1: Feature Flag Cutover

**Use Case**: Migrating API routes from Next.js to Rust

**Implementation**:
```bash
# Environment variables for gradual cutover
RUST_BACKEND_ENABLED=false           # Global kill switch
RUST_AUTH_ROUTES_ENABLED=false       # Auth endpoints
RUST_INVOICE_ROUTES_ENABLED=false    # Invoice management
RUST_RECONCILE_ENABLED=false         # Payment reconciliation
RUST_SETTLE_ENABLED=false            # Settlement execution
```

**Stages**:
1. **Stage 0**: Deploy Rust backend with all flags `false` (shadow mode)
2. **Stage 1**: Enable `RUST_AUTH_ROUTES_ENABLED=true` (10% traffic)
3. **Stage 2**: Enable `RUST_INVOICE_ROUTES_ENABLED=true` (25% traffic)
4. **Stage 3**: Enable `RUST_RECONCILE_ENABLED=true` (50% traffic)
5. **Stage 4**: Enable `RUST_SETTLE_ENABLED=true` (100% traffic)

### Strategy 2: Merchant-Based Rollout

**Use Case**: Testing with specific merchant accounts

**Implementation**:
```sql
-- Add rollout tracking to merchants table
ALTER TABLE merchants ADD COLUMN rust_backend_enabled BOOLEAN DEFAULT false;

-- Enable for specific merchants
UPDATE merchants SET rust_backend_enabled = true 
WHERE email IN ('test@merchant.com', 'pilot@merchant.com');
```

### Strategy 3: Load Balancer Split

**Use Case**: Infrastructure-level traffic splitting

**Configuration** (nginx example):
```nginx
upstream nextjs_backend {
    server nextjs:3000 weight=90;
}

upstream rust_backend {
    server rust:8080 weight=10;
}

location /api/ {
    proxy_pass http://nextjs_backend;
    # Gradually increase rust_backend weight
}
```

## Pre-Deployment Verification

### 1. Database State Validation
```sql
-- Verify no pending transactions
SELECT COUNT(*) FROM invoices WHERE status = 'pending';
SELECT COUNT(*) FROM payouts WHERE status IN ('queued', 'failed');

-- Check for orphaned records
SELECT i.id FROM invoices i 
LEFT JOIN merchants m ON i.merchant_id = m.id 
WHERE m.id IS NULL;
```

### 2. Service Health Checks
```bash
# Next.js health
curl -f http://localhost:3000/api/health || exit 1

# Rust backend health  
curl -f http://localhost:8080/health || exit 1

# Database connectivity
psql "$DATABASE_URL" -c "SELECT 1;" || exit 1
```

### 3. Critical Path Testing
```bash
# Test invoice creation flow
curl -X POST http://localhost:8080/api/invoices \
  -H "Content-Type: application/json" \
  -H "Cookie: astropay_session=<token>" \
  -d '{"amount": "10.00", "description": "Test"}'

# Test reconciliation endpoint
curl -H "Authorization: Bearer $CRON_SECRET" \
     http://localhost:8080/api/cron/reconcile
```

## Rollback Procedures

### Immediate Rollback (< 5 minutes)

**Trigger Conditions**:
- 5xx error rate > 1%
- Invoice creation failures > 0.1%
- Payment reconciliation failures
- Database connection errors

**Actions**:
```bash
# 1. Disable Rust backend immediately
export RUST_BACKEND_ENABLED=false

# 2. Route all traffic to Next.js
kubectl patch deployment rust-backend -p '{"spec":{"replicas":0}}'

# 3. Verify Next.js handling all requests
curl -f http://localhost:3000/api/invoices

# 4. Check for stuck transactions
psql "$DATABASE_URL" -c "
SELECT id, status, created_at FROM invoices 
WHERE status = 'pending' AND created_at > NOW() - INTERVAL '10 minutes';"
```

### Graceful Rollback (5-30 minutes)

**Use Case**: Non-critical issues, data inconsistencies

**Process**:
```bash
# 1. Stop new invoice creation on Rust backend
export RUST_INVOICE_ROUTES_ENABLED=false

# 2. Allow existing invoices to complete
sleep 300  # Wait 5 minutes for in-flight requests

# 3. Migrate active sessions back to Next.js
psql "$DATABASE_URL" -c "
UPDATE sessions SET metadata = jsonb_set(
  COALESCE(metadata, '{}'), 
  '{backend}', 
  '\"nextjs\"'
) WHERE expires_at > NOW();"

# 4. Disable remaining Rust routes
export RUST_AUTH_ROUTES_ENABLED=false
export RUST_RECONCILE_ENABLED=false
```

### Data Recovery Rollback (30+ minutes)

**Use Case**: Data corruption, payment state issues

**Critical Steps**:
```sql
-- 1. Identify affected invoices
CREATE TEMP TABLE affected_invoices AS
SELECT id, status, amount, created_at 
FROM invoices 
WHERE updated_at > '2024-01-01 12:00:00'  -- Deployment time
AND status IN ('paid', 'settled');

-- 2. Verify payment events integrity
SELECT i.id, i.status, COUNT(pe.id) as event_count
FROM affected_invoices i
LEFT JOIN payment_events pe ON i.id = pe.invoice_id
GROUP BY i.id, i.status
HAVING COUNT(pe.id) = 0 AND i.status = 'paid';

-- 3. Check payout consistency
SELECT p.id, p.status, i.status as invoice_status
FROM payouts p
JOIN affected_invoices i ON p.invoice_id = i.id
WHERE p.status != 'completed' AND i.status = 'settled';
```

## State Integrity Safeguards

### 1. Transaction Boundaries
```rust
// Rust backend - atomic invoice state changes
async fn mark_invoice_paid(
    tx: &mut Transaction<'_, Postgres>,
    invoice_id: Uuid,
    payment_hash: &str,
) -> Result<(), Error> {
    // Single transaction for all state changes
    let rows_affected = sqlx::query!(
        "UPDATE invoices SET status = 'paid' WHERE id = $1 AND status = 'pending'",
        invoice_id
    )
    .execute(&mut **tx)
    .await?
    .rows_affected();
    
    if rows_affected == 0 {
        return Err(Error::InvoiceAlreadyTransitioned);
    }
    
    // Insert payment event and payout in same transaction
    // ...
}
```

### 2. Idempotency Keys
```sql
-- Prevent duplicate payment processing
CREATE UNIQUE INDEX idx_payment_events_transaction_hash 
ON payment_events(transaction_hash) 
WHERE transaction_hash IS NOT NULL;

-- Prevent duplicate payouts
CREATE UNIQUE INDEX idx_payouts_invoice_id 
ON payouts(invoice_id);
```

### 3. Reconciliation Checkpoints
```bash
# Before deployment
psql "$DATABASE_URL" -c "
CREATE TABLE deployment_checkpoint AS
SELECT 
  COUNT(*) as total_invoices,
  COUNT(*) FILTER (WHERE status = 'pending') as pending_invoices,
  COUNT(*) FILTER (WHERE status = 'paid') as paid_invoices,
  SUM(amount::numeric) FILTER (WHERE status = 'paid') as total_paid_amount,
  NOW() as checkpoint_time
FROM invoices;"

# After rollback - verify consistency
psql "$DATABASE_URL" -c "
SELECT 
  'Current' as period,
  COUNT(*) as total_invoices,
  COUNT(*) FILTER (WHERE status = 'pending') as pending_invoices,
  COUNT(*) FILTER (WHERE status = 'paid') as paid_invoices,
  SUM(amount::numeric) FILTER (WHERE status = 'paid') as total_paid_amount
FROM invoices
UNION ALL
SELECT 
  'Checkpoint' as period,
  total_invoices,
  pending_invoices, 
  paid_invoices,
  total_paid_amount
FROM deployment_checkpoint;"
```

## Monitoring and Alerting

### Critical Metrics
```yaml
# Prometheus alerts
- alert: InvoiceCreationFailure
  expr: rate(invoice_creation_errors[5m]) > 0.01
  for: 1m
  
- alert: PaymentReconciliationLag
  expr: max(time() - payment_reconciliation_last_success) > 600
  for: 2m
  
- alert: PayoutQueueBacklog
  expr: payout_queue_size > 100
  for: 5m
```

### Health Check Endpoints
```rust
// Rust backend health check
#[get("/health")]
async fn health_check(pool: &PgPool) -> Result<Json<HealthStatus>, Error> {
    let db_ok = sqlx::query("SELECT 1").fetch_one(pool).await.is_ok();
    
    Ok(Json(HealthStatus {
        status: if db_ok { "healthy" } else { "unhealthy" },
        database: db_ok,
        timestamp: Utc::now(),
    }))
}
```

## Testing Procedures

### 1. Smoke Tests
```bash
#!/bin/bash
# smoke-test.sh - Run after each stage

set -e

echo "Testing authentication..."
TOKEN=$(curl -s -X POST http://localhost:8080/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email":"test@example.com","password":"test123"}' \
  | jq -r '.token // empty')

[[ -n "$TOKEN" ]] || { echo "Auth failed"; exit 1; }

echo "Testing invoice creation..."
INVOICE_ID=$(curl -s -X POST http://localhost:8080/api/invoices \
  -H "Content-Type: application/json" \
  -H "Cookie: astropay_session=$TOKEN" \
  -d '{"amount":"10.00","description":"Smoke test"}' \
  | jq -r '.id // empty')

[[ -n "$INVOICE_ID" ]] || { echo "Invoice creation failed"; exit 1; }

echo "Testing invoice retrieval..."
curl -s -f http://localhost:8080/api/invoices/$INVOICE_ID \
  -H "Cookie: astropay_session=$TOKEN" > /dev/null

echo "All smoke tests passed"
```

### 2. Load Tests
```bash
# Load test with gradual ramp-up
artillery run --config artillery.yml --target http://localhost:8080
```

### 3. Chaos Testing
```bash
# Simulate database connection loss
kubectl exec -it postgres-pod -- pg_ctl stop -m fast

# Wait 30 seconds, verify graceful degradation
sleep 30
curl -f http://localhost:8080/health

# Restore database
kubectl exec -it postgres-pod -- pg_ctl start
```

## Deployment Checklist

### Pre-Deployment
- [ ] Database backup completed
- [ ] Deployment checkpoint created
- [ ] Health checks passing
- [ ] Feature flags configured
- [ ] Monitoring alerts active
- [ ] Rollback plan reviewed

### During Deployment
- [ ] Stage 0: Shadow deployment verified
- [ ] Stage 1: 10% traffic cutover successful
- [ ] Stage 2: 25% traffic cutover successful  
- [ ] Stage 3: 50% traffic cutover successful
- [ ] Stage 4: 100% traffic cutover successful
- [ ] Post-deployment smoke tests passed

### Post-Deployment
- [ ] All metrics within normal ranges
- [ ] No error rate increases
- [ ] Invoice/payout state consistency verified
- [ ] Performance benchmarks met
- [ ] Documentation updated

## Recovery Time Objectives

| Incident Type | RTO | RPO | Rollback Method |
|---------------|-----|-----|-----------------|
| Service Unavailable | 2 minutes | 0 | Feature flag disable |
| Performance Degradation | 5 minutes | 0 | Traffic rerouting |
| Data Inconsistency | 15 minutes | 5 minutes | Graceful rollback |
| Data Corruption | 60 minutes | 15 minutes | Database restore |