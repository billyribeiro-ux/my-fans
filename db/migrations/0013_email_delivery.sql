-- Existing local inbox messages never become eligible for external delivery.
ALTER TABLE email_deliveries ADD COLUMN transport text NOT NULL DEFAULT 'sandbox'
    CHECK (transport IN ('sandbox','smtp'));
ALTER TABLE email_deliveries ADD COLUMN encrypted_body bytea;
ALTER TABLE email_deliveries ADD COLUMN recovery_digest text;
ALTER TABLE email_deliveries ADD COLUMN expires_at timestamptz NOT NULL DEFAULT now()+interval '1 day';
ALTER TABLE email_deliveries ADD COLUMN attempts integer NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 8);
ALTER TABLE email_deliveries ADD COLUMN available_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE email_deliveries ADD COLUMN locked_at timestamptz;
ALTER TABLE email_deliveries ADD COLUMN lease_id uuid;
ALTER TABLE email_deliveries ADD COLUMN sent_at timestamptz;
ALTER TABLE email_deliveries ADD COLUMN last_error text;
ALTER TABLE email_deliveries DROP CONSTRAINT delivery_status;
ALTER TABLE email_deliveries ADD CONSTRAINT delivery_status
    CHECK (status IN ('pending','sending','sent','failed','cancelled'));
ALTER TABLE email_deliveries ADD CONSTRAINT mail_private_body
    CHECK (transport <> 'smtp' OR body = 'Message queued for email delivery.');
ALTER TABLE email_deliveries ADD CONSTRAINT mail_lease
    CHECK ((status = 'sending' AND locked_at IS NOT NULL AND lease_id IS NOT NULL)
        OR (status <> 'sending' AND locked_at IS NULL AND lease_id IS NULL));
ALTER TABLE email_deliveries ADD CONSTRAINT mail_recovery_digest
    CHECK (recovery_digest IS NULL OR recovery_digest ~ '^[0-9a-f]{64}$');
CREATE INDEX smtp_mail_ready ON email_deliveries(available_at,id)
    WHERE transport='smtp' AND status='pending';
CREATE INDEX mail_recovery_token ON email_deliveries(recovery_digest)
    WHERE recovery_digest IS NOT NULL;
