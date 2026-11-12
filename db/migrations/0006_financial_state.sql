CREATE FUNCTION protect_checkout_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.user_id,NEW.creator_id,NEW.post_id,NEW.kind,NEW.amount_minor,NEW.currency,NEW.idempotency_key,NEW.request_fingerprint)
  IS DISTINCT FROM (OLD.user_id,OLD.creator_id,OLD.post_id,OLD.kind,OLD.amount_minor,OLD.currency,OLD.idempotency_key,OLD.request_fingerprint) THEN
  RAISE EXCEPTION 'Checkout identity and pricing are immutable' USING ERRCODE='23514';
 END IF;
 IF NEW.status IS DISTINCT FROM OLD.status AND NOT (
   (OLD.status='pending' AND NEW.status IN ('paid','failed')) OR (OLD.status='paid' AND NEW.status='refunded')
 ) THEN RAISE EXCEPTION 'Invalid checkout transition' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER checkout_identity BEFORE UPDATE ON checkouts FOR EACH ROW EXECUTE FUNCTION protect_checkout_identity();
ALTER TABLE checkouts ADD CONSTRAINT checkout_payment_state CHECK(
 (status='pending' AND paid_at IS NULL AND refunded_minor=0) OR
 (status='failed' AND refunded_minor=0) OR
 (status='paid' AND paid_at IS NOT NULL AND refunded_minor=0) OR
 (status='refunded' AND paid_at IS NOT NULL AND refunded_minor=amount_minor)
);
CREATE FUNCTION protect_payout_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.user_id,NEW.amount_minor,NEW.currency,NEW.idempotency_key) IS DISTINCT FROM (OLD.user_id,OLD.amount_minor,OLD.currency,OLD.idempotency_key) THEN
  RAISE EXCEPTION 'Payout identity and amount are immutable' USING ERRCODE='23514';
 END IF;
 IF NEW.status IS DISTINCT FROM OLD.status AND NOT (
  (OLD.status='pending' AND NEW.status IN ('approved','rejected','paid')) OR (OLD.status='approved' AND NEW.status='paid')
 ) THEN RAISE EXCEPTION 'Invalid payout transition' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER payout_identity BEFORE UPDATE ON payouts FOR EACH ROW EXECUTE FUNCTION protect_payout_identity();
ALTER TABLE payouts ADD CONSTRAINT payout_currency CHECK(currency='USD');
