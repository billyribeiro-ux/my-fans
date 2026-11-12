CREATE TABLE app_users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email text NOT NULL UNIQUE CHECK (email = lower(email)),
    display_name text NOT NULL CHECK (length(display_name) BETWEEN 1 AND 100),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE creators (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL UNIQUE REFERENCES app_users(id),
    handle text NOT NULL UNIQUE CHECK (handle ~ '^[a-z0-9_]{3,30}$'),
    subscription_price_minor bigint NOT NULL CHECK (subscription_price_minor > 0),
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE media_assets (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    creator_id uuid NOT NULL REFERENCES creators(id),
    storage_key text NOT NULL UNIQUE,
    state text NOT NULL DEFAULT 'quarantine' CHECK (state IN ('quarantine', 'processing', 'approved', 'rejected')),
    access_class text NOT NULL DEFAULT 'private' CHECK (access_class IN ('public', 'private')),
    size_bytes bigint NOT NULL CHECK (size_bytes > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, creator_id)
);

CREATE TABLE posts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    creator_id uuid NOT NULL REFERENCES creators(id),
    caption text NOT NULL CHECK (length(caption) <= 10000),
    state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'scheduled', 'published', 'removed')),
    visibility text NOT NULL CHECK (visibility IN ('public', 'subscribers', 'purchase')),
    purchase_price_minor bigint,
    currency text CHECK (currency ~ '^[A-Z]{3}$'),
    published_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, creator_id),
    CHECK ((visibility = 'purchase' AND purchase_price_minor IS NOT NULL AND purchase_price_minor > 0 AND currency IS NOT NULL)
        OR (visibility <> 'purchase' AND purchase_price_minor IS NULL AND currency IS NULL)),
    CHECK (state <> 'published' OR published_at IS NOT NULL)
);
CREATE INDEX posts_creator_feed ON posts (creator_id, published_at DESC, id) WHERE state = 'published';

CREATE TABLE post_media (
    post_id uuid NOT NULL,
    asset_id uuid NOT NULL,
    creator_id uuid NOT NULL,
    position smallint NOT NULL CHECK (position >= 0),
    PRIMARY KEY (post_id, asset_id),
    UNIQUE (post_id, position),
    FOREIGN KEY (post_id, creator_id) REFERENCES posts(id, creator_id),
    FOREIGN KEY (asset_id, creator_id) REFERENCES media_assets(id, creator_id)
);

CREATE TABLE subscriptions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES app_users(id),
    creator_id uuid NOT NULL REFERENCES creators(id),
    status text NOT NULL CHECK (status IN ('pending', 'active', 'past_due', 'canceled', 'expired')),
    provider text NOT NULL,
    provider_subscription_id text NOT NULL,
    current_period_start timestamptz,
    current_period_end timestamptz,
    cancel_at_period_end boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_subscription_id),
    UNIQUE (id, user_id, creator_id),
    CHECK ((current_period_start IS NULL AND current_period_end IS NULL)
        OR (current_period_start IS NOT NULL AND current_period_end IS NOT NULL AND current_period_end > current_period_start))
);
CREATE UNIQUE INDEX subscriptions_one_current ON subscriptions (user_id, creator_id) WHERE status IN ('pending', 'active', 'past_due');

CREATE TABLE entitlements (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES app_users(id),
    creator_id uuid REFERENCES creators(id),
    post_id uuid REFERENCES posts(id),
    subscription_id uuid,
    source text NOT NULL CHECK (source IN ('subscription', 'purchase', 'promotion')),
    starts_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz,
    revoked_at timestamptz,
    FOREIGN KEY (subscription_id, user_id, creator_id) REFERENCES subscriptions(id, user_id, creator_id),
    CHECK (num_nonnulls(creator_id, post_id) = 1),
    CHECK (expires_at IS NULL OR expires_at > starts_at),
    CHECK ((source = 'subscription' AND subscription_id IS NOT NULL AND creator_id IS NOT NULL)
        OR (source <> 'subscription' AND subscription_id IS NULL))
);
CREATE INDEX entitlements_viewer ON entitlements (user_id, creator_id, post_id) WHERE revoked_at IS NULL;

CREATE TABLE processor_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    provider text NOT NULL,
    provider_event_id text NOT NULL,
    payload_sha256 bytea NOT NULL CHECK (octet_length(payload_sha256) = 32),
    status text NOT NULL DEFAULT 'received' CHECK (status IN ('received', 'processed', 'failed')),
    received_at timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    UNIQUE (provider, provider_event_id)
);

CREATE TABLE ledger_accounts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id uuid REFERENCES app_users(id),
    kind text NOT NULL CHECK (kind IN ('processor_clearing', 'creator_pending', 'creator_available', 'platform_revenue', 'reserve')),
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    UNIQUE (id, currency)
);

CREATE TABLE ledger_transactions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    processor_event_id uuid REFERENCES processor_events(id),
    external_key text NOT NULL UNIQUE,
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    status text NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'posted')),
    reverses_transaction_id uuid REFERENCES ledger_transactions(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, currency)
);

CREATE TABLE ledger_postings (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    transaction_id uuid NOT NULL,
    account_id uuid NOT NULL,
    currency text NOT NULL,
    amount_minor bigint NOT NULL CHECK (amount_minor <> 0),
    FOREIGN KEY (transaction_id, currency) REFERENCES ledger_transactions(id, currency),
    FOREIGN KEY (account_id, currency) REFERENCES ledger_accounts(id, currency)
);
CREATE INDEX ledger_postings_account ON ledger_postings (account_id, transaction_id);

CREATE FUNCTION protect_ledger_account() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Ledger account identities are immutable' USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER protect_ledger_account BEFORE UPDATE OR DELETE ON ledger_accounts
    FOR EACH ROW EXECUTE FUNCTION protect_ledger_account();

CREATE FUNCTION protect_posted_transaction() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status = 'posted' THEN
        RAISE EXCEPTION 'Posted ledger transactions are immutable' USING ERRCODE = '23514';
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER protect_posted_transaction BEFORE UPDATE OR DELETE ON ledger_transactions
    FOR EACH ROW EXECUTE FUNCTION protect_posted_transaction();

CREATE FUNCTION protect_posted_posting() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE current_status text;
BEGIN
    -- A row lock serializes posting changes against the draft -> posted transition.
    IF TG_OP <> 'INSERT' THEN
        SELECT status INTO current_status FROM ledger_transactions WHERE id = OLD.transaction_id FOR UPDATE;
        IF current_status = 'posted' THEN
            RAISE EXCEPTION 'Posted ledger entries are immutable' USING ERRCODE = '23514';
        END IF;
    END IF;
    IF TG_OP <> 'DELETE' THEN
        SELECT status INTO current_status FROM ledger_transactions WHERE id = NEW.transaction_id FOR UPDATE;
        IF current_status = 'posted' THEN
            RAISE EXCEPTION 'Cannot add entries to a posted transaction' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER protect_posted_posting BEFORE INSERT OR UPDATE OR DELETE ON ledger_postings
    FOR EACH ROW EXECUTE FUNCTION protect_posted_posting();

CREATE FUNCTION check_ledger_balance() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE target_id uuid; current_status text; entry_count bigint; total numeric;
BEGIN
    IF TG_TABLE_NAME = 'ledger_transactions' THEN
        target_id := NEW.id;
    ELSE
        target_id := COALESCE(NEW.transaction_id, OLD.transaction_id);
    END IF;
    SELECT status INTO current_status FROM ledger_transactions WHERE id = target_id;
    IF current_status = 'posted' THEN
        SELECT count(*), COALESCE(sum(amount_minor), 0) INTO entry_count, total
            FROM ledger_postings WHERE transaction_id = target_id;
        IF entry_count < 2 OR total <> 0 THEN
            RAISE EXCEPTION 'Posted ledger transactions require balanced entries' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER ledger_transaction_balance AFTER INSERT OR UPDATE ON ledger_transactions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_ledger_balance();
CREATE CONSTRAINT TRIGGER ledger_posting_balance AFTER INSERT OR UPDATE OR DELETE ON ledger_postings
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_ledger_balance();

CREATE TABLE outbox_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    topic text NOT NULL,
    aggregate_id uuid NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    delivered_at timestamptz,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0)
);
CREATE INDEX outbox_pending ON outbox_events (created_at, id) WHERE delivered_at IS NULL;

CREATE TABLE audit_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id uuid REFERENCES app_users(id),
    action text NOT NULL,
    resource_type text NOT NULL,
    resource_id uuid,
    metadata jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION prevent_audit_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Audit records are append-only' USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER audit_append_only BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION prevent_audit_mutation();
