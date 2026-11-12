ALTER TABLE app_users ADD COLUMN mfa_recovery_required boolean NOT NULL DEFAULT false;
ALTER TABLE app_users ADD COLUMN mfa_replacement_pending boolean NOT NULL DEFAULT false;
ALTER TABLE app_users ADD CONSTRAINT recovery_requires_mfa CHECK(NOT mfa_recovery_required OR mfa_enabled);
ALTER TABLE app_users ADD CONSTRAINT replacement_requires_recovery CHECK(NOT mfa_replacement_pending OR mfa_recovery_required);
CREATE TABLE mfa_recovery_codes (
    code_digest text PRIMARY KEY CHECK(code_digest ~ '^[a-f0-9]{64}$'),
    user_id uuid NOT NULL REFERENCES app_users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX mfa_recovery_codes_user ON mfa_recovery_codes(user_id);
