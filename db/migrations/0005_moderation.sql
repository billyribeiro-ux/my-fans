ALTER TABLE posts ADD COLUMN moderation_blocked boolean NOT NULL DEFAULT false;
CREATE INDEX audit_recent ON audit_events(created_at DESC,id DESC);
CREATE INDEX jobs_failed ON jobs(created_at DESC) WHERE status='failed';
