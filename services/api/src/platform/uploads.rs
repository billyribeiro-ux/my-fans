use super::*;
use axum::{body::Bytes, extract::Path};
use serde::Deserialize;
use sqlx::Row;
use tokio::io::AsyncWriteExt;
const CHUNK: usize = 4 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    size_bytes: i64,
    content_type: String,
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn chunk_key(id: Uuid, offset: i64) -> String {
    let bytes = Sha256::digest(format!("myfans:upload:{id}:{offset}").as_bytes());
    let mut uuid = [0; 16];
    uuid.copy_from_slice(&bytes[..16]);
    format!("{}.chunk", Uuid::from_bytes(uuid))
}
async fn owned(s: &PlatformState, h: &HeaderMap, id: Uuid) -> Result<Uuid> {
    let u = user(s, h).await?;
    let owner: Uuid = sqlx::query_scalar(
        "SELECT c.user_id FROM media_uploads m JOIN creators c ON c.id=m.creator_id WHERE m.id=$1",
    )
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    if owner != u.id {
        return Err(forbidden());
    }
    Ok(u.id)
}
async fn status(s: &PlatformState, id: Uuid) -> Result<Json<Value>> {
    let upload:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'offset',offset_bytes,'size_bytes',size_bytes,'content_type',content_type,'chunk_size',4194304,'state',state,'expires_at',expires_at) FROM media_uploads WHERE id=$1").bind(id).fetch_one(&s.db).await?;
    Ok(Json(json!({"upload":upload})))
}
pub async fn start(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<Start>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !u.email_verified {
        return Err(bad("Verify your email before uploading."));
    }
    if ![
        "image/jpeg",
        "image/png",
        "image/webp",
        "video/mp4",
        "video/webm",
    ]
    .contains(&b.content_type.as_str())
        || b.size_bytes < 1
        || b.size_bytes > 256 * 1024 * 1024
        || b.content_type.starts_with("image/") && b.size_bytes > 10 * 1024 * 1024
    {
        return Err(bad("Upload images up to 10 MiB or videos up to 256 MiB."));
    }
    rate_limit(&s, &format!("upload:{}", u.id), 10).await?;
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE user_id=$1 FOR UPDATE")
        .bind(u.id)
        .fetch_one(&mut *tx)
        .await?;
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM media_uploads WHERE creator_id=$1 AND state IN ('uploading','assembling') AND expires_at>now()").bind(creator).fetch_one(&mut *tx).await?;
    if active >= 3 {
        return Err(bad(
            "Finish or cancel an existing upload before starting another.",
        ));
    }
    let id:Uuid=sqlx::query_scalar("INSERT INTO media_uploads(creator_id,size_bytes,content_type) VALUES($1,$2,$3) RETURNING id").bind(creator).bind(b.size_bytes).bind(b.content_type).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    status(&s, id).await
}
pub async fn get_upload(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    owned(&s, &h, id).await?;
    status(&s, id).await
}
pub async fn chunk(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    bytes: Bytes,
) -> Result<Json<Value>> {
    let owner = owned(&s, &h, id).await?;
    rate_limit(&s, &format!("upload-chunk:{owner}"), 120).await?;
    if h.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) != Some("application/octet-stream")
        || bytes.is_empty()
        || bytes.len() > CHUNK
    {
        return Err(bad("Send a binary upload chunk up to 4 MiB."));
    }
    let offset: i64 = h
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .filter(|v| *v >= 0)
        .ok_or(bad("Upload-Offset is required."))?;
    let hash = sha(&bytes);
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, owner, Assurance::Member).await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='45s'")
        .execute(&mut *tx)
        .await?;
    let row=sqlx::query("SELECT offset_bytes,size_bytes,state,expires_at>now() AS valid FROM media_uploads WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    if row.get::<String, _>("state") != "uploading" || !row.get::<bool, _>("valid") {
        return Err(bad("This upload is no longer accepting chunks."));
    }
    let current: i64 = row.get("offset_bytes");
    let total: i64 = row.get("size_bytes");
    if offset < current {
        let old:Option<(String,i32)>=sqlx::query_as("SELECT sha256,size_bytes FROM media_upload_chunks WHERE upload_id=$1 AND offset_bytes=$2").bind(id).bind(offset).fetch_optional(&mut *tx).await?;
        if old == Some((hash, bytes.len() as i32)) {
            tx.commit().await?;
            return status(&s, id).await;
        }
        return Err(Error(
            StatusCode::CONFLICT,
            "The retry does not match the stored chunk.",
        ));
    }
    if offset != current || bytes.len() as i64 != (total - current).min(CHUNK as i64) {
        return Err(Error(
            StatusCode::CONFLICT,
            "Chunk offset or size does not match the upload.",
        ));
    }
    // The committed upload UUID and deterministic offset key own even an interrupted PUT.
    let key = chunk_key(id, offset);
    s.storage
        .put(&key, bytes.to_vec(), "application/octet-stream")
        .await?;
    sqlx::query("INSERT INTO media_upload_chunks(upload_id,offset_bytes,size_bytes,storage_key,sha256) VALUES($1,$2,$3,$4,$5)").bind(id).bind(offset).bind(bytes.len() as i32).bind(key).bind(hash).execute(&mut *tx).await?;
    sqlx::query("UPDATE media_uploads SET offset_bytes=offset_bytes+$2 WHERE id=$1")
        .bind(id)
        .bind(bytes.len() as i64)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    status(&s, id).await
}
pub async fn complete(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let owner = owned(&s, &h, id).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, owner, Assurance::Member).await?;
    let row=sqlx::query("SELECT creator_id,offset_bytes,size_bytes,content_type,state,expires_at>now() AS valid FROM media_uploads WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let state: String = row.get("state");
    if state == "cancelled" || state == "uploading" && !row.get::<bool, _>("valid") {
        return Err(bad("This upload has expired or was cancelled."));
    }
    if state == "uploading" {
        if row.get::<i64, _>("offset_bytes") != row.get::<i64, _>("size_bytes") {
            return Err(bad("Finish sending the upload before completing it."));
        }
        sqlx::query("INSERT INTO media_assets(id,creator_id,storage_key,state,size_bytes,content_type) VALUES($1,$2,$3,'processing',$4,$5)").bind(id).bind(row.get::<Uuid,_>("creator_id")).bind(format!("{id}.upload")).bind(row.get::<i64,_>("size_bytes")).bind(if row.get::<String,_>("content_type").starts_with("video/") {"video/mp4"} else {"application/octet-stream"}).execute(&mut *tx).await?;
        sqlx::query("UPDATE media_uploads SET state='assembling' WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO jobs(kind,payload) VALUES('assemble_upload',$1) ON CONFLICT DO NOTHING",
        )
        .bind(json!({"upload_id":id}))
        .execute(&mut *tx)
        .await?;
        audit(&mut tx, owner, "media.upload.completed", "media", id).await?;
    }
    tx.commit().await?;
    let asset_state: String = sqlx::query_scalar("SELECT state FROM media_assets WHERE id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    Ok(Json(
        json!({"media":{"id":id,"state":asset_state,"preview_url":format!("/api/platform/media/{id}/preview")}}),
    ))
}
pub async fn cancel(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let owner = owned(&s, &h, id).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, owner, Assurance::Member).await?;
    let changed=sqlx::query("UPDATE media_uploads SET state='cancelled' WHERE id=$1 AND state IN ('uploading','cancelled')").bind(id).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(bad("A completed upload cannot be cancelled."));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn assemble(s: &PlatformState, id: Uuid) -> Result<()> {
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_uploads m JOIN creators c ON c.id=m.creator_id JOIN app_users u ON u.id=c.user_id WHERE m.id=$1 AND u.disabled_at IS NULL)").bind(id).fetch_one(&s.db).await?;
    if !active {
        sqlx::query(
            "UPDATE media_uploads SET state='cancelled' WHERE id=$1 AND state='assembling'",
        )
        .bind(id)
        .execute(&s.db)
        .await?;
        sqlx::query("UPDATE media_assets SET state='rejected',rejection_reason='Account is inactive' WHERE id=$1 AND state='processing'").bind(id).execute(&s.db).await?;
        return cleanup_upload(s, id).await;
    }
    let row = sqlx::query("SELECT state,size_bytes FROM media_uploads WHERE id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    if row.get::<String, _>("state") == "complete" {
        return cleanup(s, id, row.get("size_bytes")).await;
    }
    if row.get::<String, _>("state") != "assembling" {
        return Ok(());
    }
    let scratch = workdir::WorkDir::create(&s.media_root)?;
    let work = scratch.path();
    let result=async {
        let path=work.join("source.upload");let mut file=tokio::fs::File::create(&path).await.map_err(|_|providers::unavailable())?;
        let rows:Vec<(i64,i32,String,String)>=sqlx::query_as("SELECT offset_bytes,size_bytes,storage_key,sha256 FROM media_upload_chunks WHERE upload_id=$1 ORDER BY offset_bytes LIMIT 64").bind(id).fetch_all(&s.db).await?;
        let mut position=0;
        for (offset,size,key,hash) in rows {
            let bytes=s.storage.read(&key,CHUNK).await?;
            if offset!=position || size as usize!=bytes.len() || sha(&bytes)!=hash {return Err(bad("Upload integrity verification failed."));}
            file.write_all(&bytes).await.map_err(|_|providers::unavailable())?;position+=i64::from(size);
        }
        if position!=row.get::<i64,_>("size_bytes") {return Err(bad("Upload is missing chunks."));}
        file.sync_all().await.map_err(|_|providers::unavailable())?;drop(file);
        s.storage.put_file(&format!("{id}.upload"),&path,"application/octet-stream").await?;
        let mut tx=s.db.begin().await?;
        sqlx::query("UPDATE media_uploads SET state='complete' WHERE id=$1 AND state='assembling'").bind(id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO jobs(kind,payload) VALUES('process_media',$1) ON CONFLICT DO NOTHING").bind(json!({"media_id":id})).execute(&mut *tx).await?;
        tx.commit().await?;cleanup(s,id,position).await
    }.await;
    let _ = tokio::fs::remove_dir_all(work).await;
    result
}
async fn cleanup(s: &PlatformState, id: Uuid, total: i64) -> Result<()> {
    for offset in (0..total).step_by(CHUNK) {
        s.storage.delete(&chunk_key(id, offset)).await?;
    }
    sqlx::query("UPDATE media_uploads SET cleaned_at=now() WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await?;
    Ok(())
}
pub async fn expire(s: &PlatformState) -> Result<()> {
    sqlx::query(
        "UPDATE media_uploads SET state='cancelled' WHERE state='uploading' AND expires_at<=now()",
    )
    .execute(&s.db)
    .await?;
    sqlx::query("INSERT INTO jobs(kind,payload) SELECT 'cleanup_upload',jsonb_build_object('upload_id',id) FROM media_uploads WHERE state IN ('cancelled','complete') AND cleaned_at IS NULL AND NOT EXISTS(SELECT 1 FROM jobs WHERE kind='cleanup_upload' AND payload->>'upload_id'=media_uploads.id::text AND status IN ('pending','running','failed')) ORDER BY expires_at LIMIT 2 ON CONFLICT DO NOTHING").execute(&s.db).await?;
    Ok(())
}
pub async fn cleanup_upload(s: &PlatformState, id: Uuid) -> Result<()> {
    let size:i64=sqlx::query_scalar("SELECT LEAST(size_bytes,offset_bytes+4194304) FROM media_uploads WHERE id=$1 AND state IN ('complete','cancelled')").bind(id).fetch_one(&s.db).await?;
    cleanup(s, id, size).await
}

#[cfg(test)]
#[path = "uploads_tests.rs"]
mod tests;
