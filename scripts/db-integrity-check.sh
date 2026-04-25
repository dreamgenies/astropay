#!/bin/bash
# Database integrity check script
# Usage: ./db-integrity-check.sh [checkpoint|verify]

set -e

ACTION=${1:-"verify"}
DATABASE_URL=${DATABASE_URL:-""}

if [[ -z "$DATABASE_URL" ]]; then
    echo "❌ DATABASE_URL environment variable is required"
    exit 1
fi

case "$ACTION" in
    "checkpoint")
        echo "📊 Creating deployment checkpoint..."
        psql "$DATABASE_URL" -c "
        DROP TABLE IF EXISTS deployment_checkpoint;
        CREATE TABLE deployment_checkpoint AS
        SELECT 
            COUNT(*) as total_invoices,
            COUNT(*) FILTER (WHERE status = 'pending') as pending_invoices,
            COUNT(*) FILTER (WHERE status = 'paid') as paid_invoices,
            COUNT(*) FILTER (WHERE status = 'settled') as settled_invoices,
            COUNT(*) FILTER (WHERE status = 'expired') as expired_invoices,
            COUNT(*) FILTER (WHERE status = 'failed') as failed_invoices,
            SUM(amount::numeric) FILTER (WHERE status = 'paid') as total_paid_amount,
            SUM(amount::numeric) FILTER (WHERE status = 'settled') as total_settled_amount,
            COUNT(DISTINCT merchant_id) as active_merchants,
            NOW() as checkpoint_time
        FROM invoices;
        
        -- Also checkpoint payouts
        INSERT INTO deployment_checkpoint 
        SELECT 
            0, 0, 0, 0, 0, 0, 0, 0, 0,
            COUNT(*) FILTER (WHERE status = 'queued') as queued_payouts,
            COUNT(*) FILTER (WHERE status = 'completed') as completed_payouts,
            COUNT(*) FILTER (WHERE status = 'failed') as failed_payouts,
            COUNT(*) FILTER (WHERE status = 'dead_lettered') as dead_lettered_payouts,
            SUM(amount::numeric) FILTER (WHERE status = 'completed') as total_payout_amount,
            NOW()
        FROM payouts;
        "
        echo "✅ Checkpoint created successfully"
        ;;
        
    "verify")
        echo "🔍 Running database integrity checks..."
        
        # Check 1: Orphaned records
        echo "1. Checking for orphaned invoices..."
        ORPHANED_INVOICES=$(psql "$DATABASE_URL" -t -c "
            SELECT COUNT(*) FROM invoices i 
            LEFT JOIN merchants m ON i.merchant_id = m.id 
            WHERE m.id IS NULL;
        " | tr -d ' ')
        
        if [[ "$ORPHANED_INVOICES" -gt 0 ]]; then
            echo "❌ Found $ORPHANED_INVOICES orphaned invoices"
        else
            echo "✅ No orphaned invoices"
        fi
        
        # Check 2: Inconsistent payout states
        echo "2. Checking payout consistency..."
        INCONSISTENT_PAYOUTS=$(psql "$DATABASE_URL" -t -c "
            SELECT COUNT(*) FROM payouts p
            JOIN invoices i ON p.invoice_id = i.id
            WHERE p.status = 'completed' AND i.status != 'settled';
        " | tr -d ' ')
        
        if [[ "$INCONSISTENT_PAYOUTS" -gt 0 ]]; then
            echo "❌ Found $INCONSISTENT_PAYOUTS inconsistent payouts"
        else
            echo "✅ Payout states consistent"
        fi
        
        # Check 3: Missing payment events for paid invoices
        echo "3. Checking payment events..."
        MISSING_EVENTS=$(psql "$DATABASE_URL" -t -c "
            SELECT COUNT(*) FROM invoices i
            LEFT JOIN payment_events pe ON i.id = pe.invoice_id
            WHERE i.status = 'paid' AND pe.id IS NULL;
        " | tr -d ' ')
        
        if [[ "$MISSING_EVENTS" -gt 0 ]]; then
            echo "❌ Found $MISSING_EVENTS paid invoices without payment events"
        else
            echo "✅ All paid invoices have payment events"
        fi
        
        # Check 4: Duplicate payouts
        echo "4. Checking for duplicate payouts..."
        DUPLICATE_PAYOUTS=$(psql "$DATABASE_URL" -t -c "
            SELECT COUNT(*) FROM (
                SELECT invoice_id, COUNT(*) 
                FROM payouts 
                GROUP BY invoice_id 
                HAVING COUNT(*) > 1
            ) duplicates;
        " | tr -d ' ')
        
        if [[ "$DUPLICATE_PAYOUTS" -gt 0 ]]; then
            echo "❌ Found $DUPLICATE_PAYOUTS invoices with duplicate payouts"
        else
            echo "✅ No duplicate payouts"
        fi
        
        # Check 5: Compare with checkpoint if it exists
        echo "5. Comparing with checkpoint..."
        CHECKPOINT_EXISTS=$(psql "$DATABASE_URL" -t -c "
            SELECT EXISTS(SELECT 1 FROM information_schema.tables 
                         WHERE table_name = 'deployment_checkpoint');
        " | tr -d ' ')
        
        if [[ "$CHECKPOINT_EXISTS" == "t" ]]; then
            echo "📊 Current vs Checkpoint comparison:"
            psql "$DATABASE_URL" -c "
            WITH current_state AS (
                SELECT 
                    'Current' as period,
                    COUNT(*) as total_invoices,
                    COUNT(*) FILTER (WHERE status = 'pending') as pending_invoices,
                    COUNT(*) FILTER (WHERE status = 'paid') as paid_invoices,
                    SUM(amount::numeric) FILTER (WHERE status = 'paid') as total_paid_amount
                FROM invoices
            )
            SELECT * FROM current_state
            UNION ALL
            SELECT 
                'Checkpoint' as period,
                total_invoices,
                pending_invoices, 
                paid_invoices,
                total_paid_amount
            FROM deployment_checkpoint
            WHERE total_invoices > 0;  -- Filter out payout checkpoint row
            "
        else
            echo "⚠️  No checkpoint found for comparison"
        fi
        
        # Summary
        TOTAL_ISSUES=$((ORPHANED_INVOICES + INCONSISTENT_PAYOUTS + MISSING_EVENTS + DUPLICATE_PAYOUTS))
        
        if [[ "$TOTAL_ISSUES" -eq 0 ]]; then
            echo ""
            echo "✅ All integrity checks passed!"
        else
            echo ""
            echo "❌ Found $TOTAL_ISSUES integrity issues that need attention"
            exit 1
        fi
        ;;
        
    *)
        echo "Usage: $0 [checkpoint|verify]"
        echo "  checkpoint - Create a deployment checkpoint"
        echo "  verify     - Run integrity checks"
        exit 1
        ;;
esac