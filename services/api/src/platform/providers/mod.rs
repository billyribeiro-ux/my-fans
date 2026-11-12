use super::*;
use hmac::{Hmac, Mac, digest::KeyInit};
use sha2::Sha256;

pub mod identity;
pub mod stripe;

pub const STRIPE_VERSION: &str = "2026-09-30.endive";

#[derive(Clone, Default)]
pub struct Providers {
    pub stripe: Option<Arc<stripe::Stripe>>,
    pub identity: Option<Arc<identity::Sumsub>>,
}
impl Providers {
    pub async fn from_env() -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self {
            stripe: stripe::Stripe::from_env(http.clone()).await?.map(Arc::new),
            identity: identity::Sumsub::from_env(http).await?.map(Arc::new),
        })
    }
    pub async fn bind(&self, db: &PgPool) -> Result<()> {
        let identity = if self.stripe.is_none() && self.identity.is_none() {
            "sandbox".to_string()
        } else {
            let account = if let Some(p) = &self.stripe {
                p.platform_identity().await?
            } else {
                "sandbox".into()
            };
            let kyc = if let Some(p) = &self.identity {
                p.binding()
            } else {
                "sandbox".into()
            };
            format!("providers:{}", digest(&format!("{account}\n{kyc}")))
        };
        let mut tx = db.begin().await?;
        let previous: String =
            sqlx::query_scalar("SELECT identity FROM platform_providers WHERE id=1 FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        if previous != identity {
            let occupied:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM checkouts) OR EXISTS(SELECT 1 FROM provider_accounts) OR EXISTS(SELECT 1 FROM identity_checks) OR EXISTS(SELECT 1 FROM creators WHERE verification_status='approved')").fetch_one(&mut *tx).await?;
            if occupied {
                return Err(Error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Provider configuration changed with existing records. Use an isolated environment or a reviewed migration.",
                ));
            }
            sqlx::query("UPDATE platform_providers SET identity=$1 WHERE id=1")
                .bind(identity)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
pub async fn status(State(s): State<PlatformState>) -> Json<Value> {
    Json(
        json!({"mode":s.runtime.mode(),"payments": if s.providers.stripe.is_some() {"stripe"} else {"sandbox"}, "identity":if s.providers.identity.is_some() {"sumsub"} else {"sandbox"}, "content_policy":"general", "configured":{"payments":s.providers.stripe.is_some(),"identity":s.providers.identity.is_some(),"storage":s.storage.is_remote(),"scanner":s.scanner.enabled()}}),
    )
}
pub async fn secret(name: &str) -> std::result::Result<String, Box<dyn std::error::Error>> {
    use tokio::io::AsyncReadExt;
    let path = std::env::var_os(name).ok_or("Required provider secret file is not configured")?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "Cannot open provider secret file")?;
    let info = file
        .metadata()
        .await
        .map_err(|_| "Cannot inspect provider secret file")?;
    if !info.is_file() || info.len() > 4096 {
        return Err("Provider secret must be a regular file of at most 4096 bytes".into());
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read provider secret file")?;
    let value = String::from_utf8(bytes).map_err(|_| "Provider secret must be UTF-8")?;
    let value = value.trim_end_matches(['\n', '\r']);
    if value.len() < 16 || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("Provider secret format is invalid".into());
    }
    Ok(value.to_owned())
}
pub fn public_origin() -> std::result::Result<String, Box<dyn std::error::Error>> {
    let value =
        std::env::var("PUBLIC_ORIGIN").map_err(|_| "PUBLIC_ORIGIN is required for providers")?;
    let url = url::Url::parse(&value).map_err(|_| "Invalid PUBLIC_ORIGIN")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host_str().is_none()
    {
        return Err("PUBLIC_ORIGIN must be an HTTPS origin".into());
    }
    Ok(url.origin().ascii_serialization())
}
pub fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "The service provider is unavailable. Please retry later.",
    )
}
pub fn signature(key: &[u8], bytes: &[u8]) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| unavailable())?;
    mac.update(bytes);
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
pub fn verify_signature(key: &[u8], bytes: &[u8], hex: &str) -> bool {
    if hex.len() != 64 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let parsed: Option<Vec<u8>> = (0..32)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    let Some(parsed) = parsed else { return false };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return false;
    };
    mac.update(bytes);
    mac.verify_slice(&parsed).is_ok()
}
pub async fn response_json(mut response: reqwest::Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err(unavailable());
    }
    if response.content_length().is_some_and(|v| v > 1024 * 1024) {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}
pub fn hosted_url(value: &Value, host: &str) -> Result<String> {
    let raw = value["url"].as_str().ok_or_else(unavailable)?;
    let url = url::Url::parse(raw).map_err(|_| unavailable())?;
    if raw.len() > 8192
        || raw.chars().any(char::is_control)
        || url.scheme() != "https"
        || url.host_str() != Some(host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || (host != "checkout.stripe.com" && url.fragment().is_some())
    {
        return Err(unavailable());
    }
    Ok(url.to_string())
}
pub fn id(value: &Value, prefix: &str) -> Result<String> {
    let id = value
        .as_str()
        .or_else(|| value["id"].as_str())
        .ok_or_else(unavailable)?;
    if id.len() > 255
        || !id.starts_with(prefix)
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(unavailable());
    }
    Ok(id.to_owned())
}
