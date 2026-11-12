-- A restored database cannot serve users until the separately signed privacy
-- journal has been replayed against its new, isolated media binding.
CREATE TABLE platform_recovery (
 id smallint PRIMARY KEY CHECK(id=1),
 status text NOT NULL CHECK(status IN ('normal','quarantined','reconciled')),
 source_storage_id text,
 target_storage_id text,
 quarantined_at timestamptz,
 reconciled_at timestamptz,
 journal_sha256 text,
 request_cursor integer NOT NULL DEFAULT 0 CHECK(request_cursor>=0),
 object_cursor integer NOT NULL DEFAULT 0 CHECK(object_cursor>=0),
 frozen_accounts integer NOT NULL DEFAULT 0 CHECK(frozen_accounts>=0),
 erased_accounts integer NOT NULL DEFAULT 0 CHECK(erased_accounts>=0),
 CHECK((status='normal')=(source_storage_id IS NULL)),
 CHECK((status='reconciled')=(reconciled_at IS NOT NULL)),
 CHECK(journal_sha256 IS NULL OR journal_sha256 ~ '^[a-f0-9]{64}$')
);
INSERT INTO platform_recovery(id,status) VALUES(1,'normal');
CREATE TABLE privacy_recovery_runs (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 journal_sha256 text NOT NULL UNIQUE CHECK(journal_sha256 ~ '^[a-f0-9]{64}$'),
 journal_version text NOT NULL,
 journal_exported_at timestamptz NOT NULL,
 source_database_identifier text NOT NULL,
 source_storage_id text NOT NULL,
 target_storage_id text NOT NULL,
 requests_count integer NOT NULL CHECK(requests_count>=0),
 frozen_accounts integer NOT NULL CHECK(frozen_accounts>=0),
 erased_accounts integer NOT NULL CHECK(erased_accounts>=0),
 excluded_objects integer NOT NULL CHECK(excluded_objects>=0),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER privacy_recovery_runs_immutable BEFORE UPDATE OR DELETE ON privacy_recovery_runs
 FOR EACH ROW EXECUTE FUNCTION reject_policy_evidence_change();
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='myfans_runtime') THEN
  REVOKE ALL ON privacy_recovery_runs,platform_recovery FROM myfans_runtime;
  GRANT SELECT ON platform_recovery TO myfans_runtime;
 END IF;
END $$;
