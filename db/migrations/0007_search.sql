CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX posts_caption_search ON posts USING gin(caption gin_trgm_ops) WHERE state='published';
CREATE INDEX creators_name_search ON creators USING gin(display_name gin_trgm_ops) WHERE verification_status='approved';
CREATE INDEX creators_handle_search ON creators USING gin(handle gin_trgm_ops) WHERE verification_status='approved';
