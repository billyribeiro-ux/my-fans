use super::*;
use axum::extract::{Multipart, Path, Query, State};
use serde::Deserialize;
pub async fn get_captions(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    media(State(s), h, Path((id, "captions".into()))).await
}

pub async fn can_access(s: &PlatformState, id: Uuid, viewer: Option<Uuid>) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM posts p JOIN creators c ON c.id=p.creator_id WHERE p.id=$1 AND p.state='published' AND NOT p.moderation_blocked AND c.verification_status='approved' AND (p.visibility='public' OR c.user_id=$2 OR EXISTS(SELECT 1 FROM entitlements e WHERE e.user_id=$2 AND e.revoked_at IS NULL AND e.starts_at<=now() AND (e.expires_at IS NULL OR e.expires_at>now()) AND (e.post_id=p.id OR (p.visibility='subscribers' AND e.creator_id=p.creator_id)))))").bind(id).bind(viewer).fetch_one(&s.db).await?)
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FeedQuery {
    cursor: Option<String>,
    q: Option<String>,
    category: Option<String>,
    tab: Option<String>,
    creator: Option<Uuid>,
    post: Option<Uuid>,
}
pub async fn feed(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Result<Json<Value>> {
    let u = optional_user(&s, &h).await?;
    let viewer = u.as_ref().map(|u| u.id);
    let (cursor_date, cursor_id) = if let Some(cursor) = query.cursor {
        if cursor.len() > 90 {
            return Err(bad("Invalid feed cursor."));
        }
        let (date, id) = cursor.split_once('|').ok_or(bad("Invalid feed cursor."))?;
        (
            Some(
                date.parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|_| bad("Invalid feed cursor."))?,
            ),
            Some(Uuid::parse_str(id).map_err(|_| bad("Invalid feed cursor."))?),
        )
    } else {
        (None, None)
    };
    let term = query
        .q
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    if term.as_ref().is_some_and(|v| v.len() > 100) {
        return Err(bad("Search is limited to 100 bytes."));
    }
    let pattern = term.map(|v| {
        format!(
            "%{}%",
            v.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let category = query.category.filter(|v| v != "All");
    if category.as_ref().is_some_and(|v| {
        !["Travel", "Food", "Art", "Fitness", "Lifestyle", "Music"].contains(&v.as_str())
    }) {
        return Err(bad("Invalid creator category."));
    }
    let tab = query.tab.filter(|v| v != "all");
    if tab
        .as_ref()
        .is_some_and(|v| !["following", "saved"].contains(&v.as_str()))
    {
        return Err(bad("Invalid feed tab."));
    }
    let mut posts:Value=sqlx::query_scalar(r#"
      SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM (
        SELECT p.id,p.creator_id,p.caption,p.published_at AS created_at,p.visibility,p.purchase_price_minor AS price_minor,
          CASE WHEN access.allowed THEN COALESCE('/api/platform/media/'||m.asset_id||'/full',p.sample_media_url) END AS media_url,
          COALESCE('/api/platform/media/'||m.asset_id||'/preview',p.sample_preview_url) AS preview_url,
          NOT access.allowed AS locked,
          CASE WHEN m.content_type='video/mp4' THEN 'video' ELSE 'image' END AS media_type,
          CASE WHEN access.allowed AND m.content_type='video/mp4' THEN '/api/platform/media/'||m.asset_id||'/captions' END AS captions_url,
          CASE WHEN access.allowed AND m.hls_ready THEN '/api/platform/media/'||m.asset_id||'/hls/'||m.asset_id||'.m3u8' END AS hls_url,
          (SELECT count(*) FROM likes WHERE post_id=p.id) AS like_count,
          (SELECT count(*) FROM comments WHERE post_id=p.id) AS comment_count,
          EXISTS(SELECT 1 FROM likes WHERE user_id=$1 AND post_id=p.id) AS liked,
          EXISTS(SELECT 1 FROM bookmarks WHERE user_id=$1 AND post_id=p.id) AS saved
        FROM posts p JOIN creators c ON c.id=p.creator_id
        LEFT JOIN LATERAL(SELECT pm.asset_id,m.content_type,m.hls_ready FROM post_media pm JOIN media_assets m ON m.id=pm.asset_id WHERE pm.post_id=p.id AND m.state='approved' ORDER BY pm.position LIMIT 1)m ON true
        CROSS JOIN LATERAL(SELECT p.visibility='public' OR COALESCE(c.user_id=$1,false) OR EXISTS(
          SELECT 1 FROM entitlements e WHERE e.user_id=$1 AND e.revoked_at IS NULL AND e.starts_at<=now()
          AND (e.expires_at IS NULL OR e.expires_at>now()) AND (e.post_id=p.id OR (p.visibility='subscribers' AND e.creator_id=p.creator_id))) AS allowed)access
        WHERE p.state='published' AND NOT p.moderation_blocked AND c.verification_status='approved'
        AND EXISTS(SELECT 1 FROM app_users author WHERE author.id=c.user_id AND author.disabled_at IS NULL)
        AND NOT EXISTS(SELECT 1 FROM user_blocks b WHERE (b.user_id=$1 AND b.blocked_user_id=c.user_id) OR (b.user_id=c.user_id AND b.blocked_user_id=$1))
        AND ($2::timestamptz IS NULL OR (p.published_at,p.id)<($2,$3))
        AND ($4::text IS NULL OR p.caption ILIKE $4 OR c.display_name ILIKE $4 OR c.handle ILIKE $4)
        AND ($5::text IS NULL OR c.category=$5)
        AND ($6::text IS NULL OR ($6='saved' AND EXISTS(SELECT 1 FROM bookmarks b WHERE b.user_id=$1 AND b.post_id=p.id)) OR ($6='following' AND EXISTS(SELECT 1 FROM entitlements e WHERE e.user_id=$1 AND e.creator_id=c.id AND e.revoked_at IS NULL AND e.starts_at<=now() AND (e.expires_at IS NULL OR e.expires_at>now()))))
        AND ($7::uuid IS NULL OR c.id=$7)
        AND ($8::uuid IS NULL OR p.id=$8)
        ORDER BY p.published_at DESC,p.id DESC LIMIT 51
      )x
    "#).bind(viewer).bind(cursor_date).bind(cursor_id).bind(pattern).bind(category).bind(tab).bind(query.creator).bind(query.post).fetch_one(&s.db).await?;
    let rows = posts.as_array_mut().ok_or(bad("Invalid feed result."))?;
    let has_more = rows.len() > 50;
    if has_more {
        rows.truncate(50)
    }
    let next_cursor = if has_more {
        rows.last().and_then(|p| {
            Some(format!(
                "{}|{}",
                p["created_at"].as_str()?,
                p["id"].as_str()?
            ))
        })
    } else {
        None
    };
    let referenced: Vec<Uuid> = rows
        .iter()
        .filter_map(|p| {
            p["creator_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
        })
        .collect();
    let creators:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(x),'[]') FROM(SELECT jsonb_build_object('id',c.id,'user_id',c.user_id,'handle',c.handle,'display_name',c.display_name,'category',c.category,'bio',c.bio,'avatar_url',c.avatar_url,'cover_url',c.cover_url,'verification_status',c.verification_status,'subscription_price_minor',c.subscription_price_minor,'currency',c.currency,'subscriber_count',(SELECT count(*) FROM subscriptions WHERE creator_id=c.id AND status='active'),'subscribed',EXISTS(SELECT 1 FROM entitlements e WHERE e.user_id=$1 AND e.creator_id=c.id AND e.revoked_at IS NULL AND (e.expires_at IS NULL OR e.expires_at>now()))) x FROM creators c WHERE c.verification_status='approved' AND EXISTS(SELECT 1 FROM app_users author WHERE author.id=c.user_id AND author.disabled_at IS NULL) AND NOT EXISTS(SELECT 1 FROM user_blocks b WHERE (b.user_id=$1 AND b.blocked_user_id=c.user_id) OR (b.user_id=c.user_id AND b.blocked_user_id=$1)) AND(c.id=ANY($2) OR c.id IN(SELECT id FROM creators WHERE verification_status='approved' ORDER BY created_at LIMIT 50)) ORDER BY (c.id=ANY($2)) DESC,c.created_at LIMIT 100)t").bind(viewer).bind(&referenced).fetch_one(&s.db).await?;
    let viewer=u.map(|u|json!(u)).unwrap_or(json!({"id":"","email":"","display_name":"Guest","avatar_url":"","role":"guest","email_verified":false,"mfa_enabled":false}));
    Ok(Json(
        json!({"viewer":viewer,"creators":creators,"posts":posts,"next_cursor":next_cursor,"mode":s.runtime.mode()}),
    ))
}
/// Private, owner-scoped progress. Provider identifiers and moderation snapshots
/// are deliberately excluded from this response.
pub async fn creator_onboarding(s: &PlatformState, owner: Option<Uuid>) -> Result<Option<Value>> {
    Ok(sqlx::query_scalar(r#"
        SELECT jsonb_build_object(
            'viewer_id',u.id,'email_verified',u.email_verified,
            'mfa_enabled',u.mfa_enabled,'mfa_recovery_required',u.mfa_recovery_required,
            'identity',jsonb_build_object('status',COALESCE(i.status,'not_started'),'checked_at',i.checked_at),
            'payments',jsonb_build_object('connected',p.user_id IS NOT NULL,
                'charges_enabled',COALESCE(p.charges_enabled,false),
                'payouts_enabled',COALESCE(p.payouts_enabled,false),'checked_at',p.checked_at),
            'review',jsonb_build_object('status',c.verification_status,'reason',c.review_message,
                'reviewed_at',c.reviewed_at,'resubmission_available',c.verification_status='rejected'))
        FROM creators c JOIN app_users u ON u.id=c.user_id
        LEFT JOIN provider_accounts p ON p.user_id=u.id
        LEFT JOIN identity_checks i ON i.creator_id=c.id
        WHERE c.user_id=$1 AND u.disabled_at IS NULL
    "#).bind(owner).fetch_optional(&s.db).await?)
}
pub async fn studio(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["posts_cursor", "media_cursor"])?;
    let u = user(&s, &h).await?;
    let creator: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM creators c WHERE user_id=$1")
            .bind(u.id)
            .fetch_optional(&s.db)
            .await?;
    let onboarding = creator_onboarding(&s, Some(u.id)).await?;
    let (posts, posts_next) = pagination::read(&s.db, "SELECT p.id,p.caption,p.state,p.visibility,p.purchase_price_minor AS price_minor,p.scheduled_at,p.created_at,(SELECT asset_id FROM post_media WHERE post_id=p.id ORDER BY position LIMIT 1) AS media_id FROM posts p JOIN creators c ON c.id=p.creator_id WHERE c.user_id=$1", Some(u.id), q.cursor("posts_cursor")?, "created_at", false).await?;
    let (media, media_next) = pagination::read(&s.db, "SELECT m.id,m.state,m.content_type,m.captions_vtt IS NOT NULL AS captions_ready,CASE WHEN m.hls_ready THEN '/api/platform/media/'||m.id||'/hls/'||m.id||'.m3u8' END AS hls_url,CASE WHEN m.captions_vtt IS NOT NULL THEN '/api/platform/media/'||m.id||'/captions' END AS captions_url,'/api/platform/media/'||m.id||'/preview' AS preview_url,m.rejection_reason,m.created_at FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$1", Some(u.id), q.cursor("media_cursor")?, "created_at", false).await?;
    Ok(Json(
        json!({"next_cursors":{"posts":posts_next,"media":media_next},"creator":creator,"onboarding":onboarding,"posts":posts,"media":media}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    handle: String,
    display_name: String,
    bio: String,
    category: String,
    subscription_price_minor: i64,
}
pub async fn profile(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<Profile>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !(3..=30).contains(&b.handle.len())
        || !b
            .handle
            .bytes()
            .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'_')
    {
        return Err(bad(
            "Use a lowercase handle with 3–30 letters, numbers, or underscores.",
        ));
    }
    if b.display_name.trim().is_empty()
        || b.display_name.len() > 100
        || b.bio.len() > 2000
        || !matches!(
            b.category.as_str(),
            "Travel" | "Food" | "Art" | "Fitness" | "Lifestyle" | "Music"
        )
        || !(100..=100000).contains(&b.subscription_price_minor)
    {
        return Err(bad("Check your profile fields and price ($1–$1,000)."));
    }
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let id:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,display_name,bio,category,subscription_price_minor,currency) VALUES($1,$2,$3,$4,$5,$6,'USD') ON CONFLICT(user_id) DO UPDATE SET handle=EXCLUDED.handle,display_name=EXCLUDED.display_name,bio=EXCLUDED.bio,category=EXCLUDED.category,subscription_price_minor=EXCLUDED.subscription_price_minor RETURNING id").bind(u.id).bind(b.handle).bind(b.display_name.trim()).bind(b.bio).bind(b.category).bind(b.subscription_price_minor).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE app_users SET role='creator' WHERE id=$1 AND role='fan'")
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, u.id, "creator_profile_updated", "creator", id).await?;
    tx.commit().await?;
    studio(State(s), h, Query(pagination::PageQuery::default())).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorResubmission {
    reason: String,
}
pub async fn resubmit_creator(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<CreatorResubmission>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !u.email_verified {
        return Err(bad(
            "Verify your email before requesting another creator review.",
        ));
    }
    mfa_session(&s, &h).await?;
    let reason = b.reason.trim();
    if !(5..=2000).contains(&reason.chars().count())
        || reason
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(bad("Explain the changes in 5 to 2,000 characters."));
    }
    rate_limit(&s, &format!("creator-resubmit:{}", u.id), 3).await?;
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::RecentMfa).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    let (id, status): (Uuid, String) =
        sqlx::query_as("SELECT id,verification_status FROM creators WHERE user_id=$1 FOR UPDATE")
            .bind(u.id)
            .fetch_one(&mut *tx)
            .await?;
    if status == "pending" {
        tx.commit().await?;
        return Ok(Json(json!({"status":"pending"})));
    }
    if status != "rejected" {
        return Err(bad("Only a rejected creator profile can be resubmitted."));
    }
    maintenance::evidence(&s, &mut tx, u.id, "creator", id, "resubmit", reason).await?;
    sqlx::query("UPDATE creators SET verification_status='pending',verified_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, u.id, "creator.review.resubmitted", "creator", id).await?;
    notify(&mut tx, u.id, "Creator review requested", "Your updated profile is awaiting platform review. Identity and payment verification remain separate requirements.").await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"pending"})))
}
pub async fn upload(
    State(s): State<PlatformState>,
    h: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    rate_limit(&s, &format!("upload:{}", u.id), 20).await?;
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE user_id=$1")
        .bind(u.id)
        .fetch_optional(&s.db)
        .await?
        .ok_or(bad("Create your creator profile first."))?;
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM media_assets WHERE creator_id=$1 AND created_at>now()-interval '1 day'").bind(creator).fetch_one(&s.db).await?;
    if count >= 200 {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Daily upload limit reached.",
        ));
    }
    let field = multipart
        .next_field()
        .await
        .map_err(|_| bad("Invalid upload."))?
        .ok_or(bad("Choose an image."))?;
    if field.name() != Some("file") {
        return Err(bad("Send a file field."));
    }
    let bytes = field
        .bytes()
        .await
        .map_err(|_| bad("Image upload is too large or invalid."))?;
    if bytes.is_empty() || bytes.len() > 10 * 1024 * 1024 {
        return Err(bad("Upload an image up to 10 MB."));
    }
    let video = (bytes.len() >= 12 && &bytes[4..8] == b"ftyp")
        || bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]);
    if !video {
        let format = image::guess_format(&bytes)
            .map_err(|_| bad("Upload JPEG, PNG, WebP, MP4, or WebM."))?;
        if !matches!(
            format,
            image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP
        ) {
            return Err(bad("Upload JPEG, PNG, WebP, MP4, or WebM."));
        }
    }
    if video
        && tokio::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .is_err()
    {
        return Err(bad("Video processing is not configured."));
    }
    let id = Uuid::new_v4();
    let key = format!("{id}.upload");
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    sqlx::query("INSERT INTO media_assets(id,creator_id,storage_key,state,size_bytes,content_type) VALUES($1,$2,$3,'processing',$4,$5)").bind(id).bind(creator).bind(&key).bind(bytes.len() as i64).bind(if video{"video/mp4"}else{"application/octet-stream"}).execute(&mut *tx).await?;
    audit(&mut tx, u.id, "media_upload_started", "media", id).await?;
    tx.commit().await?;
    // Commit ownership before the external write. A crash after PUT is recovered
    // by the worker's missing-job sweep; no uploaded object loses its database owner.
    // Hold the account lock through this bounded PUT and job enqueue so ordinary
    // erasure cannot delete the key and then be overtaken by this upload. Durable
    // ownership remains necessary for interrupted or indeterminate remote writes.
    let mut tx = s.db.begin().await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='45s'")
        .execute(&mut *tx)
        .await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    s.storage
        .put(&key, bytes.to_vec(), "application/octet-stream")
        .await?;
    sqlx::query("INSERT INTO jobs(kind,payload) VALUES('process_media',$1) ON CONFLICT DO NOTHING")
        .bind(json!({"media_id":id}))
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, u.id, "media_uploaded", "media", id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"media":{"id":id,"state":"processing","preview_url":format!("/api/platform/media/{id}/preview")}}),
    ))
}
struct MediaAccess {
    key: String,
    preview: Option<String>,
    content_type: String,
    hls_ready: bool,
}
async fn media_access(
    s: &PlatformState,
    h: &HeaderMap,
    id: Uuid,
    preview_request: bool,
) -> Result<MediaAccess> {
    let u = optional_user(s, h).await?;
    let viewer = u.as_ref().map(|u| u.id);
    let (owner,state,key,preview,content_type,hls_ready):(Uuid,String,String,Option<String>,String,bool)=sqlx::query_as("SELECT c.user_id,m.state,m.storage_key,m.preview_key,m.content_type,m.hls_ready FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE m.id=$1").bind(id).fetch_one(&s.db).await?;
    let privileged = viewer == Some(owner) || admin(s, h).await.is_ok();
    let post:Option<Uuid>=sqlx::query_scalar("SELECT p.id FROM post_media pm JOIN posts p ON p.id=pm.post_id WHERE pm.asset_id=$1 AND p.state='published' AND NOT p.moderation_blocked AND EXISTS(SELECT 1 FROM creators c WHERE c.id=p.creator_id AND c.verification_status='approved') LIMIT 1").bind(id).fetch_optional(&s.db).await?;
    let allowed = if privileged {
        true
    } else if state != "approved" {
        false
    } else if preview_request {
        post.is_some()
    } else if let Some(post) = post {
        can_access(s, post, viewer).await?
    } else {
        false
    };
    if !allowed {
        return Err(forbidden());
    }
    if !matches!(state.as_str(), "approved" | "quarantine") {
        return Err(bad("Media is still processing."));
    }
    Ok(MediaAccess {
        key,
        preview,
        content_type,
        hls_ready,
    })
}
pub async fn media(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path((id, variant)): Path<(Uuid, String)>,
) -> Result<Response> {
    if !matches!(variant.as_str(), "full" | "preview" | "captions") {
        return Err(bad("Invalid media variant."));
    }
    let access = media_access(&s, &h, id, variant == "preview").await?;
    let MediaAccess {
        key,
        preview,
        content_type,
        ..
    } = access;
    if variant == "captions" {
        let captions: Option<String> =
            sqlx::query_scalar("SELECT captions_vtt FROM media_assets WHERE id=$1")
                .bind(id)
                .fetch_one(&s.db)
                .await?;
        return Ok((
            [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
            captions.ok_or(Error(StatusCode::NOT_FOUND, "Captions not available."))?,
        )
            .into_response());
    }
    let key = if variant == "preview" {
        preview.ok_or(bad("Preview not ready."))?
    } else {
        key
    };
    let content_type = if variant == "preview" {
        "image/jpeg"
    } else {
        content_type.as_str()
    };
    stream_object(&s, &h, &key, content_type).await
}
async fn stream_object(
    s: &PlatformState,
    h: &HeaderMap,
    key: &str,
    content_type: &str,
) -> Result<Response> {
    let length = s.storage.size(key).await?;
    let mut status = StatusCode::OK;
    let mut offset = 0;
    let mut count = length;
    let mut headers = HeaderMap::new();
    if let Some(range) = h.get(header::RANGE) {
        let value = range
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("bytes="))
            .filter(|v| !v.contains(','))
            .ok_or(Error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "Unsupported byte range.",
            ))?;
        let (start, end) = value.split_once('-').ok_or(Error(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "Invalid byte range.",
        ))?;
        let (start, end) = if start.is_empty() {
            let suffix = end.parse::<usize>().ok().filter(|v| *v > 0).ok_or(Error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "Invalid byte range.",
            ))?;
            (length.saturating_sub(suffix), length.saturating_sub(1))
        } else {
            let start = start
                .parse::<usize>()
                .map_err(|_| bad("Invalid byte range."))?;
            let end = if end.is_empty() {
                length.saturating_sub(1)
            } else {
                end.parse::<usize>()
                    .map_err(|_| bad("Invalid byte range."))?
                    .min(length.saturating_sub(1))
            };
            (start, end)
        };
        if start >= length || end < start {
            return Err(Error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "Invalid byte range.",
            ));
        }
        offset = start;
        count = end - start + 1;
        status = StatusCode::PARTIAL_CONTENT;
        headers.insert(
            header::CONTENT_RANGE,
            header::HeaderValue::from_str(&format!("bytes {start}-{end}/{length}"))
                .map_err(|_| bad("Invalid byte range."))?,
        );
    }
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(content_type).map_err(|_| bad("Invalid media type."))?,
    );
    headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("inline"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&count.to_string())
            .map_err(|_| bad("Invalid object size."))?,
    );
    Ok((status, headers, s.storage.stream(key, offset, count).await?).into_response())
}
pub async fn hls(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path((id, filename)): Path<(Uuid, String)>,
) -> Result<Response> {
    let suffix = filename
        .strip_prefix(&format!("{id}."))
        .ok_or(bad("Invalid streaming resource."))?;
    let playlist = matches!(suffix, "m3u8" | "720p.m3u8" | "360p.m3u8");
    let segment = ["720p-", "360p-"].iter().any(|prefix| {
        suffix
            .strip_prefix(prefix)
            .and_then(|v| v.strip_suffix(".ts"))
            .is_some_and(|v| {
                v.len() == 5
                    && v.bytes().all(|b| b.is_ascii_digit())
                    && v.parse::<u32>().is_ok_and(|n| n <= 151)
            })
    });
    if !playlist && !segment {
        return Err(bad("Invalid streaming resource."));
    }
    let access = media_access(&s, &h, id, false).await?;
    if !access.hls_ready {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Adaptive video is unavailable.",
        ));
    }
    stream_object(
        &s,
        &h,
        &filename,
        if playlist {
            "application/vnd.apple.mpegurl"
        } else {
            "video/mp2t"
        },
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostInput {
    caption: String,
    visibility: String,
    price_minor: Option<i64>,
    media_id: Option<Uuid>,
    state: String,
    scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
}
fn validate_post(b: &PostInput) -> Result<()> {
    if b.caption.trim().is_empty()
        || b.caption.len() > 10000
        || !matches!(b.visibility.as_str(), "public" | "subscribers" | "purchase")
        || !matches!(b.state.as_str(), "draft" | "scheduled" | "published")
    {
        return Err(bad("Check the post fields."));
    }
    if b.visibility == "purchase" && !b.price_minor.is_some_and(|p| (100..=1000000).contains(&p)) {
        return Err(bad("Set a paid-post price between $1 and $10,000."));
    }
    if b.state == "scheduled" && !b.scheduled_at.is_some_and(|d| d > chrono::Utc::now()) {
        return Err(bad("Choose a future publication time."));
    }
    Ok(())
}
pub async fn create_post(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<PostInput>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    validate_post(&b)?;
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let (creator, status): (Uuid, String) =
        sqlx::query_as("SELECT id,verification_status FROM creators WHERE user_id=$1 FOR SHARE")
            .bind(u.id)
            .fetch_one(&mut *tx)
            .await?;
    if b.state != "draft" && (status != "approved" || !u.email_verified) {
        return Err(bad(
            "Verify your email and complete creator review before publishing.",
        ));
    }
    if let Some(media) = b.media_id {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_assets WHERE id=$1 AND creator_id=$2 AND state='approved' AND (content_type<>'video/mp4' OR captions_vtt IS NOT NULL))").bind(media).bind(creator).fetch_one(&mut *tx).await?;
        if !valid {
            return Err(bad("Choose an approved image you own."));
        }
    }
    let price = if b.visibility == "purchase" {
        b.price_minor
    } else {
        None
    };
    let currency = price.map(|_| "USD");
    let id:Uuid=sqlx::query_scalar("INSERT INTO posts(creator_id,caption,visibility,purchase_price_minor,currency,state,published_at,scheduled_at) VALUES($1,$2,$3,$4,$5,$6,CASE WHEN $6='published' THEN now() ELSE NULL END,$7) RETURNING id").bind(creator).bind(b.caption.trim()).bind(&b.visibility).bind(price).bind(currency).bind(&b.state).bind(b.scheduled_at).fetch_one(&mut *tx).await?;
    if let Some(media) = b.media_id {
        sqlx::query(
            "INSERT INTO post_media(post_id,asset_id,creator_id,position) VALUES($1,$2,$3,0)",
        )
        .bind(id)
        .bind(media)
        .bind(creator)
        .execute(&mut *tx)
        .await?;
    }
    if b.state == "scheduled" {
        sqlx::query("INSERT INTO jobs(kind,payload,run_at) VALUES('publish',$1,$2)")
            .bind(json!({"post_id":id}))
            .bind(b.scheduled_at)
            .execute(&mut *tx)
            .await?;
    }
    audit(&mut tx, u.id, "post_created", "post", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"post":{"id":id,"state":b.state}})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostUpdate {
    state: String,
    caption: Option<String>,
    visibility: Option<String>,
    price_minor: Option<i64>,
}
pub async fn update_post(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<PostUpdate>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !matches!(b.state.as_str(), "draft" | "published" | "removed") {
        return Err(bad("Choose draft, published, or removed."));
    }
    if b.caption
        .as_ref()
        .is_some_and(|v| v.trim().is_empty() || v.len() > 10000)
    {
        return Err(bad("Invalid caption."));
    }
    // Pricing and visibility are immutable after creation to preserve purchase promises.
    if b.visibility.is_some() || b.price_minor.is_some() {
        return Err(bad("Create a new post to change access or pricing."));
    }
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let changed=sqlx::query("UPDATE posts p SET state=$3,caption=COALESCE($4,p.caption),published_at=CASE WHEN $3='published' THEN now() ELSE p.published_at END FROM creators c WHERE p.creator_id=c.id AND p.id=$1 AND c.user_id=$2 AND NOT p.moderation_blocked AND ($3<>'published' OR (c.verification_status='approved' AND $5 AND NOT EXISTS(SELECT 1 FROM post_media pm JOIN media_assets m ON m.id=pm.asset_id WHERE pm.post_id=p.id AND (m.state<>'approved' OR (m.content_type='video/mp4' AND m.captions_vtt IS NULL)))))").bind(id).bind(u.id).bind(&b.state).bind(b.caption).bind(u.email_verified).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(forbidden());
    }
    audit(&mut tx, u.id, "post_updated", "post", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"post":{"id":id,"state":b.state}})))
}
async fn interaction(
    s: PlatformState,
    h: HeaderMap,
    id: Uuid,
    table: &str,
    insert: bool,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let mut tx = s.db.begin().await?;
    if insert {
        super::community::ensure_post_contact(&mut tx, u.id, id).await?;
    }
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let q = match (table, insert) {
        ("likes", true) => {
            "INSERT INTO likes(user_id,post_id) VALUES($1,$2) ON CONFLICT DO NOTHING"
        }
        ("likes", false) => "DELETE FROM likes WHERE user_id=$1 AND post_id=$2",
        ("bookmarks", true) => {
            "INSERT INTO bookmarks(user_id,post_id) VALUES($1,$2) ON CONFLICT DO NOTHING"
        }
        ("bookmarks", false) => "DELETE FROM bookmarks WHERE user_id=$1 AND post_id=$2",
        _ => return Err(bad("Unknown interaction")),
    };
    sqlx::query(q).bind(u.id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"active":insert})))
}
pub async fn like(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    interaction(s, h, id, "likes", true).await
}
pub async fn unlike(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    interaction(s, h, id, "likes", false).await
}
pub async fn save(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    interaction(s, h, id, "bookmarks", true).await
}
pub async fn unsave(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    interaction(s, h, id, "bookmarks", false).await
}
pub async fn comments(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["cursor"])?;
    let u = optional_user(&s, &h).await?;
    let viewer = u.as_ref().map(|u| u.id);
    if !can_access(&s, id, viewer).await? {
        return Err(forbidden());
    }
    let (mut rows, next_cursor) = pagination::read_scoped(&s.db,
        "SELECT c.id,u.display_name,c.body,c.created_at FROM comments c JOIN app_users u ON u.id=c.user_id WHERE c.post_id=$1 AND NOT EXISTS(SELECT 1 FROM user_blocks b WHERE (b.user_id=$2 AND b.blocked_user_id=c.user_id) OR (b.user_id=c.user_id AND b.blocked_user_id=$2))",
        Some(id),q.cursor("cursor")?,"created_at",false,viewer).await?;
    rows.as_array_mut()
        .ok_or(bad("Invalid comments result."))?
        .reverse();
    Ok(Json(json!({"comments":rows,"next_cursor":next_cursor})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    body: String,
}
pub async fn comment(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<Comment>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !can_access(&s, id, Some(u.id)).await? {
        return Err(forbidden());
    }
    if b.body.trim().is_empty() || b.body.len() > 500 {
        return Err(bad("Write a comment up to 500 characters."));
    }
    rate_limit(&s, &format!("comment:{}", u.id), 20).await?;
    let mut tx = s.db.begin().await?;
    super::community::ensure_post_contact(&mut tx, u.id, id).await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    sqlx::query("INSERT INTO comments(user_id,post_id,body) VALUES($1,$2,$3)")
        .bind(u.id)
        .bind(id)
        .bind(b.body.trim())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    comments(
        State(s),
        h,
        Path(id),
        Query(pagination::PageQuery::default()),
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Captions {
    vtt: String,
}
pub async fn captions(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<Captions>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let vtt = validate_captions(&b.vtt)?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let changed=sqlx::query("UPDATE media_assets m SET captions_vtt=$3 FROM creators c WHERE m.creator_id=c.id AND m.id=$1 AND c.user_id=$2 AND m.content_type='video/mp4' AND m.state IN ('processing','quarantine','approved')").bind(id).bind(u.id).bind(vtt).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(forbidden());
    }
    audit(&mut tx, u.id, "media.captions.updated", "media", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"captions_ready":true})))
}

fn caption_time(value: &str) -> Option<u32> {
    let value = value.trim();
    let (h, rest) = value.split_once(':')?;
    let (m, rest) = rest.split_once(':')?;
    let (s, ms) = rest.split_once('.')?;
    if h.len() != 2
        || m.len() != 2
        || s.len() != 2
        || ms.len() != 3
        || ![h, m, s, ms]
            .iter()
            .all(|p| p.bytes().all(|v| v.is_ascii_digit()))
    {
        return None;
    }
    let h = h.parse::<u32>().ok()?;
    let m = m.parse::<u32>().ok()?;
    let s = s.parse::<u32>().ok()?;
    let ms = ms.parse::<u32>().ok()?;
    if h > 0 || m > 5 || s > 59 || ms > 999 {
        return None;
    }
    let value = (h * 3600 + m * 60 + s) * 1000 + ms;
    if value > 300000 {
        return None;
    }
    Some(value)
}
fn validate_captions(value: &str) -> Result<String> {
    let v = value.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    if v.len() > 8192
        || !v.starts_with("WEBVTT\n")
        || v.contains('<')
        || v.contains("STYLE")
        || v.contains("REGION")
    {
        return Err(bad(
            "Use plain WebVTT captions up to 8 KB, without markup or styles.",
        ));
    }
    let mut count = 0;
    for cue in v.split("\n\n").skip(1).filter(|v| !v.trim().is_empty()) {
        let lines: Vec<_> = cue.lines().collect();
        let Some(index) = lines.iter().position(|v| v.contains("-->")) else {
            return Err(bad("Every caption cue needs a timestamp."));
        };
        let Some((start, end)) = lines[index].split_once("-->") else {
            return Err(bad("Invalid caption timing."));
        };
        let start = caption_time(start).ok_or(bad(
            "Use HH:MM:SS.mmm caption timestamps within five minutes.",
        ))?;
        let end = caption_time(end).ok_or(bad(
            "Use HH:MM:SS.mmm caption timestamps within five minutes.",
        ))?;
        if end <= start || lines.iter().skip(index + 1).all(|v| v.trim().is_empty()) {
            return Err(bad("Each caption needs text and an end after its start."));
        }
        count += 1;
    }
    if count == 0 {
        return Err(bad("Add at least one caption cue."));
    }
    Ok(v)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captions_require_valid_timing_and_text() {
        let good = "WEBVTT\r\n\r\n00:00:00.000 --> 00:00:01.000\r\nBlue screen.\r\n";
        assert!(validate_captions(good).is_ok());
        assert!(
            validate_captions("WEBVTT\n\n00:04:59.000 --> 00:05:00.000\nLast second.\n").is_ok()
        );
        for v in [
            "WEBVTT\n",
            "WEBVTT\n\n00:00:02.000 --> 00:00:01.000\nReversed\n",
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n",
            "WEBVTT\n\n00:00:00.000 --> 00:05:00.001\nToo late\n",
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n<b>Markup</b>\n",
        ] {
            assert!(validate_captions(v).is_err(), "{v}");
        }
    }
}
