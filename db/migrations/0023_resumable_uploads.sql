CREATE TABLE media_uploads (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 creator_id uuid NOT NULL REFERENCES creators(id),
 size_bytes bigint NOT NULL CHECK(size_bytes BETWEEN 1 AND 268435456),
 content_type text NOT NULL CHECK(content_type IN ('image/jpeg','image/png','image/webp','video/mp4','video/webm')),
 offset_bytes bigint NOT NULL DEFAULT 0 CHECK(offset_bytes>=0 AND offset_bytes<=size_bytes),
 state text NOT NULL DEFAULT 'uploading' CHECK(state IN ('uploading','assembling','complete','cancelled')),
 created_at timestamptz NOT NULL DEFAULT now(),
 expires_at timestamptz NOT NULL DEFAULT now()+interval '1 day',
 cleaned_at timestamptz,
 CHECK(content_type LIKE 'video/%' OR size_bytes<=10485760)
);
CREATE TABLE media_upload_chunks (
 upload_id uuid NOT NULL REFERENCES media_uploads(id),
 offset_bytes bigint NOT NULL,
 size_bytes integer NOT NULL CHECK(size_bytes BETWEEN 1 AND 4194304),
 storage_key text NOT NULL UNIQUE,
 sha256 text NOT NULL CHECK(length(sha256)=64),
 PRIMARY KEY(upload_id,offset_bytes)
);
CREATE INDEX media_upload_expiry ON media_uploads(expires_at) WHERE cleaned_at IS NULL;
CREATE UNIQUE INDEX assemble_upload_job ON jobs((payload->>'upload_id'))
 WHERE kind='assemble_upload' AND status IN ('pending','running');

ALTER TABLE jobs ADD COLUMN lease_id uuid;
CREATE UNIQUE INDEX cleanup_upload_job ON jobs((payload->>'upload_id')) WHERE kind='cleanup_upload' AND status IN ('pending','running');
