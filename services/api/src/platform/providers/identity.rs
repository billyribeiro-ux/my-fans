use super::*;
use axum::body::Bytes;
use chrono::{Datelike, NaiveDate, Utc};

pub struct Sumsub {
    http: reqwest::Client,
    token: String,
    secret: String,
    webhook_secret: String,
    level: String,
    sandbox: bool,
    client_id: String,
    #[cfg(test)]
    endpoint: Option<String>,
}
impl Sumsub {
    pub fn is_live(&self) -> bool {
        !self.sandbox
    }
    pub(super) fn binding(&self) -> String {
        format!("sumsub:{}:{}:{}", self.sandbox, self.level, self.client_id)
    }
    pub async fn from_env(
        http: reqwest::Client,
    ) -> std::result::Result<Option<Self>, Box<dyn std::error::Error>> {
        match std::env::var("IDENTITY_PROVIDER").as_deref() {
            Err(_) | Ok("sandbox") => {
                if [
                    "SUMSUB_APP_TOKEN_FILE",
                    "SUMSUB_SECRET_KEY_FILE",
                    "SUMSUB_WEBHOOK_SECRET_FILE",
                    "SUMSUB_LEVEL",
                    "SUMSUB_CLIENT_ID",
                ]
                .iter()
                .any(|v| std::env::var_os(v).is_some())
                {
                    return Err(
                        "Set IDENTITY_PROVIDER=sumsub before supplying Sumsub configuration".into(),
                    );
                }
                return Ok(None);
            }
            Ok("sumsub") => {}
            _ => return Err("IDENTITY_PROVIDER must be sandbox or sumsub".into()),
        }
        let level = std::env::var("SUMSUB_LEVEL").map_err(|_| "SUMSUB_LEVEL is required")?;
        if level.is_empty() || level.len() > 100 || level.chars().any(char::is_control) {
            return Err("Invalid SUMSUB_LEVEL".into());
        }
        let sandbox = match std::env::var("SUMSUB_MODE").as_deref() {
            Ok("test") => true,
            Ok("live") => false,
            _ => return Err("SUMSUB_MODE must be test or live".into()),
        };
        let client_id =
            std::env::var("SUMSUB_CLIENT_ID").map_err(|_| "SUMSUB_CLIENT_ID is required")?;
        if client_id.is_empty()
            || client_id.len() > 128
            || !client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err("Invalid SUMSUB_CLIENT_ID".into());
        }
        if sandbox == (std::env::var("APP_MODE").as_deref() == Ok("production")) {
            return Err(
                "Sumsub mode must match APP_MODE (test for sandbox, live for production)".into(),
            );
        }
        Ok(Some(Self {
            http,
            token: secret("SUMSUB_APP_TOKEN_FILE").await?,
            secret: secret("SUMSUB_SECRET_KEY_FILE").await?,
            webhook_secret: secret("SUMSUB_WEBHOOK_SECRET_FILE").await?,
            level,
            sandbox,
            client_id,
            #[cfg(test)]
            endpoint: None,
        }))
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let timestamp = Utc::now().timestamp().to_string();
        let body = body.map(|v| v.to_string()).unwrap_or_default();
        let input = format!("{timestamp}{}{path}{body}", method.as_str());
        #[cfg(not(test))]
        let endpoint = "https://api.sumsub.com";
        #[cfg(test)]
        let endpoint = self.endpoint.as_deref().unwrap_or("https://api.sumsub.com");
        let mut request = self
            .http
            .request(method, format!("{endpoint}{path}"))
            .header("X-App-Token", &self.token)
            .header("X-App-Access-Ts", &timestamp)
            .header(
                "X-App-Access-Sig",
                signature(self.secret.as_bytes(), input.as_bytes())?,
            );
        if !body.is_empty() {
            request = request
                .header("Content-Type", "application/json")
                .body(body);
        }
        response_json(request.send().await.map_err(|_| unavailable())?).await
    }
}
pub async fn start(State(s): State<PlatformState>, h: HeaderMap) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !u.email_verified {
        return Err(bad("Verify your email before identity verification."));
    }
    mfa_session(&s, &h).await?;
    rate_limit(&s, &format!("identity:{}", u.id), 5).await?;
    let provider = s.providers.identity.as_ref().ok_or_else(unavailable)?;
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::RecentMfa).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    let creator: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE user_id=$1")
        .bind(u.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO identity_checks(creator_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(creator)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let value = provider
        .request(
            reqwest::Method::POST,
            "/resources/sdkIntegrations/levels/-/websdkLink",
            Some(json!({"levelName":provider.level,"userId":creator.to_string(),"ttlInSecs":1800})),
        )
        .await?;
    authorize_current(&s, &h, u.id, Assurance::RecentMfa).await?;
    Ok(Json(
        json!({"url":hosted_url(&value,"inquiry.sumsub.com")?}),
    ))
}
pub async fn webhook(
    State(s): State<PlatformState>,
    h: HeaderMap,
    body: Bytes,
) -> Result<StatusCode> {
    let p = s.providers.identity.as_ref().ok_or_else(unavailable)?;
    let digest_header = h
        .get("x-payload-digest")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if h.get("x-payload-digest-alg").and_then(|v| v.to_str().ok()) != Some("HMAC_SHA256_HEX")
        || !verify_signature(p.webhook_secret.as_bytes(), &body, digest_header)
    {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Invalid provider signature.",
        ));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|_| bad("Invalid provider event."))?;
    if v["testMode"] == true {
        return Ok(StatusCode::NO_CONTENT);
    }
    if v["sandboxMode"].as_bool() != Some(p.sandbox) {
        return Err(bad("Provider environment mismatch."));
    }
    let creator = v["externalUserId"]
        .as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(bad("Invalid provider subject."))?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM identity_checks WHERE creator_id=$1)")
            .bind(creator)
            .fetch_one(&s.db)
            .await?;
    if !exists {
        return Ok(StatusCode::NO_CONTENT);
    }
    let hash = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut tx = s.db.begin().await?;
    let inserted=sqlx::query("INSERT INTO payment_webhook_inbox(provider,event_id,payload_sha256) VALUES('sumsub',$1,$1) ON CONFLICT DO NOTHING").bind(&hash).execute(&mut *tx).await?;
    if inserted.rows_affected() == 1 {
        sqlx::query(
            "INSERT INTO jobs(kind,payload) VALUES('identity_sync',$1) ON CONFLICT DO NOTHING",
        )
        .bind(json!({"creator_id":creator,"event_id":hash}))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
fn adult(dob: &str, today: NaiveDate) -> bool {
    let Ok(date) = NaiveDate::parse_from_str(dob, "%Y-%m-%d") else {
        return false;
    };
    if date > today || today.year() - date.year() > 120 {
        return false;
    }
    let age = today.year()
        - date.year()
        - i32::from((today.month(), today.day()) < (date.month(), date.day()));
    age >= 18
}
pub async fn sync(s: &PlatformState, payload: &Value) -> Result<()> {
    let p = s.providers.identity.as_ref().ok_or_else(unavailable)?;
    let creator = payload["creator_id"]
        .as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(bad("Invalid identity job."))?;
    // Lock before fetching: overlapping notifications must never restore an older approval.
    let mut tx = s.db.begin().await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='45s'")
        .execute(&mut *tx)
        .await?;
    let _: Uuid =
        sqlx::query_scalar("SELECT creator_id FROM identity_checks WHERE creator_id=$1 FOR UPDATE")
            .bind(creator)
            .fetch_one(&mut *tx)
            .await?;
    let v = p
        .request(
            reqwest::Method::GET,
            &format!("/resources/applicants/-;externalUserId={creator}/one"),
            None,
        )
        .await?;
    if v["externalUserId"] != creator.to_string()
        || v["review"]["levelName"] != p.level
        || v["clientId"] != p.client_id
    {
        return Err(unavailable());
    }
    let applicant = v["id"]
        .as_str()
        .filter(|v| v.len() == 24 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(unavailable)?;
    let steps = p
        .request(
            reqwest::Method::GET,
            &format!("/resources/applicants/{applicant}/requiredIdDocsStatus"),
            None,
        )
        .await?;
    let approved_doc = &steps["IDENTITY"];
    let verified_birth = v["info"]["idDocs"]
        .as_array()
        .and_then(|docs| {
            docs.iter().find(|doc| {
                doc["idDocType"] == approved_doc["idDocType"]
                    && doc["country"] == approved_doc["country"]
            })
        })
        .and_then(|doc| doc["dob"].as_str());
    let status = if v["review"]["reviewStatus"] == "completed"
        && v["review"]["reviewResult"]["reviewAnswer"] == "GREEN"
        && v["deleted"] != true
        && approved_doc["reviewResult"]["reviewAnswer"] == "GREEN"
        && approved_doc["forbidden"] != true
        && steps["SELFIE"]["reviewResult"]["reviewAnswer"] == "GREEN"
        && adult(verified_birth.unwrap_or_default(), Utc::now().date_naive())
    {
        "verified"
    } else if v["review"]["reviewStatus"] == "completed" || v["deleted"] == true {
        "rejected"
    } else {
        "pending"
    };
    let updated=sqlx::query("UPDATE identity_checks SET applicant_id=$2,status=$3,checked_at=now(),next_check_at=now()+interval '6 hours' WHERE creator_id=$1 AND (applicant_id IS NULL OR applicant_id=$2)").bind(creator).bind(applicant).bind(status).execute(&mut *tx).await?;
    if updated.rows_affected() != 1 {
        return Err(unavailable());
    }
    if status != "verified" {
        sqlx::query("UPDATE creators SET verification_status='pending',verified_at=NULL WHERE id=$1 AND verification_status='approved'").bind(creator).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO audit_events(action,resource_type,resource_id,metadata) VALUES('identity.checked','creator',$1,$2)").bind(creator).bind(json!({"provider":"sumsub","status":status})).execute(&mut *tx).await?;
    sqlx::query("UPDATE payment_webhook_inbox SET processed_at=now() WHERE provider='sumsub' AND event_id=$1").bind(payload["event_id"].as_str().unwrap_or_default()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
#[cfg(test)]
#[path = "identity_tests.rs"]
mod contracts;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn age_uses_the_verified_birthday_and_rejects_bad_dates() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        assert!(adult("2008-10-07", today));
        assert!(!adult("2008-10-08", today));
        for dob in ["", "2000-13-01", "2027-01-01", "1900-01-01"] {
            assert!(!adult(dob, today));
        }
    }
}
