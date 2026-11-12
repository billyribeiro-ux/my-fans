-- Set only after every validated playlist and segment has been stored. Media
-- remains quarantined until an independent moderation decision approves it.
ALTER TABLE media_assets ADD COLUMN hls_ready boolean NOT NULL DEFAULT false;
