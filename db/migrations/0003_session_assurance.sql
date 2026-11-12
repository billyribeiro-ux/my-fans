ALTER TABLE sessions ADD COLUMN mfa_verified_at timestamptz;
CREATE INDEX posts_feed ON posts(published_at DESC,id DESC) WHERE state='published';
CREATE INDEX comments_post ON comments(post_id,created_at,id);
CREATE INDEX checkouts_creator ON checkouts(creator_id,paid_at DESC) WHERE status='paid';
ALTER TABLE email_deliveries ADD CONSTRAINT delivery_status CHECK(status IN ('pending','sent','failed'));
ALTER TABLE jobs ADD CONSTRAINT jobs_attempts CHECK(attempts>=0);
