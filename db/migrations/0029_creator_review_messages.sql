-- Older moderation evidence may contain internal investigation details. Never
-- backfill it into a creator-visible message. New reviews disclose their current
-- message explicitly and retain the separate immutable evidence trail.
ALTER TABLE creators ADD COLUMN review_message text
  CHECK (review_message IS NULL OR length(review_message) BETWEEN 5 AND 2000);
ALTER TABLE creators ADD COLUMN reviewed_at timestamptz;
