use super::*;
use axum::extract::{Path, Query};
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};

// Both contact writers and block writers lock the same ordered pair, preventing send/block races.
pub async fn lock_pair(tx: &mut Transaction<'_, Postgres>, a: Uuid, b: Uuid) -> Result<()> {
    let rows: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT id,disabled_at IS NULL FROM app_users WHERE id=$1 OR id=$2 ORDER BY id FOR UPDATE",
    )
    .bind(a)
    .bind(b)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != if a == b { 1 } else { 2 } || rows.iter().any(|r| !r.1) {
        return Err(forbidden());
    }
    Ok(())
}
pub async fn ensure_contact(tx: &mut Transaction<'_, Postgres>, a: Uuid, b: Uuid) -> Result<()> {
    lock_pair(tx, a, b).await?;
    let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_blocks WHERE (user_id=$1 AND blocked_user_id=$2) OR (user_id=$2 AND blocked_user_id=$1))").bind(a).bind(b).fetch_one(&mut **tx).await?;
    if blocked {
        return Err(Error(StatusCode::FORBIDDEN, "This contact is unavailable."));
    }
    Ok(())
}
pub async fn ensure_post_contact(
    tx: &mut Transaction<'_, Postgres>,
    viewer: Uuid,
    post: Uuid,
) -> Result<()> {
    let owner:Uuid=sqlx::query_scalar("SELECT c.user_id FROM posts p JOIN creators c ON c.id=p.creator_id WHERE p.id=$1 AND p.state='published' AND NOT p.moderation_blocked AND c.verification_status='approved'").bind(post).fetch_one(&mut **tx).await?;
    ensure_contact(tx, viewer, owner).await
}
pub async fn block(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    change_block(s, h, id, true).await
}
pub async fn unblock(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    change_block(s, h, id, false).await
}
async fn change_block(
    s: PlatformState,
    h: HeaderMap,
    id: Uuid,
    blocked: bool,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if u.id == id {
        return Err(bad("You cannot block yourself."));
    }
    rate_limit(&s, &format!("block:{}", u.id), 30).await?;
    let mut tx = s.db.begin().await?;
    // Unblocking disabled accounts remains possible for personal-list maintenance.
    let exists: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM app_users WHERE id=$1 OR id=$2 ORDER BY id FOR UPDATE")
            .bind(u.id)
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
    if exists.len() != 2 {
        return Err(forbidden());
    }
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let result = if blocked {
        sqlx::query(
            "INSERT INTO user_blocks(user_id,blocked_user_id) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
    } else {
        sqlx::query("DELETE FROM user_blocks WHERE user_id=$1 AND blocked_user_id=$2")
    }
    .bind(u.id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() > 0 {
        audit(
            &mut tx,
            u.id,
            if blocked {
                "user.blocked"
            } else {
                "user.unblocked"
            },
            "user",
            id,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"blocked":blocked})))
}
pub async fn safety(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["blocks_cursor", "appeals_cursor", "blocked_posts_cursor"])?;
    let u = user(&s, &h).await?;
    let (blocks, blocks_next) = pagination::read(&s.db, "SELECT b.blocked_user_id AS id,u.display_name,b.created_at FROM user_blocks b JOIN app_users u ON u.id=b.blocked_user_id WHERE b.user_id=$1", Some(u.id), q.cursor("blocks_cursor")?, "created_at", false).await?;
    let (appeals, appeals_next) = pagination::read(&s.db, "SELECT a.id,a.post_id,p.caption,a.reason,a.status,a.decision_reason,a.created_at FROM moderation_appeals a JOIN posts p ON p.id=a.post_id WHERE a.user_id=$1", Some(u.id), q.cursor("appeals_cursor")?, "created_at", false).await?;
    let (blocked_posts, blocked_posts_next) = pagination::read(&s.db, "SELECT p.id,p.caption,p.created_at,EXISTS(SELECT 1 FROM moderation_appeals a WHERE a.post_id=p.id AND a.user_id=$1 AND a.status='pending') AS appeal_pending FROM posts p JOIN creators c ON c.id=p.creator_id WHERE c.user_id=$1 AND p.moderation_blocked AND p.state='removed'", Some(u.id), q.cursor("blocked_posts_cursor")?, "created_at", false).await?;
    Ok(Json(
        json!({"next_cursors":{"blocks":blocks_next,"appeals":appeals_next,"blocked_posts":blocked_posts_next},"blocks":blocks,"appeals":appeals,"blocked_posts":blocked_posts}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appeal {
    post_id: Uuid,
    reason: String,
}
pub async fn appeal(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(p): Json<Appeal>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let reason = p.reason.trim();
    if !(5..=2000).contains(&reason.chars().count()) {
        return Err(bad("Explain your appeal in 5 to 2,000 characters."));
    }
    rate_limit(&s, &format!("appeal:{}", u.id), 5).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let eligible:Option<Uuid>=sqlx::query_scalar("SELECT p.id FROM posts p JOIN creators c ON c.id=p.creator_id WHERE p.id=$1 AND c.user_id=$2 AND p.moderation_blocked AND p.state='removed' FOR UPDATE OF p").bind(p.post_id).bind(u.id).fetch_optional(&mut *tx).await?;
    if eligible.is_none() {
        return Err(forbidden());
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO moderation_appeals(user_id,post_id,reason) VALUES($1,$2,$3) RETURNING id",
    )
    .bind(u.id)
    .bind(p.post_id)
    .bind(reason)
    .fetch_one(&mut *tx)
    .await?;
    audit(&mut tx, u.id, "appeal.submitted", "appeal", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"appeal":{"id":id,"status":"pending"}})))
}
pub async fn appeals(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["cursor"])?;
    admin(&s, &h).await?;
    let (appeals, next_cursor) = pagination::read(&s.db, "SELECT a.id,a.post_id,u.display_name,p.caption,a.reason,a.status,a.created_at,COALESCE('/api/platform/media/'||m.id||'/full',p.sample_media_url) AS full_url,COALESCE('/api/platform/media/'||m.id||'/preview',p.sample_preview_url) AS preview_url,CASE WHEN m.content_type='video/mp4' THEN 'video' ELSE 'image' END AS media_type,CASE WHEN m.content_type='video/mp4' THEN '/api/platform/media/'||m.id||'/captions' END AS captions_url,(SELECT e.metadata->>'reason' FROM audit_events e JOIN reports r ON r.id=e.resource_id WHERE r.post_id=p.id AND e.action='review.reason' AND e.resource_type='report' ORDER BY e.created_at DESC LIMIT 1) AS moderation_reason FROM moderation_appeals a JOIN app_users u ON u.id=a.user_id JOIN posts p ON p.id=a.post_id LEFT JOIN LATERAL(SELECT ma.id,ma.content_type FROM post_media pm JOIN media_assets ma ON ma.id=pm.asset_id WHERE pm.post_id=p.id ORDER BY pm.position LIMIT 1)m ON true WHERE a.status='pending'", None, q.cursor("cursor")?, "created_at", true).await?;
    Ok(Json(json!({"next_cursor":next_cursor,"appeals":appeals})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppealDecision {
    decision: String,
    reason: String,
}
pub async fn review_appeal(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(p): Json<AppealDecision>,
) -> Result<Json<Value>> {
    let u = admin(&s, &h).await?;
    let reason = p.reason.trim();
    if !["accept", "reject"].contains(&p.decision.as_str())
        || !(5..=2000).contains(&reason.chars().count())
    {
        return Err(bad(
            "Choose a decision and provide a reason of 5 to 2,000 characters.",
        ));
    }
    let mut tx = s.db.begin().await?;
    maintenance::authorize_review(&mut tx, &h, u.id, "appeal", id).await?;
    maintenance::evidence(&s, &mut tx, u.id, "appeal", id, &p.decision, reason).await?;
    let row=sqlx::query("SELECT a.user_id,a.post_id,a.status,p.state,p.moderation_blocked FROM moderation_appeals a JOIN posts p ON p.id=a.post_id WHERE a.id=$1 FOR UPDATE OF a,p").bind(id).fetch_one(&mut *tx).await?;
    let owner: Uuid = row.get("user_id");
    let post: Uuid = row.get("post_id");
    if owner == u.id {
        return Err(forbidden());
    }
    if row.get::<String, _>("status") != "pending" {
        return Err(Error(
            StatusCode::CONFLICT,
            "This appeal has already been reviewed.",
        ));
    }
    let accepted = p.decision == "accept";
    if accepted && row.get::<bool, _>("moderation_blocked") {
        if row.get::<String, _>("state") != "removed" {
            return Err(bad("The post state requires operator review."));
        }
        sqlx::query("UPDATE posts SET state='draft',moderation_blocked=false WHERE id=$1")
            .bind(post)
            .execute(&mut *tx)
            .await?;
    }
    let status = if accepted { "accepted" } else { "rejected" };
    sqlx::query("UPDATE moderation_appeals SET status=$2,decision_reason=$3,reviewed_by=$4,reviewed_at=now() WHERE id=$1").bind(id).bind(status).bind(reason).bind(u.id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_events(actor_id,action,resource_type,resource_id,metadata) VALUES($1,$2,'appeal',$3,$4)").bind(u.id).bind(format!("appeal.{status}")).bind(id).bind(json!({"post_id":post,"reason":reason})).execute(&mut *tx).await?;
    notify(
        &mut tx,
        owner,
        "Appeal reviewed",
        if accepted {
            "Your appeal was accepted. The post is a draft and requires normal publishing checks."
        } else {
            "Your appeal was rejected. See Settings for the reviewed reason."
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"status":status})))
}
