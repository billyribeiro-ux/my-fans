-- Older global jobs cannot identify a failing account. Quiesce workers before
-- migration; their durable account checkpoints remain the source of truth.
UPDATE jobs SET status='done',locked_at=NULL
 WHERE kind='provider_reconcile' AND status IN ('pending','running');
DROP INDEX provider_reconcile_job;
CREATE UNIQUE INDEX provider_reconcile_account_job ON jobs((payload->>'account'))
 WHERE kind='provider_reconcile' AND status IN ('pending','running');
