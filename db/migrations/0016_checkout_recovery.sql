-- A durable marker distinguishes requests that might have reached Stripe from
-- intents abandoned before any provider call. Existing intents are uncertain.
ALTER TABLE checkouts ADD COLUMN provider_attempted_at timestamptz;
ALTER TABLE checkouts ADD COLUMN provider_recovery_at timestamptz NOT NULL DEFAULT now();
ALTER TABLE checkouts ADD COLUMN provider_recovery_cursor text;
UPDATE checkouts SET provider_attempted_at=created_at WHERE provider='stripe';
CREATE INDEX provider_checkout_recovery ON checkouts(provider_recovery_at,id)
 WHERE provider='stripe' AND status='pending';
CREATE UNIQUE INDEX provider_checkout_recovery_job ON jobs((payload->>'checkout_id'))
 WHERE kind='stripe_checkout_recovery' AND status IN ('pending','running');
CREATE FUNCTION protect_checkout_attempt() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.created_at IS DISTINCT FROM OLD.created_at OR
    (OLD.provider_attempted_at IS NOT NULL AND NEW.provider_attempted_at IS DISTINCT FROM OLD.provider_attempted_at) THEN
  RAISE EXCEPTION 'Checkout creation and provider attempt timestamps are immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER checkout_attempt_identity BEFORE UPDATE ON checkouts FOR EACH ROW EXECUTE FUNCTION protect_checkout_attempt();
