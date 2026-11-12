-- Run as the migration owner. Grant this role to a separately managed login.
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='myfans_runtime') THEN
  CREATE ROLE myfans_runtime NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION;
 END IF;
END $$;
GRANT USAGE ON SCHEMA public TO myfans_runtime;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO myfans_runtime;
REVOKE DELETE ON moderation_appeals FROM myfans_runtime;
REVOKE DELETE ON checkouts,payouts,subscriptions,entitlements,provider_accounts,provider_receipts,payment_webhook_inbox,platform_storage,platform_providers,identity_checks,provider_reconciliation FROM myfans_runtime;
REVOKE ALL ON ledger_accounts,ledger_transactions,ledger_postings,audit_events,_sqlx_migrations FROM myfans_runtime;
GRANT SELECT,INSERT ON ledger_accounts,ledger_postings,audit_events TO myfans_runtime;
GRANT SELECT,INSERT,UPDATE ON ledger_transactions TO myfans_runtime;
GRANT SELECT ON _sqlx_migrations TO myfans_runtime;
-- Deliberately no TRUNCATE, table ownership, trigger management, or schema DDL.

REVOKE UPDATE,DELETE ON policy_acceptances,mail_feedback FROM myfans_runtime;
REVOKE DELETE ON mail_suppressions FROM myfans_runtime;

REVOKE UPDATE,DELETE ON provider_finance_actions,moderation_evidence FROM myfans_runtime;
REVOKE ALL ON encryption_key_rotations FROM myfans_runtime;
REVOKE DELETE ON provider_financial_transactions,provider_finance_issues,provider_receipt_finance,provider_payouts,provider_payout_members,provider_disputes,provider_financial_revocations,provider_finance_scans FROM myfans_runtime;

REVOKE DELETE ON media_uploads,media_upload_chunks FROM myfans_runtime;

REVOKE DELETE ON privacy_requests,privacy_objects FROM myfans_runtime;
REVOKE UPDATE,DELETE ON privacy_actions FROM myfans_runtime;

REVOKE ALL ON privacy_version_deletions FROM myfans_runtime;
REVOKE ALL ON privacy_retention_runs FROM myfans_runtime;
REVOKE EXECUTE ON FUNCTION privacy_retention_batch(uuid) FROM myfans_runtime;
REVOKE ALL ON privacy_recovery_runs,platform_recovery FROM myfans_runtime;
GRANT SELECT ON platform_recovery TO myfans_runtime;
