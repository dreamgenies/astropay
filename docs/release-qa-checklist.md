# Release QA Checklist

This checklist ensures that releases are thoroughly tested before deployment.

## Functional Tests
- [ ] Create a new invoice and verify it displays correctly
- [ ] Pay an invoice using Freighter wallet and confirm payment
- [ ] Verify invoice status updates to paid after payment
- [ ] Check that payout is queued after payment
- [ ] Run settle cron and verify payout is submitted to Stellar
- [ ] Run reconcile cron and verify payout is marked as settled
- [ ] Test expired invoice handling
- [ ] Test payment with wrong asset (should record mismatch)
- [ ] Test payment with wrong amount (should record mismatch)
- [ ] Test payment with wrong memo (should record mismatch)

## Edge Cases
- [ ] Concurrent settle runs don't double-process payouts
- [ ] Failed payouts are retried with backoff
- [ ] Dead-lettered payouts after max failures
- [ ] Orphan payments are handled
- [ ] Invalid Stellar public keys are rejected

## Performance
- [ ] Reconciliation handles large number of pending invoices
- [ ] Settlement processes batch sizes correctly
- [ ] Database queries are efficient

## Security
- [ ] Cron endpoints require proper authorization
- [ ] Sensitive data is redacted in logs
- [ ] No secrets exposed in responses

## Deployment
- [ ] Docker containers build successfully
- [ ] Database migrations run without errors
- [ ] Environment variables are properly configured
- [ ] Health checks pass