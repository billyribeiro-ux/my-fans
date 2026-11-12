ALTER TABLE app_users ADD COLUMN password_hash text;
ALTER TABLE app_users ADD COLUMN role text NOT NULL DEFAULT 'fan' CHECK(role IN ('fan','creator','admin'));
ALTER TABLE app_users ADD COLUMN email_verified boolean NOT NULL DEFAULT false;
ALTER TABLE app_users ADD COLUMN avatar_url text NOT NULL DEFAULT '';
ALTER TABLE app_users ADD COLUMN disabled_at timestamptz;
ALTER TABLE app_users ADD COLUMN mfa_secret bytea;
ALTER TABLE app_users ADD COLUMN mfa_enabled boolean NOT NULL DEFAULT false;
ALTER TABLE app_users ADD COLUMN mfa_last_step bigint NOT NULL DEFAULT 0;
ALTER TABLE creators ADD COLUMN display_name text NOT NULL DEFAULT '';
ALTER TABLE creators ADD COLUMN bio text NOT NULL DEFAULT '';
ALTER TABLE creators ADD COLUMN category text NOT NULL DEFAULT 'Lifestyle';
ALTER TABLE creators ADD COLUMN avatar_url text NOT NULL DEFAULT '';
ALTER TABLE creators ADD COLUMN cover_url text NOT NULL DEFAULT '';
ALTER TABLE creators ADD COLUMN verification_status text NOT NULL DEFAULT 'pending' CHECK(verification_status IN ('pending','approved','rejected'));
ALTER TABLE posts ADD COLUMN scheduled_at timestamptz;
ALTER TABLE posts ADD COLUMN sample_media_url text;
ALTER TABLE posts ADD COLUMN sample_preview_url text NOT NULL DEFAULT '';
ALTER TABLE media_assets ADD COLUMN preview_key text;
ALTER TABLE media_assets ADD COLUMN content_type text NOT NULL DEFAULT 'application/octet-stream';
ALTER TABLE media_assets ADD COLUMN rejection_reason text;

CREATE TABLE sessions (
 token_digest text PRIMARY KEY, user_id uuid NOT NULL REFERENCES app_users(id),
 created_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL DEFAULT now()+interval '1 day'
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE recovery_tokens (
 token_digest text PRIMARY KEY, user_id uuid NOT NULL REFERENCES app_users(id),
 kind text NOT NULL CHECK(kind IN ('reset','verify')), expires_at timestamptz NOT NULL DEFAULT now()+interval '30 minutes', used_at timestamptz
);
CREATE TABLE rate_limits (bucket text PRIMARY KEY, window_start timestamptz NOT NULL DEFAULT now(), attempts integer NOT NULL DEFAULT 1);
CREATE TABLE likes (user_id uuid REFERENCES app_users(id),post_id uuid REFERENCES posts(id),PRIMARY KEY(user_id,post_id));
CREATE TABLE bookmarks (user_id uuid REFERENCES app_users(id),post_id uuid REFERENCES posts(id),PRIMARY KEY(user_id,post_id));
CREATE TABLE comments (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_id uuid NOT NULL REFERENCES app_users(id),post_id uuid NOT NULL REFERENCES posts(id),body text NOT NULL CHECK(length(body) BETWEEN 1 AND 500),created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE conversations (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_a uuid NOT NULL REFERENCES app_users(id),user_b uuid NOT NULL REFERENCES app_users(id),created_at timestamptz NOT NULL DEFAULT now(),CHECK(user_a<user_b),UNIQUE(user_a,user_b));
CREATE TABLE messages (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),conversation_id uuid NOT NULL REFERENCES conversations(id),sender_id uuid NOT NULL REFERENCES app_users(id),body text NOT NULL CHECK(length(body) BETWEEN 1 AND 4000),created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX messages_conversation ON messages(conversation_id,created_at,id);
CREATE TABLE notifications (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_id uuid NOT NULL REFERENCES app_users(id),title text NOT NULL,body text NOT NULL,read_at timestamptz,created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX notifications_user ON notifications(user_id,created_at DESC);
CREATE TABLE email_deliveries (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_id uuid NOT NULL REFERENCES app_users(id),recipient text NOT NULL,subject text NOT NULL,body text NOT NULL,status text NOT NULL DEFAULT 'pending',created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE checkouts (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_id uuid NOT NULL REFERENCES app_users(id),creator_id uuid NOT NULL REFERENCES creators(id),post_id uuid REFERENCES posts(id),
 kind text NOT NULL CHECK(kind IN ('subscription','purchase','tip')),amount_minor bigint NOT NULL CHECK(amount_minor BETWEEN 100 AND 10000000),currency text NOT NULL DEFAULT 'USD' CHECK(currency='USD'),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','paid','refunded','failed')),idempotency_key uuid NOT NULL,request_fingerprint text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),paid_at timestamptz,refunded_minor bigint NOT NULL DEFAULT 0 CHECK(refunded_minor>=0 AND refunded_minor<=amount_minor),UNIQUE(user_id,idempotency_key)
);
CREATE TABLE payouts (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_id uuid NOT NULL REFERENCES app_users(id),amount_minor bigint NOT NULL CHECK(amount_minor>0),currency text NOT NULL DEFAULT 'USD',
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','approved','rejected','paid')),idempotency_key uuid NOT NULL,created_at timestamptz NOT NULL DEFAULT now(),UNIQUE(user_id,idempotency_key)
);
CREATE TABLE reports (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),user_id uuid NOT NULL REFERENCES app_users(id),post_id uuid NOT NULL REFERENCES posts(id),reason text NOT NULL CHECK(length(reason) BETWEEN 5 AND 2000),status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','resolved','rejected')),created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE jobs (id uuid PRIMARY KEY DEFAULT gen_random_uuid(),kind text NOT NULL,payload jsonb NOT NULL,status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','done','failed')),attempts integer NOT NULL DEFAULT 0,run_at timestamptz NOT NULL DEFAULT now(),locked_at timestamptz,last_error text,created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX jobs_ready ON jobs(run_at) WHERE status='pending';
CREATE UNIQUE INDEX ledger_owner_account ON ledger_accounts(owner_id,kind,currency) NULLS NOT DISTINCT;
CREATE TABLE payment_webhook_inbox (provider text NOT NULL,event_id text NOT NULL,payload_sha256 text NOT NULL,received_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(provider,event_id));
