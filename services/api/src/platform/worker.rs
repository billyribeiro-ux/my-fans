use super::video::process_video;
use super::*;
use sqlx::Row;

pub async fn run(s: PlatformState, mut stop: tokio::sync::watch::Receiver<bool>) {
    let mail_handle = tokio::spawn(super::mail::run(s.clone(), stop.clone()));
    let metrics_handle = tokio::spawn(telemetry::run(s.clone(), stop.clone()));
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            _ = interval.tick() => {},
        }
        if let Err(error) = tick(&s).await {
            tracing::error!(status = error.0.as_u16(), "worker_tick_failed")
        } else if let Some(path) = std::env::var_os("WORKER_HEARTBEAT_FILE")
            && tokio::fs::write(path, chrono::Utc::now().timestamp().to_string())
                .await
                .is_err()
        {
            tracing::error!("worker_heartbeat_write_failed");
        }
    }
    let _ = mail_handle.await;
    let _ = metrics_handle.await;
}
pub async fn tick(s: &PlatformState) -> Result<()> {
    uploads::expire(s).await?;
    privacy::tick(s).await?;
    sqlx::query("INSERT INTO jobs(kind,payload) SELECT 'process_media',jsonb_build_object('media_id',id) FROM media_assets WHERE state='processing' AND created_at<now()-interval '1 minute' AND NOT EXISTS(SELECT 1 FROM media_uploads mu WHERE mu.id=media_assets.id AND mu.state='assembling') AND NOT EXISTS(SELECT 1 FROM jobs WHERE kind='process_media' AND payload->>'media_id'=media_assets.id::text) LIMIT 10 ON CONFLICT DO NOTHING").execute(&s.db).await?;
    if s.providers.identity.is_some() {
        sqlx::query("WITH due AS (UPDATE identity_checks SET next_check_at=now()+interval '1 hour' WHERE creator_id IN (SELECT creator_id FROM identity_checks WHERE next_check_at<=now() ORDER BY next_check_at FOR UPDATE SKIP LOCKED LIMIT 5) RETURNING creator_id) INSERT INTO jobs(kind,payload) SELECT 'identity_sync',jsonb_build_object('creator_id',creator_id) FROM due WHERE NOT EXISTS(SELECT 1 FROM jobs WHERE kind='identity_sync' AND status IN ('pending','running') AND payload->>'creator_id'=due.creator_id::text)").execute(&s.db).await?;
    }
    if s.providers.stripe.is_some() {
        providers::stripe::finance::schedule(s).await?;
        sqlx::query("WITH due AS (UPDATE checkouts SET provider_recovery_at=now()+interval '30 minutes' WHERE id IN (SELECT id FROM checkouts WHERE provider='stripe' AND status='pending' AND created_at<now()-interval '25 minutes' AND provider_recovery_at<=now() ORDER BY provider_recovery_at,id FOR UPDATE SKIP LOCKED LIMIT 5) RETURNING id) INSERT INTO jobs(kind,payload) SELECT 'stripe_checkout_recovery',jsonb_build_object('checkout_id',id) FROM due ON CONFLICT DO NOTHING").execute(&s.db).await?;
        sqlx::query("INSERT INTO provider_reconciliation(account_id) SELECT account_id FROM provider_accounts ON CONFLICT DO NOTHING").execute(&s.db).await?;
        sqlx::query("WITH due AS (UPDATE provider_reconciliation SET next_at=now()+interval '30 minutes' WHERE account_id IN (SELECT account_id FROM provider_reconciliation WHERE next_at<=now() ORDER BY next_at,account_id FOR UPDATE SKIP LOCKED LIMIT 5) RETURNING account_id) INSERT INTO jobs(kind,payload) SELECT 'provider_reconcile',jsonb_build_object('account',account_id) FROM due ON CONFLICT DO NOTHING").execute(&s.db).await?;
    }
    sqlx::query("UPDATE jobs SET status=CASE WHEN attempts>=5 THEN 'failed' ELSE 'pending' END,locked_at=NULL,lease_id=NULL,last_error='Worker lease expired' WHERE status='running' AND locked_at<now()-interval '5 minutes'").execute(&s.db).await?;
    let job=sqlx::query("UPDATE jobs SET status='running',locked_at=now(),lease_id=gen_random_uuid(),attempts=attempts+1 WHERE id=(SELECT id FROM jobs WHERE status='pending' AND run_at<=now() ORDER BY run_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,kind,payload,attempts,lease_id").fetch_optional(&s.db).await?;
    if let Some(job) = job {
        let id: Uuid = job.get("id");
        let payload: Value = job.get("payload");
        let kind: String = job.get("kind");
        let lease: Uuid = job.get("lease_id");
        let heartbeat_db = s.db.clone();
        let (done, mut stop) = tokio::sync::watch::channel(false);
        let (lost, mut lease_lost) = tokio::sync::watch::channel(false);
        let heartbeat = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {biased; _=stop.changed()=>break,_=interval.tick()=>{}}
                match sqlx::query("UPDATE jobs SET locked_at=now() WHERE id=$1 AND lease_id=$2 AND status='running' AND locked_at>now()-interval '5 minutes'").bind(id).bind(lease).execute(&heartbeat_db).await {
                    Ok(result) if result.rows_affected()==1=>{
                        if let Some(path)=std::env::var_os("WORKER_HEARTBEAT_FILE") {let _=tokio::fs::write(path,chrono::Utc::now().timestamp().to_string()).await;}
                    },
                    _=>{tracing::error!(job_id=%id,"worker_lease_heartbeat_failed");let _=lost.send(true);break;}
                }
            }
        });
        let processing = async {
            match kind.as_str() {
                "cleanup_upload" => {
                    uploads::cleanup_upload(s, payload_id(&payload, "upload_id")?).await
                }
                "assemble_upload" => uploads::assemble(s, payload_id(&payload, "upload_id")?).await,
                "process_media" => process_media(s, &payload).await,
                "publish" => publish(s, &payload).await,
                "settle" => settle(s, &payload).await,
                "stripe_finance" => providers::stripe::finance::reconcile(s, &payload).await,
                "stripe_event" => providers::stripe::event(s, &payload).await,
                "stripe_checkout_recovery" => {
                    providers::stripe::recover_checkout(s, payload_id(&payload, "checkout_id")?)
                        .await
                }
                "identity_sync" => providers::identity::sync(s, &payload).await,
                "provider_reconcile" => providers::stripe::reconcile(s, &payload).await,
                _ => Err(bad("Unknown job type.")),
            }
        };
        let result = tokio::select! {biased; _=lease_lost.changed()=>Err(Error(StatusCode::SERVICE_UNAVAILABLE,"Job lease was lost; processing cancelled.")),result=processing=>result};
        let _ = done.send(true);
        let _ = heartbeat.await;
        match result {
            Ok(()) => {
                sqlx::query("UPDATE jobs SET status='done',locked_at=NULL,lease_id=NULL WHERE id=$1 AND lease_id=$2")
                    .bind(id).bind(lease)
                    .execute(&s.db)
                    .await?;
            }
            Err(error) => {
                let attempts: i32 = job.get("attempts");
                sqlx::query("UPDATE jobs SET status=$2,locked_at=NULL,lease_id=NULL,run_at=now()+make_interval(secs=>$3),last_error=$4 WHERE id=$1 AND lease_id=$5").bind(id).bind(if attempts>=5{"failed"}else{"pending"}).bind((2_i32.pow(attempts as u32)*5) as f64).bind(error.1).bind(lease).execute(&s.db).await?;
            }
        }
    }
    sqlx::query("UPDATE subscriptions SET status='expired' WHERE provider='sandbox' AND status IN ('active','past_due') AND current_period_end<=now()").execute(&s.db).await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at<=now()")
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM recovery_tokens WHERE expires_at<now()-interval '1 day'")
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM rate_limits WHERE window_start<now()-interval '1 hour'")
        .execute(&s.db)
        .await?;
    Ok(())
}
fn payload_id(p: &Value, key: &str) -> Result<Uuid> {
    p.get(key)
        .and_then(Value::as_str)
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(bad("Invalid job payload."))
}
async fn process_media(s: &PlatformState, p: &Value) -> Result<()> {
    let id = payload_id(p, "media_id")?;
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_assets m JOIN creators c ON c.id=m.creator_id JOIN app_users u ON u.id=c.user_id WHERE m.id=$1 AND u.disabled_at IS NULL)").bind(id).fetch_one(&s.db).await?;
    if !active {
        return Ok(());
    }
    let row = sqlx::query("SELECT storage_key,state,content_type FROM media_assets WHERE id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    if row.get::<String, _>("state") != "processing" {
        // A prior attempt may have committed its rendition then failed to delete
        // the raw object. Retrying must finish that cleanup, too.
        s.storage.delete(&format!("{id}.upload")).await?;
        return Ok(());
    }
    let source_key: String = row.get("storage_key");
    let bytes = s
        .storage
        .read(&source_key, storage::MAX_UPLOAD_BYTES)
        .await?;
    if let Err(error) = s.scanner.scan(&bytes).await {
        if error.0.is_server_error() {
            return Err(error);
        }
        sqlx::query("UPDATE media_assets SET state='rejected',rejection_reason=$2 WHERE id=$1 AND state='processing'").bind(id).bind(error.1).execute(&s.db).await?;
        s.storage.delete(&source_key).await?;
        return Ok(());
    }
    let scratch = workdir::WorkDir::create(&s.media_root)?;
    let work = scratch.path();
    let path = work.join("source.upload");
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|_| providers::unavailable())?;
    if row.get::<String, _>("content_type") == "video/mp4" {
        let result = process_video(s, id, &path).await;
        let _ = tokio::fs::remove_dir_all(&work).await;
        if result.is_ok() {
            s.storage.delete(&source_key).await?;
        }
        return result;
    }
    let rendered = media_processor::render_image(work, &path).await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    let (full, preview) = match rendered {
        Ok(value) => value,
        Err(error) => {
            if error.0.is_server_error() {
                return Err(error);
            }
            let reason = error.1;
            sqlx::query("UPDATE media_assets SET state='rejected',rejection_reason=$2 WHERE id=$1")
                .bind(id)
                .bind(reason)
                .execute(&s.db)
                .await?;
            s.storage.delete(&source_key).await?;
            return Ok(());
        }
    };
    let full_key = format!("{id}.jpg");
    let preview_key = format!("{id}.preview.jpg");
    let size = full.len() as i64;
    s.storage.put(&full_key, full, "image/jpeg").await?;
    s.storage.put(&preview_key, preview, "image/jpeg").await?;
    sqlx::query("UPDATE media_assets SET state='quarantine',storage_key=$2,preview_key=$3,content_type='image/jpeg',size_bytes=$4 WHERE id=$1 AND state='processing'").bind(id).bind(full_key).bind(preview_key).bind(size).execute(&s.db).await?;
    s.storage.delete(&source_key).await?;
    Ok(())
}
async fn publish(s: &PlatformState, p: &Value) -> Result<()> {
    let id = payload_id(p, "post_id")?;
    let mut tx = s.db.begin().await?;
    let owner: Uuid = sqlx::query_scalar(
        "SELECT c.user_id FROM posts p JOIN creators c ON c.id=p.creator_id WHERE p.id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let active: bool =
        sqlx::query_scalar("SELECT disabled_at IS NULL FROM app_users WHERE id=$1 FOR UPDATE")
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?;
    if !active {
        return Ok(());
    }
    let row=sqlx::query("SELECT p.state,c.user_id,c.verification_status,u.email_verified,p.moderation_blocked FROM posts p JOIN creators c ON c.id=p.creator_id JOIN app_users u ON u.id=c.user_id WHERE p.id=$1 FOR UPDATE OF p").bind(id).fetch_one(&mut *tx).await?;
    if row.get::<String, _>("state") != "scheduled" {
        return Ok(());
    }
    let unsafe_media:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM post_media pm JOIN media_assets m ON m.id=pm.asset_id WHERE pm.post_id=$1 AND (m.state<>'approved' OR (m.content_type='video/mp4' AND m.captions_vtt IS NULL)))").bind(id).fetch_one(&mut *tx).await?;
    let safe = !row.get::<bool, _>("moderation_blocked")
        && row.get::<String, _>("verification_status") == "approved"
        && row.get::<bool, _>("email_verified")
        && !unsafe_media;
    sqlx::query("UPDATE posts SET state=$2,published_at=CASE WHEN $3 THEN now() ELSE NULL END WHERE id=$1 AND scheduled_at<=now()").bind(id).bind(if safe{"published"}else{"draft"}).bind(safe).execute(&mut *tx).await?;
    let owner: Uuid = row.get("user_id");
    audit(
        &mut tx,
        owner,
        if safe {
            "post.scheduled.published"
        } else {
            "post.scheduled.blocked"
        },
        "post",
        id,
    )
    .await?;
    notify(
        &mut tx,
        owner,
        "Scheduled post",
        if safe {
            "Your scheduled post is now published."
        } else {
            "Your post returned to drafts because verification or media approval is incomplete."
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
async fn settle(s: &PlatformState, p: &Value) -> Result<()> {
    let id = payload_id(p, "checkout_id")?;
    let mut tx = s.db.begin().await?;
    super::billing::lock_checkout_accounts(&mut tx, id, None).await?;
    let row=sqlx::query("SELECT c.status,c.amount_minor,r.user_id FROM checkouts c JOIN creators r ON r.id=c.creator_id WHERE c.id=$1 FOR UPDATE OF c").bind(id).fetch_one(&mut *tx).await?;
    let done: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ledger_transactions WHERE external_key=$1)",
    )
    .bind(format!("settle:{id}"))
    .fetch_one(&mut *tx)
    .await?;
    if done || row.get::<String, _>("status") != "paid" {
        return Ok(());
    }
    let owner: Uuid = row.get("user_id");
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *tx)
        .await?;
    let amount: i64 = row.get("amount_minor");
    let net = amount - amount / 5;
    let pending = super::billing::account_id(&mut tx, Some(owner), "creator_pending").await?;
    let available = super::billing::account_id(&mut tx, Some(owner), "creator_available").await?;
    super::billing::post_ledger(
        &mut tx,
        &format!("settle:{id}"),
        &[(pending, -net), (available, net)],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn seed(s: &PlatformState) -> Result<()> {
    if s.runtime.production
        || s.providers.stripe.is_some()
        || s.providers.identity.is_some()
        || s.storage.is_remote()
    {
        return Err(bad(
            "Fixture seeding is restricted to isolated local sandbox providers.",
        ));
    }
    let password = super::auth::hash_password("DevelopmentOnly!2026".into()).await?;
    let mut tx = s.db.begin().await?;
    for (email, name, role) in [
        ("demo@myfans.test", "Alex Morgan", "fan"),
        ("admin@myfans.test", "Sandbox Administrator", "admin"),
    ] {
        sqlx::query("INSERT INTO app_users(email,display_name,password_hash,role,email_verified) VALUES($1,$2,$3,$4,true) ON CONFLICT(email) DO NOTHING").bind(email).bind(name).bind(&password).bind(role).execute(&mut *tx).await?;
    }
    for c in crate::seed::creators() {
        let email = format!("{}@myfans.test", c.handle);
        sqlx::query("INSERT INTO app_users(email,display_name,password_hash,role,email_verified,avatar_url) VALUES($1,$2,$3,'creator',true,$4) ON CONFLICT(email) DO NOTHING").bind(&email).bind(&c.display_name).bind(&password).bind(&c.avatar_url).execute(&mut *tx).await?;
        let user: Uuid = sqlx::query_scalar("SELECT id FROM app_users WHERE email=$1")
            .bind(email)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO creators(id,user_id,handle,display_name,bio,category,avatar_url,cover_url,subscription_price_minor,currency,verified_at,verification_status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'USD',now(),'approved') ON CONFLICT DO NOTHING").bind(Uuid::parse_str(&c.id).map_err(|_|bad("Invalid seed id"))?).bind(user).bind(c.handle).bind(c.display_name).bind(c.bio).bind(c.category).bind(c.avatar_url).bind(c.cover_url).bind(c.subscription_price_minor as i64).execute(&mut *tx).await?;
    }
    for (i, p) in crate::seed::posts().into_iter().enumerate() {
        let id = Uuid::from_u128(0x11111111111141118111111111111000 + i as u128);
        let visibility = match p.visibility {
            crate::Visibility::Public => "public",
            _ => "subscribers",
        };
        sqlx::query("INSERT INTO posts(id,creator_id,caption,state,visibility,published_at,sample_media_url,sample_preview_url) VALUES($1,$2,$3,'published',$4,$5,$6,$7) ON CONFLICT DO NOTHING").bind(id).bind(Uuid::parse_str(&p.creator_id).map_err(|_|bad("Invalid seed id"))?).bind(p.caption).bind(visibility).bind(p.created_at.parse::<chrono::DateTime<chrono::Utc>>().map_err(|_|bad("Invalid seed timestamp"))?).bind(p.media_url).bind(p.preview_url).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
