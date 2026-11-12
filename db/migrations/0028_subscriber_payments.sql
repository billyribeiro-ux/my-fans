-- Subscriber history remains owner-scoped with stable timestamp/UUID pagination.
CREATE INDEX checkouts_owner_page ON checkouts(user_id,created_at DESC,id DESC);
CREATE INDEX provider_receipts_checkout_page ON provider_receipts(checkout_id,created_at DESC,id DESC);
