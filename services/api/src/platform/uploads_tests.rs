#![allow(clippy::unwrap_used)]
use super::*;
use crate::platform::{MediaStore, PlatformState, RuntimeConfig, content};
use axum::body::to_bytes;
use tower::ServiceExt;
async fn setup(db: PgPool) -> (PlatformState, HeaderMap, HeaderMap, Uuid) {
    let root = std::env::temp_dir().join(format!("myfans-uploads-{}", Uuid::new_v4()));
    tokio::fs::create_dir(&root).await.unwrap();
    let owner:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,role,email_verified) VALUES('upload@test.invalid','Upload','creator',true) RETURNING id").fetch_one(&db).await.unwrap();
    let other:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,email_verified) VALUES('other-upload@test.invalid','Other',true) RETURNING id").fetch_one(&db).await.unwrap();
    let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status) VALUES($1,'upload',1000,'USD','approved') RETURNING id").bind(owner).fetch_one(&db).await.unwrap();
    let mut headers = Vec::new();
    for id in [owner, other] {
        let token = random_token();
        sqlx::query("INSERT INTO sessions(user_id,token_digest,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(id).bind(digest(&token)).execute(&db).await.unwrap();
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        h.insert(
            header::CONTENT_TYPE,
            "application/octet-stream".parse().unwrap(),
        );
        h.insert("upload-offset", "0".parse().unwrap());
        headers.push(h);
    }
    let s = PlatformState {
        db,
        media_root: root.clone(),
        key: Arc::new([11; 32]),
        dummy_hash: Arc::new("invalid".into()),
        mail: Default::default(),
        storage: MediaStore::local(root),
        scanner: Default::default(),
        providers: Default::default(),
        runtime: Arc::new(RuntimeConfig::default()),
    };
    (s, headers.remove(0), headers.remove(0), creator)
}
async fn begin(s: &PlatformState, h: &HeaderMap, size: i64) -> Uuid {
    let v = start(
        State(s.clone()),
        h.clone(),
        Json(Start {
            size_bytes: size,
            content_type: "video/mp4".into(),
        }),
    )
    .await
    .unwrap()
    .0;
    Uuid::parse_str(v["upload"]["id"].as_str().unwrap()).unwrap()
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn resumable_duplicate_chunks_and_completion_are_serialized_and_owned(db: PgPool) {
    let (s, h, other, _) = setup(db).await;
    let id = begin(&s, &h, CHUNK as i64 + 3).await;
    assert_eq!(
        get_upload(State(s.clone()), other.clone(), Path(id))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        chunk(
            State(s.clone()),
            other,
            Path(id),
            Bytes::from_static(b"bad")
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    let bytes = Bytes::from(vec![42; CHUNK]);
    let (a, b) = tokio::join!(
        chunk(State(s.clone()), h.clone(), Path(id), bytes.clone()),
        chunk(State(s.clone()), h.clone(), Path(id), bytes)
    );
    assert_eq!(a.unwrap().0["upload"]["offset"], CHUNK);
    assert_eq!(b.unwrap().0["upload"]["offset"], CHUNK);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM media_upload_chunks WHERE upload_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        chunk(
            State(s.clone()),
            h.clone(),
            Path(id),
            Bytes::from(vec![43; CHUNK])
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    let mut last = h.clone();
    last.insert("upload-offset", CHUNK.to_string().parse().unwrap());
    let _ = chunk(State(s.clone()), last, Path(id), Bytes::from_static(b"end"))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        complete(State(s.clone()), h.clone(), Path(id)),
        complete(State(s.clone()), h.clone(), Path(id))
    );
    assert_eq!(a.unwrap().0["media"]["id"], id.to_string());
    assert_eq!(b.unwrap().0["media"]["id"], id.to_string());
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM media_assets WHERE id=$1),(SELECT count(*) FROM jobs WHERE kind='assemble_upload' AND payload->>'upload_id'=$1::text)").bind(id).fetch_one(&s.db).await.unwrap();
    assert_eq!(counts, (1, 1));
    assemble(&s, id).await.unwrap();
    assemble(&s, id).await.unwrap();
    let raw = s
        .storage
        .read(&format!("{id}.upload"), CHUNK + 3)
        .await
        .unwrap();
    assert_eq!(&raw[..CHUNK], &vec![42; CHUNK]);
    assert_eq!(&raw[CHUNK..], b"end");
    assert!(s.storage.read(&chunk_key(id, 0), CHUNK).await.is_err());
    assert!(cancel(State(s.clone()), h, Path(id)).await.is_err());
    tokio::fs::remove_dir_all(s.media_root).await.unwrap();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn resumable_disabled_owner_is_cleaned_and_completion_replays_current_state(db: PgPool) {
    let (s, h, _, creator) = setup(db).await;
    let first = begin(&s, &h, 3).await;
    let _ = chunk(
        State(s.clone()),
        h.clone(),
        Path(first),
        Bytes::from_static(b"abc"),
    )
    .await
    .unwrap();
    let _ = complete(State(s.clone()), h.clone(), Path(first))
        .await
        .unwrap();
    assemble(&s, first).await.unwrap();
    for state in ["quarantine", "approved", "rejected"] {
        sqlx::query("UPDATE media_assets SET state=$2 WHERE id=$1")
            .bind(first)
            .bind(state)
            .execute(&s.db)
            .await
            .unwrap();
        let replay = complete(State(s.clone()), h.clone(), Path(first))
            .await
            .unwrap()
            .0;
        assert_eq!(replay["media"]["state"], state);
    }
    let second = begin(&s, &h, 3).await;
    let _ = chunk(
        State(s.clone()),
        h.clone(),
        Path(second),
        Bytes::from_static(b"abc"),
    )
    .await
    .unwrap();
    let _ = complete(State(s.clone()), h.clone(), Path(second))
        .await
        .unwrap();
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=(SELECT user_id FROM creators WHERE id=$1)").bind(creator).execute(&s.db).await.unwrap();
    assemble(&s, second).await.unwrap();
    assert!(
        s.storage
            .read(&format!("{second}.upload"), CHUNK)
            .await
            .is_err()
    );
    assert!(s.storage.read(&chunk_key(second, 0), CHUNK).await.is_err());
    let terminal:(String,bool,String)=sqlx::query_as("SELECT u.state,u.cleaned_at IS NOT NULL,a.state FROM media_uploads u JOIN media_assets a ON a.id=u.id WHERE u.id=$1").bind(second).fetch_one(&s.db).await.unwrap();
    assert_eq!(terminal, ("cancelled".into(), true, "rejected".into()));
    assert!(complete(State(s.clone()), h, Path(second)).await.is_err());
    tokio::fs::remove_dir_all(s.media_root).await.unwrap();
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn resumable_cancel_expiry_and_integrity_failure_never_publish_bytes(db: PgPool) {
    let (s, h, _, _) = setup(db).await;
    let cancelled = begin(&s, &h, 3).await;
    let _ = chunk(
        State(s.clone()),
        h.clone(),
        Path(cancelled),
        Bytes::from_static(b"abc"),
    )
    .await
    .unwrap();
    assert_eq!(
        cancel(State(s.clone()), h.clone(), Path(cancelled))
            .await
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert!(
        complete(State(s.clone()), h.clone(), Path(cancelled))
            .await
            .is_err()
    );
    assert!(
        chunk(
            State(s.clone()),
            h.clone(),
            Path(cancelled),
            Bytes::from_static(b"abc")
        )
        .await
        .is_err()
    );
    cleanup_upload(&s, cancelled).await.unwrap();
    assert!(
        s.storage
            .read(&chunk_key(cancelled, 0), CHUNK)
            .await
            .is_err()
    );
    let expired = begin(&s, &h, 3).await;
    sqlx::query("UPDATE media_uploads SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(expired)
        .execute(&s.db)
        .await
        .unwrap();
    assert!(
        chunk(
            State(s.clone()),
            h.clone(),
            Path(expired),
            Bytes::from_static(b"abc")
        )
        .await
        .is_err()
    );
    assert!(
        complete(State(s.clone()), h.clone(), Path(expired))
            .await
            .is_err()
    );
    let corrupt = begin(&s, &h, 3).await;
    let _ = chunk(
        State(s.clone()),
        h.clone(),
        Path(corrupt),
        Bytes::from_static(b"abc"),
    )
    .await
    .unwrap();
    let _ = complete(State(s.clone()), h, Path(corrupt)).await.unwrap();
    s.storage
        .put(
            &chunk_key(corrupt, 0),
            b"bad".to_vec(),
            "application/octet-stream",
        )
        .await
        .unwrap();
    assert!(assemble(&s, corrupt).await.is_err());
    assert!(
        s.storage
            .read(&format!("{corrupt}.upload"), CHUNK)
            .await
            .is_err()
    );
    let state: String = sqlx::query_scalar("SELECT state FROM media_assets WHERE id=$1")
        .bind(corrupt)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_ne!(state, "approved");
    tokio::fs::remove_dir_all(s.media_root).await.unwrap();
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn hls_segments_recheck_entitlements_ranges_and_moderation_on_every_request(db: PgPool) {
    let (s, owner, buyer, creator) = setup(db).await;
    let user: Uuid =
        sqlx::query_scalar("SELECT id FROM app_users WHERE email='other-upload@test.invalid'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let asset = Uuid::new_v4();
    let post = Uuid::new_v4();
    sqlx::query("INSERT INTO media_assets(id,creator_id,storage_key,state,size_bytes,content_type,hls_ready) VALUES($1,$2,$3,'approved',10,'video/mp4',true)").bind(asset).bind(creator).bind(format!("{asset}.mp4")).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO posts(id,creator_id,caption,state,visibility,purchase_price_minor,currency,published_at) VALUES($1,$2,'Private stream','published','purchase',1000,'USD',now())").bind(post).bind(creator).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO post_media(post_id,asset_id,creator_id,position) VALUES($1,$2,$3,0)")
        .bind(post)
        .bind(asset)
        .bind(creator)
        .execute(&s.db)
        .await
        .unwrap();
    let file = format!("{asset}.720p-00000.ts");
    s.storage
        .put(&file, b"0123456789".to_vec(), "video/mp2t")
        .await
        .unwrap();
    assert_eq!(
        content::hls(
            State(s.clone()),
            HeaderMap::new(),
            Path((asset, file.clone()))
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        content::hls(State(s.clone()), buyer.clone(), Path((asset, file.clone())))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    let entitlement: Uuid = sqlx::query_scalar(
        "INSERT INTO entitlements(user_id,post_id,source) VALUES($1,$2,'purchase') RETURNING id",
    )
    .bind(user)
    .bind(post)
    .fetch_one(&s.db)
    .await
    .unwrap();
    let mut headers = buyer.clone();
    headers.insert(header::RANGE, "bytes=2-5".parse().unwrap());
    let response = content::hls(State(s.clone()), headers, Path((asset, file.clone())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(
        &to_bytes(response.into_body(), 100).await.unwrap()[..],
        b"2345"
    );
    let mut suffix = buyer.clone();
    suffix.insert(header::RANGE, "bytes=-3".parse().unwrap());
    let response = content::hls(State(s.clone()), suffix, Path((asset, file.clone())))
        .await
        .unwrap();
    assert_eq!(
        &to_bytes(response.into_body(), 100).await.unwrap()[..],
        b"789"
    );
    let mut invalid = buyer.clone();
    invalid.insert(header::RANGE, "bytes=100-200".parse().unwrap());
    assert_eq!(
        content::hls(State(s.clone()), invalid, Path((asset, file.clone())))
            .await
            .unwrap_err()
            .0,
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    let route = format!("/v1/media/{asset}/hls/{file}");
    let request = axum::http::Request::builder()
        .uri(route)
        .header(header::AUTHORIZATION, buyer[header::AUTHORIZATION].clone())
        .body(axum::body::Body::empty())
        .unwrap();
    let response = crate::platform::router(s.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    sqlx::query("UPDATE entitlements SET revoked_at=now() WHERE id=$1")
        .bind(entitlement)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        content::hls(State(s.clone()), buyer.clone(), Path((asset, file.clone())))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE entitlements SET revoked_at=NULL WHERE id=$1")
        .bind(entitlement)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE posts SET moderation_blocked=true WHERE id=$1")
        .bind(post)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        content::hls(State(s.clone()), buyer, Path((asset, file.clone())))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        content::hls(
            State(s.clone()),
            owner,
            Path((asset, format!("{asset}.720p-00000.ts/../../password")))
        )
        .await
        .is_err()
    );
    tokio::fs::remove_dir_all(s.media_root).await.unwrap();
}
