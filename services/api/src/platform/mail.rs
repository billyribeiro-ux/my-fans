use super::*;
use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, KeyInit, Payload},
};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use sqlx::{Postgres, Transaction};
use std::{future::Future, pin::Pin};
use url::Url;

const PRIVATE_BODY: &str = "Message queued for email delivery.";
type ConfigResult<T> = std::result::Result<T, &'static str>;
#[derive(Clone, Copy)]
enum Failure {
    Transient,
    Permanent,
}
trait DeliveryTransport: Send + Sync {
    fn send(
        &self,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), Failure>> + Send + '_>>;
}
struct Smtp(AsyncSmtpTransport<Tokio1Executor>);
impl DeliveryTransport for Smtp {
    fn send(
        &self,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), Failure>> + Send + '_>> {
        Box::pin(async move {
            self.0.send(message).await.map(|_| ()).map_err(|e| {
                if e.is_permanent() {
                    Failure::Permanent
                } else {
                    Failure::Transient
                }
            })
        })
    }
}
#[derive(Clone)]
struct Configured {
    transport: Arc<dyn DeliveryTransport>,
    sender: Mailbox,
    origin: Url,
    configuration_set: Option<String>,
}
/// No external delivery occurs by default. Credentials and payloads never implement Debug.
#[derive(Clone, Default)]
pub struct MailService {
    configured: Option<Configured>,
    feedback: Option<Arc<mail_feedback::Feedback>>,
}
impl MailService {
    pub fn name(&self) -> &'static str {
        if self.configured.is_some() {
            "smtp"
        } else {
            "sandbox"
        }
    }
    pub async fn from_env() -> ConfigResult<Self> {
        let read = |key: &str| std::env::var(key).ok();
        match read("MAIL_TRANSPORT").as_deref() {
            None | Some("sandbox") => {
                if [
                    "SMTP_HOST",
                    "SMTP_USERNAME",
                    "SMTP_PASSWORD_FILE",
                    "MAIL_FROM",
                    "MAIL_PUBLIC_ORIGIN",
                ]
                .iter()
                .any(|key| read(key).is_some())
                {
                    return Err(
                        "Set MAIL_TRANSPORT=smtp explicitly when configuring email delivery",
                    );
                }
                Ok(Self::default())
            }
            Some("smtp") => {
                let password_path =
                    read("SMTP_PASSWORD_FILE").ok_or("SMTP_PASSWORD_FILE is required")?;
                let metadata = tokio::fs::metadata(&password_path)
                    .await
                    .map_err(|_| "SMTP password file cannot be read")?;
                if !metadata.is_file() || metadata.len() > 4096 {
                    return Err("SMTP password file must be a small regular file");
                }
                let password = tokio::fs::read_to_string(password_path)
                    .await
                    .map_err(|_| "SMTP password file cannot be read")?;
                let mut service =
                    Self::smtp(&read, password.trim_end_matches(['\r', '\n']).to_owned())?;
                service.feedback = mail_feedback::Feedback::from_env()
                    .await
                    .map_err(|_| "Invalid SES feedback configuration")?
                    .map(Arc::new);
                if let Some(ref feedback) = service.feedback
                    && let Some(ref mut configured) = service.configured
                {
                    configured.configuration_set = Some(feedback.configuration_set.clone());
                }
                Ok(service)
            }
            _ => Err("MAIL_TRANSPORT must be sandbox or smtp"),
        }
    }
    fn smtp(read: &impl Fn(&str) -> Option<String>, password: String) -> ConfigResult<Self> {
        let required = |key| {
            read(key)
                .filter(|v| !v.trim().is_empty())
                .ok_or("SMTP host, username, sender and public origin are required")
        };
        let host = required("SMTP_HOST")?;
        if host.len() > 253
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return Err("SMTP_HOST must be a DNS hostname");
        }
        let username = required("SMTP_USERNAME")?;
        if username.len() > 512 || password.is_empty() {
            return Err("SMTP credentials are invalid");
        }
        let security = read("SMTP_SECURITY").unwrap_or_else(|| "starttls".into());
        let builder = match security.as_str() {
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
            _ => {
                return Err(
                    "SMTP_SECURITY must be starttls or tls; plaintext delivery is disabled",
                );
            }
        }
        .map_err(|_| "Invalid SMTP TLS configuration")?;
        let port: u16 = read("SMTP_PORT")
            .unwrap_or_else(|| if security == "tls" { "465" } else { "587" }.into())
            .parse()
            .map_err(|_| "Invalid SMTP_PORT")?;
        if port == 0 {
            return Err("Invalid SMTP_PORT");
        }
        let sender: Mailbox = required("MAIL_FROM")?
            .parse()
            .map_err(|_| "Invalid MAIL_FROM")?;
        let origin = public_origin(&required("MAIL_PUBLIC_ORIGIN")?)?;
        let transport = builder
            .port(port)
            .credentials(Credentials::new(username, password))
            .timeout(Some(Duration::from_secs(10)))
            .build();
        Ok(Self {
            configured: Some(Configured {
                transport: Arc::new(Smtp(transport)),
                sender,
                origin,
                configuration_set: None,
            }),
            feedback: None,
        })
    }
}
fn public_origin(value: &str) -> ConfigResult<Url> {
    let origin = Url::parse(value).map_err(|_| "Invalid MAIL_PUBLIC_ORIGIN")?;
    let local = origin
        .host_str()
        .is_some_and(|v| ["127.0.0.1", "localhost", "[::1]"].contains(&v));
    if !(origin.scheme() == "https" || origin.scheme() == "http" && local)
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(
            "MAIL_PUBLIC_ORIGIN must be an HTTPS origin (loopback HTTP is allowed for local testing)",
        );
    }
    Ok(origin)
}
fn cipher(s: &PlatformState) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(s.key.as_ref()).map_err(|_| bad("Email service unavailable."))
}
pub(super) fn seal(s: &PlatformState, id: Uuid, recipient: &str, body: &[u8]) -> Result<Vec<u8>> {
    let entropy = Sha256::digest(random_token().as_bytes());
    let nonce: &aes_gcm::Nonce<aes_gcm::aead::consts::U12> = entropy[..12]
        .try_into()
        .map_err(|_| bad("Email service unavailable."))?;
    let aad = format!("myfans:mail:v1:{id}:{recipient}");
    let encrypted = cipher(s)?
        .encrypt(
            nonce,
            Payload {
                msg: body,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| bad("Email service unavailable."))?;
    Ok([nonce.as_slice(), encrypted.as_slice()].concat())
}
pub(super) fn open(s: &PlatformState, id: Uuid, recipient: &str, bytes: &[u8]) -> Result<String> {
    if bytes.len() < 28 {
        return Err(bad("Email service unavailable."));
    }
    let nonce: &aes_gcm::Nonce<aes_gcm::aead::consts::U12> = bytes[..12]
        .try_into()
        .map_err(|_| bad("Email service unavailable."))?;
    let aad = format!("myfans:mail:v1:{id}:{recipient}");
    let bytes = cipher(s)?
        .decrypt(
            nonce,
            Payload {
                msg: &bytes[12..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| bad("Email service unavailable."))?;
    String::from_utf8(bytes).map_err(|_| bad("Email service unavailable."))
}
pub(super) async fn enqueue(
    s: &PlatformState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    recipient: &str,
    subject: &str,
    body: &str,
    recovery_digest: Option<&str>,
) -> Result<()> {
    let suppressed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_suppressions WHERE recipient_digest=$1)",
    )
    .bind(digest(&recipient.to_lowercase()))
    .fetch_one(&mut **tx)
    .await?;
    if suppressed {
        return Ok(());
    }
    let id = Uuid::new_v4();
    let (stored, encrypted) = if let Some(configured) = &s.mail.configured {
        let body = if body.starts_with("/auth?") {
            configured
                .origin
                .join(body)
                .map_err(|_| bad("Email service unavailable."))?
                .to_string()
        } else {
            body.to_owned()
        };
        (PRIVATE_BODY, Some(seal(s, id, recipient, body.as_bytes())?))
    } else {
        (body, None)
    };
    sqlx::query("INSERT INTO email_deliveries(id,user_id,recipient,subject,body,transport,encrypted_body,recovery_digest,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,CASE WHEN $8::text IS NULL THEN now()+interval '1 day' ELSE (SELECT expires_at FROM recovery_tokens WHERE token_digest=$8) END)")
        .bind(id).bind(user).bind(recipient).bind(subject).bind(stored).bind(s.mail.name()).bind(encrypted).bind(recovery_digest).execute(&mut **tx).await?;
    Ok(())
}
#[derive(sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    recipient: String,
    subject: String,
    encrypted_body: Option<Vec<u8>>,
    attempts: i32,
}
pub(super) async fn run(s: PlatformState, mut stop: tokio::sync::watch::Receiver<bool>) {
    let feedback_handle = s
        .mail
        .feedback
        .as_ref()
        .map(|f| tokio::spawn(f.as_ref().clone().run(s.clone(), stop.clone())));
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {biased; _=stop.changed()=>break, _=interval.tick()=>{}}
        if let Err(e) = tick(&s).await {
            tracing::error!(status = e.0.as_u16(), "mail_worker_tick_failed");
        }
    }
    if let Some(handle) = feedback_handle {
        let _ = handle.await;
    }
}
/// One leased delivery per tick; SMTP acceptance is at least once, not proof of inbox delivery.
pub(super) async fn tick(s: &PlatformState) -> Result<()> {
    let Some(configured) = &s.mail.configured else {
        return Ok(());
    };
    sqlx::query("UPDATE email_deliveries e SET status='cancelled',locked_at=NULL,lease_id=NULL,encrypted_body=NULL,last_error='Recipient suppressed after provider feedback' WHERE transport='smtp' AND status IN ('pending','sending') AND EXISTS(SELECT 1 FROM mail_suppressions m WHERE m.recipient_digest=encode(sha256(convert_to(lower(e.recipient),'UTF8')),'hex'))").execute(&s.db).await?;
    // Expired/consumed reset links and cancelled resends never enter SMTP. Erase their bodies.
    sqlx::query("UPDATE email_deliveries e SET status='cancelled',locked_at=NULL,lease_id=NULL,encrypted_body=NULL,last_error='Message expired or link no longer valid' WHERE transport='smtp' AND status IN ('pending','sending') AND (expires_at<=now() OR (recovery_digest IS NOT NULL AND NOT EXISTS(SELECT 1 FROM recovery_tokens r WHERE r.token_digest=e.recovery_digest AND r.used_at IS NULL AND r.expires_at>now())))")
        .execute(&s.db).await?;
    sqlx::query("UPDATE email_deliveries SET status=CASE WHEN attempts>=8 THEN 'failed' ELSE 'pending' END,encrypted_body=CASE WHEN attempts>=8 THEN NULL ELSE encrypted_body END,locked_at=NULL,lease_id=NULL,last_error='Delivery lease expired' WHERE transport='smtp' AND status='sending' AND locked_at<now()-interval '5 minutes'")
        .execute(&s.db).await?;
    let lease = Uuid::new_v4();
    let claimed:Option<Claimed>=sqlx::query_as("UPDATE email_deliveries SET status='sending',locked_at=now(),lease_id=$1,attempts=attempts+1 WHERE id=(SELECT id FROM email_deliveries WHERE transport='smtp' AND status='pending' AND available_at<=now() AND attempts<8 ORDER BY available_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,recipient,subject,encrypted_body,attempts")
        .bind(lease).fetch_optional(&s.db).await?;
    let Some(mail) = claimed else {
        return Ok(());
    };
    let message = (|| {
        let body = open(
            s,
            mail.id,
            &mail.recipient,
            mail.encrypted_body
                .as_deref()
                .ok_or(bad("Email payload missing."))?,
        )?;
        let recipient: Mailbox = mail
            .recipient
            .parse()
            .map_err(|_| bad("Email recipient invalid."))?;
        let mut builder = Message::builder();
        if let Some(set) = &configured.configuration_set {
            use lettre::message::header::{HeaderName, HeaderValue};
            builder = builder
                .raw_header(HeaderValue::new(
                    HeaderName::new_from_ascii_str("X-SES-CONFIGURATION-SET"),
                    set.clone(),
                ))
                .raw_header(HeaderValue::new(
                    HeaderName::new_from_ascii_str("X-SES-MESSAGE-TAGS"),
                    format!("myfans_delivery={}", mail.id),
                ));
        }
        builder
            .from(configured.sender.clone())
            .to(recipient)
            .subject(mail.subject)
            .message_id(Some(format!(
                "<{}@{}>",
                mail.id,
                configured.sender.email.domain()
            )))
            .header(ContentType::TEXT_PLAIN)
            .body(body)
            .map_err(|_| bad("Email payload invalid."))
    })();
    let result = match message {
        Ok(message) => {
            tokio::time::timeout(Duration::from_secs(20), configured.transport.send(message))
                .await
                .unwrap_or(Err(Failure::Transient))
        }
        Err(_) => Err(Failure::Permanent),
    };
    let (status, reason) = match result {
        Ok(()) => ("sent", None),
        Err(Failure::Permanent) => ("failed", Some("Email delivery rejected")),
        Err(Failure::Transient) if mail.attempts >= 8 => {
            ("failed", Some("Email delivery retry limit reached"))
        }
        Err(Failure::Transient) => ("pending", Some("Email delivery temporarily unavailable")),
    };
    sqlx::query("UPDATE email_deliveries SET status=$3,locked_at=NULL,lease_id=NULL,last_error=$4,available_at=now()+make_interval(secs=>$5),sent_at=CASE WHEN $3='sent' THEN now() ELSE NULL END,encrypted_body=CASE WHEN $3 IN ('sent','failed') THEN NULL ELSE encrypted_body END WHERE id=$1 AND lease_id=$2 AND status='sending'")
        .bind(mail.id).bind(lease).bind(status).bind(reason).bind((2_i32.pow(mail.attempts as u32)*5).min(900) as f64).execute(&s.db).await?;
    tracing::info!(delivery_id=%mail.id,status,attempt=mail.attempts,"mail_delivery_completed");
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    struct Mock {
        calls: AtomicUsize,
        first_failure: Option<Failure>,
        messages: Mutex<Vec<String>>,
    }
    impl DeliveryTransport for Mock {
        fn send(
            &self,
            message: Message,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<(), Failure>> + Send + '_>> {
            Box::pin(async move {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                self.messages.lock().unwrap().push(
                    String::from_utf8_lossy(&message.formatted())
                        .replace("=\r\n", "")
                        .replace("=3D", "="),
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
                if n == 0
                    && let Some(f) = self.first_failure
                {
                    return Err(f);
                }
                Ok(())
            })
        }
    }
    async fn fixture(db: PgPool, failure: Option<Failure>) -> (PlatformState, Uuid, Arc<Mock>) {
        let mock = Arc::new(Mock {
            calls: AtomicUsize::new(0),
            first_failure: failure,
            messages: Mutex::new(Vec::new()),
        });
        let configured = Configured {
            transport: mock.clone(),
            sender: "MyFans <hello@myfans.example>".parse().unwrap(),
            origin: public_origin("https://myfans.example").unwrap(),
            configuration_set: None,
        };
        let s = PlatformState {
            runtime: Default::default(),
            db,
            media_root: PathBuf::new(),
            storage: MediaStore::local(PathBuf::new()),
            scanner: Default::default(),
            providers: Default::default(),
            key: Arc::new([3; 32]),
            dummy_hash: Arc::new(String::new()),
            mail: MailService {
                configured: Some(configured),
                feedback: None,
            },
        };
        let user=sqlx::query_scalar("INSERT INTO app_users(email,display_name,password_hash) VALUES('mail@test.example','Mail','unused') RETURNING id").fetch_one(&s.db).await.unwrap();
        (s, user, mock)
    }
    async fn queue(s: &PlatformState, user: Uuid) -> (Uuid, String) {
        let token = random_token();
        let mut tx = s.db.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO recovery_tokens(token_digest,user_id,kind) VALUES($1,$2,'verify')",
        )
        .bind(digest(&token))
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
        enqueue(
            s,
            &mut tx,
            user,
            "mail@test.example",
            "Verify your email",
            &format!("/auth?verify={token}"),
            Some(&digest(&token)),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let id = sqlx::query_scalar("SELECT id FROM email_deliveries WHERE recovery_digest=$1")
            .bind(digest(&token))
            .fetch_one(&s.db)
            .await
            .unwrap();
        (id, token)
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn smtp_outbox_is_private_leased_and_verifies_only_by_consuming_the_link(db: PgPool) {
        let (s, user, mock) = fixture(db, None).await;
        let (id, token) = queue(&s, user).await;
        let (body, encrypted): (String, Vec<u8>) =
            sqlx::query_as("SELECT body,encrypted_body FROM email_deliveries WHERE id=$1")
                .bind(id)
                .fetch_one(&s.db)
                .await
                .unwrap();
        assert_eq!(body, PRIVATE_BODY);
        assert_eq!(
            open(&s, id, "mail@test.example", &encrypted).unwrap(),
            format!("https://myfans.example/auth?verify={token}")
        );
        assert!(open(&s, Uuid::new_v4(), "mail@test.example", &encrypted).is_err());
        assert!(open(&s, id, "attacker@test.example", &encrypted).is_err());
        assert_eq!(verify_storage(&s).await.ok(), Some((0, 1)));
        let wrong_key = PlatformState {
            key: Arc::new([8; 32]),
            ..s.clone()
        };
        assert!(verify_storage(&wrong_key).await.is_err());
        let raw_session = random_token();
        sqlx::query("INSERT INTO sessions(token_digest,user_id) VALUES($1,$2)")
            .bind(digest(&raw_session))
            .bind(user)
            .execute(&s.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO email_deliveries(user_id,recipient,subject,body) VALUES($1,'mail@test.example','Old local fixture','/auth?verify=local-fixture')").bind(user).execute(&s.db).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {raw_session}").parse().unwrap(),
        );
        let inbox = auth::mail(
            State(s.clone()),
            headers,
            axum::extract::Query(pagination::PageQuery::default()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(inbox["mode"], "smtp");
        assert_eq!(inbox["deliveries"], json!([]));
        let (a, b) = tokio::join!(tick(&s), tick(&s));
        a.unwrap();
        b.unwrap();
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
        let (status, attempts, ciphertext): (String, i32, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT status,attempts,encrypted_body FROM email_deliveries WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&s.db)
        .await
        .unwrap();
        assert_eq!((status.as_str(), attempts), ("sent", 1));
        assert!(ciphertext.is_none());
        assert!(
            mock.messages.lock().unwrap()[0]
                .contains(&format!("https://myfans.example/auth?verify={token}"))
        );
        assert!(
            !sqlx::query_scalar::<_, bool>("SELECT email_verified FROM app_users WHERE id=$1")
                .bind(user)
                .fetch_one(&s.db)
                .await
                .unwrap()
        );
        let _ = auth::verify_email(
            State(s.clone()),
            Json(auth::Token {
                token: token.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(
            auth::verify_email(State(s.clone()), Json(auth::Token { token }))
                .await
                .is_err()
        );
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT email_verified FROM app_users WHERE id=$1")
                .bind(user)
                .fetch_one(&s.db)
                .await
                .unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM email_deliveries WHERE transport='sandbox' AND attempts=0"
            )
            .fetch_one(&s.db)
            .await
            .unwrap(),
            1
        );
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn smtp_retries_expiry_and_recipient_tampering_preserve_delivery_boundaries(db: PgPool) {
        let (s, user, mock) = fixture(db, Some(Failure::Transient)).await;
        let (id, _) = queue(&s, user).await;
        tick(&s).await.unwrap();
        let (status, delayed): (String, bool) =
            sqlx::query_as("SELECT status,available_at>now() FROM email_deliveries WHERE id=$1")
                .bind(id)
                .fetch_one(&s.db)
                .await
                .unwrap();
        assert_eq!(status, "pending");
        assert!(delayed);
        tick(&s).await.unwrap();
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
        // Simulate a worker crash and reclaim its old lease without sleeping five minutes.
        sqlx::query("UPDATE email_deliveries SET status='sending',lease_id=$2,locked_at=now()-interval '6 minutes' WHERE id=$1").bind(id).bind(Uuid::new_v4()).execute(&s.db).await.unwrap();
        sqlx::query(
            "UPDATE email_deliveries SET available_at=now()-interval '1 second' WHERE id=$1",
        )
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
        tick(&s).await.unwrap();
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
        let messages = mock.messages.lock().unwrap().clone();
        let message_id = |v: &str| {
            v.lines()
                .find(|line| line.starts_with("Message-ID:"))
                .unwrap()
                .to_owned()
        };
        assert_eq!(message_id(&messages[0]), message_id(&messages[1]));
        let (expired, _) = queue(&s, user).await;
        sqlx::query("UPDATE email_deliveries SET expires_at=now()-interval '1 second' WHERE id=$1")
            .bind(expired)
            .execute(&s.db)
            .await
            .unwrap();
        let (consumed, token) = queue(&s, user).await;
        sqlx::query("UPDATE recovery_tokens SET used_at=now() WHERE token_digest=$1")
            .bind(digest(&token))
            .execute(&s.db)
            .await
            .unwrap();
        tick(&s).await.unwrap();
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM email_deliveries WHERE id=ANY($1) AND status='cancelled' AND encrypted_body IS NULL").bind(vec![expired,consumed]).fetch_one(&s.db).await.unwrap(),2);
        let (tampered, _) = queue(&s, user).await;
        sqlx::query("UPDATE email_deliveries SET recipient='attacker@test.example' WHERE id=$1")
            .bind(tampered)
            .execute(&s.db)
            .await
            .unwrap();
        tick(&s).await.unwrap();
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM email_deliveries WHERE id=$1")
                .bind(tampered)
                .fetch_one(&s.db)
                .await
                .unwrap(),
            "failed"
        );
    }
    #[test]
    fn origin_and_transport_configuration_fail_closed() {
        for invalid in [
            "http://myfans.example",
            "https://user:password@example.com",
            "https://example.com/path",
            "https://example.com/?x=1",
            "https://example.com/#frag",
            "file:///tmp/mail",
        ] {
            assert!(public_origin(invalid).is_err());
        }
        for valid in [
            "https://myfans.example",
            "http://127.0.0.1:5173",
            "http://[::1]:5173",
        ] {
            assert!(public_origin(valid).is_ok());
        }
        let read = |key: &str| {
            Some(
                match key {
                    "SMTP_HOST" => "smtp.example.com",
                    "SMTP_USERNAME" => "user",
                    "MAIL_FROM" => "MyFans <hello@myfans.example>",
                    "MAIL_PUBLIC_ORIGIN" => "https://myfans.example",
                    "SMTP_SECURITY" => "none",
                    _ => return None,
                }
                .to_owned(),
            )
        };
        assert!(MailService::smtp(&read, "secret".into()).is_err());
        assert!(MailService::smtp(&read, String::new()).is_err());
    }
    #[tokio::test]
    async fn smtp_requires_starttls_before_credentials_or_message_data() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            write.write_all(b"220 localhost ESMTP\r\n").await.unwrap();
            let mut commands = String::new();
            read.read_line(&mut commands).await.unwrap();
            assert!(commands.starts_with("EHLO"));
            write
                .write_all(b"250-localhost\r\n250 OK\r\n")
                .await
                .unwrap();
            let mut extra = String::new();
            let _ = tokio::time::timeout(Duration::from_secs(1), read.read_line(&mut extra)).await;
            commands.push_str(&extra);
            commands
        });
        let smtp = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay("127.0.0.1")
            .unwrap()
            .port(port)
            .credentials(Credentials::new(
                "fixture-user".into(),
                "fixture-secret".into(),
            ))
            .timeout(Some(Duration::from_secs(2)))
            .build();
        let message = Message::builder()
            .from("sender@test.example".parse().unwrap())
            .to("recipient@test.example".parse().unwrap())
            .body("Private test message".to_owned())
            .unwrap();
        assert!(smtp.send(message).await.is_err());
        let commands = server.await.unwrap();
        assert!(!commands.contains("AUTH"));
        assert!(!commands.contains("MAIL FROM"));
        assert!(!commands.contains("DATA"));
    }
    #[sqlx::test(migrations = "../../db/migrations")]
    #[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
    async fn smtp_permanent_failures_and_exhausted_retries_are_terminal(db: PgPool) {
        let (mut s, user, mock) = fixture(db, Some(Failure::Permanent)).await;
        let (permanent, _) = queue(&s, user).await;
        tick(&s).await.unwrap();
        tick(&s).await.unwrap();
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM email_deliveries WHERE id=$1")
                .bind(permanent)
                .fetch_one(&s.db)
                .await
                .unwrap(),
            "failed"
        );
        let transient = Arc::new(Mock {
            calls: AtomicUsize::new(0),
            first_failure: Some(Failure::Transient),
            messages: Mutex::new(Vec::new()),
        });
        s.mail.configured.as_mut().unwrap().transport = transient.clone();
        let (exhausted, _) = queue(&s, user).await;
        sqlx::query("UPDATE email_deliveries SET attempts=7 WHERE id=$1")
            .bind(exhausted)
            .execute(&s.db)
            .await
            .unwrap();
        tick(&s).await.unwrap();
        tick(&s).await.unwrap();
        assert_eq!(transient.calls.load(Ordering::SeqCst), 1);
        let (status, attempts, encrypted): (String, i32, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT status,attempts,encrypted_body FROM email_deliveries WHERE id=$1",
        )
        .bind(exhausted)
        .fetch_one(&s.db)
        .await
        .unwrap();
        assert_eq!((status.as_str(), attempts), ("failed", 8));
        assert!(encrypted.is_none());
    }
}
