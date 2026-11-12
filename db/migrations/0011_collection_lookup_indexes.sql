CREATE INDEX conversations_user_a ON conversations(user_a);
CREATE INDEX conversations_user_b ON conversations(user_b);
CREATE INDEX ledger_history_page ON ledger_transactions(created_at DESC,id DESC) WHERE status='posted';
