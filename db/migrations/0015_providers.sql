-- Provider identifiers contain no document images, card details, or webhook payloads.
CREATE TABLE provider_accounts (
 user_id uuid PRIMARY KEY REFERENCES app_users(id),
 account_id text UNIQUE NOT NULL CHECK(account_id ~ '^acct_[A-Za-z0-9]+$'),
 charges_enabled boolean NOT NULL DEFAULT false,
 payouts_enabled boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT now(),
 checked_at timestamptz
);
CREATE TABLE identity_checks (
 creator_id uuid PRIMARY KEY REFERENCES creators(id),
 applicant_id text UNIQUE,
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','verified','rejected')),
 checked_at timestamptz,
 next_check_at timestamptz NOT NULL DEFAULT now(),
 created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE checkouts ADD COLUMN provider text NOT NULL DEFAULT 'sandbox' CHECK(provider IN ('sandbox','stripe'));
ALTER TABLE checkouts ADD COLUMN provider_account text REFERENCES provider_accounts(account_id);
ALTER TABLE checkouts ADD COLUMN provider_session text UNIQUE;
ALTER TABLE checkouts ADD CONSTRAINT checkout_provider CHECK((provider='sandbox' AND provider_account IS NULL AND provider_session IS NULL) OR (provider='stripe' AND provider_account IS NOT NULL));
CREATE TABLE provider_receipts (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 checkout_id uuid NOT NULL REFERENCES checkouts(id),
 account_id text NOT NULL REFERENCES provider_accounts(account_id),
 charge_id text NOT NULL,
 invoice_id text,
 subscription_id text,
 amount_minor bigint NOT NULL CHECK(amount_minor BETWEEN 100 AND 10000000),
 refunded_minor bigint NOT NULL DEFAULT 0 CHECK(refunded_minor>=0 AND refunded_minor<=amount_minor),
 disputed boolean NOT NULL DEFAULT false,
 currency text NOT NULL CHECK(currency='USD'),
 period_start timestamptz,
 period_end timestamptz,
 entitlement_id uuid REFERENCES entitlements(id),
 created_at timestamptz NOT NULL DEFAULT now(),
 checked_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(account_id,charge_id),
 UNIQUE(account_id,invoice_id),
 CHECK((period_start IS NULL AND period_end IS NULL) OR period_end>period_start)
);
CREATE INDEX provider_receipts_checkout ON provider_receipts(checkout_id,created_at);
ALTER TABLE payment_webhook_inbox ADD COLUMN processed_at timestamptz;
CREATE TABLE platform_providers (
 id smallint PRIMARY KEY CHECK(id=1),
 identity text NOT NULL
);
INSERT INTO platform_providers VALUES(1,'sandbox');
CREATE FUNCTION protect_provider_checkout() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.provider,NEW.provider_account) IS DISTINCT FROM (OLD.provider,OLD.provider_account) OR (OLD.provider_session IS NOT NULL AND NEW.provider_session IS DISTINCT FROM OLD.provider_session) THEN
  RAISE EXCEPTION 'Provider checkout identity is immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER checkout_provider_identity BEFORE UPDATE ON checkouts FOR EACH ROW EXECUTE FUNCTION protect_provider_checkout();
CREATE UNIQUE INDEX provider_pending_subscription ON checkouts(user_id,creator_id) WHERE provider='stripe' AND kind='subscription' AND status='pending';
CREATE UNIQUE INDEX provider_pending_purchase ON checkouts(user_id,post_id) WHERE provider='stripe' AND kind='purchase' AND status='pending';
CREATE TABLE provider_reconciliation (
 account_id text PRIMARY KEY REFERENCES provider_accounts(account_id),
 last_completed_at timestamptz NOT NULL DEFAULT now(),
 next_at timestamptz NOT NULL DEFAULT now(),
 window_start bigint,
 window_end bigint,
 cursor text
);
CREATE UNIQUE INDEX provider_reconcile_job ON jobs(kind) WHERE kind='provider_reconcile' AND status IN ('pending','running');
CREATE UNIQUE INDEX media_processing_job ON jobs((payload->>'media_id')) WHERE kind='process_media';
CREATE FUNCTION protect_provider_receipt() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.id,NEW.checkout_id,NEW.account_id,NEW.charge_id,NEW.invoice_id,NEW.subscription_id,NEW.amount_minor,NEW.currency,NEW.period_start,NEW.period_end,NEW.entitlement_id,NEW.created_at)
 IS DISTINCT FROM (OLD.id,OLD.checkout_id,OLD.account_id,OLD.charge_id,OLD.invoice_id,OLD.subscription_id,OLD.amount_minor,OLD.currency,OLD.period_start,OLD.period_end,OLD.entitlement_id,OLD.created_at)
 OR NEW.refunded_minor<OLD.refunded_minor OR (OLD.disputed AND NOT NEW.disputed) THEN
  RAISE EXCEPTION 'Provider receipt identities and reversals are immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER provider_receipt_identity BEFORE UPDATE ON provider_receipts FOR EACH ROW EXECUTE FUNCTION protect_provider_receipt();
CREATE FUNCTION protect_provider_account() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.user_id,NEW.account_id,NEW.created_at) IS DISTINCT FROM (OLD.user_id,OLD.account_id,OLD.created_at) THEN
  RAISE EXCEPTION 'Connected account ownership is immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER provider_account_identity BEFORE UPDATE ON provider_accounts FOR EACH ROW EXECUTE FUNCTION protect_provider_account();
