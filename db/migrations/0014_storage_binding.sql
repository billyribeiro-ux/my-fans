-- Bind a database to its object store. Changing accounts/buckets cannot silently
-- redirect existing private media to an empty or unrelated store.
CREATE TABLE platform_storage (
    id smallint PRIMARY KEY CHECK (id = 1),
    media_store_identity text NOT NULL CHECK (length(media_store_identity) BETWEEN 1 AND 80)
);
INSERT INTO platform_storage(id,media_store_identity) VALUES(1,'local:v1');
