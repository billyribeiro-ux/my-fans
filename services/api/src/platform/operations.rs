use super::*;
use axum::extract::{Path, Query};
use serde::Deserialize;
use sqlx::Row;

pub async fn operations(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["jobs_cursor", "audit_cursor", "users_cursor"])?;
    admin(&s, &h).await?;
    let (jobs, jobs_next) = pagination::read(&s.db, "SELECT id,kind,status,attempts,run_at,last_error,created_at FROM jobs WHERE status IN ('failed','pending','running')", None, q.cursor("jobs_cursor")?, "created_at", false).await?;
    let (audit, audit_next) = pagination::read(
        &s.db,
        "SELECT id,action,resource_type,resource_id,created_at FROM audit_events",
        None,
        q.cursor("audit_cursor")?,
        "created_at",
        false,
    )
    .await?;
    let (users, users_next) = pagination::read(&s.db, "SELECT id,display_name,email,role,disabled_at,created_at FROM app_users WHERE role<>'admin'", None, q.cursor("users_cursor")?, "created_at", false).await?;
    let imbalance:i64=sqlx::query_scalar("SELECT count(*) FROM(SELECT t.id FROM ledger_transactions t LEFT JOIN ledger_postings p ON p.transaction_id=t.id WHERE t.status='posted' GROUP BY t.id HAVING COALESCE(sum(p.amount_minor),0)<>0 OR count(p.id)<2)x").fetch_one(&s.db).await?;
    let mail: i64 =
        sqlx::query_scalar("SELECT count(*) FROM email_deliveries WHERE status='pending'")
            .fetch_one(&s.db)
            .await?;
    let outbox: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE delivered_at IS NULL")
            .fetch_one(&s.db)
            .await?;
    Ok(Json(
        json!({"next_cursors":{"jobs":jobs_next,"audit":audit_next,"users":users_next},"jobs":jobs,"audit":audit,"users":users,"ledger_imbalance_count":imbalance,"queued_mail_count":mail,"outbox_pending_count":outbox,"mode":s.runtime.mode()}),
    ))
}
pub async fn retry_job(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let u = admin(&s, &h).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Admin).await?;
    let count=sqlx::query("UPDATE jobs SET status='pending',attempts=0,run_at=now(),locked_at=NULL,last_error=NULL WHERE id=$1 AND status='failed'").bind(id).execute(&mut *tx).await?;
    if count.rows_affected() != 1 {
        return Err(bad("Only failed jobs can be retried."));
    }
    audit(&mut tx, u.id, "job.retried", "job", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"pending"})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountReview {
    action: String,
    reason: String,
}
pub async fn moderate_account(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<AccountReview>,
) -> Result<Json<Value>> {
    let u = admin(&s, &h).await?;
    if !["suspend", "restore"].contains(&p.action.as_str())
        || !(5..=2000).contains(&p.reason.chars().count())
    {
        return Err(bad("Choose suspend or restore and provide a reason."));
    }
    let suspend = p.action == "suspend";
    let mut tx = s.db.begin().await?;
    maintenance::authorize_review(&mut tx, &h, u.id, "user", id).await?;
    maintenance::evidence(&s, &mut tx, u.id, "user", id, &p.action, &p.reason).await?;
    if !suspend
        && sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM privacy_requests WHERE user_id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(bad(
            "An account with an erasure request cannot be restored.",
        ));
    }
    let changed=sqlx::query("UPDATE app_users SET disabled_at=CASE WHEN $2 THEN now() ELSE NULL END WHERE id=$1 AND role<>'admin'").bind(id).bind(suspend).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(forbidden());
    }
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE creators SET verification_status=$2,verified_at=NULL WHERE user_id=$1")
        .bind(id)
        .bind(if suspend { "rejected" } else { "pending" })
        .execute(&mut *tx)
        .await?;
    if suspend {
        sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1)").bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,metadata) VALUES($1,$2,'user',$3,$4)").bind(u.id).bind(format!("user.{}",p.action)).bind(id).bind(json!({"reason":p.reason})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"status":if suspend{"suspended"}else{"restored"}}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reason {
    reason: String,
}
pub async fn restore_post(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<Reason>,
) -> Result<Json<Value>> {
    let u = admin(&s, &h).await?;
    if !(5..=2000).contains(&p.reason.chars().count()) {
        return Err(bad("Provide a reason of 5 to 2,000 characters."));
    }
    let mut tx = s.db.begin().await?;
    maintenance::authorize_review(&mut tx, &h, u.id, "post", id).await?;
    maintenance::evidence(&s, &mut tx, u.id, "post", id, "restore", &p.reason).await?;
    let changed=sqlx::query("UPDATE posts SET moderation_blocked=false,state='draft' WHERE id=$1 AND moderation_blocked AND state='removed'").bind(id).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(bad("This post is not under a moderation block."));
    }
    sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,metadata) VALUES($1,'post.moderation.restored','post',$2,$3)").bind(u.id).bind(id).bind(json!({"reason":p.reason})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"draft"})))
}

pub async fn conversations(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["cursor"])?;
    let u = user(&s, &h).await?;
    let (rows, next_cursor) = pagination::read(&s.db, "SELECT c.id,r.id AS recipient_id,r.display_name,(r.disabled_at IS NOT NULL OR EXISTS(SELECT 1 FROM user_blocks b WHERE (b.user_id=$1 AND b.blocked_user_id=r.id) OR (b.user_id=r.id AND b.blocked_user_id=$1))) AS blocked,EXISTS(SELECT 1 FROM user_blocks b WHERE b.user_id=$1 AND b.blocked_user_id=r.id) AS own_blocked,COALESCE(m.body,'') AS last_message,COALESCE(m.created_at,c.created_at) AS updated_at FROM conversations c JOIN app_users r ON r.id=CASE WHEN c.user_a=$1 THEN c.user_b ELSE c.user_a END LEFT JOIN LATERAL(SELECT body,created_at FROM messages WHERE conversation_id=c.id ORDER BY created_at DESC,id DESC LIMIT 1)m ON true WHERE c.user_a=$1 OR c.user_b=$1", Some(u.id), q.cursor("cursor")?, "updated_at", false).await?;
    Ok(Json(
        json!({"next_cursor":next_cursor,"conversations":rows}),
    ))
}
#[derive(Deserialize)]
pub struct Conversation {
    recipient_id: Uuid,
}
pub async fn conversation(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(p): Json<Conversation>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !u.email_verified {
        return Err(bad("Verify your email before messaging."));
    }
    if p.recipient_id == u.id {
        return Err(bad("Choose another person to message."));
    }
    rate_limit(&s, &format!("conversation:{}", u.id), 10).await?;
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM creators c WHERE (c.user_id=$1 OR c.user_id=$2) AND c.verification_status='approved') AND EXISTS(SELECT 1 FROM app_users WHERE id=$2 AND disabled_at IS NULL)").bind(u.id).bind(p.recipient_id).fetch_one(&s.db).await?;
    if !allowed {
        return Err(forbidden());
    }
    let (a, b) = if u.id < p.recipient_id {
        (u.id, p.recipient_id)
    } else {
        (p.recipient_id, u.id)
    };
    let mut tx = s.db.begin().await?;
    super::community::ensure_contact(&mut tx, u.id, p.recipient_id).await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    sqlx::query("INSERT INTO conversations(user_a,user_b) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(a)
        .bind(b)
        .execute(&mut *tx)
        .await?;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE user_a=$1 AND user_b=$2")
        .bind(a)
        .bind(b)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"conversation":{"id":id}})))
}
async fn participant(s: &PlatformState, u: Uuid, id: Uuid) -> Result<Uuid> {
    Ok(sqlx::query_scalar("SELECT CASE WHEN user_a=$2 THEN user_b ELSE user_a END FROM conversations WHERE id=$1 AND (user_a=$2 OR user_b=$2)").bind(id).bind(u).fetch_one(&s.db).await?)
}
pub async fn messages(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["cursor"])?;
    let u = user(&s, &h).await?;
    participant(&s, u.id, id).await?;
    let (rows, next_cursor) = pagination::read(
        &s.db,
        "SELECT id,sender_id,body,created_at FROM messages WHERE conversation_id=$1",
        Some(id),
        q.cursor("cursor")?,
        "created_at",
        false,
    )
    .await?;
    let mut rows = rows;
    rows.as_array_mut()
        .ok_or(bad("Invalid message result."))?
        .reverse();
    let conversation:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',c.id,'recipient_id',r.id,'display_name',r.display_name,'blocked',r.disabled_at IS NOT NULL OR EXISTS(SELECT 1 FROM user_blocks b WHERE (b.user_id=$1 AND b.blocked_user_id=r.id) OR (b.user_id=r.id AND b.blocked_user_id=$1)),'own_blocked',EXISTS(SELECT 1 FROM user_blocks b WHERE b.user_id=$1 AND b.blocked_user_id=r.id),'last_message',COALESCE(m.body,''),'updated_at',COALESCE(m.created_at,c.created_at)) FROM conversations c JOIN app_users r ON r.id=CASE WHEN c.user_a=$1 THEN c.user_b ELSE c.user_a END LEFT JOIN LATERAL(SELECT body,created_at FROM messages WHERE conversation_id=c.id ORDER BY created_at DESC,id DESC LIMIT 1)m ON true WHERE c.id=$2 AND (c.user_a=$1 OR c.user_b=$1)").bind(u.id).bind(id).fetch_one(&s.db).await?;
    Ok(Json(
        json!({"next_cursor":next_cursor,"messages":rows,"conversation":conversation,"viewer_id":u.id}),
    ))
}
#[derive(Deserialize)]
pub struct Message {
    body: String,
}
pub async fn message(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<Message>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !u.email_verified {
        return Err(bad("Verify your email before messaging."));
    }
    let recipient = participant(&s, u.id, id).await?;
    let body = p.body.trim();
    if body.is_empty() || body.chars().count() > 4000 {
        return Err(bad("Messages must contain 1 to 4,000 characters."));
    }
    rate_limit(&s, &format!("message:{}", u.id), 30).await?;
    let mut tx = s.db.begin().await?;
    super::community::ensure_contact(&mut tx, u.id, recipient).await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    sqlx::query("INSERT INTO messages(conversation_id,sender_id,body) VALUES($1,$2,$3)")
        .bind(id)
        .bind(u.id)
        .bind(body)
        .execute(&mut *tx)
        .await?;
    notify(
        &mut tx,
        recipient,
        "New message",
        &format!("{} sent you a message.", u.display_name),
    )
    .await?;
    tx.commit().await?;
    messages(
        State(s),
        h,
        Path(id),
        Query(pagination::PageQuery::default()),
    )
    .await
}
pub async fn notifications(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["cursor"])?;
    let u = user(&s, &h).await?;
    let (rows, next_cursor) = pagination::read(
        &s.db,
        "SELECT id,title,body,read_at,created_at FROM notifications WHERE user_id=$1",
        Some(u.id),
        q.cursor("cursor")?,
        "created_at",
        false,
    )
    .await?;
    Ok(Json(
        json!({"next_cursor":next_cursor,"notifications":rows,"viewer_id":u.id}),
    ))
}
pub async fn read_notification(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let r = sqlx::query(
        "UPDATE notifications SET read_at=COALESCE(read_at,now()) WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(u.id)
    .execute(&s.db)
    .await?;
    if r.rows_affected() == 0 {
        return Err(Error(StatusCode::NOT_FOUND, "Notification not found."));
    }
    Ok(Json(json!({"read":true})))
}
#[derive(Deserialize)]
pub struct Report {
    post_id: Uuid,
    reason: String,
}
pub async fn report(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(p): Json<Report>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    rate_limit(&s, &format!("report:{}", u.id), 5).await?;
    let reason = p.reason.trim();
    if !(5..=2000).contains(&reason.chars().count()) {
        return Err(bad("Please describe the issue in 5 to 2,000 characters."));
    }
    let published: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM posts WHERE id=$1 AND state='published')")
            .bind(p.post_id)
            .fetch_one(&s.db)
            .await?;
    if !published {
        return Err(Error(StatusCode::NOT_FOUND, "Post not found."));
    }
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO reports(user_id,post_id,reason) VALUES($1,$2,$3) RETURNING id",
    )
    .bind(u.id)
    .bind(p.post_id)
    .bind(reason)
    .fetch_one(&mut *tx)
    .await?;
    audit(&mut tx, u.id, "report.created", "report", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"report":{"id":id,"status":"pending"}})))
}
pub async fn reviews(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&[
        "creators_cursor",
        "media_cursor",
        "reports_cursor",
        "payouts_cursor",
        "checkouts_cursor",
    ])?;
    admin(&s, &h).await?;
    let (creators, creators_next) = pagination::read(&s.db, "SELECT c.id,c.display_name,c.handle,c.bio,c.verification_status,c.created_at,(SELECT jsonb_build_object('decision',e.decision,'reason',e.reason,'created_at',e.created_at) FROM moderation_evidence e WHERE e.resource_type='creator' AND e.resource_id=c.id ORDER BY e.created_at DESC,e.id DESC LIMIT 1) AS review_context FROM creators c WHERE c.verification_status='pending'", None, q.cursor("creators_cursor")?, "created_at", true).await?;
    let (media, media_next) = pagination::read(&s.db, "SELECT m.id,c.display_name,m.state,m.content_type,m.size_bytes,m.created_at,'/api/platform/media/'||m.id||'/preview' AS preview_url,'/api/platform/media/'||m.id||'/full' AS full_url FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE m.state='quarantine' AND m.preview_key IS NOT NULL", None, q.cursor("media_cursor")?, "created_at", true).await?;
    let (reports, reports_next) = pagination::read(&s.db, "SELECT r.id,r.post_id,r.reason,r.created_at,p.caption FROM reports r JOIN posts p ON p.id=r.post_id WHERE r.status='pending'", None, q.cursor("reports_cursor")?, "created_at", true).await?;
    let (payouts, payouts_next) = pagination::read(&s.db, "SELECT p.id,u.display_name,p.amount_minor,p.currency,p.status,p.created_at FROM payouts p JOIN app_users u ON u.id=p.user_id WHERE p.status='pending'", None, q.cursor("payouts_cursor")?, "created_at", true).await?;
    let (checkouts, checkouts_next) = pagination::read(&s.db, "SELECT c.id,c.provider,u.display_name AS buyer,r.display_name AS creator,c.kind,c.amount_minor,c.currency,c.status,c.paid_at FROM checkouts c JOIN app_users u ON u.id=c.user_id JOIN creators r ON r.id=c.creator_id WHERE c.status IN ('paid','refunded')", None, q.cursor("checkouts_cursor")?, "paid_at", false).await?;
    Ok(Json(
        json!({"next_cursors":{"creators":creators_next,"media":media_next,"reports":reports_next,"payouts":payouts_next,"checkouts":checkouts_next},"creators":creators,"media":media,"reports":reports,"payouts":payouts,"checkouts":checkouts,"mode":s.runtime.mode()}),
    ))
}
#[derive(Deserialize)]
pub struct Review {
    kind: String,
    decision: String,
    reason: String,
}
pub async fn review(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<Review>,
) -> Result<Json<Value>> {
    let u = admin(&s, &h).await?;
    if !["approve", "reject"].contains(&p.decision.as_str())
        || !(5..=2000).contains(&p.reason.chars().count())
    {
        return Err(bad(
            "Choose a decision and provide a reason of 5 to 2,000 characters.",
        ));
    }
    let approve = p.decision == "approve";
    let mut tx = s.db.begin().await?;
    maintenance::authorize_review(&mut tx, &h, u.id, &p.kind, id).await?;
    maintenance::evidence(&s, &mut tx, u.id, &p.kind, id, &p.decision, &p.reason).await?;
    if p.kind == "creator" && approve && s.providers.identity.is_some() {
        let verified:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM identity_checks WHERE creator_id=$1 AND status='verified' AND checked_at>now()-interval '24 hours')").bind(id).fetch_one(&mut *tx).await?;
        if !verified {
            return Err(bad(
                "Identity and age verification must be completed before creator approval.",
            ));
        }
    }
    let owner:Uuid=match p.kind.as_str(){
 "creator"=>sqlx::query_scalar("UPDATE creators SET verification_status=$2,verified_at=CASE WHEN $3 THEN now() ELSE NULL END,review_message=$4,reviewed_at=now() WHERE id=$1 AND verification_status='pending' RETURNING user_id").bind(id).bind(if approve{"approved"}else{"rejected"}).bind(approve).bind(&p.reason).fetch_one(&mut *tx).await?,
 "media"=>{let row=sqlx::query("SELECT c.user_id,m.state,m.preview_key FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE m.id=$1 FOR UPDATE OF m").bind(id).fetch_one(&mut *tx).await?;if row.get::<String,_>("state")!="quarantine" || row.get::<Option<String>,_>("preview_key").is_none(){return Err(bad("Media processing must finish before review."))}sqlx::query("UPDATE media_assets SET state=$2,rejection_reason=$3 WHERE id=$1").bind(id).bind(if approve{"approved"}else{"rejected"}).bind(if approve{None}else{Some(p.reason.as_str())}).execute(&mut *tx).await?;row.get("user_id")},
 "report"=>{let row=sqlx::query("UPDATE reports SET status=$2 WHERE id=$1 AND status='pending' RETURNING user_id,post_id").bind(id).bind(if approve{"resolved"}else{"rejected"}).fetch_one(&mut *tx).await?;if approve{sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE id=$1").bind(row.get::<Uuid,_>("post_id")).execute(&mut *tx).await?;}row.get("user_id")},
 "payout"=>{let row=sqlx::query("SELECT user_id,amount_minor,status FROM payouts WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;if row.get::<String,_>("status")!="pending"{return Err(bad("This payout has already been reviewed."))}let owner:Uuid=row.get("user_id");sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE").bind(owner).execute(&mut *tx).await?;let reserve=super::billing::account_id(&mut tx,Some(owner),"reserve").await?;let destination=super::billing::account_id(&mut tx,if approve{None}else{Some(owner)},if approve{"processor_clearing"}else{"creator_available"}).await?;let amount:i64=row.get("amount_minor");super::billing::post_ledger(&mut tx,&format!("payout-review:{id}"),&[(reserve,-amount),(destination,amount)]).await?;sqlx::query("UPDATE payouts SET status=$2 WHERE id=$1").bind(id).bind(if approve{"paid"}else{"rejected"}).execute(&mut *tx).await?;owner},
 _=>return Err(bad("Unknown review type."))
 };
    audit(
        &mut tx,
        u.id,
        &format!("{}.{}.{}", p.kind, p.decision, s.runtime.mode()),
        &p.kind,
        id,
    )
    .await?;
    sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,metadata) VALUES($1,'review.reason',$2,$3,$4)").bind(u.id).bind(&p.kind).bind(id).bind(json!({"reason":p.reason})).execute(&mut *tx).await?;
    notify(
        &mut tx,
        owner,
        "Review completed",
        &format!("Your {} review was {}d.", p.kind, p.decision),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"status":if approve{"approved"}else{"rejected"},"mode":s.runtime.mode()}),
    ))
}
