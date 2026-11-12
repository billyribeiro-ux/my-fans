-- Provider money is a separate journal from the sandbox wallet. A direct charge
-- belongs to its connected account; no local wallet balance creates a payout.
CREATE TABLE provider_finance_scans (
 id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
 account_id text NOT NULL REFERENCES provider_accounts(account_id),
 kind text NOT NULL CHECK(kind IN ('balance','payout','payout_members','dispute','receipt')),
 last_completed_at timestamptz NOT NULL,
 next_at timestamptz NOT NULL DEFAULT now(),
 window_start bigint,
 window_end bigint,
 cursor text,
 PRIMARY KEY(account_id,kind)
);
CREATE UNIQUE INDEX provider_finance_job ON jobs((payload->>'account'),(payload->>'scan'))
 WHERE kind='stripe_finance' AND status IN ('pending','running');
CREATE TABLE provider_financial_transactions (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 account_id text NOT NULL REFERENCES provider_accounts(account_id),
 transaction_id text NOT NULL,
 source_id text,
 reporting_category text NOT NULL,
 currency text NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
 gross_minor bigint NOT NULL CHECK(gross_minor BETWEEN -1000000000000000 AND 1000000000000000),
 fee_minor bigint NOT NULL CHECK(fee_minor BETWEEN -1000000000000000 AND 1000000000000000),
 net_minor bigint NOT NULL CHECK(net_minor BETWEEN -1000000000000000 AND 1000000000000000),
 fee_details jsonb NOT NULL CHECK(jsonb_typeof(fee_details)='array'),
 provider_created_at timestamptz NOT NULL,
 available_at timestamptz NOT NULL,
 status text NOT NULL CHECK(status IN ('pending','available')),
 fingerprint text NOT NULL,
 observed_at timestamptz NOT NULL DEFAULT now(),
 checked_at timestamptz NOT NULL DEFAULT now(),
 CHECK(gross_minor-fee_minor=net_minor),
 UNIQUE(account_id,transaction_id)
);
-- Every transaction exposes exactly balanced legs in its own currency. Stripe
-- corrections arrive as new transactions; posted amounts are never overwritten.
CREATE VIEW provider_financial_postings AS
 SELECT id AS transaction_id,account_id,currency,'source'::text AS account,gross_minor AS amount_minor FROM provider_financial_transactions
 UNION ALL SELECT id,account_id,currency,'fees',-fee_minor FROM provider_financial_transactions
 UNION ALL SELECT id,account_id,currency,'stripe_balance',-net_minor FROM provider_financial_transactions;
CREATE FUNCTION protect_provider_financial_transaction() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' OR (NEW.id,NEW.account_id,NEW.transaction_id,NEW.source_id,NEW.reporting_category,NEW.currency,NEW.gross_minor,NEW.fee_minor,NEW.net_minor,NEW.fee_details,NEW.provider_created_at,NEW.fingerprint,NEW.observed_at)
 IS DISTINCT FROM (OLD.id,OLD.account_id,OLD.transaction_id,OLD.source_id,OLD.reporting_category,OLD.currency,OLD.gross_minor,OLD.fee_minor,OLD.net_minor,OLD.fee_details,OLD.provider_created_at,OLD.fingerprint,OLD.observed_at) THEN
  RAISE EXCEPTION 'Provider journal amounts are immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER provider_financial_transaction_identity BEFORE UPDATE OR DELETE ON provider_financial_transactions FOR EACH ROW EXECUTE FUNCTION protect_provider_financial_transaction();
CREATE TABLE provider_finance_issues (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 account_id text NOT NULL REFERENCES provider_accounts(account_id),
 object_id text NOT NULL,
 code text NOT NULL CHECK(code IN ('journal_changed','unknown_charge','charge_amount','application_fee','tax_unverified','payout_amount','payout_identity','checkout_indeterminate','payout_failed','dispute_response')),
 first_seen_at timestamptz NOT NULL DEFAULT now(),
 last_seen_at timestamptz NOT NULL DEFAULT now(),
 resolved_at timestamptz,
 acknowledged_at timestamptz,
 acknowledged_by uuid REFERENCES app_users(id),
 UNIQUE(account_id,object_id,code)
);
CREATE INDEX provider_finance_issues_open ON provider_finance_issues(first_seen_at,id) WHERE resolved_at IS NULL;
CREATE TABLE provider_receipt_finance (
 receipt_id uuid PRIMARY KEY REFERENCES provider_receipts(id),
 balance_transaction_id text,
 application_fee_id text,
 application_fee_minor bigint,
 refunded_fee_minor bigint,
 tax_minor bigint CHECK(tax_minor>=0),
 verified boolean NOT NULL DEFAULT false,
 checked_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE provider_payouts (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 account_id text NOT NULL REFERENCES provider_accounts(account_id),
 payout_id text NOT NULL,
 amount_minor bigint NOT NULL CHECK(amount_minor>0),
 currency text NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
 status text NOT NULL CHECK(status IN ('pending','in_transit','paid','failed','canceled')),
 automatic boolean NOT NULL,
 reconciliation_status text NOT NULL CHECK(reconciliation_status IN ('completed','in_progress','not_applicable')),
 balance_transaction_id text,
 failure_balance_transaction_id text,
 arrival_at timestamptz NOT NULL,
 created_at timestamptz NOT NULL,
 checked_at timestamptz NOT NULL DEFAULT now(),
 members_cursor text,
 members_complete boolean NOT NULL DEFAULT false,
 UNIQUE(account_id,payout_id)
);
CREATE TABLE provider_payout_members (
 payout_id uuid NOT NULL REFERENCES provider_payouts(id),
 transaction_id uuid NOT NULL REFERENCES provider_financial_transactions(id),
 PRIMARY KEY(payout_id,transaction_id)
);
CREATE TABLE provider_disputes (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 account_id text NOT NULL REFERENCES provider_accounts(account_id),
 dispute_id text NOT NULL,
 charge_id text NOT NULL,
 amount_minor bigint NOT NULL CHECK(amount_minor>0),
 currency text NOT NULL CHECK(currency ~ '^[A-Z]{3}$'),
 status text NOT NULL CHECK(status IN ('warning_needs_response','warning_under_review','warning_closed','needs_response','under_review','won','lost','prevented','charge_refunded')),
 due_by timestamptz,
 created_at timestamptz NOT NULL,
 checked_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(account_id,dispute_id)
);
CREATE INDEX provider_disputes_charge ON provider_disputes(account_id,charge_id);
CREATE TABLE provider_financial_revocations (
 receipt_id uuid PRIMARY KEY REFERENCES provider_receipts(id),
 entitlement_id uuid NOT NULL REFERENCES entitlements(id),
 revoked_at timestamptz NOT NULL,
 restored_at timestamptz,
 restored_by uuid REFERENCES app_users(id)
);
CREATE TABLE provider_finance_actions (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 actor_id uuid NOT NULL REFERENCES app_users(id),
 action text NOT NULL CHECK(action IN ('checkout.attach','dispute.refresh','dispute.restore','issue.acknowledge','refund.request')),
 target text NOT NULL,
 reason text NOT NULL CHECK(length(reason) BETWEEN 5 AND 500),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION protect_provider_finance_action() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Finance audit actions are immutable' USING ERRCODE='23514'; END $$;
CREATE TRIGGER provider_finance_action_immutable BEFORE UPDATE OR DELETE ON provider_finance_actions FOR EACH ROW EXECUTE FUNCTION protect_provider_finance_action();
