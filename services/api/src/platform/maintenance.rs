use super::*;
pub async fn rotate_key(db: &PgPool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .ok_or("Key rotation requires both key file paths")
    };
    let old = config::encryption_key(&path("MFA_ENCRYPTION_KEY_FILE")?).await?;
    let new = config::encryption_key(&path("NEXT_MFA_ENCRYPTION_KEY_FILE")?).await?;
    let id = std::env::var("KEY_ROTATION_ID")
        .map_err(|_| "KEY_ROTATION_ID is required")?
        .parse::<Uuid>()
        .map_err(|_| "Invalid KEY_ROTATION_ID")?;
    rotate(db, old, new, id).await.map_err(|e| e.1.into())
}
async fn rotate(db: &PgPool, old: [u8; 32], new: [u8; 32], id: Uuid) -> Result<()> {
    if old == new {
        return Err(bad("Key rotation requires different key material."));
    }
    let fingerprint = |key: &[u8; 32]| {
        Sha256::digest(key)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let old_fingerprint = fingerprint(&old);
    let new_fingerprint = fingerprint(&new);
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(672913055)")
        .execute(&mut *tx)
        .await?;
    let prior: Option<(String, String)> = sqlx::query_as(
        "SELECT old_fingerprint,new_fingerprint FROM encryption_key_rotations WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(prior) = prior {
        if prior != (old_fingerprint, new_fingerprint) {
            return Err(bad("Key rotation ID is already bound to different keys."));
        }
        return Ok(());
    }
    let clients:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename IN ('myfans_app','myfans_runtime') AND pid<>pg_backend_pid()").fetch_one(&mut *tx).await?;
    if clients != 0 {
        return Err(bad(
            "Stop every application and worker before key rotation.",
        ));
    }
    sqlx::query("LOCK TABLE app_users,email_deliveries IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    let state = |key| PlatformState {
        db: db.clone(),
        runtime: Default::default(),
        media_root: PathBuf::new(),
        key: Arc::new(key),
        dummy_hash: Arc::new(String::new()),
        mail: Default::default(),
        storage: MediaStore::local(PathBuf::new()),
        scanner: Default::default(),
        providers: Default::default(),
    };
    let old_state = state(old);
    let new_state = state(new);
    let mut cursor = None;
    let mut mfa_count = 0_i64;
    let mut mail_count = 0_i64;
    loop {
        let rows:Vec<(Uuid,Vec<u8>)>=sqlx::query_as("SELECT id,mfa_secret FROM app_users WHERE mfa_secret IS NOT NULL AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 100").bind(cursor).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            break;
        }
        for (id, bytes) in rows {
            let plain = auth::decrypt(&old_state, &bytes)?;
            let encrypted = auth::encrypt(&new_state, &plain)?;
            sqlx::query("UPDATE app_users SET mfa_secret=$2 WHERE id=$1")
                .bind(id)
                .bind(encrypted)
                .execute(&mut *tx)
                .await?;
            cursor = Some(id);
            mfa_count += 1;
        }
    }
    cursor = None;
    loop {
        let rows:Vec<(Uuid,String,Vec<u8>)>=sqlx::query_as("SELECT id,recipient,encrypted_body FROM email_deliveries WHERE encrypted_body IS NOT NULL AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 100").bind(cursor).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            break;
        }
        for (id, recipient, bytes) in rows {
            let plain = mail::open(&old_state, id, &recipient, &bytes)?;
            let encrypted = mail::seal(&new_state, id, &recipient, plain.as_bytes())?;
            sqlx::query("UPDATE email_deliveries SET encrypted_body=$2 WHERE id=$1")
                .bind(id)
                .bind(encrypted)
                .execute(&mut *tx)
                .await?;
            cursor = Some(id);
            mail_count += 1;
        }
    }
    sqlx::query("INSERT INTO encryption_key_rotations(id,old_fingerprint,new_fingerprint,mfa_count,mail_count) VALUES($1,$2,$3,$4,$5)").bind(id).bind(old_fingerprint).bind(new_fingerprint).bind(mfa_count).bind(mail_count).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

// Lock every account touched by a review before its resource rows. Financial
// reversals and erasure reviews may concern disabled users, so only the actor
// must be active; their current session/role/MFA is checked after these locks.
pub(super) async fn authorize_review(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    h: &HeaderMap,
    actor: Uuid,
    kind: &str,
    id: Uuid,
) -> Result<()> {
    let query = match kind {
        "creator" => "SELECT user_id FROM creators WHERE id=$1",
        "media" => {
            "SELECT c.user_id FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE m.id=$1"
        }
        "post" => {
            "SELECT c.user_id FROM posts p JOIN creators c ON c.id=p.creator_id WHERE p.id=$1"
        }
        "user" => "SELECT id FROM app_users WHERE id=$1",
        "report" => {
            "SELECT r.user_id FROM reports r WHERE r.id=$1 UNION SELECT c.user_id FROM reports r JOIN posts p ON p.id=r.post_id JOIN creators c ON c.id=p.creator_id WHERE r.id=$1"
        }
        "appeal" => "SELECT user_id FROM moderation_appeals WHERE id=$1",
        "payout" => "SELECT user_id FROM payouts WHERE id=$1",
        "privacy" => "SELECT user_id FROM privacy_requests WHERE id=$1",
        _ => return Err(bad("Unknown review type.")),
    };
    let mut accounts: Vec<Uuid> = sqlx::query_scalar(query)
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    accounts.push(actor);
    sqlx::query("SELECT id FROM app_users WHERE id=ANY($1) ORDER BY id FOR UPDATE")
        .bind(accounts)
        .execute(&mut **tx)
        .await?;
    authorize_tx(tx, h, actor, Assurance::Admin).await?;
    Ok(())
}

pub async fn evidence(
    s: &PlatformState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Uuid,
    kind: &str,
    id: Uuid,
    decision: &str,
    reason: &str,
) -> Result<()> {
    // Capture the reviewed version inside the same transaction as the decision.
    // Never copy document/identity-provider payloads, email, payment data or media bytes.
    let snapshot:Option<Value>=match kind {
        "creator"=>sqlx::query_scalar("SELECT jsonb_build_object('display_name',display_name,'handle',handle,'bio',bio,'category',category,'verification_status',verification_status) FROM creators WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?,
        "media"=>sqlx::query_scalar("SELECT jsonb_build_object('storage_key',storage_key,'preview_key',preview_key,'size_bytes',size_bytes,'content_type',content_type,'state',state,'captions',captions_vtt) FROM media_assets WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?,
        "report"=>sqlx::query_scalar("SELECT jsonb_build_object('post_id',p.id,'caption',p.caption,'state',p.state,'visibility',p.visibility,'reason',r.reason,'media',(SELECT jsonb_agg(jsonb_build_object('id',m.id,'key',m.storage_key,'state',m.state)) FROM post_media pm JOIN media_assets m ON m.id=pm.asset_id WHERE pm.post_id=p.id)) FROM reports r JOIN posts p ON p.id=r.post_id WHERE r.id=$1 FOR UPDATE OF r,p").bind(id).fetch_optional(&mut **tx).await?,
        "appeal"=>sqlx::query_scalar("SELECT jsonb_build_object('post_id',p.id,'caption',p.caption,'state',p.state,'appeal_reason',a.reason,'status',a.status) FROM moderation_appeals a JOIN posts p ON p.id=a.post_id WHERE a.id=$1 FOR UPDATE OF a,p").bind(id).fetch_optional(&mut **tx).await?,
        "post"=>sqlx::query_scalar("SELECT jsonb_build_object('caption',caption,'state',state,'moderation_blocked',moderation_blocked) FROM posts WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?,
        "user"=>sqlx::query_scalar("SELECT jsonb_build_object('display_name',display_name,'role',role,'disabled_at',disabled_at) FROM app_users WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?,
        _=>return Ok(()),
    };
    let snapshot = snapshot.ok_or(Error(StatusCode::NOT_FOUND, "Review resource not found."))?;
    sqlx::query("INSERT INTO moderation_evidence(actor_id,resource_type,resource_id,decision,reason,policy_version,snapshot,snapshot_sha256) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(actor).bind(kind).bind(id).bind(decision).bind(reason).bind(s.runtime.policy.version("content")).bind(&snapshot).bind(digest(&snapshot.to_string())).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn rotation_is_atomic_authenticated_and_restartable(db: PgPool) {
        let old = PlatformState {
            db: db.clone(),
            runtime: Default::default(),
            media_root: PathBuf::new(),
            key: Arc::new([4; 32]),
            dummy_hash: Arc::new(String::new()),
            mail: Default::default(),
            storage: MediaStore::local(PathBuf::new()),
            scanner: Default::default(),
            providers: Default::default(),
        };
        let next = PlatformState {
            key: Arc::new([5; 32]),
            ..old.clone()
        };
        let user:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name,mfa_secret) VALUES('rotation@test.example','Rotation',$1) RETURNING id").bind(auth::encrypt(&old,b"mfa-test-secret").unwrap()).fetch_one(&db).await.unwrap();
        let delivery = Uuid::new_v4();
        sqlx::query("INSERT INTO email_deliveries(id,user_id,recipient,subject,body,transport,encrypted_body) VALUES($1,$2,'rotation@test.example','Test','Message queued for email delivery.','smtp',$3)").bind(delivery).bind(user).bind(mail::seal(&old,delivery,"rotation@test.example",b"mail-test-secret").unwrap()).execute(&db).await.unwrap();
        let id = Uuid::new_v4();
        assert!(rotate(&db, [9; 32], [5; 32], id).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM encryption_key_rotations")
                .fetch_one(&db)
                .await
                .unwrap(),
            0
        );
        for _ in 0..2 {
            rotate(&db, [4; 32], [5; 32], id).await.unwrap();
        }
        assert!(rotate(&db, [4; 32], [6; 32], id).await.is_err());
        let mfa: Vec<u8> = sqlx::query_scalar("SELECT mfa_secret FROM app_users WHERE id=$1")
            .bind(user)
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(auth::decrypt(&old, &mfa).is_err());
        assert_eq!(auth::decrypt(&next, &mfa).unwrap(), b"mfa-test-secret");
        let email: Vec<u8> =
            sqlx::query_scalar("SELECT encrypted_body FROM email_deliveries WHERE id=$1")
                .bind(delivery)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(
            mail::open(&next, delivery, "rotation@test.example", &email).unwrap(),
            "mail-test-secret"
        );
        let counts: (i64, i64) =
            sqlx::query_as("SELECT mfa_count,mail_count FROM encryption_key_rotations WHERE id=$1")
                .bind(id)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(counts, (1, 1));
    }
}
