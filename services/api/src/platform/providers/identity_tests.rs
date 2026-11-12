#![allow(clippy::unwrap_used)]
use super::*;
use axum::{body::to_bytes, extract::Request, routing::any};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Fixture {
    applicant: Arc<Mutex<Value>>,
    steps: Arc<Mutex<Value>>,
}
async fn fixture(State(f): State<Fixture>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    let h = request.headers().clone();
    let bytes = to_bytes(request.into_body(), 16384).await.unwrap();
    let ts = h.get("x-app-access-ts").unwrap().to_str().unwrap();
    let mut message = format!("{ts}{method}{path}").into_bytes();
    message.extend_from_slice(&bytes);
    assert_eq!(h.get("x-app-token").unwrap(), "fixture_app_token");
    assert!(verify_signature(
        b"fixture_api_secret",
        &message,
        h.get("x-app-access-sig").unwrap().to_str().unwrap()
    ));
    assert!(h.get("cookie").is_none());
    if path.ends_with("/requiredIdDocsStatus") {
        Json(f.steps.lock().await.clone()).into_response()
    } else {
        Json(f.applicant.lock().await.clone()).into_response()
    }
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn identity_uses_current_verified_document_and_revokes_previous_approval(db: PgPool) {
    let owner:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('identity@test.invalid','Creator') RETURNING id").fetch_one(&db).await.unwrap();
    let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'identity',1000,'USD','pending') RETURNING id").bind(owner).fetch_one(&db).await.unwrap();
    sqlx::query("INSERT INTO identity_checks(creator_id) VALUES($1)")
        .bind(creator)
        .execute(&db)
        .await
        .unwrap();
    let v = json!({"id":"5bb8cca10a975a0000000000","externalUserId":creator.to_string(),"clientId":"fixture-client","deleted":false,"review":{"reviewStatus":"completed","levelName":"creator-18","reviewResult":{"reviewAnswer":"GREEN"}},"info":{"dob":"1980-01-01","idDocs":[{"idDocType":"PASSPORT","country":"USA","dob":"2012-01-01"}]},"fixedInfo":{"dob":"1980-01-01"}});
    let f = Fixture {
        applicant: Arc::new(Mutex::new(v)),
        steps: Arc::new(Mutex::new(
            json!({"IDENTITY":{"idDocType":"PASSPORT","country":"USA","reviewResult":{"reviewAnswer":"GREEN"}},"SELFIE":{"reviewResult":{"reviewAnswer":"GREEN"}}}),
        )),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(any(fixture)).with_state(f.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let p = Sumsub {
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
        token: "fixture_app_token".into(),
        secret: "fixture_api_secret".into(),
        webhook_secret: "fixture_webhook_secret".into(),
        level: "creator-18".into(),
        sandbox: true,
        client_id: "fixture-client".into(),
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
            stripe: None,
            identity: Some(Arc::new(p)),
        },
    };
    let payload = json!({"creator_id":creator});
    sync(&s, &payload).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM identity_checks")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(status, "rejected");
    f.applicant.lock().await["info"]["idDocs"][0]["dob"] = json!("1990-01-01");
    sync(&s, &payload).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM identity_checks")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(status, "verified");
    sqlx::query("UPDATE creators SET verification_status='approved',verified_at=now() WHERE id=$1")
        .bind(creator)
        .execute(&s.db)
        .await
        .unwrap();
    // A delayed old GREEN notification is only a wake-up signal. Current provider state wins.
    f.applicant.lock().await["review"]["reviewResult"]["reviewAnswer"] = json!("RED");
    let body=json!({"externalUserId":creator.to_string(),"sandboxMode":true,"type":"applicantReviewed","reviewResult":{"reviewAnswer":"GREEN"}}).to_string().into_bytes();
    let mut h = HeaderMap::new();
    h.insert("x-payload-digest-alg", "HMAC_SHA256_HEX".parse().unwrap());
    h.insert(
        "x-payload-digest",
        signature(b"fixture_webhook_secret", &body)
            .unwrap()
            .parse()
            .unwrap(),
    );
    for _ in 0..2 {
        webhook(State(s.clone()), h.clone(), body.clone().into())
            .await
            .unwrap();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='identity_sync'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    sync(&s, &payload).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT verification_status FROM creators")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(status, "pending");
    let audit: String = sqlx::query_scalar("SELECT jsonb_agg(metadata)::text FROM audit_events")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert!(!audit.contains("1990"));
    assert!(!audit.contains("PASSPORT"));
    task.abort();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn lifecycle_identity_never_returns_hosted_capability_after_freeze(db: PgPool) {
    #[derive(Clone, Default)]
    struct Gate {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    async fn hosted(State(g): State<Gate>, request: Request) -> Response {
        let path = request.uri().path().to_string();
        let method = request.method().to_string();
        let h = request.headers().clone();
        let bytes = to_bytes(request.into_body(), 16384).await.unwrap();
        let ts = h.get("x-app-access-ts").unwrap().to_str().unwrap();
        let mut message = format!("{ts}{method}{path}").into_bytes();
        message.extend_from_slice(&bytes);
        assert!(verify_signature(
            b"fixture_api_secret",
            &message,
            h.get("x-app-access-sig").unwrap().to_str().unwrap()
        ));
        assert_eq!(path, "/resources/sdkIntegrations/levels/-/websdkLink");
        g.entered.notify_one();
        g.release.notified().await;
        Json(json!({"url":"https://inquiry.sumsub.com/id/fixture"})).into_response()
    }
    let (mut s, owner, creator, token) = crate::platform::lifecycle_tests::fixture(db).await;
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
    let gate = Gate::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(any(hosted)).with_state(gate.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    s.providers.identity = Some(Arc::new(Sumsub {
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap(),
        token: "fixture_app_token".into(),
        secret: "fixture_api_secret".into(),
        webhook_secret: "fixture_webhook_secret".into(),
        level: "creator-18".into(),
        sandbox: true,
        client_id: "fixture-client".into(),
        endpoint: Some(endpoint),
    }));
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let state = s.clone();
    let task = tokio::spawn(async move { start(State(state), h).await });
    tokio::time::timeout(Duration::from_secs(2), gate.entered.notified())
        .await
        .unwrap();
    let stored: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM identity_checks WHERE creator_id=$1)")
            .bind(creator)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert!(
        stored,
        "provider correlation must be durable before issuing the request"
    );
    let mut tx = s.db.begin().await.unwrap();
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
        .bind(owner)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(owner)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap_err().0, StatusCode::UNAUTHORIZED);
    server.abort();
}
