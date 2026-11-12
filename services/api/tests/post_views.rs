#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use myfans_api::platform::{self, PlatformState, worker};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup(db: PgPool) -> (PlatformState, Router) {
    let root = std::env::temp_dir().join(format!("myfans-post-view-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let state = PlatformState {
        runtime: Default::default(),
        db,
        storage: platform::MediaStore::local(root.clone()),
        scanner: Default::default(),
        providers: Default::default(),
        media_root: root,
        key: Arc::new([7; 32]),
        dummy_hash: Arc::new("invalid".into()),
        mail: Default::default(),
    };
    worker::seed(&state).await.unwrap();
    (state.clone(), platform::router(state))
}
async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
async fn session(db: &PgPool, email: &str) -> (Uuid, String) {
    let user = sqlx::query_scalar("SELECT id FROM app_users WHERE email=$1")
        .bind(email)
        .fetch_one(db)
        .await
        .unwrap();
    let token = platform::random_token();
    sqlx::query("INSERT INTO sessions(token_digest,user_id) VALUES($1,$2)")
        .bind(platform::digest(&token))
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    (user, token)
}
async fn post(db: &PgPool, creator: Uuid, visibility: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO posts(creator_id,caption,state,visibility,published_at,sample_media_url,sample_preview_url) VALUES($1,'An older permalink fixture','published',$2,now()-interval '90 days','https://images.unsplash.com/private-fixture','https://images.unsplash.com/preview-fixture') RETURNING id")
        .bind(creator).bind(visibility).fetch_one(db).await.unwrap()
}

async fn enable_mfa(app: &Router, token: &str) {
    async fn request(app: &Router, token: &str, body: Value) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/account/mfa")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{value}");
        value
    }
    let setup = request(app, token, json!({"action":"setup"})).await;
    let secret = totp_rs::Secret::try_from_base32(setup["secret"].as_str().unwrap()).unwrap();
    let totp = totp_rs::Builder::new()
        .with_secret(secret)
        .with_account_name("admin@myfans.test")
        .build()
        .unwrap();
    request(
        app,
        token,
        json!({"action":"enable","code":totp.generate_current().to_string()}),
    )
    .await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn moderator_inspection_requires_current_assurance_and_preserves_public_entitlements(
    db: PgPool,
) {
    let (s, app) = setup(db).await;
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE handle='ellawild'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let id = post(&s.db, creator, "subscribers").await;
    let path = format!("/v1/admin/posts/{id}");
    let public_path = format!("/v1/feed?post={id}");
    let (_, fan) = session(&s.db, "demo@myfans.test").await;
    let (admin_id, admin) = session(&s.db, "admin@myfans.test").await;
    assert_eq!(get(&app, &path, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get(&app, &path, Some(&fan)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&app, &path, Some(&admin)).await.0,
        StatusCode::FORBIDDEN
    );
    enable_mfa(&app, &admin).await;
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM entitlements),(SELECT count(*) FROM subscriptions),(SELECT count(*) FROM ledger_postings)").fetch_one(&s.db).await.unwrap();
    let (status, inspected) = get(&app, &path, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{inspected}");
    assert_eq!(inspected["viewer_id"], admin_id.to_string());
    assert_eq!(inspected["creator"]["id"], creator.to_string());
    assert_eq!(inspected["post"]["id"], id.to_string());
    assert_eq!(inspected["post"]["locked"], false);
    assert_eq!(inspected["post"]["visibility"], "subscribers");
    assert!(inspected["creator"]["email"].is_null());
    // The explicit audited inspection must not make the public feed privileged.
    let (_, public) = get(&app, &public_path, Some(&admin)).await;
    assert_eq!(public["posts"][0]["locked"], true);
    assert!(public["posts"][0]["media_url"].is_null());
    sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    let (status, removed) = get(&app, &path, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["post"]["state"], "removed");
    assert_eq!(removed["post"]["moderation_blocked"], true);
    assert_eq!(
        get(&app, &public_path, Some(&admin)).await.1["posts"],
        json!([])
    );
    assert_eq!(
        get(
            &app,
            &format!("/v1/admin/posts/{}", Uuid::new_v4()),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    sqlx::query(
        "UPDATE sessions SET mfa_verified_at=now()-interval '16 minutes' WHERE token_digest=$1",
    )
    .bind(platform::digest(&admin))
    .execute(&s.db)
    .await
    .unwrap();
    assert_eq!(
        get(&app, &path, Some(&admin)).await.0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE sessions SET mfa_verified_at=now() WHERE token_digest=$1")
        .bind(platform::digest(&admin))
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE app_users SET mfa_recovery_required=true WHERE id=$1")
        .bind(admin_id)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        get(&app, &path, Some(&admin)).await.0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("DELETE FROM sessions WHERE token_digest=$1")
        .bind(platform::digest(&admin))
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        get(&app, &path, Some(&admin)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM entitlements),(SELECT count(*) FROM subscriptions),(SELECT count(*) FROM ledger_postings)").fetch_one(&s.db).await.unwrap();
    assert_eq!(after, before);
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE actor_id=$1 AND resource_id=$2 AND action='post.inspected'")
        .bind(admin_id).bind(id).fetch_one(&s.db).await.unwrap();
    assert_eq!(audits, 2);
    tokio::fs::remove_dir_all(&s.media_root).await.unwrap();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn moderator_private_media_is_gated_independently_of_inspection_response(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE handle='ellawild'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let id = post(&s.db, creator, "subscribers").await;
    let key = format!("{}.jpg", Uuid::new_v4());
    let bytes = b"private moderator media fixture";
    s.storage
        .put(&key, bytes.to_vec(), "image/jpeg")
        .await
        .unwrap();
    let asset: Uuid = sqlx::query_scalar("INSERT INTO media_assets(creator_id,storage_key,state,size_bytes,content_type) VALUES($1,$2,'approved',$3,'image/jpeg') RETURNING id")
        .bind(creator).bind(&key).bind(bytes.len() as i64).fetch_one(&s.db).await.unwrap();
    sqlx::query("INSERT INTO post_media(post_id,asset_id,creator_id,position) VALUES($1,$2,$3,0)")
        .bind(id)
        .bind(asset)
        .bind(creator)
        .execute(&s.db)
        .await
        .unwrap();
    let path = format!("/v1/admin/posts/{id}");
    let media_path = format!("/v1/media/{asset}/full");
    let (_, fan) = session(&s.db, "demo@myfans.test").await;
    let (_, admin) = session(&s.db, "admin@myfans.test").await;
    enable_mfa(&app, &admin).await;
    assert_eq!(
        get(&app, &media_path, Some(&fan)).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, inspected) = get(&app, &path, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{inspected}");
    assert_eq!(inspected["post"]["media_state"], "approved");
    assert_eq!(
        inspected["post"]["media_url"],
        format!("/api/platform/media/{asset}/full")
    );
    sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&media_path)
                .header("authorization", format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    assert_eq!(
        &to_bytes(response.into_body(), 1024).await.unwrap()[..],
        bytes
    );
    sqlx::query(
        "UPDATE sessions SET mfa_verified_at=now()-interval '16 minutes' WHERE token_digest=$1",
    )
    .bind(platform::digest(&admin))
    .execute(&s.db)
    .await
    .unwrap();
    assert_eq!(
        get(&app, &media_path, Some(&admin)).await.0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE sessions SET mfa_verified_at=now() WHERE token_digest=$1")
        .bind(platform::digest(&admin))
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE media_assets SET state='rejected' WHERE id=$1")
        .bind(asset)
        .execute(&s.db)
        .await
        .unwrap();
    let (status, rejected) = get(&app, &path, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["post"]["media_state"], "rejected");
    assert!(rejected["post"]["media_url"].is_null());
    assert!(rejected["post"]["captions_url"].is_null());
    assert!(rejected["post"]["hls_url"].is_null());
    assert_eq!(
        get(&app, &media_path, Some(&admin)).await.0,
        StatusCode::BAD_REQUEST
    );
    tokio::fs::remove_dir_all(&s.media_root).await.unwrap();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn permalink_resolves_older_posts_without_bypassing_visibility_or_revocation(db: PgPool) {
    let (s, app) = setup(db).await;
    let (creator, owner): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,user_id FROM creators WHERE handle='ellawild'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let public = post(&s.db, creator, "public").await;
    let paid = post(&s.db, creator, "subscribers").await;
    sqlx::query("INSERT INTO posts(creator_id,caption,state,visibility,published_at) SELECT $1,'Newer fixture '||n,'published','public',now() FROM generate_series(1,55)n").bind(creator).execute(&s.db).await.unwrap();
    let (_, feed) = get(&app, "/v1/feed", None).await;
    assert_eq!(feed["posts"].as_array().unwrap().len(), 50);
    assert!(
        !feed["posts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == public.to_string())
    );
    let public_path = format!("/v1/feed?post={public}");
    let (status, value) = get(&app, &public_path, None).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["posts"].as_array().unwrap().len(), 1);
    assert_eq!(value["posts"][0]["id"], public.to_string());
    assert_eq!(value["posts"][0]["locked"], false);
    assert!(value["next_cursor"].is_null());
    assert_eq!(
        get(&app, "/v1/feed?post=invalid", None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, &format!("/v1/feed?post={}", Uuid::new_v4()), None)
            .await
            .1["posts"],
        json!([])
    );

    let paid_path = format!("/v1/feed?post={paid}");
    let (_, guest) = get(&app, &paid_path, None).await;
    assert_eq!(guest["posts"][0]["locked"], true);
    assert!(guest["posts"][0]["media_url"].is_null());
    let (buyer, token) = session(&s.db, "demo@myfans.test").await;
    let entitlement: Uuid=sqlx::query_scalar("INSERT INTO entitlements(user_id,creator_id,source,expires_at) VALUES($1,$2,'promotion',now()+interval '1 day') RETURNING id").bind(buyer).bind(creator).fetch_one(&s.db).await.unwrap();
    let (_, entitled) = get(&app, &paid_path, Some(&token)).await;
    assert_eq!(entitled["posts"][0]["locked"], false);
    assert_eq!(
        entitled["posts"][0]["media_url"],
        "https://images.unsplash.com/private-fixture"
    );
    sqlx::query("UPDATE entitlements SET revoked_at=now() WHERE id=$1")
        .bind(entitlement)
        .execute(&s.db)
        .await
        .unwrap();
    let (_, revoked) = get(&app, &paid_path, Some(&token)).await;
    assert_eq!(revoked["posts"][0]["locked"], true);
    assert!(revoked["posts"][0]["media_url"].is_null());
    sqlx::query("INSERT INTO user_blocks(user_id,blocked_user_id) VALUES($1,$2)")
        .bind(buyer)
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        get(&app, &public_path, Some(&token)).await.1["posts"],
        json!([])
    );
    // Suspension must hide a direct permalink even if an old post row is still published.
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(get(&app, &public_path, None).await.1["posts"], json!([]));
    assert!(
        !get(&app, &public_path, None).await.1["creators"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == creator.to_string())
    );
    tokio::fs::remove_dir_all(&s.media_root).await.unwrap();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn permalink_never_exposes_draft_moderated_or_unapproved_creator_posts(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE handle='ellawild'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let id = post(&s.db, creator, "public").await;
    let path = format!("/v1/feed?post={id}");
    let (_, owner) = session(&s.db, "ellawild@myfans.test").await;
    for state in ["draft", "scheduled", "removed"] {
        sqlx::query("UPDATE posts SET state=$2 WHERE id=$1")
            .bind(id)
            .bind(state)
            .execute(&s.db)
            .await
            .unwrap();
        assert_eq!(get(&app, &path, Some(&owner)).await.1["posts"], json!([]));
        assert_eq!(get(&app, &path, None).await.1["posts"], json!([]));
    }
    sqlx::query("UPDATE posts SET state='published',moderation_blocked=true WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(get(&app, &path, None).await.1["posts"], json!([]));
    sqlx::query("UPDATE posts SET moderation_blocked=false WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE creators SET verification_status='rejected' WHERE id=$1")
        .bind(creator)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(get(&app, &path, None).await.1["posts"], json!([]));
    tokio::fs::remove_dir_all(&s.media_root).await.unwrap();
}
