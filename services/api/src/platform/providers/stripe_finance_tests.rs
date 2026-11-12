#![allow(clippy::unwrap_used)]
use super::super::contracts::{charge, setup, setup_at};
use super::*;
fn balance(key: &str, source: &str, amount: i64, fee: i64) -> Value {
    json!({"id":key,"source":source,"amount":amount,"fee":fee,"net":amount-fee,"currency":"usd","reporting_category":"charge","fee_details":[{"type":"stripe_fee","amount":fee,"currency":"usd"}],"status":"pending","created":1700000000,"available_on":1700000001})
}
async fn admin_headers(s: &PlatformState) -> (Uuid, HeaderMap) {
    let actor:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,role,email_verified,mfa_enabled) VALUES('finance-admin@test.invalid','Finance','admin',true,true) RETURNING id").fetch_one(&s.db).await.unwrap();
    let token = random_token();
    sqlx::query("INSERT INTO sessions(user_id,token_digest,expires_at,mfa_verified_at) VALUES($1,$2,now()+interval '1 hour',now())").bind(actor).bind(digest(&token)).execute(&s.db).await.unwrap();
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    (actor, h)
}
async fn receipt(s: &PlatformState, f: &super::super::contracts::Fixture, c: Uuid) -> Uuid {
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), charge());
    super::super::record(s, "acct_fixture", c, "ch_fixture", None, None, None)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM provider_receipts WHERE charge_id='ch_fixture'")
        .fetch_one(&s.db)
        .await
        .unwrap()
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_journal_balances_replays_and_refuses_provider_mutation(db: PgPool) {
    let (s, _, task, _, _) = setup(db, "purchase").await;
    let v = balance("txn_fixture", "ch_fixture", 1000, 59);
    let first = record_balance(&s, "acct_fixture", &v).await.unwrap();
    let again = record_balance(&s, "acct_fixture", &v).await.unwrap();
    assert_eq!(first, again);
    let sum: i64 = sqlx::query_scalar(
        "SELECT sum(amount_minor)::bigint FROM provider_financial_postings WHERE transaction_id=$1",
    )
    .bind(first)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(sum, 0);
    let changed = balance("txn_fixture", "ch_fixture", 2000, 59);
    assert_eq!(
        record_balance(&s, "acct_fixture", &changed).await.unwrap(),
        first
    );
    let gross: i64 =
        sqlx::query_scalar("SELECT gross_minor FROM provider_financial_transactions WHERE id=$1")
            .bind(first)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(gross, 1000);
    let issues: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM provider_finance_issues WHERE resolved_at IS NULL",
    )
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(issues, 2);
    assert!(
        sqlx::query(
            "UPDATE provider_financial_transactions SET gross_minor=2000,net_minor=1941 WHERE id=$1"
        )
        .bind(first)
        .execute(&s.db)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM provider_financial_transactions WHERE id=$1")
            .bind(first)
            .execute(&s.db)
            .await
            .is_err()
    );
    let mut bad = v.clone();
    bad["net"] = json!(0);
    assert!(record_balance(&s, "acct_fixture", &bad).await.is_err());
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_scans_recover_full_history_with_durable_nonrepeating_cursors(db: PgPool) {
    let (s, f, task, _, _) = setup(db, "purchase").await;
    schedule(&s).await.unwrap();
    f.responses.lock().await.insert(
        "/v1/balance_transactions".into(),
        json!({"data":[balance("txn_a","ch_a",100,3)],"has_more":true}),
    );
    reconcile(&s, &json!({"account":"acct_fixture","scan":"balance"}))
        .await
        .unwrap();
    let cursor:Option<String>=sqlx::query_scalar("SELECT cursor FROM provider_finance_scans WHERE account_id='acct_fixture' AND kind='balance'").fetch_one(&s.db).await.unwrap();
    assert_eq!(cursor, Some("txn_a".into()));
    assert!(
        reconcile(&s, &json!({"account":"acct_fixture","scan":"balance"}))
            .await
            .is_err()
    );
    f.responses.lock().await.insert(
        "/v1/balance_transactions".into(),
        json!({"data":[balance("txn_b","ch_b",200,6)],"has_more":false}),
    );
    reconcile(&s, &json!({"account":"acct_fixture","scan":"balance"}))
        .await
        .unwrap();
    let complete:bool=sqlx::query_scalar("SELECT cursor IS NULL AND last_completed_at>now()-interval '1 minute' FROM provider_finance_scans WHERE account_id='acct_fixture' AND kind='balance'").fetch_one(&s.db).await.unwrap();
    assert!(complete);
    let calls = f.calls.lock().await;
    assert!(calls[0].0.contains("created%5Bgte%5D=0"));
    assert!(calls[1].0.contains("starting_after=txn_a"));
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_payout_members_reconcile_net_and_manual_payouts_stay_unattributed(db: PgPool) {
    let (s, f, task, _, _) = setup(db, "purchase").await;
    let p = json!({"id":"po_fixture","livemode":false,"amount":1941,"currency":"usd","status":"paid","automatic":true,"reconciliation_status":"completed","balance_transaction":"txn_out","failure_balance_transaction":null,"arrival_date":1700000001,"created":1700000000});
    record_payout(&s, "acct_fixture", &p).await.unwrap();
    f.responses.lock().await.insert(
        "/v1/balance_transactions".into(),
        json!({"data":[balance("txn_a","ch_a",1000,29)],"has_more":true}),
    );
    payout_members(&s, "acct_fixture").await.unwrap();
    f.responses.lock().await.insert(
        "/v1/balance_transactions".into(),
        json!({"data":[balance("txn_b","ch_b",1000,30)],"has_more":false}),
    );
    payout_members(&s, "acct_fixture").await.unwrap();
    let result:(bool,i64)=sqlx::query_as("SELECT members_complete,(SELECT count(*) FROM provider_finance_issues WHERE code='payout_amount' AND resolved_at IS NULL) FROM provider_payouts WHERE payout_id='po_fixture'").fetch_one(&s.db).await.unwrap();
    assert_eq!(result, (true, 0));
    let mut manual = p.clone();
    manual["id"] = json!("po_manual");
    manual["automatic"] = json!(false);
    manual["reconciliation_status"] = json!("not_applicable");
    record_payout(&s, "acct_fixture", &manual).await.unwrap();
    payout_members(&s, "acct_fixture").await.unwrap();
    assert_eq!(f.calls.lock().await.len(), 2);
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_receipt_verifies_provider_fee_tax_and_surfaces_mismatches(db: PgPool) {
    let (s, f, task, c, _) = setup(db, "purchase").await;
    let r = receipt(&s, &f, c).await;
    sqlx::query("UPDATE checkouts SET provider_session='cs_fixture' WHERE id=$1")
        .bind(c)
        .execute(&s.db)
        .await
        .unwrap();
    let mut cv = charge();
    cv["balance_transaction"] = json!("txn_fixture");
    cv["application_fee"] = json!("fee_fixture");
    {
        let mut responses = f.responses.lock().await;
        responses.insert("/v1/charges/ch_fixture".into(), cv);
        responses.insert(
            "/v1/balance_transactions/txn_fixture".into(),
            balance("txn_fixture", "ch_fixture", 1000, 259),
        );
        responses.insert("/v1/application_fees/fee_fixture".into(),json!({"id":"fee_fixture","livemode":false,"amount":200,"amount_refunded":0,"currency":"usd","account":"acct_fixture","charge":"ch_fixture"}));
        responses.insert("/v1/checkout/sessions/cs_fixture".into(),json!({"id":"cs_fixture","livemode":false,"client_reference_id":c,"currency":"usd","amount_total":1000,"automatic_tax":{"status":"complete"},"total_details":{"amount_tax":91}}));
    }
    check_receipt(&s, "acct_fixture").await.unwrap();
    let result:(bool,i64,i64)=sqlx::query_as("SELECT verified,tax_minor,application_fee_minor FROM provider_receipt_finance WHERE receipt_id=$1").bind(r).fetch_one(&s.db).await.unwrap();
    assert_eq!(result, (true, 91, 200));
    sqlx::query("UPDATE provider_receipt_finance SET checked_at=now()-interval '7 hours'")
        .execute(&s.db)
        .await
        .unwrap();
    f.responses
        .lock()
        .await
        .get_mut("/v1/application_fees/fee_fixture")
        .unwrap()["amount"] = json!(300);
    check_receipt(&s, "acct_fixture").await.unwrap();
    let result:(bool,i64)=sqlx::query_as("SELECT verified,(SELECT count(*) FROM provider_finance_issues WHERE code='application_fee' AND resolved_at IS NULL) FROM provider_receipt_finance WHERE receipt_id=$1").bind(r).fetch_one(&s.db).await.unwrap();
    assert_eq!(result, (false, 1));
    let calls = f.calls.lock().await;
    assert!(
        calls
            .iter()
            .filter(|v| v.0.contains("application_fees"))
            .all(|v| !v.1.contains_key("stripe-account"))
    );
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_operator_cannot_attach_another_checkout_or_force_money_success(db: PgPool) {
    let (s, f, task, c, _) = setup_at(db, "purchase", 90).await;
    let (_, h) = admin_headers(&s).await;
    sqlx::query("UPDATE checkouts SET provider_attempted_at=created_at WHERE id=$1")
        .bind(c)
        .execute(&s.db)
        .await
        .unwrap();
    let mut session = json!({"id":"cs_found","livemode":false,"status":"expired","mode":"payment","currency":"usd","amount_total":1000,"client_reference_id":Uuid::new_v4()});
    f.responses
        .lock()
        .await
        .insert("/v1/checkout/sessions/cs_found".into(), session.clone());
    assert!(
        resolve_checkout(
            State(s.clone()),
            h.clone(),
            Json(CheckoutResolution {
                checkout_id: c,
                session_id: "cs_found".into(),
                reason: "Verified against provider case".into()
            })
        )
        .await
        .is_err()
    );
    session["client_reference_id"] = json!(c);
    f.responses
        .lock()
        .await
        .insert("/v1/checkout/sessions/cs_found".into(), session);
    let result = resolve_checkout(
        State(s.clone()),
        h,
        Json(CheckoutResolution {
            checkout_id: c,
            session_id: "cs_found".into(),
            reason: "Verified against provider case".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.0["status"], "failed");
    let money: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_receipts")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(money, 0);
    assert!(
        sqlx::query("DELETE FROM provider_finance_actions")
            .execute(&s.db)
            .await
            .is_err()
    );
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_dispute_win_requires_explicit_restoration_and_refunds_still_revoke(db: PgPool) {
    let (s, f, task, c, _) = setup(db, "purchase").await;
    receipt(&s, &f, c).await;
    let (_, h) = admin_headers(&s).await;
    let mut cv = charge();
    cv["disputed"] = json!(true);
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), cv.clone());
    let mut d = json!({"id":"du_fixture","livemode":false,"charge":"ch_fixture","amount":1000,"currency":"usd","status":"needs_response","created":1700000000,"evidence_details":{"due_by":1800000000},"balance_transactions":[]});
    f.responses
        .lock()
        .await
        .insert("/v1/disputes/du_fixture".into(), d.clone());
    sync_dispute(&s, "acct_fixture", "du_fixture")
        .await
        .unwrap();
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entitlements WHERE revoked_at IS NULL")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(active, 0);
    d["status"] = json!("won");
    {
        let mut responses = f.responses.lock().await;
        responses.insert("/v1/disputes/du_fixture".into(), d.clone());
        responses.insert("/v1/disputes".into(), json!({"data":[d],"has_more":false}));
    }
    sync_dispute(&s, "acct_fixture", "du_fixture")
        .await
        .unwrap();
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entitlements WHERE revoked_at IS NULL")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(active, 0);
    let result = refresh_dispute(
        State(s.clone()),
        h.clone(),
        Json(DisputeResolution {
            account_id: "acct_fixture".into(),
            dispute_id: "du_fixture".into(),
            restore_access: true,
            reason: "Merchant win independently verified".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.0["restored"], true);
    sync_reversal(&s, "acct_fixture", "ch_fixture")
        .await
        .unwrap();
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entitlements WHERE revoked_at IS NULL")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(active, 1);
    cv["amount_refunded"] = json!(1);
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), cv);
    sync_reversal(&s, "acct_fixture", "ch_fixture")
        .await
        .unwrap();
    assert!(
        refresh_dispute(
            State(s.clone()),
            h,
            Json(DisputeResolution {
                account_id: "acct_fixture".into(),
                dispute_id: "du_fixture".into(),
                restore_access: true,
                reason: "Merchant win independently verified".into()
            })
        )
        .await
        .is_err()
    );
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entitlements WHERE revoked_at IS NULL")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(active, 0);
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_acknowledgment_is_audited_and_never_resolves_money(db: PgPool) {
    let (s, _, task, _, _) = setup(db, "purchase").await;
    let (_, h) = admin_headers(&s).await;
    issue(&s, "acct_fixture", "ch_unknown", "unknown_charge", true)
        .await
        .unwrap();
    let key: Uuid = sqlx::query_scalar("SELECT id FROM provider_finance_issues")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert!(
        acknowledge(
            State(s.clone()),
            HeaderMap::new(),
            Json(Acknowledgment {
                issue_id: key,
                reason: "Investigating this transaction".into()
            })
        )
        .await
        .is_err()
    );
    let _ = acknowledge(
        State(s.clone()),
        h,
        Json(Acknowledgment {
            issue_id: key,
            reason: "Investigating this transaction".into(),
        }),
    )
    .await
    .unwrap();
    let status:(bool,bool)=sqlx::query_as("SELECT acknowledged_at IS NOT NULL,resolved_at IS NOT NULL FROM provider_finance_issues WHERE id=$1").bind(key).fetch_one(&s.db).await.unwrap();
    assert_eq!(status, (true, false));
    issue(&s, "acct_fixture", "ch_unknown", "unknown_charge", false)
        .await
        .unwrap();
    issue(&s, "acct_fixture", "ch_unknown", "unknown_charge", true)
        .await
        .unwrap();
    let status:(bool,bool)=sqlx::query_as("SELECT acknowledged_at IS NOT NULL,resolved_at IS NOT NULL FROM provider_finance_issues WHERE id=$1").bind(key).fetch_one(&s.db).await.unwrap();
    assert_eq!(status, (false, false));
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_admin_keyset_pages_are_complete_without_granting_anonymous_access(db: PgPool) {
    let (s, _, task, _, _) = setup(db, "purchase").await;
    let (_, h) = admin_headers(&s).await;
    sqlx::query("INSERT INTO provider_finance_issues(account_id,object_id,code) SELECT 'acct_fixture','ch_'||n,'unknown_charge' FROM generate_series(1,51) AS n").execute(&s.db).await.unwrap();
    assert!(
        overview(
            State(s.clone()),
            HeaderMap::new(),
            Query(PageQuery::default())
        )
        .await
        .is_err()
    );
    let first = overview(State(s.clone()), h.clone(), Query(PageQuery::default()))
        .await
        .unwrap()
        .0;
    assert_eq!(first["mode"], "stripe");
    assert_eq!(first["issues"].as_array().unwrap().len(), 50);
    assert_eq!(first["scanner"]["open_issues"], 51);
    let query: PageQuery =
        serde_json::from_value(json!({"issues_cursor":first["next_cursors"]["issues"]})).unwrap();
    let second = overview(State(s.clone()), h, Query(query)).await.unwrap().0;
    assert_eq!(second["issues"].as_array().unwrap().len(), 1);
    assert!(second["next_cursors"]["issues"].is_null());
    assert!(
        !first["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == second["issues"][0]["id"])
    );
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_production_refund_requires_reason_before_network_and_audits_request(db: PgPool) {
    let (mut s, f, task, c, _) = setup(db, "purchase").await;
    receipt(&s, &f, c).await;
    let (_, h) = admin_headers(&s).await;
    s.runtime = Arc::new(crate::platform::RuntimeConfig {
        production: true,
        ..Default::default()
    });
    let count = f.calls.lock().await.len();
    assert!(
        crate::platform::billing::refund(State(s.clone()), h.clone(), axum::extract::Path(c), None)
            .await
            .is_err()
    );
    assert_eq!(f.calls.lock().await.len(), count);
    f.responses.lock().await.insert(
        "/v1/refunds".into(),
        json!({"id":"re_fixture","status":"succeeded"}),
    );
    let mut refunded = charge();
    refunded["amount_refunded"] = json!(1000);
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), refunded);
    let request =
        serde_json::from_value(json!({"reason":"Approved full refund after customer request"}))
            .unwrap();
    let result = crate::platform::billing::refund(
        State(s.clone()),
        h,
        axum::extract::Path(c),
        Some(Json(request)),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(result["mode"], "stripe");
    assert_eq!(result["status"], "processing");
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM provider_finance_actions WHERE action='refund.request' AND target=$1",
    )
    .bind(c.to_string())
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn finance_rejects_creator_accounts_without_supported_usd_settlement(db: PgPool) {
    let (s, f, task, _, _) = setup(db, "purchase").await;
    let owner: Uuid =
        sqlx::query_scalar("SELECT user_id FROM provider_accounts WHERE account_id='acct_fixture'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let mut v = json!({"id":"acct_fixture","metadata":{"myfans_user":owner},"charges_enabled":true,"payouts_enabled":true,"default_currency":"eur"});
    f.responses
        .lock()
        .await
        .insert("/v1/accounts/acct_fixture".into(), v.clone());
    assert!(ready_account(&s, owner).await.is_err());
    v["default_currency"] = json!("usd");
    f.responses
        .lock()
        .await
        .insert("/v1/accounts/acct_fixture".into(), v);
    assert_eq!(ready_account(&s, owner).await.unwrap(), "acct_fixture");
    task.abort();
}
