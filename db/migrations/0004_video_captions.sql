ALTER TABLE media_assets ADD COLUMN captions_vtt text CHECK(captions_vtt IS NULL OR length(captions_vtt)<=8192);
