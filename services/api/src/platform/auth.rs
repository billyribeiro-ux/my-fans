use super::*;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::extract::{Query, State};
use serde::Deserialize;
use std::sync::OnceLock;
use totp_rs::{Algorithm, Builder, Secret, Totp};
static PASSWORD_SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

pub async fn export(State(s): State<PlatformState>, h: HeaderMap) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    rate_limit(&s, &format!("export:{}", u.id), 2).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    let creator: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM creators c WHERE user_id=$1")
            .bind(u.id)
            .fetch_optional(&mut *tx)
            .await?;
    let subscriptions:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT id,creator_id,status,current_period_start,current_period_end,cancel_at_period_end FROM subscriptions WHERE user_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let posts:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT p.id,p.caption,p.state,p.visibility,p.purchase_price_minor,p.created_at FROM posts p JOIN creators c ON c.id=p.creator_id WHERE c.user_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let comments:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT id,post_id,body,created_at FROM comments WHERE user_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let messages:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT id,conversation_id,body,created_at FROM messages WHERE sender_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let notifications:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT id,title,body,read_at,created_at FROM notifications WHERE user_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let checkouts:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT id,creator_id,post_id,kind,amount_minor,currency,status,created_at,paid_at,refunded_minor FROM checkouts WHERE user_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let payouts:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(x)),'[]') FROM(SELECT id,amount_minor,currency,status,created_at FROM payouts WHERE user_id=$1)x").bind(u.id).fetch_one(&mut *tx).await?;
    let blocks: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(b)),'[]') FROM user_blocks b WHERE user_id=$1",
    )
    .bind(u.id)
    .fetch_one(&mut *tx)
    .await?;
    let appeals: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(a)),'[]') FROM moderation_appeals a WHERE user_id=$1",
    )
    .bind(u.id)
    .fetch_one(&mut *tx)
    .await?;
    let consent: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(p)),'[]') FROM policy_acceptances p WHERE user_id=$1",
    )
    .bind(u.id)
    .fetch_one(&mut *tx)
    .await?;
    audit(&mut tx, u.id, "account.exported", "user", u.id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"user":u,"creator":creator,"subscriptions":subscriptions,"posts":posts,"comments":comments,"messages":messages,"notifications":notifications,"checkouts":checkouts,"payouts":payouts,"blocks":blocks,"appeals":appeals,"policy_acceptances":consent,"mode":s.runtime.mode(),"media_note":"Media files are separate; download your approved files from Creator Studio."}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deactivate {
    current_password: String,
    mfa_code: Option<String>,
}
pub async fn deactivate(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<Deactivate>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if u.role == "admin" {
        return Err(bad(
            "Administrators require an operator-managed offboarding process.",
        ));
    }
    rate_limit(&s, &format!("deactivate:{}", u.id), 5).await?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM app_users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !password_valid(b.current_password, hash).await {
        return Err(forbidden());
    }
    if s.providers.stripe.is_some() {
        let (available, pending) = providers::stripe::balances(&s, u.id).await?;
        if available != 0 || pending != 0 {
            return Err(bad(
                "Resolve your connected account balance before deactivating.",
            ));
        }
    }
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if u.role == "admin" {
        return Err(forbidden());
    }
    if u.mfa_enabled {
        check_mfa_tx(&s, &mut tx, u.id, b.mfa_code.as_deref().unwrap_or("")).await?;
    }
    if s.providers.stripe.is_some() {
        sqlx::query("SELECT id FROM creators WHERE user_id=$1 FOR UPDATE")
            .bind(u.id)
            .execute(&mut *tx)
            .await?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM subscriptions sub LEFT JOIN creators c ON c.id=sub.creator_id WHERE sub.provider='stripe' AND sub.status IN ('active','pending','past_due') AND ((sub.user_id=$1 AND NOT sub.cancel_at_period_end) OR c.user_id=$1)) OR EXISTS(SELECT 1 FROM checkouts co JOIN creators c ON c.id=co.creator_id WHERE co.provider='stripe' AND co.status='pending' AND (co.user_id=$1 OR c.user_id=$1))").bind(u.id).fetch_one(&mut *tx).await?;
        if active {
            return Err(bad(
                "Cancel recurring subscriptions and resolve pending payments or creator obligations before deactivating.",
            ));
        }
    }
    let unsettled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT a.id FROM ledger_postings p JOIN ledger_accounts a ON a.id=p.account_id JOIN ledger_transactions t ON t.id=p.transaction_id WHERE a.owner_id=$1 AND a.kind IN ('creator_pending','creator_available','reserve') AND t.status='posted' GROUP BY a.id HAVING sum(p.amount_minor)<>0)").bind(u.id).fetch_one(&mut *tx).await?;
    if unsettled {
        return Err(bad(
            "Resolve your creator balance and payouts before deactivating.",
        ));
    }
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE subscriptions SET cancel_at_period_end=true WHERE user_id=$1 AND status='active'",
    )
    .bind(u.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE creators SET verification_status='rejected',verified_at=NULL WHERE user_id=$1",
    )
    .bind(u.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE creator_id IN(SELECT id FROM creators WHERE user_id=$1)").bind(u.id).execute(&mut *tx).await?;
    audit(&mut tx, u.id, "account.deactivated", "user", u.id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"message":"Account deactivated. Records remain subject to the platform retention policy."}),
    ))
}
async fn password_slot() -> Result<tokio::sync::OwnedSemaphorePermit> {
    PASSWORD_SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| {
            Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Password service unavailable.",
            )
        })
}

pub async fn mail(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["cursor"])?;
    let u = user(&s, &h).await?;
    if s.mail.name() != "sandbox" {
        return Ok(Json(
            json!({"next_cursor":null,"deliveries":[],"mode":"smtp","warning":"Check your email inbox. Verification and reset links are never available here."}),
        ));
    }
    let (deliveries, next_cursor) = pagination::read(
        &s.db,
        "SELECT id,subject,body,created_at FROM email_deliveries WHERE user_id=$1 AND transport='sandbox'",
        Some(u.id),
        q.cursor("cursor")?,
        "created_at",
        false,
    )
    .await?;
    Ok(Json(
        json!({"next_cursor":next_cursor,"deliveries":deliveries,"mode":"sandbox","warning":"Local test inbox; this does not prove email ownership."}),
    ))
}

pub async fn hash_password(password: String) -> Result<String> {
    let permit = password_slot().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|v| v.to_string())
            .map_err(|_| bad("Password hashing failed."))
    })
    .await
    .map_err(|_| bad("Password service unavailable."))?
}
pub(super) async fn password_valid(password: String, hash: String) -> bool {
    let Ok(permit) = password_slot().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(&hash).is_ok_and(|v| {
            Argon2::default()
                .verify_password(password.as_bytes(), &v)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}
fn validate_password(value: &str) -> Result<()> {
    if !(12..=128).contains(&value.len()) {
        return Err(bad("Use a password between 12 and 128 bytes."));
    }
    Ok(())
}
fn email(value: &str) -> Result<String> {
    let v = value.trim().to_ascii_lowercase();
    let invalid = || bad("Enter a valid email address.");
    if v.len() > 254 || !v.is_ascii() {
        return Err(invalid());
    }
    let (local, domain) = v.split_once('@').ok_or_else(invalid)?;
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        || !domain.contains('.')
        || domain.split('.').any(|part| {
            part.is_empty()
                || part.len() > 63
                || part.starts_with('-')
                || part.ends_with('-')
                || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid());
    }
    Ok(v)
}
async fn session(s: &PlatformState, u: User) -> Result<Json<Value>> {
    let mut tx = s.db.begin().await?;
    let result = session_tx(&mut tx, &u, false, s.runtime.mode()).await?;
    tx.commit().await?;
    Ok(result)
}
async fn session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    u: &User,
    assured: bool,
    mode: &str,
) -> Result<Json<Value>> {
    let token = random_token();
    sqlx::query("INSERT INTO sessions(token_digest,user_id,mfa_verified_at) VALUES($1,$2,CASE WHEN $3 THEN now() ELSE NULL END)").bind(digest(&token)).bind(u.id).bind(assured && !u.mfa_recovery_required).execute(&mut **tx).await?;
    Ok(Json(
        json!({"token":token,"user":u,"expires_in":86400,"mode":mode,"message":if u.mfa_recovery_required {"Recovery code accepted. Replace your authenticator in Settings before administrative actions or payouts."} else {"Signed in."}}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    email: String,
    password: String,
    display_name: Option<String>,
    mfa_code: Option<String>,
    recovery_code: Option<String>,
    adult_confirmed: Option<bool>,
    country_code: Option<String>,
    terms_version: Option<String>,
    privacy_version: Option<String>,
    content_policy_version: Option<String>,
}
pub async fn register(
    State(s): State<PlatformState>,
    Json(b): Json<Credentials>,
) -> Result<Json<Value>> {
    let consent_supplied = b.adult_confirmed.is_some()
        || b.country_code.is_some()
        || b.terms_version.is_some()
        || b.privacy_version.is_some()
        || b.content_policy_version.is_some();
    if !s.runtime.policy.eligibility.registration_enabled {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Registration is currently closed.",
        ));
    }
    if (s.runtime.production || consent_supplied)
        && (b.adult_confirmed != Some(true)
            || !s
                .runtime
                .policy
                .eligibility
                .countries
                .iter()
                .any(|c| Some(c.code.as_str()) == b.country_code.as_deref())
            || b.terms_version.as_deref() != Some(s.runtime.policy.version("terms"))
            || b.privacy_version.as_deref() != Some(s.runtime.policy.version("privacy"))
            || b.content_policy_version.as_deref() != Some(s.runtime.policy.version("content")))
    {
        return Err(bad(
            "Confirm you are at least 18, select a supported country, and accept the current policies.",
        ));
    }
    rate_limit(&s, "public_auth_global", 120).await?;
    let address = email(&b.email)?;
    rate_limit(&s, &format!("register:{address}"), 5).await?;
    validate_password(&b.password)?;
    let name = b.display_name.unwrap_or_default().trim().to_owned();
    if !(1..=100).contains(&name.len()) {
        return Err(bad("Enter your display name."));
    }
    let hash = hash_password(b.password).await?;
    let mut tx = s.db.begin().await?;
    let u=sqlx::query_as::<_,User>("INSERT INTO app_users(email,display_name,password_hash) VALUES($1,$2,$3) RETURNING id,email,display_name,avatar_url,role,email_verified,mfa_enabled,mfa_recovery_required").bind(address.clone()).bind(name).bind(hash).fetch_one(&mut *tx).await?;
    if s.runtime.production || consent_supplied {
        sqlx::query("INSERT INTO policy_acceptances(user_id,country_code,terms_version,privacy_version,content_policy_version,minimum_age) VALUES($1,$2,$3,$4,$5,18)").bind(u.id).bind(&b.country_code).bind(&b.terms_version).bind(&b.privacy_version).bind(&b.content_policy_version).execute(&mut *tx).await?;
    }
    let token = random_token();
    sqlx::query("INSERT INTO recovery_tokens(token_digest,user_id,kind) VALUES($1,$2,'verify')")
        .bind(digest(&token))
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    super::mail::enqueue(
        &s,
        &mut tx,
        u.id,
        &address,
        "Verify your email",
        &format!("/auth?verify={token}"),
        Some(&digest(&token)),
    )
    .await?;
    notify(
        &mut tx,
        u.id,
        "Welcome to MyFans",
        "Your account is ready. Verify your email before publishing.",
    )
    .await?;
    tx.commit().await?;
    session(&s, u).await
}
pub async fn login(
    State(s): State<PlatformState>,
    Json(b): Json<Credentials>,
) -> Result<Json<Value>> {
    rate_limit(&s, "public_auth_global", 120).await?;
    let address = email(&b.email)?;
    rate_limit(&s, &format!("login:{address}"), 10).await?;
    if b.password.len() > 128 {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Email or password is incorrect.",
        ));
    }
    let row:Option<(Uuid,String)>=sqlx::query_as("SELECT id,COALESCE(password_hash,'') FROM app_users WHERE email=$1 AND disabled_at IS NULL").bind(address).fetch_optional(&s.db).await?;
    let hash = row
        .as_ref()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| s.dummy_hash.as_ref().clone());
    if !password_valid(b.password, hash.clone()).await || row.is_none() {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Email or password is incorrect.",
        ));
    }
    let id = row.ok_or(forbidden())?.0;
    if b.recovery_code.is_some() && b.mfa_code.is_some() {
        return Err(bad("Use an authenticator code or a recovery code."));
    }
    let mut tx = s.db.begin().await?;
    let mut u=sqlx::query_as::<_,User>("SELECT u.id,u.email,u.display_name,u.avatar_url,u.role,u.email_verified,u.mfa_enabled,u.mfa_recovery_required FROM app_users u WHERE id=$1 AND password_hash=$2 AND disabled_at IS NULL FOR UPDATE")
        .bind(id).bind(hash).fetch_optional(&mut *tx).await?.ok_or(Error(StatusCode::UNAUTHORIZED,"Email or password is incorrect."))?;
    if let Some(code) = b.recovery_code.as_deref() {
        if !u.mfa_enabled {
            return Err(Error(
                StatusCode::UNAUTHORIZED,
                "Enter a valid, unused recovery code.",
            ));
        }
        super::recovery::consume(&s, &mut tx, id, code).await?;
        u.mfa_recovery_required = true;
    } else if u.mfa_enabled {
        check_mfa_tx(&s, &mut tx, id, b.mfa_code.as_deref().unwrap_or("")).await?;
    }
    let result = session_tx(
        &mut tx,
        &u,
        u.mfa_enabled && b.recovery_code.is_none(),
        s.runtime.mode(),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
pub async fn logout(State(s): State<PlatformState>, h: HeaderMap) -> Result<StatusCode> {
    user(&s, &h).await?;
    if let Some(token) = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        sqlx::query("DELETE FROM sessions WHERE token_digest=$1")
            .bind(digest(token))
            .execute(&s.db)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
pub async fn account(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["subscriptions_cursor", "payments_cursor"])?;
    let u = optional_user(&s, &h).await?;
    let id = u.as_ref().map(|u| u.id);
    let onboarding = content::creator_onboarding(&s, id).await?;
    let creator: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM creators c WHERE user_id=$1")
            .bind(id)
            .fetch_optional(&s.db)
            .await?;
    let (subscriptions, subscriptions_next) = pagination::read(&s.db,"SELECT s.id,c.id AS creator_id,c.display_name,s.status,s.current_period_end,s.cancel_at_period_end,s.created_at FROM subscriptions s JOIN creators c ON c.id=s.creator_id WHERE s.user_id=$1",id,q.cursor("subscriptions_cursor")?,"created_at",false).await?;
    let (payments, payments_next) = pagination::read(
        &s.db,
        SUBSCRIBER_PAYMENTS,
        id,
        q.cursor("payments_cursor")?,
        "created_at",
        false,
    )
    .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mfa_recovery_codes WHERE user_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await?;
    Ok(Json(
        json!({"mail_transport":s.mail.name(),"recovery_codes_remaining":remaining,"next_cursors":{"subscriptions":subscriptions_next,"payments":payments_next},"user":u,"creator":creator,"onboarding":onboarding,"subscriptions":subscriptions,"payments":payments,"mode":s.runtime.mode()}),
    ))
}

// A checkout can have many recurring receipts. Once a receipt exists, show those actual
// charges rather than counting the original checkout intent as an additional payment.
// Receipt dispute flags are monotonic evidence, not the current dispute/access status.
const SUBSCRIBER_PAYMENTS: &str = r#"
SELECT r.id,x.id AS checkout_id,c.id AS creator_id,c.display_name AS creator_name,
       x.kind,CASE WHEN r.refunded_minor=r.amount_minor THEN 'refunded'
                   WHEN r.refunded_minor>0 THEN 'partially_refunded' ELSE 'paid' END AS status,
       r.amount_minor,r.refunded_minor,r.currency,r.created_at,r.period_start,r.period_end,
       x.provider,r.disputed AS dispute_recorded
FROM checkouts x JOIN provider_receipts r ON r.checkout_id=x.id
JOIN creators c ON c.id=x.creator_id WHERE x.user_id=$1 AND x.provider='stripe'
UNION ALL
SELECT x.id,x.id AS checkout_id,c.id AS creator_id,c.display_name AS creator_name,
       x.kind,x.status,x.amount_minor,x.refunded_minor,x.currency,x.created_at,
       NULL::timestamptz AS period_start,NULL::timestamptz AS period_end,
       x.provider,false AS dispute_recorded
FROM checkouts x JOIN creators c ON c.id=x.creator_id
WHERE x.user_id=$1 AND (x.provider='sandbox' OR NOT EXISTS(
    SELECT 1 FROM provider_receipts r WHERE r.checkout_id=x.id))
"#;

#[cfg(test)]
#[path = "auth_payments_tests.rs"]
mod payment_tests;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Name {
    display_name: String,
}
pub async fn update_account(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<Name>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let name = b.display_name.trim();
    if !(1..=100).contains(&name.len()) {
        return Err(bad("Use a display name between 1 and 100 characters."));
    }
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    sqlx::query("UPDATE app_users SET display_name=$2 WHERE id=$1")
        .bind(u.id)
        .bind(name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    account(State(s), h, Query(pagination::PageQuery::default())).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Password {
    current_password: String,
    password: String,
}
pub async fn change_password(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<Password>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    validate_password(&b.password)?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM app_users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !password_valid(b.current_password, hash).await {
        return Err(forbidden());
    }
    let hash = hash_password(b.password).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    sqlx::query("UPDATE app_users SET password_hash=$2 WHERE id=$1")
        .bind(u.id)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, u.id, "password_changed", "user", u.id).await?;
    tx.commit().await?;
    Ok(Json(json!({"message":"Password changed. Sign in again."})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    email: String,
}
pub async fn resend_verification(
    State(s): State<PlatformState>,
    h: HeaderMap,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    rate_limit(&s, "verification_resend_global", 120).await?;
    rate_limit(&s, &format!("verification_resend:{}", u.id), 1).await?;
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if !u.email_verified {
        sqlx::query("UPDATE recovery_tokens SET used_at=now() WHERE user_id=$1 AND kind='verify' AND used_at IS NULL").bind(u.id).execute(&mut *tx).await?;
        let token = random_token();
        sqlx::query(
            "INSERT INTO recovery_tokens(token_digest,user_id,kind) VALUES($1,$2,'verify')",
        )
        .bind(digest(&token))
        .bind(u.id)
        .execute(&mut *tx)
        .await?;
        super::mail::enqueue(
            &s,
            &mut tx,
            u.id,
            &u.email,
            "Verify your email",
            &format!("/auth?verify={token}"),
            Some(&digest(&token)),
        )
        .await?;
        audit(&mut tx, u.id, "email.verification.resent", "user", u.id).await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"message":"If verification is still needed, a message has been queued."}),
    ))
}
pub async fn recover(
    State(s): State<PlatformState>,
    Json(b): Json<Recovery>,
) -> Result<Json<Value>> {
    rate_limit(&s, "public_auth_global", 120).await?;
    let address = email(&b.email)?;
    rate_limit(&s, &format!("recover:{address}"), 3).await?;
    let mut tx = s.db.begin().await?;
    let id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM app_users WHERE email=$1 AND disabled_at IS NULL FOR UPDATE",
    )
    .bind(&address)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = id {
        let token = random_token();
        sqlx::query("INSERT INTO recovery_tokens(token_digest,user_id,kind) VALUES($1,$2,'reset')")
            .bind(digest(&token))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        super::mail::enqueue(
            &s,
            &mut tx,
            id,
            &address,
            "Reset your password",
            &format!("/auth?reset={token}"),
            Some(&digest(&token)),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(
        json!({"message":"If that account exists, a recovery message has been queued."}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reset {
    token: String,
    password: String,
    mfa_code: Option<String>,
    recovery_code: Option<String>,
}
pub async fn reset(State(s): State<PlatformState>, Json(b): Json<Reset>) -> Result<Json<Value>> {
    validate_password(&b.password)?;
    if b.recovery_code.is_some() && b.mfa_code.is_some() {
        return Err(bad("Use an authenticator code or a recovery code."));
    }
    rate_limit(&s, &format!("reset:{}", digest(&b.token)), 5).await?;
    let mut tx = s.db.begin().await?;
    let id:Option<Uuid>=sqlx::query_scalar("SELECT user_id FROM recovery_tokens WHERE token_digest=$1 AND kind='reset' AND used_at IS NULL AND expires_at>now()").bind(digest(&b.token)).fetch_optional(&mut *tx).await?;
    let id = id.ok_or(bad("Recovery link is invalid or expired."))?;
    let enabled: bool = sqlx::query_scalar(
        "SELECT mfa_enabled FROM app_users WHERE id=$1 AND disabled_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let valid: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM recovery_tokens WHERE token_digest=$1 AND user_id=$2 AND kind='reset' AND used_at IS NULL AND expires_at>now() FOR UPDATE")
        .bind(digest(&b.token)).bind(id).fetch_optional(&mut *tx).await?;
    if valid.is_none() {
        return Err(bad("Recovery link is invalid or expired."));
    }
    if enabled {
        if let Some(code) = b.recovery_code.as_deref() {
            super::recovery::consume(&s, &mut tx, id, code).await?;
        } else {
            check_mfa_tx(&s, &mut tx, id, b.mfa_code.as_deref().unwrap_or("")).await?;
        }
    } else if b.recovery_code.is_some() {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Enter a valid, unused recovery code.",
        ));
    }
    let hash = hash_password(b.password).await?;
    sqlx::query("UPDATE app_users SET password_hash=$2 WHERE id=$1")
        .bind(id)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE recovery_tokens SET used_at=now() WHERE user_id=$1 AND kind='reset' AND used_at IS NULL").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, id, "password_reset", "user", id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"message":if b.recovery_code.is_some() {"Password reset. Sign in with another unused recovery code, then replace your authenticator in Settings."} else {"Password reset. You can sign in now."}}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token {
    pub(super) token: String,
}
pub async fn verify_email(
    State(s): State<PlatformState>,
    Json(b): Json<Token>,
) -> Result<Json<Value>> {
    rate_limit(&s, "verification_global", 120).await?;
    if b.token.len() != 64 {
        return Err(bad("Verification link is invalid or expired."));
    }
    rate_limit(&s, &format!("verification:{}", digest(&b.token)), 5).await?;
    let mut tx = s.db.begin().await?;
    let id:Option<Uuid>=sqlx::query_scalar("SELECT user_id FROM recovery_tokens WHERE token_digest=$1 AND kind='verify' AND used_at IS NULL AND expires_at>now()").bind(digest(&b.token)).fetch_optional(&mut *tx).await?;
    let id = id.ok_or(bad("Verification link is invalid or expired."))?;
    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM app_users WHERE id=$1 AND disabled_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if active.is_none() {
        return Err(bad("Verification link is invalid or expired."));
    }
    let consumed:Option<Uuid>=sqlx::query_scalar("UPDATE recovery_tokens SET used_at=now() WHERE token_digest=$1 AND kind='verify' AND used_at IS NULL AND expires_at>now() RETURNING user_id").bind(digest(&b.token)).fetch_optional(&mut *tx).await?;
    if consumed.is_none() {
        return Err(bad("Verification link is invalid or expired."));
    }
    sqlx::query("UPDATE recovery_tokens SET used_at=now() WHERE user_id=$1 AND kind='verify' AND used_at IS NULL").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE app_users SET email_verified=true WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"message":"Email verified."})))
}
fn totp(secret: Vec<u8>, email: String) -> Result<Totp> {
    Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(Secret::new(secret.into_boxed_slice()))
        .with_issuer(Some("MyFans"))
        .with_account_name(email)
        .build()
        .map_err(|_| bad("Invalid MFA configuration."))
}
pub(super) fn encrypt(s: &PlatformState, bytes: &[u8]) -> Result<Vec<u8>> {
    let nonce_id = Uuid::new_v4();
    let nonce: Nonce<aes_gcm::aead::consts::U12> = nonce_id.as_bytes()[..12]
        .try_into()
        .map_err(|_| bad("MFA unavailable."))?;
    let cipher = Aes256Gcm::new_from_slice(s.key.as_ref()).map_err(|_| bad("MFA unavailable."))?;
    let encrypted = cipher
        .encrypt(&nonce, bytes)
        .map_err(|_| bad("MFA unavailable."))?;
    let mut result = nonce.to_vec();
    result.extend(encrypted);
    Ok(result)
}
pub(super) fn decrypt(s: &PlatformState, bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 28 {
        return Err(bad("Invalid MFA configuration."));
    }
    let nonce: Nonce<aes_gcm::aead::consts::U12> = bytes[..12]
        .try_into()
        .map_err(|_| bad("MFA unavailable."))?;
    Aes256Gcm::new_from_slice(s.key.as_ref())
        .map_err(|_| bad("MFA unavailable."))?
        .decrypt(&nonce, &bytes[12..])
        .map_err(|_| bad("MFA unavailable."))
}
pub(super) async fn check_mfa_tx(
    s: &PlatformState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    code: &str,
) -> Result<()> {
    let (secret, email, last): (Option<Vec<u8>>, String, i64) = sqlx::query_as(
        "SELECT mfa_secret,email,mfa_last_step FROM app_users WHERE id=$1 AND disabled_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let secret = secret.ok_or(bad("Set up two-factor authentication before enabling it."))?;
    let totp = totp(decrypt(s, &secret)?, email)?;
    let now = chrono::Utc::now().timestamp() as u64;
    let step = totp.check(code, now).ok_or(Error(
        StatusCode::UNAUTHORIZED,
        "Enter a valid, unused two-factor code.",
    ))?;
    if step <= last as u64 {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Enter a valid, unused two-factor code.",
        ));
    }
    let changed =
        sqlx::query("UPDATE app_users SET mfa_last_step=$2 WHERE id=$1 AND mfa_last_step<$2")
            .bind(id)
            .bind(step as i64)
            .execute(&mut **tx)
            .await?;
    if changed.rows_affected() != 1 {
        return Err(forbidden());
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mfa {
    action: String,
    code: Option<String>,
    current_password: Option<String>,
}
pub async fn mfa(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(b): Json<Mfa>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    rate_limit(&s, &format!("mfa:{}", u.id), 10).await?;
    match b.action.as_str() {
        "setup" => {
            let mut tx = s.db.begin().await?;
            let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
            if u.mfa_enabled && !u.mfa_recovery_required {
                return Err(bad("Two-factor authentication is already enabled."));
            }
            let secret = Uuid::new_v4()
                .as_bytes()
                .iter()
                .chain(Uuid::new_v4().as_bytes().iter())
                .copied()
                .collect::<Vec<_>>();
            let t = totp(secret.clone(), u.email)?;
            let changed = sqlx::query("UPDATE app_users SET mfa_secret=$2,mfa_last_step=0,mfa_replacement_pending=mfa_recovery_required WHERE id=$1 AND (NOT mfa_enabled OR mfa_recovery_required)")
                .bind(u.id)
                .bind(encrypt(&s, &secret)?)
                .execute(&mut *tx)
                .await?;
            if changed.rows_affected() != 1 {
                return Err(bad("Two-factor authentication is already enabled."));
            }
            tx.commit().await?;
            Ok(Json(
                json!({"secret":Secret::new(secret.into_boxed_slice()).to_base32(),"otpauth_url":t.to_url().map_err(|_|bad("MFA URL unavailable."))?}),
            ))
        }
        "enable" => {
            let mut tx = s.db.begin().await?;
            authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
            check_mfa_tx(&s, &mut tx, u.id, b.code.as_deref().unwrap_or("")).await?;
            let (enabled, required, pending): (bool, bool, bool) = sqlx::query_as(
                "SELECT mfa_enabled,mfa_recovery_required,mfa_replacement_pending FROM app_users WHERE id=$1",
            )
            .bind(u.id)
            .fetch_one(&mut *tx)
            .await?;
            if required && !pending {
                return Err(bad(
                    "Set up a replacement authenticator before enabling it.",
                ));
            }
            let codes = if !enabled || required {
                Some(super::recovery::rotate(&mut tx, u.id).await?)
            } else {
                None
            };
            if required {
                sqlx::query("DELETE FROM sessions WHERE user_id=$1 AND token_digest<>$2")
                    .bind(u.id)
                    .bind(digest(
                        h.get(header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.strip_prefix("Bearer "))
                            .ok_or(forbidden())?,
                    ))
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query(
                "UPDATE app_users SET mfa_enabled=true,mfa_recovery_required=false,mfa_replacement_pending=false WHERE id=$1",
            )
            .bind(u.id)
            .execute(&mut *tx)
            .await?;
            let token = h
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .ok_or(forbidden())?;
            sqlx::query(
                "UPDATE sessions SET mfa_verified_at=now() WHERE token_digest=$1 AND user_id=$2",
            )
            .bind(digest(token))
            .bind(u.id)
            .execute(&mut *tx)
            .await?;
            audit(&mut tx, u.id, "mfa.enabled", "user", u.id).await?;
            tx.commit().await?;
            Ok(Json(
                json!({"message":"Two-factor authentication enabled.","recovery_codes":codes}),
            ))
        }
        "recovery_codes" => {
            let password = b
                .current_password
                .ok_or(bad("Enter your current password."))?;
            if password.len() > 128 {
                return Err(forbidden());
            }
            let hash: String =
                sqlx::query_scalar("SELECT password_hash FROM app_users WHERE id=$1")
                    .bind(u.id)
                    .fetch_one(&s.db)
                    .await?;
            if !password_valid(password, hash.clone()).await {
                return Err(forbidden());
            }
            let mut tx = s.db.begin().await?;
            authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
            let eligible:bool=sqlx::query_scalar("SELECT mfa_enabled AND NOT mfa_recovery_required AND password_hash=$2 FROM app_users WHERE id=$1 AND disabled_at IS NULL FOR UPDATE").bind(u.id).bind(hash).fetch_one(&mut *tx).await?;
            if !eligible {
                return Err(bad(
                    "Complete authenticator enrollment before generating recovery codes.",
                ));
            }
            check_mfa_tx(&s, &mut tx, u.id, b.code.as_deref().unwrap_or("")).await?;
            let codes = super::recovery::rotate(&mut tx, u.id).await?;
            let token = h
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .ok_or(forbidden())?;
            sqlx::query("DELETE FROM sessions WHERE user_id=$1 AND token_digest<>$2")
                .bind(u.id)
                .bind(digest(token))
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE sessions SET mfa_verified_at=now() WHERE user_id=$1 AND token_digest=$2",
            )
            .bind(u.id)
            .bind(digest(token))
            .execute(&mut *tx)
            .await?;
            audit(&mut tx, u.id, "mfa.recovery.rotated", "user", u.id).await?;
            notify(&mut tx,u.id,"Recovery codes replaced","New recovery codes were generated. All previous recovery codes and other sessions were revoked.").await?;
            tx.commit().await?;
            Ok(Json(
                json!({"message":"Recovery codes replaced. Save the new codes now; they will not be shown again.","recovery_codes":codes}),
            ))
        }
        _ => Err(bad("Choose setup, enable, or recovery_codes.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passwords_are_bounded() {
        assert!(validate_password("short").is_err());
        assert!(validate_password(&"x".repeat(12)).is_ok());
        assert!(validate_password(&"x".repeat(128)).is_ok());
        assert!(validate_password(&"x".repeat(129)).is_err());
    }
    #[test]
    fn addresses_are_normalized_and_validated() {
        assert_eq!(
            email(" User+tag@EXAMPLE.test ").ok().as_deref(),
            Some("user+tag@example.test")
        );
        for v in [
            "@example.test",
            "user@",
            "a@@example.test",
            "a..b@example.test",
            "a@-example.test",
            "a@exa mple.test",
            "a@example..test",
        ] {
            assert!(email(v).is_err(), "{v}");
        }
    }
    #[tokio::test]
    async fn hashes_are_salted_and_passwords_verified() {
        let a = hash_password("StrongPassword!2026".into())
            .await
            .unwrap_or_default();
        let b = hash_password("StrongPassword!2026".into())
            .await
            .unwrap_or_default();
        assert!(!a.is_empty());
        assert_ne!(a, b);
        assert!(password_valid("StrongPassword!2026".into(), a.clone()).await);
        assert!(!password_valid("WrongPassword!2026".into(), a).await);
    }
    #[tokio::test]
    async fn mfa_encryption_authenticates_ciphertext() {
        let state = PlatformState {
            runtime: Default::default(),
            db: PgPoolOptions::new()
                .connect_lazy("postgresql://localhost/test")
                .unwrap_or_else(|_| panic!("invalid fixture")),
            media_root: PathBuf::new(),
            storage: MediaStore::local(PathBuf::new()),
            scanner: Default::default(),
            providers: Default::default(),
            key: Arc::new([9; 32]),
            dummy_hash: Arc::new(String::new()),
            mail: Default::default(),
        };
        let encrypted = encrypt(&state, b"secret").unwrap_or_default();
        assert_eq!(decrypt(&state, &encrypted).ok(), Some(b"secret".to_vec()));
        let mut changed = encrypted;
        changed[12] ^= 1;
        assert!(decrypt(&state, &changed).is_err());
    }
}
