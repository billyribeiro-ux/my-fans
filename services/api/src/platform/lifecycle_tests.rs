#![allow(clippy::unwrap_used)]
use super::*;
use axum::{body::Body, http::Request};
use sqlx::{Postgres, Transaction};
use tower::ServiceExt;

pub(super) async fn fixture(db: PgPool) -> (PlatformState, Uuid, Uuid, String) {
    let owner:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,email_verified,role) VALUES('lifecycle@test.invalid','Private account',true,'creator') RETURNING id").fetch_one(&db).await.unwrap();
    let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,display_name,bio,subscription_price_minor,currency,verification_status) VALUES($1,'lifecycle','Private creator','Private biography',500,'USD','approved') RETURNING id").bind(owner).fetch_one(&db).await.unwrap();
    let token = random_token();
    sqlx::query("INSERT INTO sessions(user_id,token_digest) VALUES($1,$2)")
        .bind(owner)
        .bind(digest(&token))
        .execute(&db)
        .await
        .unwrap();
    let s = PlatformState {
        db,
        runtime: Default::default(),
        media_root: PathBuf::new(),
        key: Arc::new([9; 32]),
        dummy_hash: Arc::new("invalid".into()),
        mail: Default::default(),
        storage: MediaStore::local(PathBuf::new()),
        scanner: Default::default(),
        providers: Default::default(),
    };
    (s, owner, creator, token)
}
async fn request(s: PlatformState, token: String, path: String, value: Value) -> StatusCode {
    request_method(s, token, "POST", path, value).await
}
async fn request_method(
    s: PlatformState,
    token: String,
    method: &str,
    path: String,
    value: Value,
) -> StatusCode {
    router(s)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}
// Uncommitted account/creator changes remain invisible to the initial auth read.
// The next write must wait for this real PostgreSQL row lock, then recheck state.
async fn freeze(db: &PgPool, owner: Uuid, creator: Uuid) -> Transaction<'_, Postgres> {
    let mut tx = db.begin().await.unwrap();
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE app_users SET disabled_at=now(),erased_at=now(),display_name='Deleted account',email='erased@test.invalid',password_hash=NULL,mfa_secret=NULL,mfa_enabled=false WHERE id=$1").bind(owner).execute(&mut *tx).await.unwrap();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(owner)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE creators SET display_name='Deleted creator',bio='',verification_status='rejected',verified_at=NULL WHERE id=$1").bind(creator).execute(&mut *tx).await.unwrap();
    tx
}
pub(super) async fn waiting(db: &PgPool, tx: &mut Transaction<'_, Postgres>) {
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **tx)
        .await
        .unwrap();
    for _ in 0..150 {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
        )
        .bind(pid)
        .fetch_one(db)
        .await
        .unwrap();
        if blocked {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("mutation did not reach the held account/creator row");
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_profile_cannot_repopulate_erased_creator(db: PgPool) {
    let (s, owner, creator, token) = fixture(db).await;
    let mut erasure = freeze(&s.db, owner, creator).await;
    let task = tokio::spawn(request(
        s.clone(),
        token,
        "/v1/creators/profile".into(),
        json!({"handle":"lifecycle","display_name":"Reintroduced private name","bio":"Reintroduced private biography","category":"Art","subscription_price_minor":500}),
    ));
    waiting(&s.db, &mut erasure).await;
    erasure.commit().await.unwrap();
    let _ = task.await.unwrap();
    let row: (String, String) = sqlx::query_as("SELECT display_name,bio FROM creators WHERE id=$1")
        .bind(creator)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(row, ("Deleted creator".into(), "".into()));
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_checkout_cannot_create_obligation_after_freeze(db: PgPool) {
    let (s, owner, creator, token) = fixture(db).await;
    let seller:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('seller@test.invalid','Seller') RETURNING id").fetch_one(&s.db).await.unwrap();
    let seller_creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'seller',500,'USD','approved') RETURNING id").bind(seller).fetch_one(&s.db).await.unwrap();
    let mut erasure = freeze(&s.db, owner, creator).await;
    let task = tokio::spawn(request(
        s.clone(),
        token,
        "/v1/checkout".into(),
        json!({"kind":"tip","creator_id":seller_creator,"amount_minor":500,"idempotency_key":Uuid::new_v4()}),
    ));
    waiting(&s.db, &mut erasure).await;
    erasure.commit().await.unwrap();
    let status = task.await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM checkouts WHERE user_id=$1")
        .bind(owner)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "request returned {status} and created a new obligation after freeze"
    );
    assert!(!status.is_success());
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_account_and_authenticator_cannot_repopulate_erased_fields(db: PgPool) {
    let (s, owner, creator, token) = fixture(db).await;
    let mut erasure = freeze(&s.db, owner, creator).await;
    let name = tokio::spawn(request_method(
        s.clone(),
        token.clone(),
        "PATCH",
        "/v1/account".into(),
        json!({"display_name":"Returned private name"}),
    ));
    waiting(&s.db, &mut erasure).await;
    let mfa = tokio::spawn(request(
        s.clone(),
        token,
        "/v1/account/mfa".into(),
        json!({"action":"setup"}),
    ));
    // Both handlers see the previously active account, then block on its row.
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *erasure)
        .await
        .unwrap();
    for _ in 0..150 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))",
        )
        .bind(pid)
        .fetch_one(&s.db)
        .await
        .unwrap();
        if n >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    erasure.commit().await.unwrap();
    assert_eq!(name.await.unwrap(), StatusCode::UNAUTHORIZED);
    assert_eq!(mfa.await.unwrap(), StatusCode::UNAUTHORIZED);
    let row: (String, Option<Vec<u8>>) =
        sqlx::query_as("SELECT display_name,mfa_secret FROM app_users WHERE id=$1")
            .bind(owner)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(row, ("Deleted account".into(), None));
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_profile_rechecks_the_same_session_after_recovery(db: PgPool) {
    let (s, owner, creator, token) = fixture(db).await;
    let mut recovery = s.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *recovery)
        .await
        .unwrap();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(owner)
        .execute(&mut *recovery)
        .await
        .unwrap();
    let task = tokio::spawn(request(
        s.clone(),
        token,
        "/v1/creators/profile".into(),
        json!({"handle":"lifecycle","display_name":"Stale session name","bio":"Stale session text","category":"Art","subscription_price_minor":500}),
    ));
    waiting(&s.db, &mut recovery).await;
    recovery.commit().await.unwrap();
    assert_eq!(task.await.unwrap(), StatusCode::UNAUTHORIZED);
    let name: String = sqlx::query_scalar("SELECT display_name FROM creators WHERE id=$1")
        .bind(creator)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(name, "Private creator");
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_confirm_cannot_move_money_after_buyer_freeze(db: PgPool) {
    let (s, owner, creator, token) = fixture(db).await;
    let seller:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('seller@test.invalid','Seller') RETURNING id").fetch_one(&s.db).await.unwrap();
    let seller_creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'seller',500,'USD','approved') RETURNING id").bind(seller).fetch_one(&s.db).await.unwrap();
    let checkout:Uuid=sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,kind,amount_minor,idempotency_key,request_fingerprint) VALUES($1,$2,'tip',500,$3,'freeze') RETURNING id").bind(owner).bind(seller_creator).bind(Uuid::new_v4()).fetch_one(&s.db).await.unwrap();
    let mut erasure = freeze(&s.db, owner, creator).await;
    let task = tokio::spawn(request(
        s.clone(),
        token,
        format!("/v1/checkout/{checkout}/confirm"),
        json!({}),
    ));
    waiting(&s.db, &mut erasure).await;
    erasure.commit().await.unwrap();
    assert!(!task.await.unwrap().is_success());
    let status: String = sqlx::query_scalar("SELECT status FROM checkouts WHERE id=$1")
        .bind(checkout)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(status, "pending");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_transactions")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_admin_rechecks_role_and_recent_mfa_after_wait(db: PgPool) {
    let (s, actor, _, token) = fixture(db).await;
    sqlx::query("UPDATE app_users SET role='admin',mfa_enabled=true WHERE id=$1")
        .bind(actor)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET mfa_verified_at=now() WHERE user_id=$1")
        .bind(actor)
        .execute(&s.db)
        .await
        .unwrap();
    let job:Uuid=sqlx::query_scalar("INSERT INTO jobs(kind,payload,status,attempts,last_error) VALUES('privacy_erasure','{}','failed',5,'fixture') RETURNING id").fetch_one(&s.db).await.unwrap();
    for demote in [true, false] {
        let mut revoke = s.db.begin().await.unwrap();
        sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
            .bind(actor)
            .execute(&mut *revoke)
            .await
            .unwrap();
        if demote {
            sqlx::query("UPDATE app_users SET role='creator' WHERE id=$1")
                .bind(actor)
                .execute(&mut *revoke)
                .await
                .unwrap();
        } else {
            sqlx::query(
                "UPDATE sessions SET mfa_verified_at=now()-interval '16 minutes' WHERE user_id=$1",
            )
            .bind(actor)
            .execute(&mut *revoke)
            .await
            .unwrap();
        }
        let task = tokio::spawn(request(
            s.clone(),
            token.clone(),
            format!("/v1/admin/jobs/{job}/retry"),
            json!({}),
        ));
        waiting(&s.db, &mut revoke).await;
        revoke.commit().await.unwrap();
        assert_eq!(task.await.unwrap(), StatusCode::FORBIDDEN);
        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert_eq!(status, "failed");
        sqlx::query("UPDATE app_users SET role='admin' WHERE id=$1")
            .bind(actor)
            .execute(&s.db)
            .await
            .unwrap();
    }
    let audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action='job.retried'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(audit, 0);
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_payout_rechecks_mfa_before_reserving_funds(db: PgPool) {
    let (s, owner, _, token) = fixture(db).await;
    sqlx::query("UPDATE app_users SET mfa_enabled=true WHERE id=$1")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET mfa_verified_at=now() WHERE user_id=$1")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    let mut funding = s.db.begin().await.unwrap();
    let source = billing::account_id(&mut funding, None, "processor_clearing")
        .await
        .unwrap();
    let available = billing::account_id(&mut funding, Some(owner), "creator_available")
        .await
        .unwrap();
    billing::post_ledger(
        &mut funding,
        "lifecycle-funding",
        &[(source, -1000), (available, 1000)],
    )
    .await
    .unwrap();
    funding.commit().await.unwrap();
    let mut recovery = s.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *recovery)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET mfa_verified_at=NULL WHERE user_id=$1")
        .bind(owner)
        .execute(&mut *recovery)
        .await
        .unwrap();
    let task = tokio::spawn(request(
        s.clone(),
        token,
        "/v1/payouts".into(),
        json!({"amount_minor":500,"idempotency_key":Uuid::new_v4()}),
    ));
    waiting(&s.db, &mut recovery).await;
    recovery.commit().await.unwrap();
    assert_eq!(task.await.unwrap(), StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payouts")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let posted: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_transactions")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(posted, 1);
    let balance: i64 = sqlx::query_scalar(
        "SELECT sum(amount_minor)::bigint FROM ledger_postings WHERE account_id=$1",
    )
    .bind(available)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(balance, 1000);
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_password_change_cannot_overwrite_after_session_recovery(db: PgPool) {
    let (s, owner, _, token) = fixture(db).await;
    let password = "Lifecycle-old-password-42!";
    let old_hash = auth::hash_password(password.into()).await.unwrap();
    sqlx::query("UPDATE app_users SET password_hash=$2 WHERE id=$1")
        .bind(owner)
        .bind(&old_hash)
        .execute(&s.db)
        .await
        .unwrap();
    let mut recovery = s.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *recovery)
        .await
        .unwrap();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(owner)
        .execute(&mut *recovery)
        .await
        .unwrap();
    let task = tokio::spawn(request(
        s.clone(),
        token,
        "/v1/account/password".into(),
        json!({"current_password":password,"password":"Lifecycle-forbidden-password-43!"}),
    ));
    waiting(&s.db, &mut recovery).await;
    recovery.commit().await.unwrap();
    assert_eq!(task.await.unwrap(), StatusCode::UNAUTHORIZED);
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM app_users WHERE id=$1")
        .bind(owner)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(hash, old_hash);
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_refund_locks_buyer_before_checkout_and_subscription(db: PgPool) {
    let (s, buyer, _, token) = fixture(db).await;
    let seller:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('seller@test.invalid','Seller') RETURNING id").fetch_one(&s.db).await.unwrap();
    let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'seller',500,'USD','approved') RETURNING id").bind(seller).fetch_one(&s.db).await.unwrap();
    let checkout:Uuid=sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,kind,amount_minor,idempotency_key,request_fingerprint) VALUES($1,$2,'subscription',500,$3,'lifecycle-refund') RETURNING id").bind(buyer).bind(creator).bind(Uuid::new_v4()).fetch_one(&s.db).await.unwrap();
    assert_eq!(
        request(
            s.clone(),
            token,
            format!("/v1/checkout/{checkout}/confirm"),
            json!({})
        )
        .await,
        StatusCode::OK
    );
    let actor:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,role,mfa_enabled) VALUES('operator@test.invalid','Operator','admin',true) RETURNING id").fetch_one(&s.db).await.unwrap();
    let admin_token = random_token();
    sqlx::query("INSERT INTO sessions(user_id,token_digest,mfa_verified_at) VALUES($1,$2,now())")
        .bind(actor)
        .bind(digest(&admin_token))
        .execute(&s.db)
        .await
        .unwrap();
    let mut buyer_request = s.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(buyer)
        .execute(&mut *buyer_request)
        .await
        .unwrap();
    let refund = tokio::spawn(request(
        s.clone(),
        admin_token,
        format!("/v1/admin/refunds/{checkout}"),
        json!({"reason":"Requested subscription refund"}),
    ));
    waiting(&s.db, &mut buyer_request).await;
    sqlx::query("SELECT id FROM checkouts WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(checkout)
        .execute(&mut *buyer_request)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM subscriptions WHERE user_id=$1 FOR UPDATE NOWAIT")
        .bind(buyer)
        .execute(&mut *buyer_request)
        .await
        .unwrap();
    buyer_request.commit().await.unwrap();
    assert_eq!(refund.await.unwrap(), StatusCode::OK);
    let status: String = sqlx::query_scalar("SELECT status FROM checkouts WHERE id=$1")
        .bind(checkout)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(status, "refunded");
}
