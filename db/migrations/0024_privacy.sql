ALTER TABLE app_users ADD COLUMN erased_at timestamptz;

CREATE TABLE privacy_requests (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 user_id uuid NOT NULL UNIQUE REFERENCES app_users(id),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','approved','purging','application_erased','completed')),
 privacy_version text NOT NULL,
 retention jsonb NOT NULL CHECK(jsonb_typeof(retention)='object'),
 created_at timestamptz NOT NULL DEFAULT now(),
 not_before timestamptz NOT NULL,
 legal_hold boolean NOT NULL DEFAULT false,
 approved_by uuid REFERENCES app_users(id),
 approved_at timestamptz,
 asset_cursor uuid,
 objects_enumerated boolean NOT NULL DEFAULT false,
 application_erased_at timestamptz,
 completed_at timestamptz,
 versions_erased_at timestamptz,
 purge_bucket smallint NOT NULL DEFAULT 0 CHECK(purge_bucket BETWEEN 0 AND 2),
 purge_key_marker text,
 purge_version_marker text,
 next_at timestamptz NOT NULL DEFAULT now(),
 next_cancel_at timestamptz NOT NULL DEFAULT now(),
 CHECK(approved_by IS NULL OR approved_by<>user_id),
 CHECK((approved_by IS NULL)=(approved_at IS NULL)),
 CHECK((status IN ('application_erased','completed'))=(application_erased_at IS NOT NULL)),
 CHECK((status='completed')=(completed_at IS NOT NULL))
);
CREATE INDEX privacy_requests_due ON privacy_requests(next_at,id) WHERE status IN ('approved','purging') AND NOT legal_hold;

CREATE TABLE privacy_objects (
 request_id uuid NOT NULL REFERENCES privacy_requests(id),
 storage_key text NOT NULL,
 removed_at timestamptz,
 attempts integer NOT NULL DEFAULT 0 CHECK(attempts>=0),
 next_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(request_id,storage_key)
);
CREATE INDEX privacy_objects_pending ON privacy_objects(next_at,request_id) WHERE removed_at IS NULL;

-- Owner-only cloud maintenance enumerates first, deletes second. Deleting while
-- paginating would invalidate version markers and risk skipping historical data.
CREATE TABLE privacy_version_deletions (
 request_id uuid NOT NULL REFERENCES privacy_requests(id),
 bucket text NOT NULL,
 object_key text NOT NULL,
 version_id text NOT NULL,
 replication_checked boolean NOT NULL DEFAULT false,
 removed_at timestamptz,
 PRIMARY KEY(request_id,bucket,object_key,version_id)
);

CREATE TABLE privacy_actions (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 request_id uuid NOT NULL REFERENCES privacy_requests(id),
 actor_id uuid NOT NULL REFERENCES app_users(id),
 action text NOT NULL CHECK(action IN ('requested','approved','hold','release','completed')),
 reason text NOT NULL CHECK(length(reason) BETWEEN 5 AND 500),
 evidence jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(evidence)='object'),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER privacy_actions_immutable BEFORE UPDATE OR DELETE ON privacy_actions FOR EACH ROW EXECUTE FUNCTION reject_policy_evidence_change();

-- Content writers and erasure requests serialize on the same account row.
-- Existing financial references remain valid against the pseudonymous user UUID.
CREATE FUNCTION privacy_content_owner_active() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owner_id uuid; disabled timestamptz;
BEGIN
 IF TG_TABLE_NAME IN ('posts','media_assets','media_uploads') THEN
  SELECT user_id INTO owner_id FROM creators WHERE id=NEW.creator_id;
 ELSIF TG_TABLE_NAME='messages' THEN owner_id:=NEW.sender_id;
 ELSE owner_id:=NEW.user_id;
 END IF;
 SELECT disabled_at INTO disabled FROM app_users WHERE id=owner_id FOR SHARE;
 IF disabled IS NOT NULL THEN
  RAISE EXCEPTION 'Disabled accounts cannot create content' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER privacy_post_insert BEFORE INSERT ON posts FOR EACH ROW EXECUTE FUNCTION privacy_content_owner_active();
CREATE TRIGGER privacy_asset_insert BEFORE INSERT ON media_assets FOR EACH ROW EXECUTE FUNCTION privacy_content_owner_active();
CREATE TRIGGER privacy_upload_insert BEFORE INSERT ON media_uploads FOR EACH ROW EXECUTE FUNCTION privacy_content_owner_active();
CREATE TRIGGER privacy_comment_insert BEFORE INSERT ON comments FOR EACH ROW EXECUTE FUNCTION privacy_content_owner_active();
CREATE TRIGGER privacy_message_insert BEFORE INSERT ON messages FOR EACH ROW EXECUTE FUNCTION privacy_content_owner_active();

CREATE FUNCTION privacy_refuse_republication() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE disabled timestamptz;
BEGIN
 IF NEW.state<>'removed' THEN
  SELECT u.disabled_at INTO disabled FROM app_users u JOIN creators c ON c.user_id=u.id WHERE c.id=NEW.creator_id FOR SHARE OF u;
  IF disabled IS NOT NULL THEN RAISE EXCEPTION 'Disabled account content cannot be published' USING ERRCODE='23514'; END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER privacy_post_update BEFORE UPDATE ON posts FOR EACH ROW EXECUTE FUNCTION privacy_refuse_republication();

CREATE FUNCTION privacy_has_obligations(subject uuid) RETURNS boolean LANGUAGE sql STABLE AS $$
 SELECT
 EXISTS(SELECT 1 FROM subscriptions x JOIN creators c ON c.id=x.creator_id WHERE (x.user_id=subject OR c.user_id=subject) AND x.status IN ('pending','active','past_due')) OR
 EXISTS(SELECT 1 FROM checkouts x JOIN creators c ON c.id=x.creator_id WHERE (x.user_id=subject OR c.user_id=subject) AND x.status='pending') OR
 EXISTS(SELECT 1 FROM payouts WHERE user_id=subject AND status IN ('pending','approved')) OR
 EXISTS(SELECT a.id FROM ledger_accounts a JOIN ledger_postings p ON p.account_id=a.id JOIN ledger_transactions t ON t.id=p.transaction_id WHERE a.owner_id=subject AND a.kind IN ('creator_pending','creator_available','reserve') AND t.status='posted' GROUP BY a.id HAVING sum(p.amount_minor)<>0) OR
 EXISTS(SELECT 1 FROM provider_finance_issues x JOIN provider_accounts a ON a.account_id=x.account_id WHERE a.user_id=subject AND x.resolved_at IS NULL) OR
 EXISTS(SELECT 1 FROM provider_payouts x JOIN provider_accounts a ON a.account_id=x.account_id WHERE a.user_id=subject AND x.status IN ('pending','in_transit')) OR
 EXISTS(SELECT 1 FROM provider_disputes d JOIN provider_receipts r ON r.account_id=d.account_id AND r.charge_id=d.charge_id JOIN checkouts x ON x.id=r.checkout_id JOIN creators c ON c.id=x.creator_id WHERE (x.user_id=subject OR c.user_id=subject) AND d.status IN ('warning_needs_response','warning_under_review','needs_response','under_review')) OR
 EXISTS(SELECT 1 FROM provider_financial_transactions t JOIN provider_accounts a ON a.account_id=t.account_id WHERE a.user_id=subject AND t.status='pending') OR
 EXISTS(SELECT 1 FROM media_uploads x JOIN creators c ON c.id=x.creator_id WHERE c.user_id=subject AND x.state='assembling') OR
 EXISTS(SELECT 1 FROM jobs j WHERE j.status='running' AND (EXISTS(SELECT 1 FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=subject AND j.payload->>'media_id'=m.id::text) OR EXISTS(SELECT 1 FROM media_uploads m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=subject AND j.payload->>'upload_id'=m.id::text)));
$$;
