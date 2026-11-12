#![allow(clippy::unwrap_used)]
use super::*;
use std::collections::HashSet;

async fn member(db: &PgPool, address: &str) -> (Uuid, HeaderMap) {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO app_users(email,display_name,email_verified) VALUES($1,'Member',true) RETURNING id",
    )
    .bind(address)
    .fetch_one(db)
    .await
    .unwrap();
    let token = random_token();
    sqlx::query("INSERT INTO sessions(user_id,token_digest,expires_at) VALUES($1,$2,now()+interval '1 hour')")
        .bind(id).bind(digest(&token)).execute(db).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    (id, headers)
}

async fn creator(db: &PgPool) -> Uuid {
    let (owner, _) = member(db, "creator@payments.invalid").await;
    let creator: Uuid = sqlx::query_scalar("INSERT INTO creators(user_id,handle,display_name,subscription_price_minor,currency) VALUES($1,'payment_history','History Creator',1000,'USD') RETURNING id")
        .bind(owner).fetch_one(db).await.unwrap();
    sqlx::query("INSERT INTO provider_accounts(user_id,account_id) VALUES($1,'acct_history')")
        .bind(owner)
        .execute(db)
        .await
        .unwrap();
    creator
}

fn state(db: PgPool) -> PlatformState {
    PlatformState {
        runtime: Default::default(),
        db,
        media_root: PathBuf::new(),
        key: Arc::new([7; 32]),
        dummy_hash: Arc::new(String::new()),
        mail: Default::default(),
        storage: MediaStore::local(PathBuf::new()),
        scanner: Default::default(),
        providers: Default::default(),
    }
}

async fn checkout(db: &PgPool, buyer: Uuid, creator: Uuid, provider: &str, status: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,kind,amount_minor,idempotency_key,request_fingerprint,provider,provider_account,status,paid_at,refunded_minor,created_at) VALUES($1,$2,'subscription',1000,$3,'history',$4,CASE WHEN $4='stripe' THEN 'acct_history' END,$5,CASE WHEN $5 IN ('paid','refunded') THEN '2026-09-01T00:00:00Z'::timestamptz END,CASE WHEN $5='refunded' THEN 1000 ELSE 0 END,'2026-09-01T00:00:00Z') RETURNING id")
        .bind(buyer).bind(creator).bind(Uuid::new_v4()).bind(provider).bind(status).fetch_one(db).await.unwrap()
}

async fn receipt(db: &PgPool, checkout: Uuid, refunded: i64, disputed: bool, month: i32) -> Uuid {
    sqlx::query_scalar("INSERT INTO provider_receipts(checkout_id,account_id,charge_id,invoice_id,subscription_id,amount_minor,refunded_minor,disputed,currency,period_start,period_end,created_at) VALUES($1,'acct_history',$2,$3,'sub_history',1000,$4,$5,'USD','2026-01-01'::timestamptz+make_interval(months=>$6),'2026-02-01'::timestamptz+make_interval(months=>$6),'2026-09-01T00:00:00Z') RETURNING id")
        .bind(checkout).bind(format!("ch_{}", Uuid::new_v4().simple())).bind(format!("in_{}", Uuid::new_v4().simple())).bind(refunded).bind(disputed).bind(month).fetch_one(db).await.unwrap()
}

async fn page(s: &PlatformState, h: &HeaderMap, cursor: Option<&str>) -> Value {
    let query = cursor.map_or_else(pagination::PageQuery::default, |cursor| {
        serde_json::from_value(json!({"payments_cursor":cursor})).unwrap()
    });
    account(State(s.clone()), h.clone(), Query(query))
        .await
        .unwrap()
        .0
}

async fn financial_snapshot(db: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('checkouts',(SELECT jsonb_agg(to_jsonb(x) ORDER BY id) FROM checkouts x),'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM provider_receipts r),'ledger',(SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM ledger_postings p))")
        .fetch_one(db).await.unwrap()
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn subscriber_payments_include_renewals_refunds_pending_and_are_private_reads(db: PgPool) {
    let (buyer, headers) = member(&db, "buyer@payments.invalid").await;
    let (other, other_headers) = member(&db, "other@payments.invalid").await;
    let creator = creator(&db).await;
    let recurring = checkout(&db, buyer, creator, "stripe", "paid").await;
    let first = receipt(&db, recurring, 0, false, 0).await;
    let partial = receipt(&db, recurring, 250, true, 1).await;
    let refunded = receipt(&db, recurring, 1000, false, 2).await;
    let pending = checkout(&db, buyer, creator, "stripe", "pending").await;
    let failed = checkout(&db, buyer, creator, "stripe", "failed").await;
    let sandbox_paid = checkout(&db, buyer, creator, "sandbox", "paid").await;
    let sandbox_refunded = checkout(&db, buyer, creator, "sandbox", "refunded").await;
    let sandbox_pending = checkout(&db, buyer, creator, "sandbox", "pending").await;
    let unrelated = checkout(&db, other, creator, "stripe", "paid").await;
    let unrelated_receipt = receipt(&db, unrelated, 0, false, 0).await;
    let before = financial_snapshot(&db).await;
    let s = state(db.clone());
    let account = page(&s, &headers, None).await;
    let rows = account["payments"].as_array().unwrap();
    assert_eq!(rows.len(), 8);
    assert!(account["next_cursors"]["payments"].is_null());
    let find = |id: Uuid| rows.iter().find(|r| r["id"] == id.to_string()).unwrap();
    assert_eq!(find(first)["checkout_id"], recurring.to_string());
    assert_eq!(find(first)["status"], "paid");
    assert_eq!(find(first)["period_start"], "2026-01-01T00:00:00+00:00");
    assert_eq!(find(first)["period_end"], "2026-02-01T00:00:00+00:00");
    assert_eq!(find(partial)["status"], "partially_refunded");
    assert_eq!(find(partial)["refunded_minor"], 250);
    assert_eq!(find(partial)["dispute_recorded"], true);
    assert_eq!(find(refunded)["status"], "refunded");
    assert_eq!(find(refunded)["refunded_minor"], 1000);
    assert_eq!(find(pending)["status"], "pending");
    assert_eq!(find(failed)["status"], "failed");
    assert_eq!(find(sandbox_paid)["provider"], "sandbox");
    assert!(find(sandbox_paid)["period_start"].is_null());
    assert_eq!(find(sandbox_refunded)["status"], "refunded");
    assert_eq!(find(sandbox_pending)["status"], "pending");
    assert!(rows.iter().all(|r| r["id"] != recurring.to_string()));
    let public_fields: HashSet<_> = [
        "id",
        "checkout_id",
        "creator_id",
        "creator_name",
        "kind",
        "status",
        "amount_minor",
        "refunded_minor",
        "currency",
        "created_at",
        "period_start",
        "period_end",
        "provider",
        "dispute_recorded",
    ]
    .into_iter()
    .collect();
    for row in rows {
        assert_eq!(row["creator_id"], creator.to_string());
        assert_eq!(row["creator_name"], "History Creator");
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
            public_fields
        );
    }
    assert_eq!(
        page(&s, &HeaderMap::new(), None).await["payments"],
        json!([])
    );
    let other_page = page(&s, &other_headers, None).await;
    assert_eq!(other_page["payments"].as_array().unwrap().len(), 1);
    assert_eq!(
        other_page["payments"][0]["id"],
        unrelated_receipt.to_string()
    );
    assert_eq!(financial_snapshot(&db).await, before);
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn subscriber_payments_page_tied_renewals_and_intents_without_duplicates(db: PgPool) {
    let (buyer, headers) = member(&db, "buyer@payments.invalid").await;
    let (_, other_headers) = member(&db, "other@payments.invalid").await;
    let creator = creator(&db).await;
    let recurring = checkout(&db, buyer, creator, "stripe", "paid").await;
    let mut expected = HashSet::new();
    for month in 0..57 {
        expected.insert(receipt(&db, recurring, 0, false, month).await.to_string());
    }
    for _ in 0..4 {
        expected.insert(
            checkout(&db, buyer, creator, "sandbox", "pending")
                .await
                .to_string(),
        );
    }
    let s = state(db.clone());
    let first = page(&s, &headers, None).await;
    assert_eq!(first["payments"].as_array().unwrap().len(), 50);
    let cursor = first["next_cursors"]["payments"].as_str().unwrap();
    let second = page(&s, &headers, Some(cursor)).await;
    assert_eq!(second["payments"].as_array().unwrap().len(), 11);
    assert!(second["next_cursors"]["payments"].is_null());
    let ids: Vec<_> = first["payments"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["payments"].as_array().unwrap())
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
    assert_eq!(ids.into_iter().collect::<HashSet<_>>(), expected);
    // A valid cursor from another account never authorizes access to its rows.
    assert_eq!(
        page(&s, &other_headers, Some(cursor)).await["payments"],
        json!([])
    );
    assert_eq!(
        page(&s, &HeaderMap::new(), Some(cursor)).await["payments"],
        json!([])
    );
    let invalid = serde_json::from_value(json!({"payments_cursor":"not-a-cursor"})).unwrap();
    assert_eq!(
        account(State(s.clone()), headers.clone(), Query(invalid))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
    let forged = serde_json::from_value(json!({"user_id":buyer})).unwrap();
    assert_eq!(
        account(State(s), other_headers, Query(forged))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_REQUEST
    );
}
