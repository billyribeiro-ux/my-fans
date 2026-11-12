CREATE TABLE mail_suppressions (
 recipient_digest text PRIMARY KEY CHECK(length(recipient_digest)=64),
 reason text NOT NULL CHECK(reason IN ('bounce','complaint')),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE mail_feedback (
 event_digest text PRIMARY KEY CHECK(length(event_digest)=64),
 delivery_id uuid NOT NULL REFERENCES email_deliveries(id),
 kind text NOT NULL CHECK(kind IN ('bounce','complaint','delivery','transient_bounce')),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX mail_feedback_delivery ON mail_feedback(delivery_id);
