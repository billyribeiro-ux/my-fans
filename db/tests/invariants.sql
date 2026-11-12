\set ON_ERROR_STOP on
BEGIN;

DO $$
DECLARE v_user uuid; v_creator uuid; v_account_a uuid; v_account_b uuid; v_account_eur uuid;
    v_transaction uuid; v_unbalanced uuid; v_audit uuid;
BEGIN
    INSERT INTO app_users (email, display_name) VALUES ('test@example.com', 'Schema test') RETURNING id INTO v_user;
    INSERT INTO creators (user_id, handle, subscription_price_minor, currency)
        VALUES (v_user, 'test_creator', 1200, 'USD') RETURNING id INTO v_creator;
    INSERT INTO ledger_accounts (kind, currency) VALUES ('processor_clearing', 'USD') RETURNING id INTO v_account_a;
    INSERT INTO ledger_accounts (owner_id, kind, currency) VALUES (v_user, 'creator_pending', 'USD') RETURNING id INTO v_account_b;
    INSERT INTO ledger_accounts (kind, currency) VALUES ('processor_clearing', 'EUR') RETURNING id INTO v_account_eur;
    INSERT INTO ledger_transactions (external_key, currency) VALUES ('test:balanced', 'USD') RETURNING id INTO v_transaction;
    INSERT INTO ledger_postings (transaction_id, account_id, currency, amount_minor)
        VALUES (v_transaction, v_account_a, 'USD', -1200), (v_transaction, v_account_b, 'USD', 1200);
    UPDATE ledger_transactions SET status = 'posted' WHERE id = v_transaction;
    SET CONSTRAINTS ALL IMMEDIATE;

    BEGIN
        UPDATE ledger_postings SET amount_minor = 100 WHERE transaction_id = v_transaction;
        RAISE EXCEPTION 'Posting update unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO ledger_postings (transaction_id, account_id, currency, amount_minor)
            VALUES (v_transaction, v_account_a, 'USD', 1);
        RAISE EXCEPTION 'Posting append unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    BEGIN
        DELETE FROM ledger_transactions WHERE id = v_transaction;
        RAISE EXCEPTION 'Posted transaction deletion unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO ledger_transactions (external_key, currency) VALUES ('test:unbalanced', 'USD') RETURNING id INTO v_unbalanced;
        INSERT INTO ledger_postings (transaction_id, account_id, currency, amount_minor)
            VALUES (v_unbalanced, v_account_a, 'USD', 1200);
        UPDATE ledger_transactions SET status = 'posted' WHERE id = v_unbalanced;
        RAISE EXCEPTION 'Unbalanced posting unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO ledger_transactions (external_key, currency) VALUES ('test:foreign-currency', 'USD') RETURNING id INTO v_unbalanced;
        INSERT INTO ledger_postings (transaction_id, account_id, currency, amount_minor)
            VALUES (v_unbalanced, v_account_eur, 'USD', 1200);
        RAISE EXCEPTION 'Currency mismatch unexpectedly succeeded';
    EXCEPTION WHEN foreign_key_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO ledger_transactions (external_key, currency) VALUES ('test:balanced', 'USD');
        RAISE EXCEPTION 'Duplicate transaction unexpectedly succeeded';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO posts (creator_id, caption, state, visibility, purchase_price_minor, currency)
            VALUES (v_creator, 'Invalid price', 'draft', 'purchase', -1, 'USD');
        RAISE EXCEPTION 'Negative price unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    INSERT INTO audit_events (actor_id, action, resource_type) VALUES (v_user, 'test', 'schema') RETURNING id INTO v_audit;
    BEGIN
        INSERT INTO posts (creator_id, caption, state, visibility, currency)
            VALUES (v_creator, 'Missing price', 'draft', 'purchase', 'USD');
        RAISE EXCEPTION 'Missing purchase price unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    BEGIN
        UPDATE ledger_accounts SET owner_id = NULL WHERE id = v_account_b;
        RAISE EXCEPTION 'Account identity update unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    BEGIN
        DELETE FROM audit_events WHERE id = v_audit;
        RAISE EXCEPTION 'Audit deletion unexpectedly succeeded';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
    RAISE NOTICE 'Financial and audit invariants passed';
END;
$$;
ROLLBACK;
