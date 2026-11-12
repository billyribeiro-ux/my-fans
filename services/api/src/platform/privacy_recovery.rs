//! Owner-only replay of a separately signed, immutable privacy journal.
//! Cloud orchestration verifies its KMS signature; this boundary independently
//! verifies the exact S3 version, SHA-256, source binding and isolated database.
use super::*;
use aws_sdk_s3::{
    config::{BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig},
    primitives::ByteStream,
    types::ServerSideEncryption,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeSet, HashSet};
use tokio::io::AsyncReadExt;

type Outcome<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type CapturedRequest = (Uuid, String, Value, DateTime<Utc>, DateTime<Utc>);
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    source_database_identifier: String,
    source_storage_id: String,
    exported_at: DateTime<Utc>,
    requests: Vec<Request>,
    erased_storage_keys: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: Uuid,
    user_id: Uuid,
    status: String,
    legal_hold: bool,
    retention: privacy::RetentionPolicy,
    privacy_version: String,
    created_at: DateTime<Utc>,
    not_before: DateTime<Utc>,
    application_erased_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}
impl Request {
    fn erased(&self) -> bool {
        matches!(self.status.as_str(), "application_erased" | "completed")
    }
}
fn storage_id(value: &str) -> bool {
    value.strip_prefix("s3:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn media_key(value: &str) -> bool {
    let Some((id, suffix)) = value.split_once('.') else {
        return false;
    };
    Uuid::parse_str(id).is_ok_and(|u| u.to_string() == id)
        && (matches!(
            suffix,
            "upload" | "chunk" | "jpg" | "preview.jpg" | "mp4" | "m3u8" | "720p.m3u8" | "360p.m3u8"
        ) || ["720p-", "360p-"].iter().any(|p| {
            suffix
                .strip_prefix(p)
                .and_then(|s| s.strip_suffix(".ts"))
                .is_some_and(|n| {
                    n.len() == 5
                        && n.bytes().all(|b| b.is_ascii_digit())
                        && n.parse::<u16>().is_ok_and(|v| v <= 151)
                })
        }))
}
fn database_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_lowercase()
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
impl Journal {
    fn validate(&self, database: &str, storage: &str) -> Outcome<()> {
        if self.version != 1
            || self.source_database_identifier != database
            || !database_id(database)
            || self.source_storage_id != storage
            || !storage_id(storage)
            || self.exported_at > Utc::now() + chrono::Duration::minutes(1)
            || self.requests.len() > MAX_RECORDS
            || self.erased_storage_keys.len() > MAX_RECORDS
        {
            return Err("Privacy journal version, source, timestamp or bounds are invalid".into());
        }
        let mut users = HashSet::new();
        let mut ids = HashSet::new();
        for r in &self.requests {
            if !users.insert(r.user_id)
                || !ids.insert(r.id)
                || r.id.is_nil()
                || r.user_id.is_nil()
                || !matches!(
                    r.status.as_str(),
                    "pending" | "approved" | "purging" | "application_erased" | "completed"
                )
                || r.privacy_version.is_empty()
                || r.privacy_version.len() > 100
                || r.privacy_version.chars().any(char::is_control)
                || r.created_at > self.exported_at
                || r.not_before < r.created_at
                || r.erased() != r.application_erased_at.is_some()
                || (r.status == "completed") != r.completed_at.is_some()
                || r.application_erased_at
                    .is_some_and(|t| t < r.created_at || t > self.exported_at)
                || r.completed_at.is_some_and(|t| {
                    t < r.application_erased_at.unwrap_or(r.created_at) || t > self.exported_at
                })
            {
                return Err(
                    "Privacy journal request is malformed, duplicated or inconsistent".into(),
                );
            }
            r.retention.validate()?;
        }
        let mut keys = HashSet::new();
        if self
            .erased_storage_keys
            .iter()
            .any(|k| !media_key(k) || !keys.insert(k))
        {
            return Err("Privacy journal contains an unsafe or duplicate media key".into());
        }
        Ok(())
    }
}

pub async fn require_reconciled(db: &PgPool) -> Outcome<()> {
    let safe: bool =
        sqlx::query_scalar("SELECT status<>'quarantined' FROM platform_recovery WHERE id=1")
            .fetch_one(db)
            .await?;
    if !safe {
        return Err(
            "Restored database is quarantined until signed privacy-journal replay completes".into(),
        );
    }
    Ok(())
}
pub async fn quarantine(
    tx: &mut Transaction<'_, Postgres>,
    source: &str,
    target: &str,
) -> Outcome<()> {
    if !storage_id(source) || !storage_id(target) || source == target {
        return Err("Recovery requires different valid storage identities".into());
    }
    sqlx::query("UPDATE platform_recovery SET status='quarantined',source_storage_id=$1,target_storage_id=$2,quarantined_at=now(),reconciled_at=NULL,journal_sha256=NULL,request_cursor=0,object_cursor=0,frozen_accounts=0,erased_accounts=0 WHERE id=1")
        .bind(source).bind(target).execute(&mut **tx).await?;
    Ok(())
}
async fn owner_exclusive(tx: &mut Transaction<'_, Postgres>) -> Outcome<()> {
    let owner:bool=sqlx::query_scalar("SELECT session_user=current_user AND current_user=pg_get_userbyid(relowner) FROM pg_class WHERE oid='privacy_requests'::regclass").fetch_one(&mut **tx).await?;
    let clients:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND backend_type='client backend' AND pid<>pg_backend_pid()").fetch_one(&mut **tx).await?;
    if !owner || clients != 0 {
        return Err(
            "Privacy maintenance requires the schema owner and no other database clients".into(),
        );
    }
    Ok(())
}
struct ObjectStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
    owner: String,
    region: String,
}
fn required(name: &str) -> Outcome<String> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing {name}").into())
}
impl ObjectStore {
    async fn load() -> Outcome<Self> {
        let bucket = required("PRIVACY_JOURNAL_BUCKET")?;
        let key = required("PRIVACY_JOURNAL_KEY")?;
        let owner = required("S3_EXPECTED_BUCKET_OWNER")?;
        let region = required("AWS_REGION")?;
        let journal_id = key
            .strip_prefix("privacy-journals/privacy-")
            .and_then(|s| s.strip_suffix("/journal.json"));
        if !(3..=63).contains(&bucket.len())
            || !bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !journal_id.is_some_and(|id| Uuid::parse_str(id).is_ok_and(|u| u.to_string() == id))
            || owner.len() != 12
            || !owner.bytes().all(|b| b.is_ascii_digit())
            || region.is_empty()
            || region.len() > 32
            || !region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || ["AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL_S3"]
                .iter()
                .any(|n| std::env::var_os(n).is_some())
        {
            return Err("Invalid privacy journal storage configuration".into());
        }
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.clone()))
            .retry_config(RetryConfig::standard().with_max_attempts(3))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(3))
                    .operation_timeout(Duration::from_secs(60))
                    .operation_attempt_timeout(Duration::from_secs(30))
                    .build(),
            )
            .load()
            .await;
        let client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::config::Builder::from(&config)
                .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
                .build(),
        );
        Ok(Self {
            client,
            bucket,
            key,
            owner,
            region,
        })
    }
}

async fn capture(db: &PgPool, database: &str) -> Outcome<Journal> {
    let mut tx = db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    owner_exclusive(&mut tx).await?;
    let storage: String =
        sqlx::query_scalar("SELECT media_store_identity FROM platform_storage WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'user_id',user_id,'status',status,'legal_hold',legal_hold,'retention',retention,'privacy_version',privacy_version,'created_at',created_at,'not_before',not_before,'application_erased_at',application_erased_at,'completed_at',completed_at) FROM privacy_requests ORDER BY id LIMIT 100001").fetch_all(&mut *tx).await?;
    if rows.len() > MAX_RECORDS {
        return Err(
            "Privacy journal request bound exceeded; split recovery scope before exporting".into(),
        );
    }
    let requests = rows
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<Vec<Request>, _>>()?;
    let keys:Vec<String>=sqlx::query_scalar("SELECT storage_key FROM privacy_objects o JOIN privacy_requests r ON r.id=o.request_id WHERE r.status IN ('application_erased','completed') UNION SELECT x.storage_key FROM media_upload_chunks x JOIN media_uploads m ON m.id=x.upload_id JOIN creators c ON c.id=m.creator_id JOIN privacy_requests r ON r.user_id=c.user_id WHERE r.status IN ('application_erased','completed') LIMIT 100001").fetch_all(&mut *tx).await?;
    let mut keys: BTreeSet<String> = keys.into_iter().collect();
    let assets:Vec<(Uuid,String)>=sqlx::query_as("SELECT m.id,m.content_type FROM media_assets m JOIN creators c ON c.id=m.creator_id JOIN privacy_requests r ON r.user_id=c.user_id WHERE r.status IN ('application_erased','completed') ORDER BY m.id LIMIT 100001").fetch_all(&mut *tx).await?;
    for (id, kind) in assets {
        keys.extend(privacy::asset_keys(id, kind.starts_with("video/")));
        if keys.len() > MAX_RECORDS {
            return Err(
                "Privacy journal object bound exceeded; split recovery scope before exporting"
                    .into(),
            );
        }
    }
    let exported_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    let journal = Journal {
        version: 1,
        source_database_identifier: database.to_owned(),
        source_storage_id: storage.clone(),
        exported_at,
        requests,
        erased_storage_keys: keys.into_iter().collect(),
    };
    journal.validate(database, &storage)?;
    tx.commit().await?;
    Ok(journal)
}
pub async fn export(db: &PgPool) -> Outcome<()> {
    let database = required("PRIVACY_SOURCE_DATABASE_ID")?;
    let options = config::database_options().await?;
    if !database_id(&database)
        || options.get_username() != "myfans_owner"
        || !options.get_host().starts_with(&format!("{database}."))
    {
        return Err("Privacy export requires the exact source RDS database and owner login".into());
    }
    let journal = capture(db, &database).await?;
    let bytes = serde_json::to_vec(&journal)?;
    if bytes.len() > MAX_BYTES {
        return Err("Privacy journal exceeds the 32 MiB bound".into());
    }
    let sha = digest_bytes(&bytes);
    let store = ObjectStore::load().await?;
    let kms = required("PRIVACY_JOURNAL_KMS_KEY_ID")?;
    let key_prefix = format!("arn:aws:kms:{}:{}:key/", store.region, store.owner);
    if !kms
        .strip_prefix(&key_prefix)
        .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|u| u.to_string() == id))
    {
        return Err(
            "Privacy journal requires an exact same-account, same-region KMS key ARN".into(),
        );
    }
    let response = store
        .client
        .put_object()
        .bucket(&store.bucket)
        .key(&store.key)
        .expected_bucket_owner(&store.owner)
        .if_none_match("*")
        .server_side_encryption(ServerSideEncryption::AwsKms)
        .ssekms_key_id(kms)
        .content_type("application/json")
        .body(ByteStream::from(bytes))
        .send()
        .await
        .map_err(|_| "Privacy journal upload failed")?;
    if !response
        .version_id()
        .is_some_and(|v| !v.is_empty() && v != "null")
    {
        return Err("Privacy journal storage must preserve an immutable object version".into());
    }
    println!(
        "{}",
        json!({"privacy_journal_exported":true,"requests":journal.requests.len(),"erased_objects":journal.erased_storage_keys.len(),"sha256":sha})
    );
    Ok(())
}
fn digest_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub async fn replay(db: &PgPool) -> Outcome<()> {
    let database = required("PRIVACY_SOURCE_DATABASE_ID")?;
    let source = required("PRIVACY_SOURCE_STORAGE_ID")?;
    let host = required("RECOVERY_DATABASE_HOST")?;
    let options = config::database_options().await?;
    if options.get_host() != host
        || options.get_username() != "myfans_owner"
        || host.starts_with(&format!("{database}."))
        || !host.ends_with(".rds.amazonaws.com")
    {
        return Err(
            "Privacy replay requires the exact isolated restored RDS endpoint and owner".into(),
        );
    }
    let version = required("PRIVACY_JOURNAL_VERSION")?;
    let expected = required("PRIVACY_JOURNAL_SHA256")?;
    if version == "null"
        || version.len() > 1024
        || version.chars().any(char::is_control)
        || expected.len() != 64
        || !expected
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Privacy replay requires the signed exact object version and SHA-256".into());
    }
    let store = ObjectStore::load().await?;
    let object = store
        .client
        .get_object()
        .bucket(&store.bucket)
        .key(&store.key)
        .version_id(&version)
        .expected_bucket_owner(&store.owner)
        .send()
        .await
        .map_err(|_| "Cannot read the exact privacy journal version")?;
    if object.version_id() != Some(version.as_str())
        || !object
            .content_length()
            .is_some_and(|n| n > 0 && n <= MAX_BYTES as i64)
    {
        return Err("Privacy journal object version or size is invalid".into());
    }
    let mut bytes = Vec::new();
    object
        .body
        .into_async_read()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > MAX_BYTES || digest_bytes(&bytes) != expected {
        return Err("Privacy journal differs from its verified signature hash".into());
    }
    let journal: Journal = serde_json::from_slice(&bytes)
        .map_err(|_| "Privacy journal is not valid versioned JSON")?;
    journal.validate(&database, &source)?;
    apply(db, &journal, &expected, &version).await?;
    println!(
        "{}",
        json!({"privacy_reconciled":true,"requests":journal.requests.len(),"excluded_objects":journal.erased_storage_keys.len(),"sha256":expected})
    );
    Ok(())
}

async fn apply(db: &PgPool, journal: &Journal, sha: &str, version: &str) -> Outcome<()> {
    // Each batch commits its cursor with its effects. A crash keeps the startup
    // gate closed, and the same immutable journal safely resumes the next batch.
    loop {
        let mut tx = db.begin().await?;
        owner_exclusive(&mut tx).await?;
        let row=sqlx::query("SELECT status,source_storage_id,target_storage_id,journal_sha256,request_cursor,object_cursor,frozen_accounts,erased_accounts FROM platform_recovery WHERE id=1 FOR UPDATE").fetch_one(&mut *tx).await?;
        let status: String = row.get("status");
        let bound: Option<String> = row.get("journal_sha256");
        let source: Option<String> = row.get("source_storage_id");
        let target: Option<String> = row.get("target_storage_id");
        let actual: String = sqlx::query_scalar(
            "SELECT media_store_identity FROM platform_storage WHERE id=1 FOR SHARE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if source.as_deref() != Some(&journal.source_storage_id)
            || target.as_deref() != Some(&actual)
            || actual == journal.source_storage_id
            || bound.as_ref().is_some_and(|h| h != sha)
            || !matches!(status.as_str(), "quarantined" | "reconciled")
        {
            return Err("Privacy journal does not match this quarantined recovery or its in-progress journal".into());
        }
        if status == "reconciled" {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE platform_recovery SET journal_sha256=$1 WHERE id=1")
            .bind(sha)
            .execute(&mut *tx)
            .await?;
        let request_cursor = row.get::<i32, _>("request_cursor") as usize;
        let object_cursor = row.get::<i32, _>("object_cursor") as usize;
        if request_cursor > journal.requests.len()
            || object_cursor > journal.erased_storage_keys.len()
        {
            return Err("Privacy replay cursor exceeds its immutable journal".into());
        }
        if let Some(request) = journal.requests.get(request_cursor) {
            let present: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_users WHERE id=$1)")
                    .bind(request.user_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let finished = if present {
                restore_request(&mut tx, request).await?
            } else {
                true
            };
            if finished {
                sqlx::query("UPDATE platform_recovery SET request_cursor=request_cursor+1,frozen_accounts=frozen_accounts+$1,erased_accounts=erased_accounts+$2 WHERE id=1").bind(i32::from(present)).bind(i32::from(present&&request.erased())).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            continue;
        }
        if object_cursor < journal.erased_storage_keys.len() {
            let end = (object_cursor + 100).min(journal.erased_storage_keys.len());
            let keys = &journal.erased_storage_keys[object_cursor..end];
            let ids: Vec<Uuid> = keys
                .iter()
                .filter_map(|k| {
                    k.split_once('.')
                        .and_then(|(id, _)| Uuid::parse_str(id).ok())
                })
                .collect();
            sqlx::query("DELETE FROM post_media WHERE asset_id=ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE media_assets SET state='rejected',hls_ready=false,captions_vtt=NULL,rejection_reason='Excluded by signed privacy recovery journal' WHERE id=ANY($1)").bind(&ids).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM media_upload_chunks WHERE storage_key=ANY($1)")
                .bind(keys)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE platform_recovery SET object_cursor=$1 WHERE id=1")
                .bind(end as i32)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            continue;
        }
        sqlx::query("INSERT INTO privacy_recovery_runs(journal_sha256,journal_version,journal_exported_at,source_database_identifier,source_storage_id,target_storage_id,requests_count,frozen_accounts,erased_accounts,excluded_objects) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(sha).bind(version).bind(journal.exported_at).bind(&journal.source_database_identifier).bind(&journal.source_storage_id).bind(&actual).bind(journal.requests.len() as i32).bind(row.get::<i32,_>("frozen_accounts")).bind(row.get::<i32,_>("erased_accounts")).bind(journal.erased_storage_keys.len() as i32).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE platform_recovery SET status='reconciled',reconciled_at=now() WHERE id=1",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(());
    }
}

async fn restore_request(tx: &mut Transaction<'_, Postgres>, r: &Request) -> Outcome<bool> {
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(r.user_id)
        .execute(&mut **tx)
        .await?;
    let existing:Option<CapturedRequest>=sqlx::query_as("SELECT id,privacy_version,retention,created_at,not_before FROM privacy_requests WHERE user_id=$1 FOR UPDATE").bind(r.user_id).fetch_optional(&mut **tx).await?;
    let retention = json!(&r.retention);
    if let Some((id, policy, schedule, created, not_before)) = existing {
        if id != r.id
            || policy != r.privacy_version
            || schedule != retention
            || created != r.created_at
            || not_before != r.not_before
        {
            return Err("Restored privacy scope differs from the signed immutable request".into());
        }
    } else {
        sqlx::query("INSERT INTO privacy_requests(id,user_id,privacy_version,retention,created_at,not_before) VALUES($1,$2,$3,$4,$5,$6)").bind(r.id).bind(r.user_id).bind(&r.privacy_version).bind(retention).bind(r.created_at).bind(r.not_before).execute(&mut **tx).await?;
    }
    // Never downgrade an existing hold or a more advanced erasure. A signed
    // completed request retains its original dates, not the restoration date.
    sqlx::query("UPDATE privacy_requests SET legal_hold=legal_hold OR $2,status=CASE WHEN status='completed' THEN status WHEN $3='completed' THEN $3 WHEN status='application_erased' THEN status WHEN $3='application_erased' THEN $3 WHEN status='purging' THEN status WHEN $3='purging' THEN $3 WHEN status='approved' THEN status ELSE $3 END,application_erased_at=coalesce(application_erased_at,$4),completed_at=coalesce(completed_at,$5) WHERE id=$1")
        .bind(r.id).bind(r.legal_hold).bind(&r.status).bind(r.application_erased_at).bind(r.completed_at).execute(&mut **tx).await?;
    sqlx::query("UPDATE app_users SET disabled_at=coalesce(disabled_at,$2) WHERE id=$1")
        .bind(r.user_id)
        .bind(r.created_at)
        .execute(&mut **tx)
        .await?;
    for query in [
        "DELETE FROM sessions WHERE user_id=$1",
        "DELETE FROM recovery_tokens WHERE user_id=$1",
        "UPDATE creators SET verification_status='rejected',verified_at=NULL WHERE user_id=$1",
        "UPDATE posts SET state='removed',moderation_blocked=true WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1)",
        "UPDATE media_uploads SET state='cancelled',expires_at=now() WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1) AND state IN ('uploading','assembling')",
        "UPDATE media_assets SET state='rejected',hls_ready=false WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1)",
        "UPDATE jobs SET status='done',locked_at=NULL,lease_id=NULL,last_error='Privacy recovery' WHERE status IN ('pending','running','failed') AND ((kind='process_media' AND payload->>'media_id' IN(SELECT m.id::text FROM media_assets m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$1)) OR (kind IN ('assemble_upload','cleanup_upload') AND payload->>'upload_id' IN(SELECT m.id::text FROM media_uploads m JOIN creators c ON c.id=m.creator_id WHERE c.user_id=$1)) OR (kind='publish' AND payload->>'post_id' IN(SELECT p.id::text FROM posts p JOIN creators c ON c.id=p.creator_id WHERE c.user_id=$1)))",
    ] {
        sqlx::query(query)
            .bind(r.user_id)
            .execute(&mut **tx)
            .await?;
    }
    if !r.erased() {
        return Ok(true);
    }
    if !privacy::scrub_batch(tx, r.user_id)
        .await
        .map_err(|_| "Privacy recovery scrub failed")?
    {
        return Ok(false);
    }
    sqlx::query(
        "DELETE FROM post_media WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1)",
    )
    .bind(r.user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE media_assets SET captions_vtt=NULL,rejection_reason='Account erased before recovery' WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1)").bind(r.user_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE app_users SET email='erased-'||id::text||'@invalid.example',display_name='Deleted account',avatar_url='',password_hash=NULL,email_verified=false,mfa_secret=NULL,mfa_enabled=false,mfa_recovery_required=false,mfa_replacement_pending=false,mfa_last_step=0,erased_at=coalesce(erased_at,$2) WHERE id=$1")
        .bind(r.user_id).bind(r.application_erased_at).execute(&mut **tx).await?;
    sqlx::query("UPDATE creators SET handle='erased_'||left(replace(id::text,'-',''),23),display_name='Deleted creator',bio='',avatar_url='',cover_url='',verification_status='rejected',verified_at=NULL,review_message=NULL,reviewed_at=NULL WHERE user_id=$1").bind(r.user_id).execute(&mut **tx).await?;
    Ok(true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn schedule() -> privacy::RetentionPolicy {
        privacy::RetentionPolicy {
            erasure_delay_days: 0,
            backup_days: 35,
            security_days: 30,
            financial_days: 365,
            policy_evidence_days: 365,
            moderation_evidence_days: 365,
        }
    }
    fn request(id: Uuid, time: DateTime<Utc>, erased: bool) -> Request {
        Request {
            id: Uuid::new_v4(),
            user_id: id,
            status: if erased {
                "application_erased"
            } else {
                "pending"
            }
            .into(),
            legal_hold: !erased,
            retention: schedule(),
            privacy_version: "v1".into(),
            created_at: time,
            not_before: time,
            application_erased_at: erased.then_some(time),
            completed_at: None,
        }
    }
    fn journal(requests: Vec<Request>) -> Journal {
        Journal {
            version: 1,
            source_database_identifier: "source-database".into(),
            source_storage_id: format!("s3:{}", "a".repeat(64)),
            exported_at: Utc::now(),
            requests,
            erased_storage_keys: vec![],
        }
    }
    #[test]
    fn journal_rejects_wrong_sources_unsafe_objects_and_inconsistent_erasure() {
        let mut j = journal(vec![request(
            Uuid::new_v4(),
            Utc::now() - chrono::Duration::hours(1),
            true,
        )]);
        assert!(j.validate("source-database", &j.source_storage_id).is_ok());
        assert!(j.validate("other-database", &j.source_storage_id).is_err());
        j.erased_storage_keys.push("../../secret".into());
        assert!(j.validate("source-database", &j.source_storage_id).is_err());
        j.erased_storage_keys = vec![format!("{}.720p-00151.ts", Uuid::new_v4())];
        assert!(j.validate("source-database", &j.source_storage_id).is_ok());
        j.requests[0].application_erased_at = None;
        assert!(j.validate("source-database", &j.source_storage_id).is_err());
        assert_eq!(
            digest_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    async fn exclusive(db: PgPool) -> PgPool {
        let options = (*db.connect_options()).clone();
        db.close().await;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap()
    }
    async fn quarantine_fixture(db: &PgPool, source: &str) {
        let target = format!("s3:{}", "b".repeat(64));
        let mut tx = db.begin().await.unwrap();
        sqlx::query("UPDATE platform_storage SET media_store_identity=$1 WHERE id=1")
            .bind(&target)
            .execute(&mut *tx)
            .await
            .unwrap();
        quarantine(&mut tx, source, &target).await.unwrap();
        tx.commit().await.unwrap();
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn recovery_replays_new_erasure_and_hold_without_resurrecting_paid_media(db: PgPool) {
        let db = exclusive(db).await;
        let erased:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('restored-erased@test.invalid','Private name') RETURNING id").fetch_one(&db).await.unwrap();
        let held:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('restored-held@test.invalid','Held name') RETURNING id").fetch_one(&db).await.unwrap();
        let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,subscription_price_minor,currency,verification_status,bio) VALUES($1,'restored_creator',500,'USD','approved','Private bio') RETURNING id").bind(erased).fetch_one(&db).await.unwrap();
        let asset = Uuid::new_v4();
        sqlx::query("INSERT INTO media_assets(id,creator_id,storage_key,size_bytes,state,hls_ready,content_type) VALUES($1,$2,$3,100,'approved',true,'video/mp4')").bind(asset).bind(creator).bind(format!("{asset}.mp4")).execute(&db).await.unwrap();
        let post:Uuid=sqlx::query_scalar("INSERT INTO posts(creator_id,caption,state,visibility,published_at) VALUES($1,'Private caption','published','subscribers',now()) RETURNING id").bind(creator).fetch_one(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO post_media(post_id,asset_id,creator_id,position) VALUES($1,$2,$3,0)",
        )
        .bind(post)
        .bind(asset)
        .bind(creator)
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO sessions(user_id,token_digest,expires_at) VALUES($1,'restore-erased-session',now()+interval '1 day'),($2,'restore-held-session',now()+interval '1 day')").bind(erased).bind(held).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO comments(user_id,post_id,body) SELECT $1,$2,'Restored personal content' FROM generate_series(1,205)").bind(erased).bind(post).execute(&db).await.unwrap();
        let checkout:Uuid=sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,kind,amount_minor,status,paid_at,idempotency_key,request_fingerprint) VALUES($1,$2,'tip',500,'paid',now(),gen_random_uuid(),'restored-tip') RETURNING id").bind(held).bind(creator).fetch_one(&db).await.unwrap();
        let time: DateTime<Utc> = sqlx::query_scalar("SELECT now()-interval '1 hour'")
            .fetch_one(&db)
            .await
            .unwrap();
        let mut j = journal(vec![
            request(erased, time, true),
            request(held, time, false),
            request(Uuid::new_v4(), time, true),
        ]);
        j.erased_storage_keys = vec![format!("{asset}.mp4"), format!("{asset}.720p-00000.ts")];
        j.validate("source-database", &j.source_storage_id).unwrap();
        quarantine_fixture(&db, &j.source_storage_id).await;
        assert!(require_reconciled(&db).await.is_err());
        let sha = digest_bytes(&serde_json::to_vec(&j).unwrap());
        apply(&db, &j, &sha, "immutable-version").await.unwrap();
        require_reconciled(&db).await.unwrap();
        let identity: (String, bool, bool) = sqlx::query_as(
            "SELECT email,disabled_at IS NOT NULL,erased_at IS NOT NULL FROM app_users WHERE id=$1",
        )
        .bind(erased)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(
            identity,
            (format!("erased-{erased}@invalid.example"), true, true)
        );
        let held_state:(String,bool,bool)=sqlx::query_as("SELECT u.email,u.disabled_at IS NOT NULL,r.legal_hold FROM app_users u JOIN privacy_requests r ON r.user_id=u.id WHERE u.id=$1").bind(held).fetch_one(&db).await.unwrap();
        assert_eq!(
            held_state,
            ("restored-held@test.invalid".into(), true, true)
        );
        for query in [
            "SELECT count(*) FROM sessions",
            "SELECT count(*) FROM comments",
            "SELECT count(*) FROM post_media",
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(query)
                    .fetch_one(&db)
                    .await
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM media_assets WHERE id=$1")
                .bind(asset)
                .fetch_one(&db)
                .await
                .unwrap(),
            "rejected"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT amount_minor FROM checkouts WHERE id=$1")
                .bind(checkout)
                .fetch_one(&db)
                .await
                .unwrap(),
            500
        );
        apply(&db, &j, &sha, "immutable-version").await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM privacy_recovery_runs")
                .fetch_one(&db)
                .await
                .unwrap(),
            1
        );
        assert!(
            sqlx::query("DELETE FROM privacy_recovery_runs")
                .execute(&db)
                .await
                .is_err()
        );
        let captured = capture(&db, "source-database").await.unwrap();
        assert_eq!(captured.requests.len(), 2);
        assert_eq!(captured.erased_storage_keys.len(), 311);
        assert!(
            captured
                .erased_storage_keys
                .contains(&format!("{asset}.720p-00151.ts"))
        );
        db.close().await;
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn recovery_resumes_only_the_same_journal_and_owner(db: PgPool) {
        let db = exclusive(db).await;
        let subject:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('resume@test.invalid','Before restore') RETURNING id").fetch_one(&db).await.unwrap();
        let time: DateTime<Utc> = sqlx::query_scalar("SELECT now()-interval '1 hour'")
            .fetch_one(&db)
            .await
            .unwrap();
        let j = journal(vec![request(subject, time, true)]);
        quarantine_fixture(&db, &j.source_storage_id).await;
        let sha = digest_bytes(&serde_json::to_vec(&j).unwrap());
        let mut tx = db.begin().await.unwrap();
        assert!(restore_request(&mut tx, &j.requests[0]).await.unwrap());
        sqlx::query("UPDATE platform_recovery SET journal_sha256=$1,request_cursor=1,frozen_accounts=1,erased_accounts=1 WHERE id=1").bind(&sha).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        assert!(require_reconciled(&db).await.is_err());
        assert!(
            apply(&db, &j, &"c".repeat(64), "wrong-version")
                .await
                .is_err()
        );
        assert!(require_reconciled(&db).await.is_err());
        sqlx::raw_sql(include_str!("../../../../db/roles/runtime.sql"))
            .execute(&db)
            .await
            .unwrap();
        let mut tx = db.begin().await.unwrap();
        sqlx::query("SET LOCAL ROLE myfans_runtime")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(owner_exclusive(&mut tx).await.is_err());
        assert!(
            sqlx::query(
                "UPDATE platform_recovery SET status='reconciled',reconciled_at=now() WHERE id=1"
            )
            .execute(&mut *tx)
            .await
            .is_err()
        );
        tx.rollback().await.unwrap();
        apply(&db, &j, &sha, "immutable-version").await.unwrap();
        require_reconciled(&db).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT erased_accounts FROM privacy_recovery_runs")
                .fetch_one(&db)
                .await
                .unwrap(),
            1
        );
        db.close().await;
    }
}
