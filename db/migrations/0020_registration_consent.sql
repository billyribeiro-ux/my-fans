CREATE TABLE policy_acceptances (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 user_id uuid NOT NULL REFERENCES app_users(id),
 country_code text NOT NULL CHECK (country_code ~ '^[A-Z]{2}$'),
 terms_version text NOT NULL,
 privacy_version text NOT NULL,
 content_policy_version text NOT NULL,
 minimum_age integer NOT NULL CHECK (minimum_age=18),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION reject_policy_evidence_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Policy acceptance evidence is immutable'; END $$;
CREATE TRIGGER policy_acceptances_immutable BEFORE UPDATE OR DELETE ON policy_acceptances
 FOR EACH ROW EXECUTE FUNCTION reject_policy_evidence_change();
CREATE INDEX policy_acceptances_user ON policy_acceptances(user_id,created_at);
