use super::*;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig},
    primitives::ByteStream,
    types::ServerSideEncryption,
};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

pub const MAX_OBJECT_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
pub struct MediaStore {
    backend: Backend,
    identity: String,
}
#[derive(Clone)]
enum Backend {
    Local(PathBuf),
    S3(Arc<S3Store>),
}
struct S3Store {
    client: Client,
    bucket: String,
    prefix: String,
    owner: String,
    kms_key: Option<String>,
}
fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Private media storage is unavailable.",
    )
}
fn valid_key(key: &str) -> bool {
    // Only server-generated UUID filenames are accepted. No paths or hidden files.
    let Some((id, suffix)) = key.split_once('.') else {
        return false;
    };
    Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
        && (matches!(
            suffix,
            "upload" | "chunk" | "jpg" | "preview.jpg" | "mp4" | "m3u8" | "720p.m3u8" | "360p.m3u8"
        ) || ["720p-", "360p-"].iter().any(|prefix| {
            suffix
                .strip_prefix(prefix)
                .and_then(|v| v.strip_suffix(".ts"))
                .is_some_and(|v| {
                    v.len() == 5
                        && v.bytes().all(|b| b.is_ascii_digit())
                        && v.parse::<u32>().is_ok_and(|n| n <= 151)
                })
        }))
}
fn required(name: &str) -> std::result::Result<String, &'static str> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or("S3 configuration is incomplete")
}
impl MediaStore {
    pub fn local(root: PathBuf) -> Self {
        Self {
            backend: Backend::Local(root),
            identity: "local:v1".into(),
        }
    }
    pub fn is_remote(&self) -> bool {
        matches!(self.backend, Backend::S3(_))
    }
    pub async fn from_env(root: PathBuf) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        match std::env::var("MEDIA_STORAGE").as_deref() {
            Err(_) | Ok("local") => {
                if [
                    "S3_BUCKET",
                    "S3_PREFIX",
                    "S3_EXPECTED_BUCKET_OWNER",
                    "S3_KMS_KEY_ID",
                ]
                .iter()
                .any(|k| std::env::var_os(k).is_some())
                {
                    return Err("S3 settings require MEDIA_STORAGE=s3".into());
                }
                Ok(Self::local(root))
            }
            Ok("s3") => {
                let bucket = required("S3_BUCKET")?;
                let owner = required("S3_EXPECTED_BUCKET_OWNER")?;
                let region = required("AWS_REGION")?;
                let prefix = std::env::var("S3_PREFIX").unwrap_or_else(|_| "myfans/".into());
                if !(3..=63).contains(&bucket.len())
                    || !bucket
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    || !bucket.as_bytes()[0].is_ascii_alphanumeric()
                    || !bucket.as_bytes()[bucket.len() - 1].is_ascii_alphanumeric()
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
                    || !prefix
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_/".contains(&b))
                    || prefix.contains("//")
                {
                    return Err("Invalid S3 bucket, owner, region, or prefix".into());
                }
                // Production uses AWS endpoints and the SDK credential chain (task/instance/web identity roles).
                // Arbitrary endpoint overrides could exfiltrate signed credentials and media.
                if ["AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL_S3"]
                    .iter()
                    .any(|k| std::env::var_os(k).is_some())
                {
                    return Err("Custom S3 endpoint overrides are not supported".into());
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
                let client = Client::from_conf(
                    aws_sdk_s3::config::Builder::from(&config)
                        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
                        .build(),
                );
                let kms_key = std::env::var("S3_KMS_KEY_ID").ok();
                if kms_key.as_ref().is_some_and(|s| {
                    s.is_empty() || s.len() > 2048 || s.chars().any(char::is_control)
                }) {
                    return Err("Invalid S3 encryption configuration".into());
                }
                let identity = format!(
                    "s3:{}",
                    digest(&format!("{region}\n{owner}\n{bucket}\n{prefix}"))
                );
                let store = Self {
                    backend: Backend::S3(Arc::new(S3Store {
                        client,
                        bucket,
                        prefix,
                        owner,
                        kms_key,
                    })),
                    identity,
                };
                store.probe().await.map_err(
                    |_| "S3 access check failed; verify role, bucket, region and expected owner",
                )?;
                Ok(store)
            }
            _ => Err("MEDIA_STORAGE must be local or s3".into()),
        }
    }
    pub async fn bind(&self, db: &PgPool) -> Result<()> {
        let mut tx = db.begin().await?;
        let current: String = sqlx::query_scalar(
            "SELECT media_store_identity FROM platform_storage WHERE id=1 FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        if current != self.identity {
            let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_assets)")
                .fetch_one(&mut *tx)
                .await?;
            if occupied {
                return Err(Error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Media storage changed with existing assets; migrate and verify storage before startup.",
                ));
            }
            sqlx::query("UPDATE platform_storage SET media_store_identity=$1 WHERE id=1")
                .bind(&self.identity)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn probe(&self) -> Result<()> {
        match &self.backend {
            Backend::Local(root) => {
                if !tokio::fs::metadata(root)
                    .await
                    .map_err(|_| unavailable())?
                    .is_dir()
                {
                    return Err(unavailable());
                }
            }
            Backend::S3(s) => {
                s.client
                    .head_bucket()
                    .bucket(&s.bucket)
                    .expected_bucket_owner(&s.owner)
                    .send()
                    .await
                    .map_err(|_| unavailable())?;
            }
        }
        Ok(())
    }
    pub async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        if !valid_key(key) || bytes.is_empty() || bytes.len() > MAX_OBJECT_BYTES {
            return Err(bad("Invalid media object."));
        }
        match &self.backend {
            Backend::Local(root) => {
                let temp = root.join(format!(".object-{}", Uuid::new_v4()));
                let result = async {
                    use tokio::io::AsyncWriteExt;
                    let mut options = tokio::fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)]
                    options.mode(0o600);
                    let mut file = options.open(&temp).await?;
                    file.write_all(&bytes).await?;
                    file.sync_all().await?;
                    drop(file);
                    tokio::fs::rename(&temp, root.join(key)).await?;
                    #[cfg(unix)]
                    tokio::fs::File::open(root).await?.sync_all().await?;
                    Ok::<_, std::io::Error>(())
                }
                .await;
                if result.is_err() {
                    let _ = tokio::fs::remove_file(temp).await;
                }
                result.map_err(|_| unavailable())?;
            }
            Backend::S3(s) => {
                let mut request = s
                    .client
                    .put_object()
                    .bucket(&s.bucket)
                    .key(format!("{}{key}", s.prefix))
                    .expected_bucket_owner(&s.owner)
                    .content_type(content_type)
                    .cache_control("private, no-store")
                    .body(ByteStream::from(bytes));
                if let Some(kms) = &s.kms_key {
                    request = request
                        .server_side_encryption(ServerSideEncryption::AwsKms)
                        .ssekms_key_id(kms);
                } else {
                    request = request.server_side_encryption(ServerSideEncryption::Aes256);
                }
                request.send().await.map_err(|_| unavailable())?;
            }
        }
        Ok(())
    }
    pub async fn read(&self, key: &str, max: usize) -> Result<Vec<u8>> {
        if !valid_key(key) || max > MAX_OBJECT_BYTES {
            return Err(bad("Invalid media object."));
        }
        let bytes = match &self.backend {
            Backend::Local(root) => {
                let file = tokio::fs::File::open(root.join(key))
                    .await
                    .map_err(|_| unavailable())?;
                let metadata = file.metadata().await.map_err(|_| unavailable())?;
                if !metadata.is_file() || metadata.len() > max as u64 {
                    return Err(unavailable());
                }
                let mut bytes = Vec::new();
                file.take(max as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| unavailable())?;
                bytes
            }
            Backend::S3(s) => {
                let response = s
                    .client
                    .get_object()
                    .bucket(&s.bucket)
                    .key(format!("{}{key}", s.prefix))
                    .expected_bucket_owner(&s.owner)
                    .send()
                    .await
                    .map_err(|_| unavailable())?;
                if response
                    .content_length()
                    .is_none_or(|size| size < 0 || size > max as i64)
                {
                    return Err(unavailable());
                }
                let mut bytes = Vec::new();
                tokio::time::timeout(
                    Duration::from_secs(20),
                    response
                        .body
                        .into_async_read()
                        .take(max as u64 + 1)
                        .read_to_end(&mut bytes),
                )
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
                bytes
            }
        };
        if bytes.is_empty() || bytes.len() > max {
            return Err(unavailable());
        }
        Ok(bytes)
    }
    pub async fn size(&self, key: &str) -> Result<usize> {
        if !valid_key(key) {
            return Err(bad("Invalid media object."));
        }
        let size = match &self.backend {
            Backend::Local(root) => tokio::fs::metadata(root.join(key))
                .await
                .map_err(|_| unavailable())?
                .len(),
            Backend::S3(s) => s
                .client
                .head_object()
                .bucket(&s.bucket)
                .key(format!("{}{key}", s.prefix))
                .expected_bucket_owner(&s.owner)
                .send()
                .await
                .map_err(|_| unavailable())?
                .content_length()
                .filter(|v| *v > 0)
                .ok_or_else(unavailable)? as u64,
        };
        if size == 0 || size > MAX_OBJECT_BYTES as u64 {
            return Err(unavailable());
        }
        Ok(size as usize)
    }
    /// Backpressure reaches the private store; range requests never buffer an entire video.
    pub async fn stream(&self, key: &str, start: usize, length: usize) -> Result<axum::body::Body> {
        use tokio::io::AsyncSeekExt;
        if !valid_key(key)
            || length == 0
            || start
                .checked_add(length)
                .is_none_or(|n| n > MAX_OBJECT_BYTES)
        {
            return Err(bad("Invalid media range."));
        }
        let reader: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = match &self.backend {
            Backend::Local(root) => {
                let mut file = tokio::fs::File::open(root.join(key))
                    .await
                    .map_err(|_| unavailable())?;
                file.seek(std::io::SeekFrom::Start(start as u64))
                    .await
                    .map_err(|_| unavailable())?;
                Box::pin(file)
            }
            Backend::S3(s) => {
                let result = s
                    .client
                    .get_object()
                    .bucket(&s.bucket)
                    .key(format!("{}{key}", s.prefix))
                    .expected_bucket_owner(&s.owner)
                    .range(format!("bytes={start}-{}", start + length - 1))
                    .send()
                    .await
                    .map_err(|_| unavailable())?;
                if result.content_length() != Some(length as i64) {
                    return Err(unavailable());
                }
                Box::pin(result.body.into_async_read())
            }
        };
        Ok(axum::body::Body::from_stream(
            tokio_util::io::ReaderStream::new(reader.take(length as u64)),
        ))
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        if !valid_key(key) {
            return Err(bad("Invalid media object."));
        }
        match &self.backend {
            Backend::Local(root) => match tokio::fs::remove_file(root.join(key)).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(unavailable()),
            },
            Backend::S3(s) => {
                s.client
                    .delete_object()
                    .bucket(&s.bucket)
                    .key(format!("{}{key}", s.prefix))
                    .expected_bucket_owner(&s.owner)
                    .send()
                    .await
                    .map_err(|_| unavailable())?;
            }
        }
        Ok(())
    }
    pub async fn put_file(&self, key: &str, path: &Path, content_type: &str) -> Result<()> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|_| unavailable())?;
        let mut bytes = Vec::new();
        file.take(MAX_OBJECT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| unavailable())?;
        self.put(key, bytes, content_type).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn s3_requests_are_signed_owner_bound_private_and_size_limited() {
        use axum::{
            body::{Body, to_bytes},
            extract::Request,
            http::Method,
            routing::any,
        };
        use tokio::sync::Mutex;
        type Objects = Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>;
        async fn server(State(objects): State<Objects>, request: Request) -> Response {
            let path = request.uri().path().to_owned();
            assert!(path.starts_with("/fixture-bucket"));
            let h = request.headers();
            assert_eq!(
                h.get("x-amz-expected-bucket-owner").unwrap(),
                "123456789012"
            );
            assert!(
                h.get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("AWS4-HMAC-SHA256 ")
            );
            assert!(h.get("x-amz-acl").is_none());
            match *request.method() {
                Method::HEAD => StatusCode::OK.into_response(),
                Method::PUT => {
                    assert_eq!(h.get("x-amz-server-side-encryption").unwrap(), "AES256");
                    assert_eq!(h.get("cache-control").unwrap(), "private, no-store");
                    let bytes = to_bytes(request.into_body(), 1024).await.unwrap();
                    objects.lock().await.insert(path, bytes.to_vec());
                    StatusCode::OK.into_response()
                }
                Method::GET => match objects.lock().await.get(&path) {
                    Some(bytes) => Response::builder()
                        .status(200)
                        .header("content-length", bytes.len())
                        .body(Body::from(bytes.clone()))
                        .unwrap(),
                    None => StatusCode::NOT_FOUND.into_response(),
                },
                Method::DELETE => {
                    objects.lock().await.remove(&path);
                    StatusCode::NO_CONTENT.into_response()
                }
                _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(any(server))
            .with_state(Objects::default());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let conf = aws_sdk_s3::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "fixture-access",
                "fixture-secret",
                None,
                None,
                "fixture",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build();
        let store = MediaStore {
            backend: Backend::S3(Arc::new(S3Store {
                client: Client::from_conf(conf),
                bucket: "fixture-bucket".into(),
                prefix: "private/".into(),
                owner: "123456789012".into(),
                kms_key: None,
            })),
            identity: "fixture".into(),
        };
        store.probe().await.unwrap();
        let key = format!("{}.jpg", Uuid::new_v4());
        store
            .put(&key, b"fixture".to_vec(), "image/jpeg")
            .await
            .unwrap();
        assert_eq!(store.read(&key, 7).await.unwrap(), b"fixture");
        assert!(store.read(&key, 6).await.is_err());
        store.delete(&key).await.unwrap();
        assert!(store.read(&key, 7).await.is_err());
        task.abort();
    }
    #[tokio::test]
    async fn private_store_bounds_paths_and_preserves_objects()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("myfans-store-{}", Uuid::new_v4()));
        tokio::fs::create_dir(&root).await?;
        let store = MediaStore::local(root.clone());
        let key = format!("{}.jpg", Uuid::new_v4());
        assert!(
            store
                .put(&key, b"fixture".to_vec(), "image/jpeg")
                .await
                .is_ok()
        );
        assert_eq!(store.read(&key, 10).await.ok(), Some(b"fixture".to_vec()));
        assert!(store.read(&key, 2).await.is_err());
        for invalid in [
            "../secret",
            ".mfa-key",
            "nested/object.jpg",
            "/absolute",
            "not-a-uuid.jpg",
        ] {
            assert!(store.read(invalid, 10).await.is_err());
            assert!(store.delete(invalid).await.is_err());
            assert!(store.put(invalid, vec![1], "image/jpeg").await.is_err());
        }
        assert!(store.delete(&key).await.is_ok());
        assert!(store.read(&key, 10).await.is_err());
        assert!(store.delete(&key).await.is_ok());
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod lifecycle_tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        extract::Request,
        http::Method,
        routing::any,
    };
    use tokio::sync::{Mutex, Notify};
    use tower::ServiceExt;
    #[derive(Clone, Default)]
    struct Gate {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        objects: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    }
    async fn server(State(g): State<Gate>, request: Request) -> Response {
        let path = request.uri().path().to_owned();
        assert_eq!(
            request
                .headers()
                .get("x-amz-expected-bucket-owner")
                .unwrap(),
            "123456789012"
        );
        match *request.method() {
            Method::PUT => {
                let bytes = to_bytes(request.into_body(), 1024).await.unwrap();
                g.entered.notify_one();
                g.release.notified().await;
                g.objects.lock().await.insert(path, bytes.to_vec());
                StatusCode::OK.into_response()
            }
            Method::DELETE => {
                g.objects.lock().await.remove(&path);
                StatusCode::NO_CONTENT.into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn lifecycle_erasure_waits_for_legacy_put_then_removes_its_current_object(db: PgPool) {
        let (mut s, owner, _, token) = crate::platform::lifecycle_tests::fixture(db).await;
        let password = "Lifecycle-private-password-42!";
        let hash = auth::hash_password(password.into()).await.unwrap();
        sqlx::query("UPDATE app_users SET password_hash=$2 WHERE id=$1")
            .bind(owner)
            .bind(hash)
            .execute(&s.db)
            .await
            .unwrap();
        let gate = Gate::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(any(server)).with_state(gate.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let conf = aws_sdk_s3::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "fixture-access",
                "fixture-secret",
                None,
                None,
                "fixture",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(20))
                    .build(),
            )
            .build();
        s.storage = MediaStore {
            backend: Backend::S3(Arc::new(S3Store {
                client: Client::from_conf(conf),
                bucket: "fixture-bucket".into(),
                prefix: "private/".into(),
                owner: "123456789012".into(),
                kms_key: None,
            })),
            identity: "fixture".into(),
        };
        let mut body=b"--lifecycle\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
        body.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        body.extend_from_slice(b"\r\n--lifecycle--\r\n");
        let upload = router(s.clone()).oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/media/upload")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=lifecycle",
                )
                .body(Body::from(body))
                .unwrap(),
        );
        let upload = tokio::spawn(upload);
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        // The ownership record is already committed while PUT is still pending.
        let key: String = sqlx::query_scalar("SELECT storage_key FROM media_assets")
            .fetch_one(&s.db)
            .await
            .unwrap();
        let freeze = router(s.clone()).oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/privacy/erasure")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"current_password":password,"confirmation":"ERASE"}).to_string(),
                ))
                .unwrap(),
        );
        let freeze = tokio::spawn(freeze);
        let mut blocked = false;
        for _ in 0..250 {
            blocked=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND cardinality(pg_blocking_pids(pid))>0)").fetch_one(&s.db).await.unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            blocked,
            "erasure must wait for the bounded PUT account lock"
        );
        assert!(!freeze.is_finished());
        gate.release.notify_one();
        assert_eq!(upload.await.unwrap().unwrap().status(), StatusCode::OK);
        assert_eq!(freeze.await.unwrap().unwrap().status(), StatusCode::OK);
        assert!(
            gate.objects
                .lock()
                .await
                .contains_key(&format!("/fixture-bucket/private/{key}"))
        );
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='process_media' AND status IN ('pending','running')").fetch_one(&s.db).await.unwrap();
        assert_eq!(pending, 0, "freeze must cancel the just-enqueued media job");
        sqlx::query("UPDATE privacy_requests SET status='approved',next_at=now() WHERE user_id=$1")
            .bind(owner)
            .execute(&s.db)
            .await
            .unwrap();
        for _ in 0..12 {
            sqlx::query("UPDATE privacy_requests SET next_at=now() WHERE user_id=$1")
                .bind(owner)
                .execute(&s.db)
                .await
                .unwrap();
            privacy::tick(&s).await.unwrap();
            let done: bool = sqlx::query_scalar(
                "SELECT application_erased_at IS NOT NULL FROM privacy_requests WHERE user_id=$1",
            )
            .bind(owner)
            .fetch_one(&s.db)
            .await
            .unwrap();
            if done {
                break;
            }
        }
        let done: bool = sqlx::query_scalar(
            "SELECT application_erased_at IS NOT NULL FROM privacy_requests WHERE user_id=$1",
        )
        .bind(owner)
        .fetch_one(&s.db)
        .await
        .unwrap();
        assert!(done);
        assert!(gate.objects.lock().await.is_empty());
        let pending: i64 =
            sqlx::query_scalar("SELECT count(*) FROM privacy_objects WHERE removed_at IS NULL")
                .fetch_one(&s.db)
                .await
                .unwrap();
        assert_eq!(pending, 0);
        server.abort();
    }
}
