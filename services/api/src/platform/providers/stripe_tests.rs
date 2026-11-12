#![allow(clippy::unwrap_used)]
use super::*;
use axum::{
    body::to_bytes,
    extract::{Path, Request},
    routing::any,
};
use std::collections::HashMap;
use tokio::sync::Mutex;
type ResponseGate = (String, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

type Captured = Vec<(String, HeaderMap, Vec<u8>)>;
#[derive(Clone, Default)]
pub(super) struct Fixture {
    pub(super) responses: Arc<Mutex<HashMap<String, Value>>>,
    pub(super) calls: Arc<Mutex<Captured>>,
    gate: Arc<Mutex<Option<ResponseGate>>>,
}
async fn fixture(State(f): State<Fixture>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let target = request.uri().path_and_query().unwrap().to_string();
    let headers = request.headers().clone();
    assert_eq!(headers.get("stripe-version").unwrap(), STRIPE_VERSION);
    assert_eq!(
        headers.get("authorization").unwrap(),
        "Bearer sk_test_fixture_only"
    );
    assert!(headers.get("cookie").is_none());
    let body = to_bytes(request.into_body(), 16384).await.unwrap().to_vec();
    let scoped = format!(
        "{}:{target}",
        headers
            .get("stripe-account")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    );
    f.calls.lock().await.push((target.clone(), headers, body));
    let gate = f.gate.lock().await.clone();
    if let Some((expected, entered, release)) = gate
        && target == expected
    {
        entered.notify_one();
        release.notified().await;
    }
    let responses = f.responses.lock().await;
    match responses
        .get(&scoped)
        .or_else(|| responses.get(&target))
        .or_else(|| responses.get(&path))
    {
        Some(value) => Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
pub(super) async fn setup(
    db: PgPool,
    kind: &str,
) -> (
    PlatformState,
    Fixture,
    tokio::task::JoinHandle<()>,
    Uuid,
    Uuid,
) {
    setup_at(db, kind, 0).await
}
pub(super) async fn setup_at(
    db: PgPool,
    kind: &str,
    minutes_ago: i32,
) -> (
    PlatformState,
    Fixture,
    tokio::task::JoinHandle<()>,
    Uuid,
    Uuid,
) {
    let fixture_state = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(any(fixture))
        .with_state(fixture_state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let buyer:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,email_verified) VALUES('buyer@test.invalid','Buyer',true) RETURNING id").fetch_one(&db).await.unwrap();
    let owner:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,role,email_verified) VALUES('creator@test.invalid','Creator','creator',true) RETURNING id").fetch_one(&db).await.unwrap();
    let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'fixture',1000,'USD','approved') RETURNING id").bind(owner).fetch_one(&db).await.unwrap();
    let post:Uuid=sqlx::query_scalar("INSERT INTO posts(creator_id,caption,state,visibility,purchase_price_minor,currency,published_at) VALUES($1,'Fixture','published','purchase',1000,'USD',now()) RETURNING id").bind(creator).fetch_one(&db).await.unwrap();
    sqlx::query("INSERT INTO provider_accounts(user_id,account_id,charges_enabled,payouts_enabled) VALUES($1,'acct_fixture',true,true)").bind(owner).execute(&db).await.unwrap();
    let checkout:Uuid=sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,post_id,kind,amount_minor,idempotency_key,request_fingerprint,provider,provider_account,created_at) VALUES($1,$2,$3,$4,1000,$5,'fixture','stripe','acct_fixture',now()-make_interval(mins=>$6)) RETURNING id").bind(buyer).bind(creator).bind(if kind=="purchase"{Some(post)}else{None}).bind(kind).bind(Uuid::new_v4()).bind(minutes_ago).fetch_one(&db).await.unwrap();
    let p = Stripe {
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
        key: "sk_test_fixture_only".into(),
        webhook_secret: "whsec_fixture_secret".into(),
        origin: "https://myfans.test".into(),
        country: "US".into(),
        live: false,
        endpoint: Some(endpoint),
    };
    let s = PlatformState {
        runtime: Default::default(),
        db,
        media_root: PathBuf::new(),
        key: Arc::new([7; 32]),
        dummy_hash: Arc::new("invalid".into()),
        mail: Default::default(),
        storage: MediaStore::local(PathBuf::new()),
        scanner: Default::default(),
        providers: Providers {
            stripe: Some(Arc::new(p)),
            identity: None,
        },
    };
    (s, fixture_state, task, checkout, buyer)
}
pub(super) fn charge() -> Value {
    json!({"id":"ch_fixture","livemode":false,"paid":true,"status":"succeeded","currency":"usd","amount":1000,"amount_captured":1000,"amount_refunded":0,"disputed":false})
}
fn event_value(kind: &str, object: Value) -> Value {
    json!({"id":"evt_fixture","livemode":false,"api_version":STRIPE_VERSION,"account":"acct_fixture","type":kind,"data":{"object":object}})
}
fn headers(body: &[u8]) -> HeaderMap {
    let now = Utc::now().timestamp();
    let mut bytes = format!("{now}.").into_bytes();
    bytes.extend_from_slice(body);
    let signature = signature(b"whsec_fixture_secret", &bytes).unwrap();
    let mut h = HeaderMap::new();
    h.insert(
        "stripe-signature",
        format!("t={now},v1={signature}").parse().unwrap(),
    );
    h
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn failing_connected_account_does_not_block_other_account_reconciliation(db: PgPool) {
    let (s, f, task, _, _) = setup(db, "purchase").await;
    let owner:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('other@test.invalid','Other') RETURNING id").fetch_one(&s.db).await.unwrap();
    sqlx::query("INSERT INTO provider_accounts(user_id,account_id) VALUES($1,'acct_other')")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO provider_reconciliation(account_id,last_completed_at) VALUES('acct_fixture',now()-interval '30 days'),('acct_other',now()-interval '10 minutes')").execute(&s.db).await.unwrap();
    f.responses
        .lock()
        .await
        .insert("/v1/events".into(), json!({"data":[],"has_more":false}));
    finance::schedule(&s).await.unwrap();
    sqlx::query("UPDATE provider_finance_scans SET next_at=now()+interval '1 hour'")
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET run_at=now()+interval '1 hour' WHERE kind='stripe_finance'")
        .execute(&s.db)
        .await
        .unwrap();
    // Two independently leased jobs are queued. The expired account's failure
    // must not turn the healthy account into a thirty-minute global outage.
    super::super::super::worker::tick(&s).await.unwrap();
    super::super::super::worker::tick(&s).await.unwrap();
    let healthy:bool=sqlx::query_scalar("SELECT last_completed_at>now()-interval '1 minute' FROM provider_reconciliation WHERE account_id='acct_other'").fetch_one(&s.db).await.unwrap();
    assert!(healthy);
    let failed:(String,i32)=sqlx::query_as("SELECT status,attempts FROM jobs WHERE kind='provider_reconcile' AND payload->>'account'='acct_fixture'").fetch_one(&s.db).await.unwrap();
    assert_eq!(failed, ("pending".into(), 1));
    let calls = f.calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["stripe-account"], "acct_other");
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn unattempted_checkout_expiry_is_durable_and_never_contacts_stripe(db: PgPool) {
    let (s, f, task, c, _) = setup_at(db, "purchase", 30).await;
    recover_checkout(&s, c).await.unwrap();
    recover_checkout(&s, c).await.unwrap();
    let state: (String, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT status,provider_attempted_at FROM checkouts WHERE id=$1")
            .bind(c)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(state, ("failed".into(), None));
    assert!(checkout(&s, c).await.is_err());
    assert!(f.calls.lock().await.is_empty());
    assert!(
        sqlx::query("UPDATE checkouts SET created_at=now() WHERE id=$1")
            .bind(c)
            .execute(&s.db)
            .await
            .is_err()
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='checkout.expired.unattempted'",
    )
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn unknown_checkout_outcome_preserves_reservation_and_surfaces_operator_failure(db: PgPool) {
    let (s, f, task, c, _) = setup_at(db, "purchase", 90).await;
    sqlx::query("UPDATE checkouts SET provider_attempted_at=created_at WHERE id=$1")
        .bind(c)
        .execute(&s.db)
        .await
        .unwrap();
    f.responses.lock().await.insert(
        "/v1/checkout/sessions".into(),
        json!({"data":[],"has_more":false}),
    );
    assert_eq!(
        recover_checkout(&s, c).await.unwrap_err().0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let state: String = sqlx::query_scalar("SELECT status FROM checkouts WHERE id=$1")
        .bind(c)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(state, "pending");
    assert!(checkout(&s, c).await.is_err());
    assert!(
        sqlx::query("UPDATE checkouts SET provider_attempted_at=NULL WHERE id=$1")
            .bind(c)
            .execute(&s.db)
            .await
            .is_err()
    );
    let calls = f.calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].2.is_empty());
    assert_eq!(calls[0].1["stripe-account"], "acct_fixture");
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lost_session_response_is_found_through_durable_bounded_pagination(db: PgPool) {
    let (s, f, task, c, _) = setup_at(db, "purchase", 90).await;
    sqlx::query("UPDATE checkouts SET provider_attempted_at=created_at WHERE id=$1")
        .bind(c)
        .execute(&s.db)
        .await
        .unwrap();
    f.responses.lock().await.insert("/v1/checkout/sessions".into(),json!({"data":[{"id":"cs_unrelated","livemode":false,"client_reference_id":Uuid::new_v4()}],"has_more":true}));
    recover_checkout(&s, c).await.unwrap();
    let row: (String, String) =
        sqlx::query_as("SELECT status,provider_recovery_cursor FROM checkouts WHERE id=$1")
            .bind(c)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(row, ("pending".into(), "cs_unrelated".into()));
    let found = json!({"id":"cs_recovered","livemode":false,"client_reference_id":c.to_string(),"status":"expired"});
    {
        let mut r = f.responses.lock().await;
        r.insert(
            "/v1/checkout/sessions".into(),
            json!({"data":[found.clone()],"has_more":false}),
        );
        r.insert("/v1/checkout/sessions/cs_recovered".into(), found);
    }
    recover_checkout(&s, c).await.unwrap();
    let state: (String, String, Option<String>) = sqlx::query_as(
        "SELECT status,provider_session,provider_recovery_cursor FROM checkouts WHERE id=$1",
    )
    .bind(c)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(state, ("failed".into(), "cs_recovered".into(), None));
    let calls = f.calls.lock().await;
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c.2.is_empty()));
    let first = url::Url::parse(&format!("https://fixture.invalid{}", calls[0].0)).unwrap();
    let second = url::Url::parse(&format!("https://fixture.invalid{}", calls[1].0)).unwrap();
    let first: HashMap<_, _> = first.query_pairs().into_owned().collect();
    let second: HashMap<_, _> = second.query_pairs().into_owned().collect();
    assert_eq!(first["created[gte]"], second["created[gte]"]);
    assert_eq!(first["created[lte]"], second["created[lte]"]);
    assert_eq!(second["starting_after"], "cs_unrelated");
    assert_eq!(second["limit"], "20");
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn concurrent_checkout_retries_serialize_provider_creation_and_keep_attempt_evidence(
    db: PgPool,
) {
    let (s, f, task, c, _) = setup(db, "purchase").await;
    let session = json!({"id":"cs_fixture","livemode":false,"status":"open","client_reference_id":c.to_string(),"currency":"usd","amount_total":1000,"url":"https://checkout.stripe.com/c/pay/cs_fixture"});
    {
        let mut r = f.responses.lock().await;
        r.insert("/v1/checkout/sessions".into(), session.clone());
        r.insert("/v1/checkout/sessions/cs_fixture".into(), session);
    }
    let (a, b) = tokio::join!(checkout(&s, c), checkout(&s, c));
    assert!(a.is_ok() && b.is_ok());
    let state: (bool, String) = sqlx::query_as(
        "SELECT provider_attempted_at IS NOT NULL,provider_session FROM checkouts WHERE id=$1",
    )
    .bind(c)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(state, (true, "cs_fixture".into()));
    let calls = f.calls.lock().await;
    assert_eq!(calls.iter().filter(|v| !v.2.is_empty()).count(), 1);
    assert_eq!(calls.len(), 2);
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn signed_payment_retries_and_partial_refunds_preserve_access_boundaries(db: PgPool) {
    let (s, f, task, c, buyer) = setup(db, "purchase").await;
    let session = json!({"id":"cs_fixture","livemode":false,"status":"complete","payment_status":"paid","client_reference_id":c.to_string(),"payment_intent":"pi_fixture","mode":"payment"});
    let event_json = event_value("checkout.session.completed", json!({"id":"cs_fixture"}));
    {
        let mut r = f.responses.lock().await;
        r.insert("/v1/events/evt_fixture".into(), event_json.clone());
        r.insert("/v1/checkout/sessions/cs_fixture".into(), session);
        r.insert(
            "/v1/payment_intents/pi_fixture".into(),
            json!({"id":"pi_fixture","livemode":false,"latest_charge":"ch_fixture"}),
        );
        r.insert("/v1/charges/ch_fixture".into(), charge());
    }
    let body = event_json.to_string().into_bytes();
    assert_eq!(
        webhook(State(s.clone()), HeaderMap::new(), body.clone().into())
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..2 {
        assert_eq!(
            webhook(State(s.clone()), headers(&body), body.clone().into())
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='stripe_event'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let payload = json!({"event_id":"evt_fixture","account":"acct_fixture"});
    for _ in 0..2 {
        event(&s, &payload).await.unwrap();
    }
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM provider_receipts),(SELECT count(*) FROM entitlements WHERE user_id=$1 AND revoked_at IS NULL)").bind(buyer).fetch_one(&s.db).await.unwrap();
    assert_eq!(counts, (1, 1));
    let status: String = sqlx::query_scalar("SELECT status FROM checkouts WHERE id=$1")
        .bind(c)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(status, "paid");
    let mut refunded = charge();
    refunded["amount_refunded"] = json!(250);
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), refunded);
    sync_reversal(&s, "acct_fixture", "ch_fixture")
        .await
        .unwrap();
    // An older observation never reduces a refund or restores an entitlement.
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), charge());
    sync_reversal(&s, "acct_fixture", "ch_fixture")
        .await
        .unwrap();
    let result:(i64,i64)=sqlx::query_as("SELECT (SELECT refunded_minor FROM provider_receipts),(SELECT count(*) FROM entitlements WHERE revoked_at IS NULL)").fetch_one(&s.db).await.unwrap();
    assert_eq!(result, (250, 0));
    assert!(
        sqlx::query("UPDATE checkouts SET provider='sandbox',provider_account=NULL WHERE id=$1")
            .bind(c)
            .execute(&s.db)
            .await
            .is_err()
    );
    for (_, headers, _) in f.calls.lock().await.iter() {
        assert_eq!(headers.get("stripe-account").unwrap(), "acct_fixture");
    }
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn checkout_uses_pinned_contract_prices_and_never_grants_access_from_creation(db: PgPool) {
    let (s, f, task, c, _) = setup(db, "subscription").await;
    f.responses.lock().await.insert("/v1/checkout/sessions".into(),json!({"id":"cs_fixture","livemode":false,"status":"open","client_reference_id":c.to_string(),"currency":"usd","amount_total":1000,"url":"https://checkout.stripe.com/c/pay/cs_fixture#fragment"}));
    let Json(result) = checkout(&s, c).await.unwrap();
    assert_eq!(result["confirmation_required"], false);
    let calls = f.calls.lock().await;
    let (_, h, body) = calls.last().unwrap();
    assert_eq!(
        h.get("idempotency-key").unwrap(),
        &format!("myfans-checkout:{c}")
    );
    let fields: HashMap<_, _> = url::form_urlencoded::parse(body).into_owned().collect();
    assert_eq!(fields["line_items[0][price_data][unit_amount]"], "1000");
    assert_eq!(fields["allowed_payment_method_types[0]"], "card");
    assert!(!fields.contains_key("payment_method_types[0]"));
    assert_eq!(fields["subscription_data[application_fee_percent]"], "20");
    assert_eq!(fields["automatic_tax[enabled]"], "true");
    assert_eq!(
        fields["success_url"],
        "https://myfans.test/settings?checkout=returned"
    );
    assert_eq!(
        fields["cancel_url"],
        "https://myfans.test/?checkout=canceled"
    );
    assert_eq!(
        fields["line_items[0][price_data][tax_behavior]"],
        "inclusive"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM entitlements")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    task.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn paid_invoice_renewals_create_one_entitlement_per_verified_period(db: PgPool) {
    let (s, f, task, c, _) = setup(db, "subscription").await;
    let start = Utc::now().timestamp() - 60;
    let end = start + 30 * 86400;
    let invoice = json!({"id":"in_fixture","livemode":false,"status":"paid","amount_remaining":0,"amount_paid_off_stripe":0,"parent":{"subscription_details":{"subscription":"sub_fixture"}},"lines":{"data":[{"currency":"usd","period":{"start":start,"end":end}}],"has_more":false},"payments":{"data":[{"status":"paid","payment":{"payment_intent":"pi_fixture"}}],"has_more":false}});
    {
        let mut r = f.responses.lock().await;
        r.insert("/v1/invoices/in_fixture".into(), invoice.clone());
        r.insert("/v1/subscriptions/sub_fixture".into(),json!({"id":"sub_fixture","livemode":false,"status":"active","cancel_at_period_end":false,"metadata":{"myfans_checkout":c.to_string()}}));
        r.insert(
            "/v1/payment_intents/pi_fixture".into(),
            json!({"id":"pi_fixture","livemode":false,"latest_charge":"ch_fixture"}),
        );
        r.insert("/v1/charges/ch_fixture".into(), charge());
    }
    for _ in 0..2 {
        sync_invoice(&s, "acct_fixture", "in_fixture")
            .await
            .unwrap();
    }
    let mut renewed = invoice;
    renewed["id"] = json!("in_renewal");
    renewed["lines"]["data"][0]["period"] = json!({"start":end,"end":end+30*86400});
    renewed["payments"]["data"][0]["payment"]["payment_intent"] = json!("pi_renewal");
    {
        let mut r = f.responses.lock().await;
        r.insert("/v1/invoices/in_renewal".into(), renewed);
        r.insert(
            "/v1/payment_intents/pi_renewal".into(),
            json!({"id":"pi_renewal","livemode":false,"latest_charge":"ch_renewal"}),
        );
        let mut ch = charge();
        ch["id"] = json!("ch_renewal");
        r.insert("/v1/charges/ch_renewal".into(), ch);
    }
    sync_invoice(&s, "acct_fixture", "in_renewal")
        .await
        .unwrap();
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM provider_receipts),(SELECT count(*) FROM entitlements),(SELECT count(*) FROM subscriptions)").fetch_one(&s.db).await.unwrap();
    assert_eq!(counts, (2, 2, 1));
    let paid_until: DateTime<Utc> =
        sqlx::query_scalar("SELECT current_period_end FROM subscriptions")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(paid_until.timestamp(), end + 30 * 86400);
    task.abort();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn past_due_cancellation_confirms_provider_is_owned_and_preserves_paid_access(db: PgPool) {
    let (s, f, task, checkout, buyer) = setup(db, "subscription").await;
    let creator: Uuid = sqlx::query_scalar("SELECT creator_id FROM checkouts WHERE id=$1")
        .bind(checkout)
        .fetch_one(&s.db)
        .await
        .unwrap();
    let subscription:Uuid=sqlx::query_scalar("INSERT INTO subscriptions(user_id,creator_id,status,provider,provider_subscription_id,current_period_start,current_period_end) VALUES($1,$2,'past_due','stripe','sub_cancel',now()-interval '1 day',now()+interval '29 days') RETURNING id").bind(buyer).bind(creator).fetch_one(&s.db).await.unwrap();
    sqlx::query("INSERT INTO entitlements(user_id,creator_id,subscription_id,source,expires_at) SELECT user_id,creator_id,id,'subscription',current_period_end FROM subscriptions WHERE id=$1").bind(subscription).execute(&s.db).await.unwrap();
    let original_period: DateTime<Utc> =
        sqlx::query_scalar("SELECT current_period_end FROM subscriptions WHERE id=$1")
            .bind(subscription)
            .fetch_one(&s.db)
            .await
            .unwrap();
    let other:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('another@test.invalid','Other') RETURNING id").fetch_one(&s.db).await.unwrap();
    let mut headers = Vec::new();
    for account in [buyer, other] {
        let token = random_token();
        sqlx::query("INSERT INTO sessions(user_id,token_digest) VALUES($1,$2)")
            .bind(account)
            .bind(digest(&token))
            .execute(&s.db)
            .await
            .unwrap();
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers.push(h);
    }
    let denied =
        crate::platform::billing::cancel(State(s.clone()), headers[1].clone(), Path(subscription))
            .await
            .unwrap_err();
    assert_eq!(denied.0, StatusCode::NOT_FOUND);
    assert!(f.calls.lock().await.is_empty());

    // A provider failure must not record a local cancellation or an audit success.
    assert!(
        crate::platform::billing::cancel(State(s.clone()), headers[0].clone(), Path(subscription),)
            .await
            .is_err()
    );
    let canceled: bool =
        sqlx::query_scalar("SELECT cancel_at_period_end FROM subscriptions WHERE id=$1")
            .bind(subscription)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert!(!canceled);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='subscription.canceled'",
    )
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(audit_count, 0);
    f.calls.lock().await.clear();
    f.responses.lock().await.insert(
        "/v1/subscriptions/sub_cancel".into(),
        json!({"id":"sub_cancel","livemode":false,"status":"past_due","cancel_at_period_end":true}),
    );

    let (first, concurrent) = tokio::join!(
        crate::platform::billing::cancel(State(s.clone()), headers[0].clone(), Path(subscription)),
        crate::platform::billing::cancel(State(s.clone()), headers[0].clone(), Path(subscription)),
    );
    assert_eq!(first.unwrap().0["cancel_at_period_end"], true);
    assert_eq!(concurrent.unwrap().0["cancel_at_period_end"], true);
    let calls = f.calls.lock().await.clone();
    assert!(!calls.is_empty());
    for (path, h, body) in &calls {
        assert_eq!(path, "/v1/subscriptions/sub_cancel");
        assert_eq!(h.get("stripe-account").unwrap(), "acct_fixture");
        assert_eq!(
            h.get("idempotency-key").unwrap().to_str().unwrap(),
            format!("myfans-cancel:{subscription}")
        );
        assert_eq!(body, b"cancel_at_period_end=true");
    }
    let repeated =
        crate::platform::billing::cancel(State(s.clone()), headers[0].clone(), Path(subscription))
            .await
            .unwrap();
    assert_eq!(repeated.0["cancel_at_period_end"], true);
    assert_eq!(f.calls.lock().await.len(), calls.len());
    let result:(String,bool,DateTime<Utc>,bool,i64)=sqlx::query_as("SELECT status,cancel_at_period_end,current_period_end,(SELECT revoked_at IS NULL FROM entitlements WHERE subscription_id=$1),(SELECT count(*) FROM audit_events WHERE action='subscription.canceled' AND resource_id=$1) FROM subscriptions WHERE id=$1").bind(subscription).fetch_one(&s.db).await.unwrap();
    assert_eq!(result, ("past_due".into(), true, original_period, true, 1));

    // Neither uncertain pending payments nor terminal rows initiate provider calls.
    for status in ["pending", "canceled", "expired"] {
        sqlx::query("UPDATE subscriptions SET status=$2,cancel_at_period_end=false WHERE id=$1")
            .bind(subscription)
            .bind(status)
            .execute(&s.db)
            .await
            .unwrap();
        let rejected = crate::platform::billing::cancel(
            State(s.clone()),
            headers[0].clone(),
            Path(subscription),
        )
        .await
        .unwrap_err();
        assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
        assert_eq!(f.calls.lock().await.len(), calls.len());
    }
    task.abort();
}

async fn capability_session(db: &PgPool, user: Uuid) -> HeaderMap {
    let token = random_token();
    sqlx::query("UPDATE app_users SET mfa_enabled=true WHERE id=$1")
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions(user_id,token_digest,mfa_verified_at) VALUES($1,$2,now())")
        .bind(user)
        .bind(digest(&token))
        .execute(db)
        .await
        .unwrap();
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    h
}
async fn gate(f: &Fixture, path: &str) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *f.gate.lock().await = Some((path.into(), entered.clone(), release.clone()));
    (entered, release)
}
async fn revoke(db: &PgPool, user: Uuid, freeze: bool) {
    let mut tx = db.begin().await.unwrap();
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    if freeze {
        sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_stripe_creation_retains_external_id_but_withholds_hosted_capability(db: PgPool) {
    let (s, f, server, _, _) = setup(db, "tip").await;
    let owner:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,email_verified,role) VALUES('connect-lifecycle@test.invalid','Creator',true,'creator') RETURNING id").fetch_one(&s.db).await.unwrap();
    sqlx::query("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'connect_lifecycle',1000,'USD','pending')").bind(owner).execute(&s.db).await.unwrap();
    let h = capability_session(&s.db, owner).await;
    f.responses.lock().await.insert(
        "/v1/accounts".into(),
        json!({"id":"acct_lifecycle","livemode":false}),
    );
    let (entered, release) = gate(&f, "/v1/accounts").await;
    let state = s.clone();
    let call = tokio::spawn(async move { account(State(state), h).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    revoke(&s.db, owner, true).await;
    release.notify_one();
    assert_eq!(call.await.unwrap().unwrap_err().0, StatusCode::UNAUTHORIZED);
    let stored: String =
        sqlx::query_scalar("SELECT account_id FROM provider_accounts WHERE user_id=$1")
            .bind(owner)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(stored, "acct_lifecycle");
    assert_eq!(
        f.calls
            .lock()
            .await
            .iter()
            .filter(|(p, _, _)| p == "/v1/account_links")
            .count(),
        0
    );
    server.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_stripe_checkout_retains_session_but_never_returns_url_after_freeze(db: PgPool) {
    let (s, f, server, checkout_id, buyer) = setup(db, "tip").await;
    let h = capability_session(&s.db, buyer).await;
    f.responses.lock().await.insert("/v1/checkout/sessions".into(),json!({"id":"cs_lifecycle","livemode":false,"status":"open","client_reference_id":checkout_id.to_string(),"currency":"usd","amount_total":1000,"url":"https://checkout.stripe.com/c/pay/cs_lifecycle"}));
    let (entered, release) = gate(&f, "/v1/checkout/sessions").await;
    let state = s.clone();
    let call =
        tokio::spawn(async move { authorized_checkout(&state, &h, buyer, checkout_id).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    revoke(&s.db, buyer, true).await;
    release.notify_one();
    assert_eq!(call.await.unwrap().unwrap_err().0, StatusCode::UNAUTHORIZED);
    let record:(String,Option<String>,bool)=sqlx::query_as("SELECT status,provider_session,provider_attempted_at IS NOT NULL FROM checkouts WHERE id=$1").bind(checkout_id).fetch_one(&s.db).await.unwrap();
    assert_eq!(
        record,
        ("pending".into(), Some("cs_lifecycle".into()), true)
    );
    server.abort();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_stripe_dashboard_rejects_session_recovered_during_provider_wait(db: PgPool) {
    let (s, f, server, _, _) = setup(db, "tip").await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM provider_accounts")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let h = capability_session(&s.db, owner).await;
    let path = "/v1/accounts/acct_fixture/login_links";
    f.responses.lock().await.insert(
        path.into(),
        json!({"url":"https://connect.stripe.com/express/fixture"}),
    );
    let (entered, release) = gate(&f, path).await;
    let state = s.clone();
    let call = tokio::spawn(async move { dashboard(State(state), h).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    revoke(&s.db, owner, false).await;
    release.notify_one();
    assert_eq!(call.await.unwrap().unwrap_err().0, StatusCode::UNAUTHORIZED);
    server.abort();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_stripe_cancellation_retains_remote_success_after_session_recovery(db: PgPool) {
    let (s, f, server, checkout, buyer) = setup(db, "subscription").await;
    let creator: Uuid = sqlx::query_scalar("SELECT creator_id FROM checkouts WHERE id=$1")
        .bind(checkout)
        .fetch_one(&s.db)
        .await
        .unwrap();
    let subscription:Uuid=sqlx::query_scalar("INSERT INTO subscriptions(user_id,creator_id,status,provider,provider_subscription_id,current_period_start,current_period_end) VALUES($1,$2,'past_due','stripe','sub_lifecycle',now(),now()+interval '1 day') RETURNING id").bind(buyer).bind(creator).fetch_one(&s.db).await.unwrap();
    let period: DateTime<Utc> =
        sqlx::query_scalar("SELECT current_period_end FROM subscriptions WHERE id=$1")
            .bind(subscription)
            .fetch_one(&s.db)
            .await
            .unwrap();
    let h = capability_session(&s.db, buyer).await;
    let path = "/v1/subscriptions/sub_lifecycle";
    f.responses.lock().await.insert(
        path.into(),
        json!({"id":"sub_lifecycle","livemode":false,"cancel_at_period_end":true}),
    );
    let (entered, release) = gate(&f, path).await;
    let state = s.clone();
    let call = tokio::spawn(async move {
        crate::platform::billing::cancel(State(state), h, Path(subscription)).await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    revoke(&s.db, buyer, false).await;
    release.notify_one();
    assert_eq!(call.await.unwrap().unwrap_err().0, StatusCode::UNAUTHORIZED);
    let current: (bool, String, DateTime<Utc>) = sqlx::query_as(
        "SELECT cancel_at_period_end,status,current_period_end FROM subscriptions WHERE id=$1",
    )
    .bind(subscription)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(current, (true, "past_due".into(), period));
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action='subscription.canceled' AND resource_id=$1",
    )
    .bind(subscription)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    server.abort();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_provider_receipt_locks_accounts_first_and_records_frozen_payment(db: PgPool) {
    let (s, f, server, checkout, buyer) = setup(db, "tip").await;
    f.responses
        .lock()
        .await
        .insert("/v1/charges/ch_fixture".into(), charge());
    let mut freeze = s.db.begin().await.unwrap();
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
        .bind(buyer)
        .execute(&mut *freeze)
        .await
        .unwrap();
    let state = s.clone();
    let observation = tokio::spawn(async move {
        record(
            &state,
            "acct_fixture",
            checkout,
            "ch_fixture",
            None,
            None,
            None,
        )
        .await
    });
    crate::platform::lifecycle_tests::waiting(&s.db, &mut freeze).await;
    // A checkout-first implementation is already holding this row while it
    // waits for the buyer's implicit foreign-key lock, forming a deadlock with
    // a buyer-authorized operation. NOWAIT makes that inversion deterministic.
    sqlx::query("SELECT id FROM checkouts WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(checkout)
        .execute(&mut *freeze)
        .await
        .unwrap();
    freeze.commit().await.unwrap();
    observation.await.unwrap().unwrap();
    let state:(String,i64)=sqlx::query_as("SELECT status,(SELECT count(*) FROM provider_receipts WHERE checkout_id=$1) FROM checkouts WHERE id=$1").bind(checkout).fetch_one(&s.db).await.unwrap();
    assert_eq!(state, ("paid".into(), 1));
    server.abort();
}
