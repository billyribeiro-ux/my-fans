//! Explicit moderator inspection. Public permalinks continue to use the feed's
//! normal publication, blocking and entitlement projection.
use super::*;
use axum::extract::Path;

pub async fn inspect(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let actor = admin(&s, &h).await?;
    rate_limit(&s, &format!("post-inspection:{}", actor.id), 60).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, actor.id, Assurance::Admin).await?;
    let value: Option<Value> = sqlx::query_scalar(r#"
      SELECT jsonb_build_object(
        'viewer_id',$2::uuid,
        'post',jsonb_build_object(
          'id',p.id,'creator_id',p.creator_id,'caption',p.caption,
          'created_at',COALESCE(p.published_at,p.created_at),'state',p.state,
          'moderation_blocked',p.moderation_blocked,'visibility',p.visibility,
          'price_minor',p.purchase_price_minor,'locked',false,
          'media_state',m.state,
          'media_type',CASE WHEN m.content_type='video/mp4' THEN 'video' ELSE 'image' END,
          'media_url',CASE WHEN m.asset_id IS NULL THEN p.sample_media_url
            WHEN m.state IN ('approved','quarantine') THEN '/api/platform/media/'||m.asset_id||'/full' END,
          'preview_url',CASE WHEN m.asset_id IS NULL THEN p.sample_preview_url
            WHEN m.state IN ('approved','quarantine') THEN '/api/platform/media/'||m.asset_id||'/preview' ELSE '' END,
          'captions_url',CASE WHEN m.state IN ('approved','quarantine') AND m.content_type='video/mp4' AND m.captions_ready THEN '/api/platform/media/'||m.asset_id||'/captions' END,
          'hls_url',CASE WHEN m.state IN ('approved','quarantine') AND m.hls_ready THEN '/api/platform/media/'||m.asset_id||'/hls/'||m.asset_id||'.m3u8' END,
          'like_count',(SELECT count(*) FROM likes WHERE post_id=p.id),
          'comment_count',(SELECT count(*) FROM comments WHERE post_id=p.id),
          'liked',EXISTS(SELECT 1 FROM likes WHERE user_id=$2 AND post_id=p.id),
          'saved',EXISTS(SELECT 1 FROM bookmarks WHERE user_id=$2 AND post_id=p.id)),
        'creator',jsonb_build_object(
          'id',c.id,'user_id',c.user_id,'handle',c.handle,'display_name',c.display_name,
          'category',c.category,'bio',c.bio,'avatar_url',c.avatar_url,'cover_url',c.cover_url,
          'verification_status',c.verification_status,'subscription_price_minor',c.subscription_price_minor,
          'currency',c.currency,'subscriber_count',(SELECT count(*) FROM subscriptions WHERE creator_id=c.id AND status='active'),
          'subscribed',EXISTS(SELECT 1 FROM entitlements e WHERE e.user_id=$2 AND e.creator_id=c.id AND e.revoked_at IS NULL AND e.starts_at<=now() AND (e.expires_at IS NULL OR e.expires_at>now()))))
      FROM posts p JOIN creators c ON c.id=p.creator_id
      LEFT JOIN LATERAL(SELECT pm.asset_id,m.state,m.content_type,m.hls_ready,m.captions_vtt IS NOT NULL AS captions_ready
        FROM post_media pm JOIN media_assets m ON m.id=pm.asset_id
        WHERE pm.post_id=p.id ORDER BY pm.position LIMIT 1)m ON true
      WHERE p.id=$1
    "#).bind(id).bind(actor.id).fetch_optional(&mut *tx).await?;
    let mut value = value.ok_or(Error(StatusCode::NOT_FOUND, "Post not found."))?;
    audit(&mut tx, actor.id, "post.inspected", "post", id).await?;
    tx.commit().await?;
    value["mode"] = json!(s.runtime.mode());
    Ok(Json(value))
}
