//! Account erasure is an auditable workflow, not a cascading ledger deletion.
//! Live-object removal and external-store completion are deliberately separate.
use super::*;
use axum::extract::{Path, Query};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    pub erasure_delay_days: u16,
    pub backup_days: u16,
    pub security_days: u16,
    pub financial_days: u16,
    pub policy_evidence_days: u16,
    pub moderation_evidence_days: u16,
}
impl RetentionPolicy {
    pub fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.erasure_delay_days > 365
            || !(35..=3650).contains(&self.backup_days)
            || [
                self.security_days,
                self.financial_days,
                self.policy_evidence_days,
                self.moderation_evidence_days,
            ]
            .iter()
            .any(|days| !(1..=36500).contains(days))
        {
            return Err(
                "Explicit bounded retention periods are required; backups must cover the 35-day RDS recovery window",
            );
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErasureRequest {
    current_password: String,
    mfa_code: Option<String>,
    confirmation: String,
}
pub async fn request(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(p): Json<ErasureRequest>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if u.role == "admin" {
        return Err(bad(
            "Administrators require an operator-managed offboarding process.",
        ));
    }
    if p.confirmation != "ERASE" || p.current_password.len() > 128 {
        return Err(bad("Confirm erasure and enter your current password."));
    }
    rate_limit(&s, &format!("privacy:{}", u.id), 3).await?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM app_users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !auth::password_valid(p.current_password, hash.clone()).await {
        return Err(forbidden());
    }
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if u.role == "admin" {
        return Err(forbidden());
    }
    let current: Option<(bool,)> = sqlx::query_as("SELECT mfa_enabled FROM app_users WHERE id=$1 AND password_hash=$2 AND disabled_at IS NULL FOR UPDATE")
        .bind(u.id).bind(hash).fetch_optional(&mut *tx).await?;
    let (mfa,) = current.ok_or_else(forbidden)?;
    if mfa {
        auth::check_mfa_tx(&s, &mut tx, u.id, p.mfa_code.as_deref().unwrap_or("")).await?;
    }
    let id = freeze(&s, &mut tx, u.id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"request":{"id":id,"status":"pending"},"message":"Erasure requested. Your account is disabled and sessions are revoked. Keep this reference for your privacy contact. Financial obligations, legal holds, provider records and backups require review before completion."}),
    ))
}
async fn freeze(
    s: &PlatformState,
    tx: &mut Transaction<'_, Postgres>,
    subject: Uuid,
) -> Result<Uuid> {
    let retention = &s.runtime.policy.retention;
    let id: Uuid = sqlx::query_scalar("INSERT INTO privacy_requests(user_id,privacy_version,retention,not_before) VALUES($1,$2,$3,now()+make_interval(days=>$4)) RETURNING id")
        .bind(subject).bind(s.runtime.policy.version("privacy")).bind(json!(retention)).bind(i32::from(retention.erasure_delay_days)).fetch_one(&mut **tx).await?;
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
        .bind(subject)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(subject)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM recovery_tokens WHERE user_id=$1")
        .bind(subject)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE creators SET verification_status='rejected',verified_at=NULL WHERE user_id=$1",
    )
    .bind(subject)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE creator_id IN (SELECT id FROM creators WHERE user_id=$1)").bind(subject).execute(&mut **tx).await?;
    sqlx::query("UPDATE media_uploads SET state='cancelled',expires_at=now() WHERE creator_id IN (SELECT id FROM creators WHERE user_id=$1) AND state='uploading'").bind(subject).execute(&mut **tx).await?;
    sqlx::query("UPDATE jobs SET status='done',locked_at=NULL,lease_id=NULL,last_error='Account erasure requested' WHERE status='pending' AND ((kind='process_media' AND payload->>'media_id' IN(SELECT m.id::text FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$1)) OR (kind='assemble_upload' AND payload->>'upload_id' IN(SELECT m.id::text FROM media_uploads m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$1)) OR (kind='publish' AND payload->>'post_id' IN(SELECT p.id::text FROM posts p JOIN creators c ON c.id=p.creator_id WHERE c.user_id=$1)))").bind(subject).execute(&mut **tx).await?;
    sqlx::query("UPDATE subscriptions SET cancel_at_period_end=true WHERE provider='sandbox' AND user_id=$1 AND status='active'").bind(subject).execute(&mut **tx).await?;
    action(
        tx,
        id,
        subject,
        "requested",
        "Authenticated account erasure request",
        json!({}),
    )
    .await?;
    audit(tx, subject, "privacy.requested", "privacy_request", id).await?;
    Ok(id)
}
pub async fn status(State(s): State<PlatformState>, h: HeaderMap) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let value: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(x) FROM (SELECT id,status,created_at,not_before,legal_hold,application_erased_at,completed_at FROM privacy_requests WHERE user_id=$1)x")
        .bind(u.id).fetch_optional(&s.db).await?;
    Ok(Json(json!({"request":value})))
}
pub async fn list(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    admin(&s, &h).await?;
    q.validate(&["cursor"])?;
    let (requests, next_cursor) = pagination::read(&s.db,
        "SELECT r.id,r.user_id,u.display_name,r.status,r.created_at,r.not_before,r.legal_hold,r.retention,r.application_erased_at,r.versions_erased_at,r.completed_at,privacy_has_obligations(r.user_id) AS has_obligations,(SELECT count(*) FROM privacy_objects o WHERE o.request_id=r.id AND o.removed_at IS NULL) AS objects_pending,(SELECT jsonb_build_object('action',a.action,'reason',a.reason,'evidence',a.evidence,'created_at',a.created_at) FROM privacy_actions a WHERE a.request_id=r.id ORDER BY a.created_at DESC,a.id DESC LIMIT 1) AS latest_action FROM privacy_requests r JOIN app_users u ON u.id=r.user_id",
        None, q.cursor("cursor")?, "created_at", true).await?;
    Ok(Json(json!({"requests":requests,"next_cursor":next_cursor})))
}
fn reason(value: &str) -> Result<&str> {
    let value = value.trim();
    if !(5..=500).contains(&value.chars().count()) || value.chars().any(char::is_control) {
        return Err(bad(
            "Provide a review reason of 5 to 500 characters without private documents.",
        ));
    }
    Ok(value)
}
async fn action(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
    actor: Uuid,
    kind: &str,
    why: &str,
    evidence: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO privacy_actions(request_id,actor_id,action,reason,evidence) VALUES($1,$2,$3,$4,$5)")
        .bind(request).bind(actor).bind(kind).bind(reason(why)?).bind(evidence).execute(&mut **tx).await?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    decision: String,
    reason: String,
}
pub async fn review(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<Review>,
) -> Result<Json<Value>> {
    let actor = admin(&s, &h).await?;
    reason(&p.reason)?;
    if !["approve", "hold", "release"].contains(&p.decision.as_str()) {
        return Err(bad("Choose approve, hold or release."));
    }
    let subject: Uuid = sqlx::query_scalar("SELECT user_id FROM privacy_requests WHERE id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    if subject == actor.id {
        return Err(forbidden());
    }
    if p.decision == "approve" && s.providers.stripe.is_some() {
        let balances = providers::stripe::balances(&s, subject).await?;
        if balances != (0, 0) {
            return Err(bad(
                "Resolve the connected account balance before approval.",
            ));
        }
    }
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, actor.id, Assurance::Admin).await?;
    let row = sqlx::query("SELECT status,legal_hold,privacy_has_obligations(user_id) AS obligations FROM privacy_requests WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let status: String = row.get("status");
    if status == "completed" {
        return Err(bad("Completed erasure cannot be reopened."));
    }
    match p.decision.as_str() {
        "approve" => {
            if status != "pending"
                || row.get::<bool, _>("legal_hold")
                || row.get::<bool, _>("obligations")
            {
                return Err(bad(
                    "Resolve obligations and legal holds before approving this pending request.",
                ));
            }
            sqlx::query("UPDATE privacy_requests SET status='approved',approved_by=$2,approved_at=now(),next_at=now() WHERE id=$1").bind(id).bind(actor.id).execute(&mut *tx).await?;
        }
        "hold" => {
            sqlx::query("UPDATE privacy_requests SET legal_hold=true WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        _ => {
            sqlx::query("UPDATE privacy_requests SET legal_hold=false,next_at=now() WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    action(
        &mut tx,
        id,
        actor.id,
        if p.decision == "approve" {
            "approved"
        } else {
            &p.decision
        },
        &p.reason,
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"reviewed":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    reason: String,
    storage_evidence: String,
    backup_evidence: String,
    provider_evidence: String,
}
fn evidence_reference(s: &str) -> bool {
    (8..=200).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}
pub async fn complete(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<Completion>,
) -> Result<Json<Value>> {
    let actor = admin(&s, &h).await?;
    if [
        &p.storage_evidence,
        &p.backup_evidence,
        &p.provider_evidence,
    ]
    .iter()
    .any(|v| !evidence_reference(v))
    {
        return Err(bad(
            "Record bounded evidence references for historical objects, backups and provider processing.",
        ));
    }
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, actor.id, Assurance::Admin).await?;
    let subject: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM privacy_requests WHERE id=$1 AND status='application_erased' AND NOT legal_hold AND versions_erased_at IS NOT NULL AND application_erased_at+make_interval(days=>(retention->>'backup_days')::integer)<=now() AND NOT privacy_has_obligations(user_id) FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    if subject.is_none() || subject == Some(actor.id) {
        return Err(bad(
            "Application and historical-object erasure must finish, the backup retention window must pass, and holds must be resolved first.",
        ));
    }
    action(&mut tx,id,actor.id,"completed",&p.reason,json!({"storage":p.storage_evidence,"backups":p.backup_evidence,"providers":p.provider_evidence})).await?;
    sqlx::query("UPDATE privacy_requests SET status='completed',completed_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"status":"completed","basis":"Operator reviewed the recorded external-store evidence; restricted statutory records follow the captured retention schedule."}),
    ))
}

/// Two remote deletes per tick bound work to the storage client's operation deadline.
/// A request row lock serializes holds, enumeration, deletion and completion.
pub async fn tick(s: &PlatformState) -> Result<()> {
    cancel_recurring(s).await?;
    let mut tx = s.db.begin().await?;
    let row=sqlx::query("SELECT id,user_id,asset_cursor,objects_enumerated FROM privacy_requests WHERE status IN ('approved','purging') AND NOT legal_hold AND not_before<=now() AND next_at<=now() ORDER BY next_at,id FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        return Ok(());
    };
    let id: Uuid = row.get("id");
    let subject: Uuid = row.get("user_id");
    // Erasure and interactive mutations take account locks before content rows.
    // Privacy administrators lock only their own account before this request;
    // they do not hold the subject account while waiting for the request lock.
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(subject)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE privacy_requests SET next_at=now()+interval '30 seconds' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if sqlx::query_scalar::<_, bool>("SELECT privacy_has_obligations($1)")
        .bind(subject)
        .fetch_one(&mut *tx)
        .await?
    {
        tx.commit().await?;
        return Ok(());
    }
    sqlx::query("UPDATE privacy_requests SET status='purging' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if !row.get::<bool, _>("objects_enumerated") {
        enumerate(&mut tx, id, subject, row.get("asset_cursor")).await?;
        tx.commit().await?;
        return Ok(());
    }
    let keys:Vec<String>=sqlx::query_scalar("SELECT storage_key FROM privacy_objects WHERE request_id=$1 AND removed_at IS NULL AND next_at<=now() ORDER BY storage_key LIMIT 2 FOR UPDATE").bind(id).fetch_all(&mut *tx).await?;
    for key in keys {
        if s.storage.delete(&key).await.is_ok() {
            sqlx::query("UPDATE privacy_objects SET removed_at=now(),attempts=attempts+1 WHERE request_id=$1 AND storage_key=$2").bind(id).bind(key).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE privacy_objects SET attempts=attempts+1,next_at=now()+make_interval(secs=>LEAST(3600,30*(attempts+1))) WHERE request_id=$1 AND storage_key=$2").bind(id).bind(key).execute(&mut *tx).await?;
            tracing::error!(request_id=%id,"privacy_object_removal_failed");
        }
    }
    let remaining: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM privacy_objects WHERE request_id=$1 AND removed_at IS NULL)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if !remaining && scrub_batch(&mut tx, subject).await? {
        if s.providers.stripe.is_some() && providers::stripe::balances(s, subject).await? != (0, 0)
        {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE app_users SET email='erased-'||id::text||'@invalid.example',display_name='Deleted account',avatar_url='',password_hash=NULL,email_verified=false,mfa_secret=NULL,mfa_enabled=false,mfa_recovery_required=false,mfa_replacement_pending=false,mfa_last_step=0,erased_at=now() WHERE id=$1 AND disabled_at IS NOT NULL").bind(subject).execute(&mut *tx).await?;
        sqlx::query("UPDATE creators SET handle='erased_'||left(replace(id::text,'-',''),23),display_name='Deleted creator',bio='',avatar_url='',cover_url='',verification_status='rejected',verified_at=NULL,review_message=NULL,reviewed_at=NULL WHERE user_id=$1").bind(subject).execute(&mut *tx).await?;
        sqlx::query("UPDATE privacy_requests SET status='application_erased',application_erased_at=now(),versions_erased_at=CASE WHEN $2 THEN now() ELSE versions_erased_at END WHERE id=$1").bind(id).bind(!s.storage.is_remote()).execute(&mut *tx).await?;
        audit(
            &mut tx,
            subject,
            "privacy.application_erased",
            "privacy_request",
            id,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn cancel_recurring(s: &PlatformState) -> Result<()> {
    if s.providers.stripe.is_none() {
        return Ok(());
    }
    // Cancellation is idempotent at Stripe. A failed provider call leaves the
    // obligation visible and blocks erasure; it is never simulated locally.
    let mut tx = s.db.begin().await?;
    let request:Option<(Uuid,Uuid)>=sqlx::query_as("SELECT id,user_id FROM privacy_requests WHERE status<>'completed' AND next_cancel_at<=now() ORDER BY next_cancel_at,id FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
    let Some((request, subject)) = request else {
        return Ok(());
    };
    let subscription:Option<(Uuid,Uuid)>=sqlx::query_as("SELECT x.user_id,x.id FROM subscriptions x JOIN creators c ON c.id=x.creator_id WHERE (x.user_id=$1 OR c.user_id=$1) AND x.provider='stripe' AND x.status IN ('active','past_due') AND NOT x.cancel_at_period_end ORDER BY x.id LIMIT 1").bind(subject).fetch_optional(&mut *tx).await?;
    sqlx::query(
        "UPDATE privacy_requests SET next_cancel_at=now()+interval '10 minutes' WHERE id=$1",
    )
    .bind(request)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if let Some((subscriber, id)) = subscription {
        match providers::stripe::cancel(s, subscriber, id).await {
            Ok(true) => {
                sqlx::query("UPDATE subscriptions SET cancel_at_period_end=true WHERE id=$1")
                    .bind(id)
                    .execute(&s.db)
                    .await?;
                sqlx::query("UPDATE privacy_requests SET next_cancel_at=now() WHERE id=$1")
                    .bind(request)
                    .execute(&s.db)
                    .await?;
            }
            _ => tracing::error!(request_id=%request,"privacy_subscription_cancellation_failed"),
        }
    }
    Ok(())
}
pub(super) fn asset_keys(id: Uuid, video: bool) -> Vec<String> {
    let mut result: Vec<String> = ["upload", "jpg", "preview.jpg"]
        .map(|s| format!("{id}.{s}"))
        .into();
    if video {
        result.extend(["mp4", "m3u8", "720p.m3u8", "360p.m3u8"].map(|s| format!("{id}.{s}")));
        for resolution in ["720p", "360p"] {
            for segment in 0..=151 {
                result.push(format!("{id}.{resolution}-{segment:05}.ts"));
            }
        }
    }
    result
}
async fn enumerate(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
    subject: Uuid,
    cursor: Option<Uuid>,
) -> Result<()> {
    let assets:Vec<(Uuid,String)>=sqlx::query_as("SELECT m.id,m.content_type FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$1 AND ($2::uuid IS NULL OR m.id>$2) ORDER BY m.id LIMIT 5").bind(subject).bind(cursor).fetch_all(&mut **tx).await?;
    for (asset, content_type) in &assets {
        for key in asset_keys(*asset, content_type.starts_with("video/")) {
            sqlx::query("INSERT INTO privacy_objects(request_id,storage_key) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(request).bind(key).execute(&mut **tx).await?;
        }
        sqlx::query("UPDATE media_assets SET state='rejected',captions_vtt=NULL,rejection_reason='Account erasure requested' WHERE id=$1").bind(asset).execute(&mut **tx).await?;
    }
    if let Some((last, _)) = assets.last() {
        sqlx::query("UPDATE privacy_requests SET asset_cursor=$2 WHERE id=$1")
            .bind(request)
            .bind(last)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }
    let inserted=sqlx::query("INSERT INTO privacy_objects(request_id,storage_key) SELECT $1,x.storage_key FROM media_upload_chunks x JOIN media_uploads m ON m.id=x.upload_id JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$2 AND NOT EXISTS(SELECT 1 FROM privacy_objects o WHERE o.request_id=$1 AND o.storage_key=x.storage_key) ORDER BY x.storage_key LIMIT 100 ON CONFLICT DO NOTHING").bind(request).bind(subject).execute(&mut **tx).await?;
    if inserted.rows_affected() == 0 {
        sqlx::query("UPDATE privacy_requests SET objects_enumerated=true WHERE id=$1")
            .bind(request)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}
pub(super) async fn scrub_batch(tx: &mut Transaction<'_, Postgres>, subject: Uuid) -> Result<bool> {
    // Every statement is bounded. Fixed SQL identifiers only; user values are bound.
    for query in [
        "DELETE FROM comments WHERE id IN(SELECT id FROM comments WHERE user_id=$1 LIMIT 100)",
        "DELETE FROM messages WHERE id IN(SELECT id FROM messages WHERE sender_id=$1 LIMIT 100)",
        "DELETE FROM notifications WHERE id IN(SELECT id FROM notifications WHERE user_id=$1 LIMIT 100)",
        "DELETE FROM email_deliveries WHERE id IN(SELECT id FROM email_deliveries WHERE user_id=$1 LIMIT 100)",
        "DELETE FROM likes WHERE ctid IN(SELECT ctid FROM likes WHERE user_id=$1 LIMIT 100)",
        "DELETE FROM bookmarks WHERE ctid IN(SELECT ctid FROM bookmarks WHERE user_id=$1 LIMIT 100)",
        "DELETE FROM user_blocks WHERE ctid IN(SELECT ctid FROM user_blocks WHERE user_id=$1 OR blocked_user_id=$1 LIMIT 100)",
        "DELETE FROM mfa_recovery_codes WHERE ctid IN(SELECT ctid FROM mfa_recovery_codes WHERE user_id=$1 LIMIT 100)",
        "UPDATE posts SET caption='',sample_media_url=NULL,sample_preview_url='' WHERE id IN(SELECT p.id FROM posts p JOIN creators c ON c.id=p.creator_id WHERE c.user_id=$1 AND (p.caption<>'' OR p.sample_media_url IS NOT NULL OR p.sample_preview_url<>'') LIMIT 100)",
        "UPDATE reports SET reason='Erased by account request' WHERE id IN(SELECT id FROM reports WHERE user_id=$1 AND reason<>'Erased by account request' LIMIT 100)",
        "UPDATE moderation_appeals SET reason='Erased by account request',decision_reason=CASE WHEN decision_reason IS NULL THEN NULL ELSE 'Restricted review evidence retained' END WHERE id IN(SELECT id FROM moderation_appeals WHERE user_id=$1 AND reason<>'Erased by account request' LIMIT 100)",
    ] {
        if sqlx::query(query)
            .bind(subject)
            .execute(&mut **tx)
            .await?
            .rows_affected()
            > 0
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Expire one bounded batch of personal evidence using the captured schedule.
/// The database function owns hold checks, immutable-row exceptions and audit.
pub async fn purge_retention(db: &PgPool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let id: Uuid = std::env::var("PRIVACY_REQUEST_ID")?.parse()?;
    let counts: Value = sqlx::query_scalar("SELECT privacy_retention_batch($1)")
        .bind(id)
        .fetch_one(db)
        .await?;
    println!(
        "{}",
        json!({"request_id":id,"expired_personal_evidence":counts,"money_journals":"preserved","next":"Repeat while any count is 100; otherwise review again after remaining captured retention periods expire."})
    );
    Ok(())
}

/// Run only in the separately permissioned owner maintenance task. The normal
/// application role has neither version deletion IAM nor access to this queue.
pub async fn purge_versions(db: &PgPool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use aws_sdk_s3::{
        config::{BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig},
        types::{Delete, ObjectIdentifier},
    };
    let required = |name| std::env::var(name).map_err(|_| format!("Missing {name}"));
    let id: Uuid = required("PRIVACY_REQUEST_ID")?.parse()?;
    let primary = required("S3_BUCKET")?;
    let recovery = required("S3_RECOVERY_BUCKET")?;
    let prefix = required("S3_PREFIX")?;
    let owner = required("S3_EXPECTED_BUCKET_OWNER")?;
    let region = required("AWS_REGION")?;
    if primary == recovery
        || [&primary, &recovery].iter().any(|s| {
            !(3..=63).contains(&s.len())
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        || owner.len() != 12
        || !owner.bytes().all(|b| b.is_ascii_digit())
        || region.len() > 32
        || !region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || prefix.is_empty()
        || prefix.len() > 128
        || !prefix.ends_with('/')
        || prefix.starts_with('/')
        || prefix.contains("//")
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-".contains(&b))
        || ["AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL_S3"]
            .iter()
            .any(|key| std::env::var_os(key).is_some())
    {
        return Err("Invalid owner erasure storage configuration".into());
    }
    let identity = format!(
        "s3:{}",
        digest(&format!("{region}\n{owner}\n{primary}\n{prefix}"))
    );
    let actual: String =
        sqlx::query_scalar("SELECT media_store_identity FROM platform_storage WHERE id=1")
            .fetch_one(db)
            .await?;
    let owns:bool=sqlx::query_scalar("SELECT current_user=pg_get_userbyid(relowner) FROM pg_class WHERE oid='privacy_requests'::regclass").fetch_one(db).await?;
    if actual != identity || !owns {
        return Err("Erasure requires the original storage binding and database owner".into());
    }
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region.clone()))
        .retry_config(RetryConfig::standard().with_max_attempts(3))
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(Duration::from_secs(3))
                .operation_timeout(Duration::from_secs(20))
                .operation_attempt_timeout(Duration::from_secs(8))
                .build(),
        )
        .load()
        .await;
    let client = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(&config)
            .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
            .build(),
    );
    // Bounded invocation; rerunning resumes exact persisted markers/versions.
    for _ in 0..50 {
        let mut tx = db.begin().await?;
        let row=sqlx::query("SELECT user_id,purge_bucket,purge_key_marker,purge_version_marker,versions_erased_at IS NOT NULL AS finished FROM privacy_requests WHERE id=$1 AND status='application_erased' AND NOT legal_hold FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            return Err("Erasure requires application completion and no legal hold".into());
        };
        if row.get::<bool, _>("finished") {
            println!("{}", json!({"privacy_request":id,"versions_erased":true}));
            return Ok(());
        }
        let bucket_index: i16 = row.get("purge_bucket");
        if bucket_index == 1 {
            let pending:Vec<(String,String)>=sqlx::query_as("SELECT object_key,version_id FROM privacy_version_deletions WHERE request_id=$1 AND bucket=$2 AND NOT replication_checked ORDER BY object_key,version_id LIMIT 2").bind(id).bind(&primary).fetch_all(&mut *tx).await?;
            if !pending.is_empty() {
                for (key, version) in pending {
                    match client.head_object().bucket(&primary).key(&key).version_id(&version).expected_bucket_owner(&owner).send().await {
                        Ok(value) if value.replication_status().is_some_and(|v|v.as_str()=="COMPLETED")=>{},
                        Err(error) if error.as_service_error().is_some_and(|e|e.is_not_found())=>{},
                        _=>return Err("Media replication must complete before historical erasure; retry after replication recovery".into()),
                    }
                    sqlx::query("UPDATE privacy_version_deletions SET replication_checked=true WHERE request_id=$1 AND bucket=$2 AND object_key=$3 AND version_id=$4").bind(id).bind(&primary).bind(key).bind(version).execute(&mut *tx).await?;
                }
                tx.commit().await?;
                continue;
            }
        }
        if bucket_index < 2 {
            let bucket = if bucket_index == 0 {
                &primary
            } else {
                &recovery
            };
            let key_marker: Option<String> = row.get("purge_key_marker");
            let version_marker: Option<String> = row.get("purge_version_marker");
            let page = client
                .list_object_versions()
                .bucket(bucket)
                .expected_bucket_owner(&owner)
                .prefix(if bucket_index == 0 {
                    prefix.as_str()
                } else {
                    ""
                })
                .max_keys(1000)
                .set_key_marker(key_marker.clone())
                .set_version_id_marker(version_marker.clone())
                .send()
                .await?;
            let candidates: Vec<(String, String, String, bool)> = page
                .versions()
                .iter()
                .filter_map(|v| {
                    v.key()
                        .zip(v.version_id())
                        .map(|(key, version)| (key, version, false))
                })
                .chain(page.delete_markers().iter().filter_map(|v| {
                    v.key()
                        .zip(v.version_id())
                        .map(|(key, version)| (key, version, true))
                }))
                .filter_map(|(key, version, marker)| {
                    stored_asset(key, &prefix, bucket_index == 1).map(|asset| {
                        (
                            key.to_string(),
                            version.to_string(),
                            asset.to_string(),
                            marker,
                        )
                    })
                })
                .collect();
            let assets: Vec<String> = candidates.iter().map(|v| v.2.clone()).collect();
            let selected:Vec<String>=sqlx::query_scalar("SELECT storage_key FROM privacy_objects WHERE request_id=$1 AND storage_key=ANY($2)").bind(id).bind(&assets).fetch_all(&mut *tx).await?;
            let selected: std::collections::HashSet<_> = selected.into_iter().collect();
            for (key, version, asset, marker) in candidates {
                if selected.contains(&asset) {
                    if version.is_empty() || version == "null" || version.len() > 1024 {
                        return Err("Versioned erasure found an invalid object version".into());
                    }
                    sqlx::query("INSERT INTO privacy_version_deletions(request_id,bucket,object_key,version_id,replication_checked) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(id).bind(bucket).bind(key).bind(version).bind(marker||bucket_index==1).execute(&mut *tx).await?;
                }
            }
            if page.is_truncated() == Some(true) {
                let next_key = page
                    .next_key_marker()
                    .ok_or("Missing version pagination key")?;
                let next_version = page
                    .next_version_id_marker()
                    .ok_or("Missing version pagination ID")?;
                if key_marker.as_deref() == Some(next_key)
                    && version_marker.as_deref() == Some(next_version)
                {
                    return Err("Version enumeration failed to advance".into());
                }
                sqlx::query("UPDATE privacy_requests SET purge_key_marker=$2,purge_version_marker=$3 WHERE id=$1").bind(id).bind(next_key).bind(next_version).execute(&mut *tx).await?;
            } else {
                sqlx::query("UPDATE privacy_requests SET purge_bucket=purge_bucket+1,purge_key_marker=NULL,purge_version_marker=NULL WHERE id=$1").bind(id).execute(&mut *tx).await?;
            }
        } else {
            let batch:Vec<(String,String,String)>=sqlx::query_as("SELECT bucket,object_key,version_id FROM privacy_version_deletions WHERE request_id=$1 AND removed_at IS NULL ORDER BY bucket,object_key,version_id LIMIT 50").bind(id).fetch_all(&mut *tx).await?;
            if let Some((bucket, _, _)) = batch.first() {
                let batch: Vec<_> = batch.iter().filter(|v| &v.0 == bucket).collect();
                let mut objects = Vec::new();
                for (_, key, version) in &batch {
                    objects.push(
                        ObjectIdentifier::builder()
                            .key(key)
                            .version_id(version)
                            .build()?,
                    );
                }
                let result = client
                    .delete_objects()
                    .bucket(bucket)
                    .expected_bucket_owner(&owner)
                    .delete(
                        Delete::builder()
                            .set_objects(Some(objects))
                            .quiet(false)
                            .build()?,
                    )
                    .send()
                    .await?;
                if !result.errors().is_empty() {
                    return Err(
                        "Historical version deletion failed; durable queue remains retryable"
                            .into(),
                    );
                }
                for (_, key, version) in batch {
                    sqlx::query("UPDATE privacy_version_deletions SET removed_at=now() WHERE request_id=$1 AND bucket=$2 AND object_key=$3 AND version_id=$4").bind(id).bind(bucket).bind(key).bind(version).execute(&mut *tx).await?;
                }
            } else {
                sqlx::query("UPDATE privacy_requests SET versions_erased_at=now() WHERE id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                audit(
                    &mut tx,
                    row.get("user_id"),
                    "privacy.versions_erased",
                    "privacy_request",
                    id,
                )
                .await
                .map_err(|e| e.1)?;
            }
        }
        tx.commit().await?;
    }
    println!(
        "{}",
        json!({"privacy_request":id,"versions_erased":false,"next":"Rerun the same request to resume the bounded purge"})
    );
    Ok(())
}
fn stored_asset<'a>(key: &'a str, prefix: &str, recovery: bool) -> Option<&'a str> {
    if let Some(asset) = key.strip_prefix(prefix) {
        return Some(asset);
    }
    if !recovery {
        return None;
    }
    let (checkpoint, rest) = key.split_once("/objects/")?;
    Uuid::parse_str(checkpoint.strip_prefix("checkpoint-")?).ok()?;
    rest.strip_prefix(prefix)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn evidence_references_and_media_enumeration_are_bounded() {
        assert!(evidence_reference("sha256:0123456789abcdef"));
        assert!(!evidence_reference("private email@example.com"));
        assert!(!evidence_reference(&"a".repeat(201)));
        let id = Uuid::new_v4();
        let keys = asset_keys(id, true);
        assert_eq!(keys.len(), 311);
        assert_eq!(
            keys.iter().collect::<std::collections::HashSet<_>>().len(),
            311
        );
        assert!(keys.contains(&format!("{id}.360p-00151.ts")));
        assert_eq!(asset_keys(id, false).len(), 3);
        assert_eq!(
            stored_asset("myfans/a.jpg", "myfans/", false),
            Some("a.jpg")
        );
        assert_eq!(
            stored_asset("not-a-checkpoint/objects/myfans/a.jpg", "myfans/", true),
            None
        );
        assert_eq!(
            stored_asset(
                &format!("checkpoint-{id}/objects/myfans/a.jpg"),
                "myfans/",
                true
            ),
            Some("a.jpg")
        );
        assert_eq!(stored_asset("other/a.jpg", "myfans/", true), None);
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn evidence_retention_is_owner_only_bounded_and_preserves_money(db: PgPool) {
        let subject:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('retention@example.test','Deleted account') RETURNING id").fetch_one(&db).await.unwrap();
        let operator:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('retention-admin@example.test','Operator') RETURNING id").fetch_one(&db).await.unwrap();
        let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency) VALUES($1,'retention_creator',500,'USD') RETURNING id").bind(subject).fetch_one(&db).await.unwrap();
        let post:Uuid=sqlx::query_scalar("INSERT INTO posts(creator_id,caption,state,visibility) VALUES($1,'Personal creator caption','draft','public') RETURNING id").bind(creator).fetch_one(&db).await.unwrap();
        let report:Uuid=sqlx::query_scalar("INSERT INTO reports(user_id,post_id,reason,status) VALUES($1,$2,'Personal reported content','resolved') RETURNING id").bind(operator).bind(post).fetch_one(&db).await.unwrap();
        sqlx::query("UPDATE app_users SET disabled_at=now(),erased_at=now() WHERE id=$1")
            .bind(subject)
            .execute(&db)
            .await
            .unwrap();
        let checkout:Uuid=sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,kind,amount_minor,status,paid_at,idempotency_key,request_fingerprint) VALUES($1,$2,'tip',500,'paid',now(),gen_random_uuid(),'retention') RETURNING id").bind(operator).bind(creator).fetch_one(&db).await.unwrap();
        let schedule = json!({"erasure_delay_days":0,"backup_days":35,"security_days":1,"financial_days":1,"policy_evidence_days":1,"moderation_evidence_days":1});
        let request:Uuid=sqlx::query_scalar("INSERT INTO privacy_requests(user_id,status,privacy_version,retention,not_before,application_erased_at) VALUES($1,'application_erased','v1',$2,now(),now()) RETURNING id").bind(subject).bind(&schedule).fetch_one(&db).await.unwrap();
        sqlx::query("INSERT INTO policy_acceptances(user_id,country_code,terms_version,privacy_version,content_policy_version,minimum_age,created_at) SELECT $1,'US','v1','v1','v1',18,now()-interval '4 days' FROM generate_series(1,101)").bind(subject).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,created_at) VALUES($1,'password_changed','user',$1,now()-interval '4 days')").bind(subject).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO provider_finance_actions(actor_id,action,target,reason,created_at) VALUES($1,'refund.request',$2,'Personal review evidence',now()-interval '4 days')").bind(operator).bind(checkout.to_string()).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO moderation_evidence(actor_id,resource_type,resource_id,decision,reason,policy_version,snapshot,snapshot_sha256,created_at) VALUES($1,'user',$2,'disable','Personal moderation evidence','v1','{}',repeat('a',64),now()-interval '4 days')").bind(operator).bind(subject).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO moderation_evidence(actor_id,resource_type,resource_id,decision,reason,policy_version,snapshot,snapshot_sha256,created_at) VALUES($1,'report',$2,'resolve','Personal creator content','v1','{}',repeat('a',64),now()-interval '4 days')").bind(operator).bind(report).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,metadata,created_at) VALUES($1,'review.reason','report',$2,'{\"reason\":\"Personal report narrative\"}',now()-interval '4 days')").bind(operator).bind(report).execute(&db).await.unwrap();
        let payout:Uuid=sqlx::query_scalar("INSERT INTO payouts(user_id,amount_minor,status,idempotency_key) VALUES($1,500,'rejected',gen_random_uuid()) RETURNING id").bind(subject).fetch_one(&db).await.unwrap();
        sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,metadata,created_at) VALUES($1,'review.reason','payout',$2,'{\"reason\":\"Personal payout narrative\"}',now()-interval '4 days')").bind(operator).bind(payout).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO privacy_actions(request_id,actor_id,action,reason,created_at) VALUES($1,$2,'requested','Personal privacy review narrative',now()-interval '4 days')").bind(request).bind(subject).execute(&db).await.unwrap();
        let public_execute:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_proc p, LATERAL aclexplode(coalesce(p.proacl,acldefault('f',p.proowner))) a WHERE p.oid='privacy_retention_batch(uuid)'::regprocedure AND a.grantee=0 AND a.privilege_type='EXECUTE')").fetch_one(&db).await.unwrap();
        assert!(!public_execute);
        sqlx::raw_sql(include_str!("../../../../db/roles/runtime.sql"))
            .execute(&db)
            .await
            .unwrap();
        assert!(!sqlx::query_scalar::<_,bool>("SELECT has_function_privilege('myfans_runtime','privacy_retention_batch(uuid)','EXECUTE')")
            .fetch_one(&db).await.unwrap());
        let mut restricted = db.acquire().await.unwrap();
        sqlx::query("SET ROLE myfans_runtime")
            .execute(&mut *restricted)
            .await
            .unwrap();
        assert!(
            sqlx::query("SELECT privacy_retention_batch($1)")
                .bind(request)
                .execute(&mut *restricted)
                .await
                .is_err()
        );
        sqlx::query("RESET ROLE")
            .execute(&mut *restricted)
            .await
            .unwrap();
        drop(restricted);
        assert!(
            sqlx::query("UPDATE privacy_requests SET retention='{}' WHERE id=$1")
                .bind(request)
                .execute(&db)
                .await
                .is_err()
        );
        let first: Value = sqlx::query_scalar("SELECT privacy_retention_batch($1)")
            .bind(request)
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(first.as_object().unwrap().values().all(|v| v == 0));
        sqlx::query("UPDATE privacy_requests SET application_erased_at=now()-interval '3 days',legal_hold=true WHERE id=$1").bind(request).execute(&db).await.unwrap();
        assert!(
            sqlx::query("SELECT privacy_retention_batch($1)")
                .bind(request)
                .execute(&db)
                .await
                .is_err()
        );
        sqlx::query("UPDATE privacy_requests SET legal_hold=false WHERE id=$1")
            .bind(request)
            .execute(&db)
            .await
            .unwrap();
        let related:Uuid=sqlx::query_scalar("INSERT INTO privacy_requests(user_id,privacy_version,retention,not_before,legal_hold) VALUES($1,'v1',$2,now(),true) RETURNING id").bind(operator).bind(&schedule).fetch_one(&db).await.unwrap();
        assert!(
            sqlx::query("SELECT privacy_retention_batch($1)")
                .bind(request)
                .execute(&db)
                .await
                .is_err()
        );
        sqlx::query("UPDATE privacy_requests SET legal_hold=false WHERE id=$1")
            .bind(related)
            .execute(&db)
            .await
            .unwrap();
        let counts: Value = sqlx::query_scalar("SELECT privacy_retention_batch($1)")
            .bind(request)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(counts["security"], 1);
        assert_eq!(counts["financial_personal_evidence"], 1);
        assert_eq!(counts["policy_evidence"], 100);
        assert_eq!(counts["moderation_evidence"], 2);
        assert_eq!(counts["moderation_audit_narratives"], 1);
        assert_eq!(counts["financial_audit_narratives"], 1);
        assert_eq!(counts["privacy_review_narratives"], 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT amount_minor FROM checkouts WHERE id=$1")
                .bind(checkout)
                .fetch_one(&db)
                .await
                .unwrap(),
            500
        );
        assert!(
            sqlx::query("DELETE FROM policy_acceptances WHERE user_id=$1")
                .bind(subject)
                .execute(&db)
                .await
                .is_err()
        );
        sqlx::query(
            "UPDATE privacy_requests SET status='completed',completed_at=now() WHERE id=$1",
        )
        .bind(request)
        .execute(&db)
        .await
        .unwrap();
        let second: Value = sqlx::query_scalar("SELECT privacy_retention_batch($1)")
            .bind(request)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(second["policy_evidence"], 1);
        assert_eq!(second["privacy_review_narratives"], 1);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT reason FROM privacy_actions WHERE request_id=$1"
            )
            .bind(request)
            .fetch_one(&db)
            .await
            .unwrap(),
            "Expired personal review narrative"
        );
        assert!(
            sqlx::query("UPDATE privacy_actions SET reason='Altered receipt' WHERE request_id=$1")
                .bind(request)
                .execute(&db)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM privacy_retention_runs WHERE request_id=$1")
                .bind(request)
                .execute(&db)
                .await
                .is_err()
        );
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn holds_and_financial_obligations_block_erasure_then_cleanup_is_durable(db: PgPool) {
        let root = std::env::temp_dir().join(format!("myfans-privacy-{}", Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let mut runtime = RuntimeConfig::default();
        runtime.policy.retention.erasure_delay_days = 0;
        let s = PlatformState {
            db: db.clone(),
            runtime: Arc::new(runtime),
            media_root: root.clone(),
            key: Arc::new([4; 32]),
            dummy_hash: Arc::new(String::new()),
            mail: Default::default(),
            storage: MediaStore::local(root.clone()),
            scanner: Default::default(),
            providers: Default::default(),
        };
        let subject:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('privacy@example.test','Private name') RETURNING id").fetch_one(&db).await.unwrap();
        let operator:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('privacy-admin@example.test','Operator') RETURNING id").fetch_one(&db).await.unwrap();
        let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,bio) VALUES($1,'private_creator',500,'USD','Private biography') RETURNING id").bind(subject).fetch_one(&db).await.unwrap();
        let asset:Uuid=sqlx::query_scalar("INSERT INTO media_assets(creator_id,storage_key,state,size_bytes,content_type) VALUES($1,$2,'approved',1,'image/jpeg') RETURNING id").bind(creator).bind(format!("{}.jpg",Uuid::new_v4())).fetch_one(&db).await.unwrap();
        sqlx::query("UPDATE media_assets SET storage_key=$2 WHERE id=$1")
            .bind(asset)
            .bind(format!("{asset}.jpg"))
            .execute(&db)
            .await
            .unwrap();
        tokio::fs::write(root.join(format!("{asset}.jpg")), b"private image")
            .await
            .unwrap();
        sqlx::query("INSERT INTO posts(creator_id,caption,state,visibility) VALUES($1,'Private caption','draft','public')").bind(creator).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO sessions(user_id,token_digest,expires_at) VALUES($1,'privacy-session',now()+interval '1 hour')").bind(subject).execute(&db).await.unwrap();
        let ledger:Uuid=sqlx::query_scalar("INSERT INTO ledger_accounts(owner_id,kind,currency) VALUES($1,'creator_available','USD') RETURNING id").bind(subject).fetch_one(&db).await.unwrap();
        let subscription:Uuid=sqlx::query_scalar("INSERT INTO subscriptions(user_id,creator_id,status,provider,provider_subscription_id) VALUES($1,$2,'pending','sandbox','privacy-subscription') RETURNING id").bind(operator).bind(creator).fetch_one(&db).await.unwrap();
        let mut tx = db.begin().await.unwrap();
        let id = freeze(&s, &mut tx, subject).await.unwrap();
        tx.commit().await.unwrap();
        sqlx::query("UPDATE app_users SET role='admin',mfa_enabled=true WHERE id=$1")
            .bind(operator)
            .execute(&db)
            .await
            .unwrap();
        let admin_token = "a".repeat(64);
        sqlx::query("INSERT INTO sessions(user_id,token_digest,expires_at,mfa_verified_at) VALUES($1,$2,now()+interval '1 hour',now())").bind(operator).bind(digest(&admin_token)).execute(&db).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {admin_token}").parse().unwrap(),
        );
        let input = serde_json::from_value(
            json!({"action":"restore","reason":"Attempt to restore a frozen privacy request"}),
        )
        .unwrap();
        let denied =
            operations::moderate_account(State(s.clone()), headers, Path(subject), Json(input))
                .await
                .unwrap_err();
        assert_eq!(denied.0, StatusCode::BAD_REQUEST);
        assert!(denied.1.contains("erasure request"));
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT disabled_at IS NOT NULL FROM app_users WHERE id=$1"
            )
            .bind(subject)
            .fetch_one(&db)
            .await
            .unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id=$1")
                .bind(subject)
                .fetch_one(&db)
                .await
                .unwrap(),
            0
        );
        assert!(sqlx::query("INSERT INTO posts(creator_id,caption,state,visibility) VALUES($1,'race','draft','public')").bind(creator).execute(&db).await.is_err());
        sqlx::query("UPDATE privacy_requests SET status='approved',approved_by=$2,approved_at=now(),legal_hold=true WHERE id=$1").bind(id).bind(operator).execute(&db).await.unwrap();
        tick(&s).await.unwrap();
        assert!(root.join(format!("{asset}.jpg")).exists());
        sqlx::query("UPDATE privacy_requests SET legal_hold=false WHERE id=$1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
        tick(&s).await.unwrap();
        assert!(root.join(format!("{asset}.jpg")).exists());
        sqlx::query("UPDATE subscriptions SET status='expired' WHERE id=$1")
            .bind(subscription)
            .execute(&db)
            .await
            .unwrap();
        for _ in 0..20 {
            sqlx::query("UPDATE privacy_requests SET next_at=now() WHERE id=$1")
                .bind(id)
                .execute(&db)
                .await
                .unwrap();
            tick(&s).await.unwrap();
        }
        let result:(String,String,bool)=sqlx::query_as("SELECT u.email,r.status,r.versions_erased_at IS NOT NULL FROM app_users u JOIN privacy_requests r ON r.user_id=u.id WHERE u.id=$1").bind(subject).fetch_one(&db).await.unwrap();
        assert!(result.0.starts_with("erased-"));
        assert_eq!(result.1, "application_erased");
        assert!(result.2);
        assert!(!root.join(format!("{asset}.jpg")).exists());
        assert_eq!(
            sqlx::query_scalar::<_, Uuid>("SELECT owner_id FROM ledger_accounts WHERE id=$1")
                .bind(ledger)
                .fetch_one(&db)
                .await
                .unwrap(),
            subject
        );
        assert!(
            sqlx::query("DELETE FROM privacy_actions WHERE request_id=$1")
                .bind(id)
                .execute(&db)
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
