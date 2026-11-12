CREATE TABLE user_blocks (
 user_id uuid NOT NULL REFERENCES app_users(id),
 blocked_user_id uuid NOT NULL REFERENCES app_users(id),
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(user_id,blocked_user_id),
 CHECK(user_id<>blocked_user_id)
);
CREATE INDEX user_blocks_target ON user_blocks(blocked_user_id,user_id);
CREATE TABLE moderation_appeals (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 user_id uuid NOT NULL REFERENCES app_users(id),
 post_id uuid NOT NULL REFERENCES posts(id),
 reason text NOT NULL CHECK(length(reason) BETWEEN 5 AND 2000),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted','rejected')),
 decision_reason text CHECK(length(decision_reason) BETWEEN 5 AND 2000),
 reviewed_by uuid REFERENCES app_users(id),
 created_at timestamptz NOT NULL DEFAULT now(),
 reviewed_at timestamptz,
 CHECK((status='pending' AND reviewed_at IS NULL AND reviewed_by IS NULL AND decision_reason IS NULL) OR (status<>'pending' AND reviewed_at IS NOT NULL AND reviewed_by IS NOT NULL AND decision_reason IS NOT NULL)),
 CHECK(reviewed_by IS NULL OR reviewed_by<>user_id)
);
CREATE UNIQUE INDEX moderation_appeals_pending ON moderation_appeals(post_id) WHERE status='pending';
CREATE INDEX moderation_appeals_owner ON moderation_appeals(user_id,created_at DESC,id DESC);
