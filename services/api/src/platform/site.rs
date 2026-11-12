use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Operator {
    legal_name: String,
    address: String,
    support_email: String,
    privacy_email: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    heading: String,
    body: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub kind: String,
    title: String,
    pub version: String,
    updated_at: chrono::NaiveDate,
    sections: Vec<Section>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Country {
    pub code: String,
    name: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Eligibility {
    minimum_age: u8,
    pub countries: Vec<Country>,
    pub registration_enabled: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SitePolicy {
    pub retention: privacy::RetentionPolicy,
    operator: Operator,
    pub policies: Vec<Policy>,
    pub eligibility: Eligibility,
}

#[derive(Clone)]
pub struct RuntimeConfig {
    pub production: bool,
    pub policy: SitePolicy,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self { production: false, policy: SitePolicy {
            retention:privacy::RetentionPolicy {erasure_delay_days:0,backup_days:35,security_days:30,financial_days:365,policy_evidence_days:365,moderation_evidence_days:365},
            operator: Operator { legal_name: "Local development sandbox".into(), address: "Not a public service".into(), support_email: "support@myfans.test".into(), privacy_email: "privacy@myfans.test".into() },
            policies: ["terms", "privacy", "content", "refunds"].into_iter().map(|kind| Policy {
                kind: kind.into(), title: format!("Sandbox {kind}"), version: "sandbox-1".into(),
                updated_at: chrono::NaiveDate::from_ymd_opt(2026,10,7).unwrap_or_default(),
                sections: vec![Section { heading: "Development use".into(), body: "This local sandbox uses test accounts and simulated payments. Use only non-sensitive test data. Explicit adult content is prohibited. Public operation requires the operator's published policies and approved providers.".into() }],
            }).collect(),
            eligibility: Eligibility { minimum_age: 18, countries: vec![Country { code: "US".into(), name: "United States (sandbox)".into() }], registration_enabled: true },
        }}
    }
}
fn text_valid(s: &str, max: usize) -> bool {
    !s.trim().is_empty()
        && s.len() <= max
        && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}
impl RuntimeConfig {
    pub fn mode(&self) -> &'static str {
        if self.production {
            "production"
        } else {
            "sandbox"
        }
    }
    pub async fn from_env() -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let production = match std::env::var("APP_MODE").as_deref() {
            Err(_) | Ok("sandbox") => false,
            Ok("production") => true,
            _ => return Err("APP_MODE must be sandbox or production".into()),
        };
        let mut config = Self {
            production,
            ..Self::default()
        };
        if let Some(path) = std::env::var_os("OPERATOR_POLICY_FILE") {
            use tokio::io::AsyncReadExt;
            let file = tokio::fs::File::open(path)
                .await
                .map_err(|_| "Cannot open OPERATOR_POLICY_FILE")?;
            let info = file.metadata().await?;
            if !info.is_file() || info.len() > 128 * 1024 {
                return Err("Operator policy must be a regular file of at most 128 KiB".into());
            }
            let mut bytes = Vec::new();
            file.take(128 * 1024 + 1).read_to_end(&mut bytes).await?;
            if bytes.len() > 128 * 1024 {
                return Err("Operator policy exceeds 128 KiB".into());
            }
            config.policy = serde_json::from_slice(&bytes)
                .map_err(|_| "Operator policy JSON does not match the required schema")?;
            config.policy.validate()?;
        } else if production {
            return Err(
                "Production requires OPERATOR_POLICY_FILE with the operator's published policies"
                    .into(),
            );
        }
        if production {
            let origin = providers::public_origin()?;
            let raw = url::Url::parse(&origin)?;
            let host = raw.host_str().unwrap_or_default();
            if host == "localhost"
                || host.ends_with(".test")
                || host.ends_with(".example")
                || host.ends_with(".invalid")
                || host.parse::<std::net::IpAddr>().is_ok()
            {
                return Err("Production requires a real HTTPS domain".into());
            }
            if std::env::var("MAIL_PUBLIC_ORIGIN").ok().as_deref() != Some(origin.as_str()) {
                return Err("MAIL_PUBLIC_ORIGIN must exactly match PUBLIC_ORIGIN".into());
            }
            for (name, expected) in [
                ("PAYMENT_PROVIDER", "stripe"),
                ("IDENTITY_PROVIDER", "sumsub"),
                ("SUMSUB_MODE", "live"),
                ("MEDIA_STORAGE", "s3"),
                ("MAIL_TRANSPORT", "smtp"),
                ("APP_RUN_MIGRATIONS", "false"),
            ] {
                if std::env::var(name).ok().as_deref() != Some(expected) {
                    return Err(format!("Production requires {name}={expected}").into());
                }
            }
            for name in [
                "MFA_ENCRYPTION_KEY_FILE",
                "DATABASE_URL_FILE",
                "MAIL_FEEDBACK_QUEUE_URL",
                "SES_CONFIGURATION_SET",
            ] {
                if std::env::var_os(name).is_none() {
                    return Err(format!("Production requires {name}").into());
                }
            }
            if std::env::var_os("DATABASE_URL").is_some() {
                return Err("Production database credentials must use DATABASE_URL_FILE".into());
            }
        }
        Ok(config)
    }
}
impl SitePolicy {
    fn validate(&self) -> std::result::Result<(), &'static str> {
        self.retention.validate()?;
        if !text_valid(&self.operator.legal_name, 200) || !text_valid(&self.operator.address, 1000)
        {
            return Err("Operator legal name and address are required");
        }
        for email in [&self.operator.support_email, &self.operator.privacy_email] {
            let address: lettre::Address = email
                .parse()
                .map_err(|_| "Operator contact email is invalid")?;
            if email.len() > 254
                || address.domain().ends_with(".test")
                || address.domain().ends_with(".example")
            {
                return Err("Real operator contact email is required");
            }
        }
        if self.eligibility.minimum_age != 18
            || self.eligibility.countries.is_empty()
            || self.eligibility.countries.len() > 249
        {
            return Err("An 18+ policy and explicit supported countries are required");
        }
        let mut codes = std::collections::HashSet::new();
        for c in &self.eligibility.countries {
            if c.code.len() != 2
                || !c.code.bytes().all(|b| b.is_ascii_uppercase())
                || !text_valid(&c.name, 100)
                || !codes.insert(&c.code)
            {
                return Err("Invalid or duplicate eligibility country");
            }
        }
        if self.policies.len() != 4 {
            return Err("Terms, privacy, content and refunds policies are required");
        }
        for kind in ["terms", "privacy", "content", "refunds"] {
            let p = self
                .policies
                .iter()
                .find(|p| p.kind == kind)
                .ok_or("Missing required policy")?;
            if !text_valid(&p.title, 200)
                || p.version.is_empty()
                || p.version.len() > 64
                || !p
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || p.sections.is_empty()
                || p.sections.len() > 40
                || p.updated_at > chrono::Utc::now().date_naive()
            {
                return Err("Invalid policy metadata");
            }
            if p.sections
                .iter()
                .any(|s| !text_valid(&s.heading, 200) || !text_valid(&s.body, 12000))
            {
                return Err("Policy section text is missing or too long");
            }
        }
        Ok(())
    }
    pub fn version(&self, kind: &str) -> &str {
        self.policies
            .iter()
            .find(|p| p.kind == kind)
            .map_or("", |p| p.version.as_str())
    }
}
pub async fn site(State(s): State<PlatformState>) -> Json<Value> {
    let mut value = serde_json::to_value(&s.runtime.policy).unwrap_or_else(|_| json!({}));
    value["mode"] = json!(s.runtime.mode());
    value["ready"] = json!(true);
    Json(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> SitePolicy {
        let mut p = RuntimeConfig::default().policy;
        p.operator.support_email = "support@operator.org".into();
        p.operator.privacy_email = "privacy@operator.org".into();
        p
    }
    #[test]
    fn policy_requires_complete_current_documents_contacts_and_retention() {
        assert!(policy().validate().is_ok());
        let mut p = policy();
        p.policies.pop();
        assert!(p.validate().is_err());
        let mut p = policy();
        p.policies[0].kind = "privacy".into();
        assert!(p.validate().is_err());
        let mut p = policy();
        p.policies[0].sections[0].body = " ".into();
        assert!(p.validate().is_err());
        let mut p = policy();
        p.operator.privacy_email = "privacy@example.test".into();
        assert!(p.validate().is_err());
        let mut p = policy();
        p.eligibility.minimum_age = 17;
        assert!(p.validate().is_err());
        let mut p = policy();
        p.eligibility.countries.clear();
        assert!(p.validate().is_err());
        let mut p = policy();
        p.retention.backup_days = 1;
        assert!(p.validate().is_err());
        let mut p = policy();
        p.policies[0].version = "version\nforged".into();
        assert!(p.validate().is_err());
    }
}
