use super::*;
use aws_sdk_sqs::{
    Client,
    config::{BehaviorVersion, Region, retry::RetryConfig, timeout::TimeoutConfig},
};

#[derive(Clone)]
pub struct Feedback {
    client: Client,
    queue: String,
    account: String,
    pub configuration_set: String,
}
impl Feedback {
    pub async fn from_env() -> std::result::Result<Option<Self>, Box<dyn std::error::Error>> {
        let queue = std::env::var("MAIL_FEEDBACK_QUEUE_URL").ok();
        let set = std::env::var("SES_CONFIGURATION_SET").ok();
        let (queue, set) =
            match (queue, set) {
                (None, None) => return Ok(None),
                (Some(q), Some(s)) => (q, s),
                _ => return Err(
                    "SES_CONFIGURATION_SET and MAIL_FEEDBACK_QUEUE_URL must be configured together"
                        .into(),
                ),
            };
        let region =
            std::env::var("AWS_REGION").map_err(|_| "AWS_REGION is required for mail feedback")?;
        let account = std::env::var("S3_EXPECTED_BUCKET_OWNER")
            .map_err(|_| "S3_EXPECTED_BUCKET_OWNER is required for mail feedback")?;
        validate(&queue, &region, &account, &set)?;
        if ["AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL_SQS"]
            .iter()
            .any(|v| std::env::var_os(v).is_some())
        {
            return Err("Custom SQS endpoints are unsupported".into());
        }
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region))
            .retry_config(RetryConfig::standard().with_max_attempts(3))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(3))
                    .operation_timeout(Duration::from_secs(30))
                    .operation_attempt_timeout(Duration::from_secs(25))
                    .build(),
            )
            .load()
            .await;
        Ok(Some(Self {
            client: Client::new(&config),
            queue,
            account,
            configuration_set: set,
        }))
    }
    pub async fn run(self, s: PlatformState, mut stop: tokio::sync::watch::Receiver<bool>) {
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! { biased; _=stop.changed()=>break, result=self.tick(&s)=> {
                if let Err(e)=result { tracing::error!(status=e.0.as_u16(),"mail_feedback_poll_failed");
                    tokio::select! { _=stop.changed()=>break,_=tokio::time::sleep(Duration::from_secs(10))=>{} }
                }
            }}
        }
    }
    async fn tick(&self, s: &PlatformState) -> Result<()> {
        let result = self
            .client
            .receive_message()
            .queue_url(&self.queue)
            .wait_time_seconds(20)
            .visibility_timeout(120)
            .max_number_of_messages(5)
            .send()
            .await
            .map_err(|_| providers::unavailable())?;
        for message in result.messages() {
            let body = message.body().ok_or_else(providers::unavailable)?;
            // RawMessageDelivery on the source-scoped SNS subscription. No public endpoint accepts this data.
            match ingest(s, &self.account, &self.configuration_set, body).await {
                Ok(()) => {
                    self.client
                        .delete_message()
                        .queue_url(&self.queue)
                        .receipt_handle(
                            message
                                .receipt_handle()
                                .ok_or_else(providers::unavailable)?,
                        )
                        .send()
                        .await
                        .map_err(|_| providers::unavailable())?;
                }
                Err(e) => tracing::error!(status = e.0.as_u16(), "mail_feedback_message_rejected"),
            }
        }
        Ok(())
    }
}
fn validate(
    queue: &str,
    region: &str,
    account: &str,
    set: &str,
) -> std::result::Result<(), &'static str> {
    let url = url::Url::parse(queue).map_err(|_| "Invalid mail feedback queue URL")?;
    let expected = format!("sqs.{region}.amazonaws.com");
    if account.len() != 12
        || !account.bytes().all(|b| b.is_ascii_digit())
        || region.len() > 32
        || !region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || url.scheme() != "https"
        || url.host_str() != Some(expected.as_str())
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with(&format!("/{account}/"))
        || url.path().split('/').count() != 3
        || url.path().ends_with('/')
        || set.is_empty()
        || set.len() > 64
        || !set
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(
            "Mail feedback must use the deployment account's regional SQS queue and a valid SES configuration set",
        );
    }
    Ok(())
}
async fn ingest(s: &PlatformState, account: &str, set: &str, body: &str) -> Result<()> {
    if body.len() > 256 * 1024 {
        return Err(bad("Mail feedback exceeds its size limit."));
    }
    let v: Value = serde_json::from_str(body).map_err(|_| bad("Invalid mail feedback JSON."))?;
    let tags = &v["mail"]["tags"];
    if v["mail"]["sendingAccountId"] != account || tags["ses:configuration-set"] != json!([set]) {
        return Err(forbidden());
    }
    let ids = tags["myfans_delivery"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or(bad("Mail feedback has no delivery correlation."))?;
    let id = ids[0]
        .as_str()
        .and_then(|x| Uuid::parse_str(x).ok())
        .ok_or(bad("Mail feedback delivery is invalid."))?;
    let (kind, recipients) = match v["eventType"].as_str() {
        Some("Bounce") => (
            if v["bounce"]["bounceType"] == "Permanent" {
                "bounce"
            } else {
                "transient_bounce"
            },
            v["bounce"]["bouncedRecipients"].as_array(),
        ),
        Some("Complaint") => (
            "complaint",
            v["complaint"]["complainedRecipients"].as_array(),
        ),
        Some("Delivery") => ("delivery", None),
        _ => return Err(bad("Unsupported mail feedback event.")),
    };
    let mut tx = s.db.begin().await?;
    let recipient: Option<String> = sqlx::query_scalar(
        "SELECT recipient FROM email_deliveries WHERE id=$1 AND transport='smtp' FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let recipient = recipient.ok_or(bad("Mail feedback delivery is unknown."))?;
    let destinations = v["mail"]["destination"]
        .as_array()
        .ok_or(bad("Mail feedback recipient is missing."))?;
    if destinations.len() != 1
        || destinations[0]
            .as_str()
            .map(|a| a.to_lowercase())
            .as_deref()
            != Some(recipient.as_str())
    {
        return Err(bad("Mail feedback recipient does not match delivery."));
    }
    if let Some(recipients) = recipients {
        if recipients.len() != 1
            || recipients[0]["emailAddress"]
                .as_str()
                .map(|a| a.to_lowercase())
                .as_deref()
                != Some(recipient.as_str())
        {
            return Err(bad(
                "Mail feedback affected recipient does not match delivery.",
            ));
        }
    } else if kind != "delivery" {
        return Err(bad("Mail feedback affected recipient is missing."));
    }
    let inserted=sqlx::query("INSERT INTO mail_feedback(event_digest,delivery_id,kind) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(digest(body)).bind(id).bind(kind).execute(&mut *tx).await?.rows_affected();
    if inserted > 0 && matches!(kind, "bounce" | "complaint") {
        sqlx::query("INSERT INTO mail_suppressions(recipient_digest,reason) VALUES($1,$2) ON CONFLICT(recipient_digest) DO UPDATE SET reason=CASE WHEN EXCLUDED.reason='complaint' THEN 'complaint' ELSE mail_suppressions.reason END").bind(digest(&recipient)).bind(kind).execute(&mut *tx).await?;
        sqlx::query("UPDATE email_deliveries SET status='cancelled',encrypted_body=NULL,lease_id=NULL,locked_at=NULL,last_error='Recipient suppressed after provider feedback' WHERE recipient=$1 AND transport='smtp' AND status IN ('pending','sending')").bind(recipient).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn feedback_suppresses_only_correlated_recipient_and_is_idempotent(db: PgPool) {
        let s = PlatformState {
            runtime: Default::default(),
            db,
            media_root: PathBuf::new(),
            key: Arc::new([1; 32]),
            dummy_hash: Arc::new(String::new()),
            mail: Default::default(),
            storage: MediaStore::local(PathBuf::new()),
            scanner: Default::default(),
            providers: Default::default(),
        };
        let user:Uuid=sqlx::query_scalar("INSERT INTO app_users(email,display_name) VALUES('feedback@test.example','Feedback') RETURNING id").fetch_one(&s.db).await.unwrap_or_else(|_|panic!("fixture user"));
        let delivery:Uuid=sqlx::query_scalar("INSERT INTO email_deliveries(user_id,recipient,subject,body,transport,status) VALUES($1,'feedback@test.example','Test','Message queued for email delivery.','smtp','sent') RETURNING id").bind(user).fetch_one(&s.db).await.unwrap_or_else(|_|panic!("fixture delivery"));
        let mut v = json!({"eventType":"Bounce","mail":{"sendingAccountId":"123456789012","destination":["feedback@test.example"],"tags":{"ses:configuration-set":["myfans"],"myfans_delivery":[delivery.to_string()]}},"bounce":{"bounceType":"Permanent","bouncedRecipients":[{"emailAddress":"other@test.example"}]}});
        assert!(
            ingest(&s, "123456789012", "myfans", &v.to_string())
                .await
                .is_err()
        );
        v["bounce"]["bouncedRecipients"][0]["emailAddress"] = json!("feedback@test.example");
        assert!(
            ingest(&s, "000000000000", "myfans", &v.to_string())
                .await
                .is_err()
        );
        for _ in 0..2 {
            assert!(
                ingest(&s, "123456789012", "myfans", &v.to_string())
                    .await
                    .is_ok()
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_feedback")
                .fetch_one(&s.db)
                .await
                .ok(),
            Some(1)
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_suppressions")
                .fetch_one(&s.db)
                .await
                .ok(),
            Some(1)
        );
        let mut tx = s.db.begin().await.unwrap_or_else(|_| panic!("transaction"));
        assert!(
            mail::enqueue(
                &s,
                &mut tx,
                user,
                "feedback@test.example",
                "Test",
                "private",
                None
            )
            .await
            .is_ok()
        );
        assert!(tx.commit().await.is_ok());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM email_deliveries")
                .fetch_one(&s.db)
                .await
                .ok(),
            Some(1)
        );
    }
    #[test]
    fn feedback_queue_rejects_untrusted_hosts_and_accounts() {
        let queue = "https://sqs.us-east-1.amazonaws.com/123456789012/feedback";
        assert!(validate(queue, "us-east-1", "123456789012", "myfans").is_ok());
        for q in [
            "http://sqs.us-east-1.amazonaws.com/123456789012/feedback",
            "https://sqs.us-east-1.amazonaws.com.attacker.test/123456789012/feedback",
            "https://sqs.us-east-1.amazonaws.com/000000000000/feedback",
            "https://sqs.us-east-1.amazonaws.com/123456789012/feedback?test=1",
        ] {
            assert!(validate(q, "us-east-1", "123456789012", "myfans").is_err());
        }
    }
}
