mod auth;
mod billing;
mod community;
mod config;
mod content;
mod discovery;
mod mail;
mod mail_feedback;
mod maintenance;
pub mod media_processor;
mod operations;
mod pagination;
mod post_view;
mod privacy;
mod privacy_recovery;
mod providers;
mod recovery;
mod scanner;
mod site;
mod storage;
mod telemetry;
mod uploads;
mod video;
mod workdir;
pub mod worker;
pub use mail::MailService;
pub use providers::Providers;
pub use scanner::MediaScanner;
pub use site::RuntimeConfig;
pub use storage::MediaStore;
pub use video::verify_pipeline as verify_media_pipeline;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tower_http::timeout::TimeoutLayer;
use tracing::Instrument;
use uuid::Uuid;

#[derive(Clone)]
pub struct PlatformState {
    pub runtime: Arc<RuntimeConfig>,
    pub db: PgPool,
    pub media_root: PathBuf,
    pub key: Arc<[u8; 32]>,
    pub dummy_hash: Arc<String>,
    pub mail: MailService,
    pub storage: MediaStore,
    pub scanner: MediaScanner,
    pub providers: Providers,
}

#[derive(Debug)]
pub struct Error(pub StatusCode, pub &'static str);
pub type Result<T> = std::result::Result<T, Error>;
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({"status":self.0.as_u16(),"message":self.1})),
        )
            .into_response()
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db) = error {
            match db.code().as_deref() {
                Some("23505") => return Self(StatusCode::CONFLICT, "This record already exists."),
                Some("23503" | "23514") => {
                    return Self(
                        StatusCode::BAD_REQUEST,
                        "This change violates a business rule.",
                    );
                }
                _ => {}
            }
        }
        if matches!(error, sqlx::Error::RowNotFound) {
            return Self(StatusCode::NOT_FOUND, "Record not found.");
        }
        tracing::error!(code=?error.as_database_error().and_then(|e|e.code()),"database_operation_failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The operation could not be completed.",
        )
    }
}
pub fn bad(message: &'static str) -> Error {
    Error(StatusCode::BAD_REQUEST, message)
}
pub fn forbidden() -> Error {
    Error(
        StatusCode::FORBIDDEN,
        "You do not have access to this resource.",
    )
}
pub fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
pub fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[derive(Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub avatar_url: String,
    pub role: String,
    pub email_verified: bool,
    pub mfa_enabled: bool,
    pub mfa_recovery_required: bool,
}
pub async fn optional_user(s: &PlatformState, h: &HeaderMap) -> Result<Option<User>> {
    let Some(value) = h.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let token = value
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .ok_or(Error(StatusCode::UNAUTHORIZED, "Your session is invalid."))?;
    let user=sqlx::query_as::<_,User>("SELECT u.id,u.email,u.display_name,u.avatar_url,u.role,u.email_verified,u.mfa_enabled,u.mfa_recovery_required FROM app_users u JOIN sessions s ON s.user_id=u.id WHERE s.token_digest=$1 AND s.expires_at>now() AND u.disabled_at IS NULL").bind(digest(token)).fetch_optional(&s.db).await?;
    user.map(Some).ok_or(Error(
        StatusCode::UNAUTHORIZED,
        "Your session expired. Please sign in again.",
    ))
}
pub async fn user(s: &PlatformState, h: &HeaderMap) -> Result<User> {
    optional_user(s, h).await?.ok_or(Error(
        StatusCode::UNAUTHORIZED,
        "Please sign in to continue.",
    ))
}
pub async fn mfa_session(s: &PlatformState, h: &HeaderMap) -> Result<()> {
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(forbidden())?;
    let verified:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions s JOIN app_users u ON u.id=s.user_id WHERE s.token_digest=$1 AND s.mfa_verified_at>now()-interval '15 minutes' AND s.expires_at>now() AND u.mfa_enabled AND NOT u.mfa_recovery_required AND u.disabled_at IS NULL)").bind(digest(token)).fetch_one(&s.db).await?;
    if !verified {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Sign in with two-factor authentication before this action.",
        ));
    }
    Ok(())
}
pub async fn admin(s: &PlatformState, h: &HeaderMap) -> Result<User> {
    let u = user(s, h).await?;
    if u.role != "admin" {
        return Err(forbidden());
    }
    if !u.mfa_enabled {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Enable two-factor authentication before administrative actions.",
        ));
    }
    mfa_session(s, h).await?;
    Ok(u)
}
#[derive(Clone, Copy)]
pub(super) enum Assurance {
    Member,
    RecentMfa,
    Admin,
}
// A request's first auth read is only a snapshot. Serialize its mutation with
// account freezing and credential recovery, then revalidate the same session.
// Acquire any additional account rows in UUID order before calling this helper;
// account locks must precede creator/content/payment resource locks.
pub(super) async fn authorize_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    h: &HeaderMap,
    id: Uuid,
    assurance: Assurance,
) -> Result<User> {
    let expired = || {
        Error(
            StatusCode::UNAUTHORIZED,
            "Your session expired. Please sign in again.",
        )
    };
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .ok_or_else(expired)?;
    let u=sqlx::query_as::<_,User>("SELECT id,email,display_name,avatar_url,role,email_verified,mfa_enabled,mfa_recovery_required FROM app_users WHERE id=$1 AND disabled_at IS NULL FOR UPDATE")
        .bind(id).fetch_optional(&mut **tx).await?.ok_or_else(expired)?;
    // Separate statement after the lock: under READ COMMITTED it sees session
    // deletions committed by a password/MFA recovery while we were waiting.
    let recent:Option<bool>=sqlx::query_scalar("SELECT COALESCE(mfa_verified_at>clock_timestamp()-interval '15 minutes',false) FROM sessions WHERE user_id=$1 AND token_digest=$2 AND expires_at>clock_timestamp()")
        .bind(id).bind(digest(token)).fetch_optional(&mut **tx).await?;
    let recent = recent.ok_or_else(expired)?;
    if matches!(assurance, Assurance::Admin) && u.role != "admin" {
        return Err(forbidden());
    }
    if !matches!(assurance, Assurance::Member)
        && (!u.mfa_enabled || u.mfa_recovery_required || !recent)
    {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Sign in with two-factor authentication before this action.",
        ));
    }
    Ok(u)
}
pub(super) async fn authorize_current(
    s: &PlatformState,
    h: &HeaderMap,
    id: Uuid,
    assurance: Assurance,
) -> Result<User> {
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, h, id, assurance).await?;
    tx.commit().await?;
    Ok(u)
}
pub async fn rate_limit(s: &PlatformState, bucket: &str, max: i32) -> Result<()> {
    let attempts:i32=sqlx::query_scalar("INSERT INTO rate_limits(bucket) VALUES($1) ON CONFLICT(bucket) DO UPDATE SET attempts=CASE WHEN rate_limits.window_start<now()-interval '1 minute' THEN 1 ELSE rate_limits.attempts+1 END,window_start=CASE WHEN rate_limits.window_start<now()-interval '1 minute' THEN now() ELSE rate_limits.window_start END RETURNING attempts").bind(digest(bucket)).fetch_one(&s.db).await?;
    if attempts > max {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests. Please wait a minute.",
        ));
    }
    Ok(())
}
pub async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Uuid,
    action: &str,
    kind: &str,
    id: Uuid,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_events(actor_id,action,resource_type,resource_id) VALUES($1,$2,$3,$4)",
    )
    .bind(actor)
    .bind(action)
    .bind(kind)
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub async fn notify(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: Uuid,
    title: &str,
    body: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO notifications(user_id,title,body) VALUES($1,$2,$3)")
        .bind(user)
        .bind(title)
        .bind(body)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn load_mfa_key(
    root: &std::path::Path,
    has_encrypted_secrets: bool,
) -> std::io::Result<[u8; 32]> {
    use std::io::{Error, ErrorKind};
    use tokio::io::AsyncWriteExt;
    let path = root.join(".mfa-key");
    let decode = |bytes: Vec<u8>| {
        bytes
            .try_into()
            .map_err(|_| Error::new(ErrorKind::InvalidData, "Invalid stored MFA encryption key"))
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => return decode(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if has_encrypted_secrets {
        return Err(Error::new(
            ErrorKind::NotFound,
            "MFA encryption key is missing; restore the matching key before startup",
        ));
    }
    let key: [u8; 32] = Sha256::digest(random_token().as_bytes()).into();
    let temp = root.join(format!(".mfa-key-{}.tmp", Uuid::new_v4()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temp).await?;
    let result = async {
        file.write_all(&key).await?;
        file.sync_all().await?;
        // Publishing a fully written inode is atomic and never overwrites another process's key.
        let published = match tokio::fs::hard_link(&temp, &path).await {
            Ok(()) => Ok(key),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                decode(tokio::fs::read(&path).await?)
            }
            Err(error) => Err(error),
        }?;
        #[cfg(unix)]
        tokio::fs::File::open(root).await?.sync_all().await?;
        Ok::<_, std::io::Error>(published)
    }
    .await;
    drop(file);
    let _ = tokio::fs::remove_file(temp).await;
    result
}

pub async fn connect() -> std::result::Result<PlatformState, Box<dyn std::error::Error>> {
    connect_inner(false).await
}
pub async fn connect_for_storage_verification()
-> std::result::Result<PlatformState, Box<dyn std::error::Error>> {
    connect_inner(true).await
}
async fn connect_inner(
    storage_verification: bool,
) -> std::result::Result<PlatformState, Box<dyn std::error::Error>> {
    if std::env::var("CONTENT_POLICY").is_ok_and(|v| v != "general") {
        return Err("This release supports only CONTENT_POLICY=general; explicit adult content is prohibited".into());
    }
    let runtime = Arc::new(RuntimeConfig::from_env().await?);
    let database_options = config::database_options().await?;
    let db = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|connection,_|Box::pin(async move{sqlx::raw_sql("SET statement_timeout='5s'; SET lock_timeout='3s'; SET idle_in_transaction_session_timeout='15s'").execute(connection).await?;Ok(())}))
        .connect_with(database_options)
        .await?;
    let migrator = sqlx::migrate!("../../db/migrations");
    let run_migrations = match std::env::var("APP_RUN_MIGRATIONS").as_deref() {
        Ok("false") => false,
        Ok("true") | Err(_) => true,
        _ => return Err("APP_RUN_MIGRATIONS must be true or false".into()),
    };
    if run_migrations {
        migrator.run(&db).await?;
    } else {
        let expected: Vec<_> = migrator
            .iter()
            .filter(|m| m.migration_type.is_up_migration())
            .collect();
        let actual: Vec<(i64, Vec<u8>, bool)> = sqlx::query_as(
            "SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&db)
        .await?;
        if expected.is_empty()
            || actual.len() != expected.len()
            || !actual
                .iter()
                .zip(&expected)
                .all(|((version, checksum, success), migration)| {
                    *success
                        && *version == migration.version
                        && checksum.as_slice() == migration.checksum.as_ref()
                })
        {
            return Err("Database migration versions and checksums must match the bundled release before startup".into());
        }
    }
    if !storage_verification {
        privacy_recovery::require_reconciled(&db).await?;
    }
    let media_root =
        PathBuf::from(std::env::var("MEDIA_ROOT").unwrap_or_else(|_| ".local/media".into()));
    tokio::fs::create_dir_all(&media_root).await?;
    let storage = MediaStore::from_env(media_root.clone()).await?;
    storage.bind(&db).await.map_err(|e| e.1)?;
    let has_encrypted_secrets: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_users WHERE mfa_secret IS NOT NULL) OR EXISTS(SELECT 1 FROM email_deliveries WHERE encrypted_body IS NOT NULL)")
            .fetch_one(&db)
            .await?;
    let key=match std::env::var_os("MFA_ENCRYPTION_KEY_FILE"){
        Some(path)=>config::encryption_key(std::path::Path::new(&path)).await?,
        None if storage.is_remote()=>return Err("S3 deployments require a shared MFA_ENCRYPTION_KEY_FILE containing exactly 32 random bytes".into()),
        None=>load_mfa_key(&media_root,has_encrypted_secrets).await?,
    };
    let dummy_hash = auth::hash_password(random_token())
        .await
        .map_err(|_| "Password hashing unavailable")?;
    let state = PlatformState {
        runtime,
        db,
        media_root,
        key: Arc::new(key),
        dummy_hash: Arc::new(dummy_hash),
        mail: MailService::from_env().await?,
        storage,
        scanner: MediaScanner::from_env()?,
        providers: Providers::from_env().await?,
    };
    verify_storage(&state).await.map_err(
        |_| "Stored ciphertext verification failed; restore the matching key and unmodified data",
    )?;
    state.providers.bind(&state.db).await.map_err(|e| e.1)?;
    if state.runtime.production {
        media_processor::verify_isolation(&state.media_root)
            .await
            .map_err(|e| e.1)?;
        if !state.storage.is_remote()
            || !state.scanner.enabled()
            || state.mail.name() != "smtp"
            || !state.providers.stripe.as_ref().is_some_and(|p| p.is_live())
            || !state
                .providers
                .identity
                .as_ref()
                .is_some_and(|p| p.is_live())
        {
            return Err("Production requires live payment and identity providers, private S3, scanning and SMTP".into());
        }
        let fixtures: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_users WHERE email LIKE '%@myfans.test') OR EXISTS(SELECT 1 FROM posts WHERE sample_media_url IS NOT NULL OR sample_preview_url IS NOT NULL) OR EXISTS(SELECT 1 FROM checkouts WHERE provider='sandbox') OR EXISTS(SELECT 1 FROM email_deliveries WHERE transport='sandbox')").fetch_one(&state.db).await?;
        if fixtures {
            return Err(
                "Production refuses sandbox records; provision a separate production database"
                    .into(),
            );
        }
        let unsafe_role: bool = sqlx::query_scalar("SELECT rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR pg_has_role(current_user,(SELECT tableowner FROM pg_tables WHERE schemaname='public' AND tablename='app_users'),'MEMBER') FROM pg_roles WHERE rolname=current_user").fetch_one(&state.db).await?;
        if unsafe_role {
            return Err("Production must use a restricted runtime database role".into());
        }
    }
    Ok(state)
}
/// Administrative commands connect only to PostgreSQL; provider credentials are never loaded.
pub async fn database_admin(command: &str) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(config::database_options().await?)
        .await
        .map_err(|_| "Administrative database connection failed")?;
    if command == "migrate" {
        sqlx::migrate!("../../db/migrations")
            .run(&db)
            .await
            .map_err(|_| "Database migration failed; inspect the database migration status")?;
    } else if command == "provision-runtime" {
        let password = providers::secret("DATABASE_RUNTIME_PASSWORD_FILE").await?;
        if password.len() < 32
            || password.len() > 128
            || !password.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(
                "Runtime password must contain 32–128 random alphanumeric characters".into(),
            );
        }
        let mut tx = db.begin().await?;
        sqlx::raw_sql(include_str!("../../../../db/roles/runtime.sql"))
            .execute(&mut *tx)
            .await
            .map_err(|_| "Runtime grants could not be applied")?;
        sqlx::raw_sql("DO $$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='myfans_app') THEN CREATE ROLE myfans_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION; END IF; END $$; GRANT myfans_runtime TO myfans_app;").execute(&mut *tx).await.map_err(|_|"Runtime login could not be provisioned")?;
        // PostgreSQL performs literal quoting; neither identifiers nor credentials are interpolated by the application.
        let statement:String=sqlx::query_scalar("SELECT format('ALTER ROLE myfans_app LOGIN PASSWORD %L NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION', $1::text)").bind(password).fetch_one(&mut *tx).await.map_err(|_|"Runtime credentials could not be prepared")?;
        sqlx::QueryBuilder::<sqlx::Postgres>::new("")
            .push(statement)
            .build()
            .execute(&mut *tx)
            .await
            .map_err(|_| "Runtime credentials could not be updated")?;
        tx.commit().await?;
    } else if command == "privacy-purge" {
        privacy::purge_versions(&db).await?;
    } else if command == "privacy-retention" {
        privacy::purge_retention(&db).await?;
    } else if command == "privacy-export" {
        privacy_recovery::export(&db).await?;
    } else if command == "privacy-replay" {
        privacy_recovery::replay(&db).await?;
    } else if command == "rotate-key" {
        maintenance::rotate_key(&db).await?;
    } else if command == "rebind-storage" {
        if std::env::var("RECOVERY_REBIND").as_deref() != Ok("true") {
            return Err("Recovery storage rebinding requires RECOVERY_REBIND=true".into());
        }
        let expected_host = std::env::var("RECOVERY_DATABASE_HOST")
            .map_err(|_| "RECOVERY_DATABASE_HOST is required")?;
        let opts = config::database_options().await?;
        if opts.get_host() != expected_host || opts.get_username() != "myfans_owner" {
            return Err(
                "Recovery database endpoint and owner must match the isolated restore".into(),
            );
        }
        let old = std::env::var("EXPECTED_PREVIOUS_STORAGE_ID")
            .map_err(|_| "Expected previous storage identity is required")?;
        let new = std::env::var("RESTORE_STORAGE_ID")
            .map_err(|_| "Restore storage identity is required")?;
        for value in [&old, &new] {
            if !value
                .strip_prefix("s3:")
                .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("Invalid recovery storage identity".into());
            }
        }
        if old == new {
            return Err("Recovery must target a separate media storage identity".into());
        }
        let mut tx = db.begin().await?;
        sqlx::query("LOCK TABLE platform_storage IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let clients:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='myfans_app'").fetch_one(&mut *tx).await?;
        if clients != 0 {
            return Err(
                "Recovery storage rebinding requires no application database connections".into(),
            );
        }
        if sqlx::query("UPDATE platform_storage SET media_store_identity=$2 WHERE id=1 AND media_store_identity=$1").bind(&old).bind(&new).execute(&mut *tx).await?.rows_affected()!=1 { return Err("The restored storage identity does not match the checkpoint".into()); }
        privacy_recovery::quarantine(&mut tx, &old, &new).await?;
        tx.commit().await?;
    } else if command == "bootstrap-admin" {
        let email = std::env::var("ADMIN_BOOTSTRAP_EMAIL")
            .map_err(|_| "ADMIN_BOOTSTRAP_EMAIL is required")?;
        let mut tx = db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(672913054)")
            .execute(&mut *tx)
            .await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM app_users WHERE role='admin')")
                .fetch_one(&mut *tx)
                .await?;
        if exists {
            return Err("An administrator already exists; bootstrap cannot be repeated".into());
        }
        let id:Option<Uuid>=sqlx::query_scalar("UPDATE app_users SET role='admin' WHERE email=$1 AND email_verified AND mfa_enabled AND NOT mfa_recovery_required AND disabled_at IS NULL RETURNING id").bind(email.trim().to_lowercase()).fetch_optional(&mut *tx).await?;
        let id = id.ok_or(
            "Bootstrap requires an existing active user with verified email and enrolled MFA",
        )?;
        audit(&mut tx, id, "admin.bootstrapped", "user", id)
            .await
            .map_err(|e| e.1)?;
        tx.commit().await?;
    } else {
        return Err("Unsupported administrative command".into());
    }
    db.close().await;
    Ok(())
}
/// Configuration/probe evidence only; never an authorization to enable live traffic.
pub async fn doctor(s: &PlatformState) -> Value {
    let storage = s.storage.probe().await.is_ok();
    let scanner = s.scanner.enabled() && s.scanner.probe().await.is_ok();
    let media_isolation = media_processor::verify_isolation(&s.media_root)
        .await
        .is_ok();
    let failed_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE status='failed'")
        .fetch_one(&s.db)
        .await
        .unwrap_or(-1);
    json!({"app_mode":s.runtime.mode(),"public_release_enabled":s.runtime.production,"payments":s.providers.stripe.is_some(),"identity":s.providers.identity.is_some(),"private_object_storage":s.storage.is_remote() && storage,"storage_probe":storage,"scanner_probe":scanner,"media_isolation":media_isolation,"smtp_configured":s.mail.name()=="smtp","failed_jobs":failed_jobs,"provider_acceptance_completed":false})
}
async fn headers(mut r: Response) -> Response {
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    r.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    r
}
/// Validate authenticated ciphertext in bounded pages without returning secrets or changing data.
/// The caller must quiesce writers; this is used on an isolated restored database/media pair.
pub async fn verify_storage(s: &PlatformState) -> Result<(u64, u64)> {
    let mut cursor = None;
    let mut mfa_count = 0;
    loop {
        let rows:Vec<(Uuid,Vec<u8>)>=sqlx::query_as("SELECT id,mfa_secret FROM app_users WHERE mfa_secret IS NOT NULL AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 100").bind(cursor).fetch_all(&s.db).await?;
        if rows.is_empty() {
            break;
        }
        for (id, bytes) in rows {
            auth::decrypt(s, &bytes)?;
            cursor = Some(id);
            mfa_count += 1;
        }
    }
    cursor = None;
    let mut mail_count = 0;
    loop {
        let rows:Vec<(Uuid,String,Vec<u8>)>=sqlx::query_as("SELECT id,recipient,encrypted_body FROM email_deliveries WHERE encrypted_body IS NOT NULL AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 100").bind(cursor).fetch_all(&s.db).await?;
        if rows.is_empty() {
            break;
        }
        for (id, recipient, bytes) in rows {
            mail::open(s, id, &recipient, &bytes)?;
            cursor = Some(id);
            mail_count += 1;
        }
    }
    Ok((mfa_count, mail_count))
}
async fn request_context(request: axum::extract::Request, next: middleware::Next) -> Response {
    let id = Uuid::new_v4();
    let span = tracing::info_span!("request",request_id=%id,method=%request.method(),path=%request.uri().path());
    let started = std::time::Instant::now();
    async move {
        let mut response = next.run(request).await;
        tracing::info!(
            status = response.status().as_u16(),
            duration_ms = started.elapsed().as_millis() as u64,
            "request_completed"
        );
        response.headers_mut().insert(
            "x-request-id",
            header::HeaderValue::from_str(&id.to_string())
                .unwrap_or_else(|_| header::HeaderValue::from_static("unavailable")),
        );
        response
    }
    .instrument(span)
    .await
}
pub fn router(s: PlatformState) -> Router {
    Router::new()
        .route(
            "/health/live",
            get(|State(s): State<PlatformState>| async move {
                Json(json!({"status":"ok","mode":s.runtime.mode()}))
            }),
        )
        .route(
            "/health/ready",
            get(|State(s): State<PlatformState>| async move {
                if sqlx::query("SELECT 1").execute(&s.db).await.is_ok() {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }),
        )
        .route("/v1/site", get(site::site))
        .route("/v1/creators", get(discovery::creators))
        .route("/v1/auth/register", post(auth::register))
        .route("/v1/integrations/status", get(providers::status))
        .route(
            "/v1/creators/payout-account",
            post(providers::stripe::account),
        )
        .route(
            "/v1/creators/payout-dashboard",
            post(providers::stripe::dashboard),
        )
        .route(
            "/v1/creators/verification",
            post(providers::identity::start),
        )
        .route(
            "/v1/webhooks/stripe",
            post(providers::stripe::webhook).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/v1/webhooks/sumsub",
            post(providers::identity::webhook).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/auth/recover", post(auth::recover))
        .route("/v1/auth/reset", post(auth::reset))
        .route("/v1/auth/verify", post(auth::verify_email))
        .route(
            "/v1/account",
            get(auth::account).patch(auth::update_account),
        )
        .route(
            "/v1/privacy/erasure",
            get(privacy::status).post(privacy::request),
        )
        .route("/v1/admin/privacy", get(privacy::list))
        .route("/v1/admin/privacy/{id}/review", post(privacy::review))
        .route("/v1/admin/privacy/{id}/complete", post(privacy::complete))
        .route("/v1/account/safety", get(community::safety))
        .route(
            "/v1/users/{id}/block",
            put(community::block).delete(community::unblock),
        )
        .route("/v1/appeals", post(community::appeal))
        .route(
            "/v1/admin/finance",
            get(providers::stripe::finance::overview),
        )
        .route(
            "/v1/admin/finance/checkout",
            post(providers::stripe::finance::resolve_checkout),
        )
        .route(
            "/v1/admin/finance/dispute",
            post(providers::stripe::finance::refresh_dispute),
        )
        .route(
            "/v1/admin/finance/issues/acknowledge",
            post(providers::stripe::finance::acknowledge),
        )
        .route("/v1/admin/appeals", get(community::appeals))
        .route("/v1/admin/appeals/{id}", post(community::review_appeal))
        .route("/v1/account/mail", get(auth::mail))
        .route(
            "/v1/account/verification/resend",
            post(auth::resend_verification),
        )
        .route("/v1/account/export", get(auth::export))
        .route("/v1/account/deactivate", post(auth::deactivate))
        .route("/v1/account/password", post(auth::change_password))
        .route("/v1/account/mfa", post(auth::mfa))
        .route("/v1/feed", get(content::feed))
        .route("/v1/studio", get(content::studio))
        .route("/v1/creators/profile", post(content::profile))
        .route("/v1/creators/resubmit", post(content::resubmit_creator))
        .route(
            "/v1/media/upload",
            post(content::upload).layer(DefaultBodyLimit::max(12 * 1024 * 1024)),
        )
        .route("/v1/media/uploads", post(uploads::start))
        .route(
            "/v1/media/uploads/{id}",
            get(uploads::get_upload)
                .patch(uploads::chunk)
                .delete(uploads::cancel)
                .layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
        .route("/v1/media/uploads/{id}/complete", post(uploads::complete))
        .route("/v1/media/{id}/hls/{filename}", get(content::hls))
        .route("/v1/media/{id}/{variant}", get(content::media))
        .route(
            "/v1/media/{id}/captions",
            post(content::captions).get(content::get_captions),
        )
        .route("/v1/posts", post(content::create_post))
        .route("/v1/posts/{id}", patch(content::update_post))
        .route(
            "/v1/posts/{id}/like",
            put(content::like).delete(content::unlike),
        )
        .route(
            "/v1/posts/{id}/save",
            put(content::save).delete(content::unsave),
        )
        .route(
            "/v1/posts/{id}/comments",
            get(content::comments).post(content::comment),
        )
        .route("/v1/checkout", post(billing::checkout))
        .route("/v1/checkout/{id}/confirm", post(billing::confirm))
        .route("/v1/subscriptions/{id}", delete(billing::cancel))
        .route("/v1/earnings", get(billing::earnings))
        .route("/v1/payouts", post(billing::payout))
        .route("/v1/admin/refunds/{id}", post(billing::refund))
        .route(
            "/v1/conversations",
            get(operations::conversations).post(operations::conversation),
        )
        .route(
            "/v1/conversations/{id}/messages",
            get(operations::messages).post(operations::message),
        )
        .route("/v1/notifications", get(operations::notifications))
        .route(
            "/v1/notifications/{id}/read",
            patch(operations::read_notification),
        )
        .route("/v1/reports", post(operations::report))
        .route("/v1/admin/reviews", get(operations::reviews))
        .route("/v1/admin/posts/{id}", get(post_view::inspect))
        .route("/v1/admin/reviews/{id}", post(operations::review))
        .route("/v1/admin/operations", get(operations::operations))
        .route("/v1/admin/jobs/{id}/retry", post(operations::retry_job))
        .route(
            "/v1/admin/accounts/{id}",
            post(operations::moderate_account),
        )
        .route(
            "/v1/admin/posts/{id}/restore",
            post(operations::restore_post),
        )
        .fallback(|| async { Error(StatusCode::NOT_FOUND, "Endpoint not found.") })
        .layer(DefaultBodyLimit::max(16384))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(middleware::map_response(headers))
        .layer(middleware::from_fn(request_context))
        .with_state(s)
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn concurrent_key_startup_agrees_and_missing_key_fails_closed()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("myfans-key-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await?;
        assert!(load_mfa_key(&root, true).await.is_err());
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let root = root.clone();
            tasks.spawn(async move { load_mfa_key(&root, false).await });
        }
        let mut keys = Vec::new();
        while let Some(result) = tasks.join_next().await {
            keys.push(result??);
        }
        assert!(keys.iter().all(|key| key == &keys[0]));
        assert_eq!(load_mfa_key(&root, true).await?, keys[0]);
        tokio::fs::write(root.join(".mfa-key"), []).await?;
        assert!(load_mfa_key(&root, false).await.is_err());
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }
    #[test]
    fn tokens_and_digests_have_fixed_width() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.bytes().all(|v| v.is_ascii_hexdigit()));
        assert_eq!(digest(&a).len(), 64);
        assert_ne!(a, digest(&a));
    }
}
