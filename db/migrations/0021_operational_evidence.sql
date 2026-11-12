CREATE TABLE encryption_key_rotations (
 id uuid PRIMARY KEY,
 old_fingerprint text NOT NULL CHECK(length(old_fingerprint)=64),
 new_fingerprint text NOT NULL CHECK(length(new_fingerprint)=64),
 mfa_count bigint NOT NULL,
 mail_count bigint NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 CHECK(old_fingerprint<>new_fingerprint)
);
CREATE TABLE moderation_evidence (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 actor_id uuid NOT NULL REFERENCES app_users(id),
 resource_type text NOT NULL CHECK(resource_type IN ('creator','media','report','appeal','post','user')),
 resource_id uuid NOT NULL,
 decision text NOT NULL,
 reason text NOT NULL,
 policy_version text NOT NULL,
 snapshot jsonb NOT NULL,
 snapshot_sha256 text NOT NULL CHECK(length(snapshot_sha256)=64),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX moderation_evidence_resource ON moderation_evidence(resource_type,resource_id,created_at);
CREATE TRIGGER moderation_evidence_immutable BEFORE UPDATE OR DELETE ON moderation_evidence
 FOR EACH ROW EXECUTE FUNCTION reject_policy_evidence_change();
CREATE TRIGGER encryption_key_rotations_immutable BEFORE UPDATE OR DELETE ON encryption_key_rotations
 FOR EACH ROW EXECUTE FUNCTION reject_policy_evidence_change();
