#!/bin/bash
# Emergency rollback script
# Usage: ./emergency-rollback.sh [reason]

set -e

REASON=${1:-"Manual rollback"}
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

echo "🚨 EMERGENCY ROLLBACK INITIATED"
echo "Reason: $REASON"
echo "Time: $TIMESTAMP"

# 1. Disable Rust backend immediately via environment variables
echo "1. Disabling Rust backend features..."
export RUST_BACKEND_ENABLED=false
export RUST_AUTH_ROUTES_ENABLED=false
export RUST_INVOICE_ROUTES_ENABLED=false
export RUST_RECONCILE_ENABLED=false
export RUST_SETTLE_ENABLED=false

# 2. Scale down Rust backend deployment (if using Kubernetes)
if command -v kubectl &> /dev/null; then
    echo "2. Scaling down Rust backend deployment..."
    kubectl patch deployment rust-backend -p '{"spec":{"replicas":0}}' || echo "⚠️  kubectl not available or deployment not found"
fi

# 3. Verify Next.js is handling requests
echo "3. Verifying Next.js availability..."
NEXTJS_URL=${NEXTJS_URL:-"http://localhost:3000"}
curl -s -f "$NEXTJS_URL/api/health" > /dev/null || {
    echo "❌ Next.js not responding! Manual intervention required."
    exit 1
}
echo "✅ Next.js is responding"

# 4. Check for stuck transactions
echo "4. Checking for stuck transactions..."
if [[ -n "$DATABASE_URL" ]]; then
    STUCK_COUNT=$(psql "$DATABASE_URL" -t -c "
        SELECT COUNT(*) FROM invoices 
        WHERE status = 'pending' 
        AND created_at > NOW() - INTERVAL '10 minutes';
    " | tr -d ' ')
    
    if [[ "$STUCK_COUNT" -gt 0 ]]; then
        echo "⚠️  Found $STUCK_COUNT potentially stuck transactions"
        echo "   Manual review required for invoices created in last 10 minutes"
    else
        echo "✅ No stuck transactions detected"
    fi
else
    echo "⚠️  DATABASE_URL not set, skipping transaction check"
fi

# 5. Log rollback event
echo "5. Logging rollback event..."
if [[ -n "$DATABASE_URL" ]]; then
    psql "$DATABASE_URL" -c "
        INSERT INTO deployment_events (event_type, reason, metadata, created_at)
        VALUES (
            'emergency_rollback',
            '$REASON',
            '{\"timestamp\": \"$TIMESTAMP\", \"script\": \"emergency-rollback.sh\"}',
            NOW()
        );
    " || echo "⚠️  Could not log to database"
fi

# 6. Send notification (if webhook URL is configured)
if [[ -n "$ROLLBACK_WEBHOOK_URL" ]]; then
    echo "6. Sending notification..."
    curl -s -X POST "$ROLLBACK_WEBHOOK_URL" \
        -H "Content-Type: application/json" \
        -d "{
            \"text\": \"🚨 Emergency rollback executed\",
            \"reason\": \"$REASON\",
            \"timestamp\": \"$TIMESTAMP\"
        }" || echo "⚠️  Notification failed"
fi

echo ""
echo "✅ ROLLBACK COMPLETED"
echo ""
echo "Next steps:"
echo "1. Verify application functionality: $NEXTJS_URL"
echo "2. Check monitoring dashboards for error rates"
echo "3. Review logs for root cause analysis"
echo "4. Update incident documentation"
echo ""
echo "To re-enable Rust backend:"
echo "  export RUST_BACKEND_ENABLED=true"
echo "  kubectl scale deployment rust-backend --replicas=1"