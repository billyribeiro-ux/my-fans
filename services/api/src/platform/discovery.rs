use super::*;
use axum::extract::Query;
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    q: Option<String>,
    category: Option<String>,
    cursor: Option<String>,
}
pub async fn creators(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<Search>,
) -> Result<Json<Value>> {
    let viewer = optional_user(&s, &h).await?.map(|u| u.id);
    let term = q.q.unwrap_or_default().trim().to_owned();
    if term.len() > 100 {
        return Err(bad("Search is limited to 100 bytes."));
    }
    let categories = ["Travel", "Food", "Art", "Fitness", "Lifestyle", "Music"];
    let category = q.category.filter(|v| !v.is_empty() && v != "All");
    if category
        .as_ref()
        .is_some_and(|v| !categories.contains(&v.as_str()))
    {
        return Err(bad("Invalid creator category."));
    }
    let (date, id) = if let Some(cursor) = q.cursor {
        if cursor.len() > 90 {
            return Err(bad("Invalid discovery cursor."));
        }
        let (date, id) = cursor
            .split_once('|')
            .ok_or(bad("Invalid discovery cursor."))?;
        (
            Some(
                date.parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|_| bad("Invalid discovery cursor."))?,
            ),
            Some(
                id.parse::<Uuid>()
                    .map_err(|_| bad("Invalid discovery cursor."))?,
            ),
        )
    } else {
        (None, None)
    };
    let pattern = format!(
        "%{}%",
        term.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut creators:Value=sqlx::query_scalar(r#"SELECT COALESCE(jsonb_agg(to_jsonb(x) ORDER BY x.created_at DESC,x.id DESC),'[]') FROM (
      SELECT c.id,c.user_id,c.handle,c.display_name,c.category,c.bio,c.avatar_url,c.cover_url,c.verification_status,c.subscription_price_minor,c.currency,c.created_at,
        (SELECT count(*) FROM subscriptions WHERE creator_id=c.id AND status='active') AS subscriber_count,
        EXISTS(SELECT 1 FROM entitlements e WHERE e.user_id=$1 AND e.creator_id=c.id AND e.revoked_at IS NULL AND e.starts_at<=now() AND (e.expires_at IS NULL OR e.expires_at>now())) AS subscribed
      FROM creators c JOIN app_users u ON u.id=c.user_id
      WHERE c.verification_status='approved' AND u.disabled_at IS NULL
        AND NOT EXISTS(SELECT 1 FROM user_blocks b WHERE (b.user_id=$1 AND b.blocked_user_id=c.user_id) OR (b.user_id=c.user_id AND b.blocked_user_id=$1))
        AND (c.display_name ILIKE $2 OR c.handle ILIKE $2 OR c.bio ILIKE $2)
        AND ($3::text IS NULL OR c.category=$3)
        AND ($4::timestamptz IS NULL OR (c.created_at,c.id)<($4,$5))
      ORDER BY c.created_at DESC,c.id DESC LIMIT 51) x"#).bind(viewer).bind(pattern).bind(category).bind(date).bind(id).fetch_one(&s.db).await?;
    let rows = creators
        .as_array_mut()
        .ok_or(bad("Invalid discovery result."))?;
    let more = rows.len() > 50;
    rows.truncate(50);
    let next = if more {
        rows.last().and_then(|c| {
            Some(format!(
                "{}|{}",
                c["created_at"].as_str()?,
                c["id"].as_str()?
            ))
        })
    } else {
        None
    };
    Ok(Json(
        json!({"creators":creators,"categories":categories,"next_cursor":next}),
    ))
}
