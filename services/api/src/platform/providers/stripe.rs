use super::*;
use axum::body::Bytes;
use chrono::{DateTime, Utc};
use sqlx::Row;

#[path = "stripe_finance.rs"]
pub mod finance;

pub struct Stripe {
    http: reqwest::Client,
    key: String,
    webhook_secret: String,
    origin: String,
    country: String,
    pub live: bool,
    #[cfg(test)]
    endpoint: Option<String>,
}
impl Stripe {
    pub(super) async fn platform_identity(&self) -> Result<String> {
        let account = self.get("/account", None).await?;
        Ok(format!("stripe:{}:{}", self.live, id(&account, "acct_")?))
    }
    pub async fn from_env(
        http: reqwest::Client,
    ) -> std::result::Result<Option<Self>, Box<dyn std::error::Error>> {
        match std::env::var("PAYMENT_PROVIDER").as_deref() {
            Err(_) | Ok("sandbox") => {
                if [
                    "STRIPE_SECRET_KEY_FILE",
                    "STRIPE_WEBHOOK_SECRET_FILE",
                    "STRIPE_ACCOUNT_COUNTRY",
                ]
                .iter()
                .any(|v| std::env::var_os(v).is_some())
                {
                    return Err(
                        "Set PAYMENT_PROVIDER=stripe before supplying Stripe configuration".into(),
                    );
                }
                return Ok(None);
            }
            Ok("stripe") => {}
            _ => return Err("PAYMENT_PROVIDER must be sandbox or stripe".into()),
        }
        let key = secret("STRIPE_SECRET_KEY_FILE").await?;
        let live = if key.starts_with("sk_test_") {
            false
        } else if key.starts_with("sk_live_") {
            true
        } else {
            return Err("Stripe secret key has an unsupported format".into());
        };
        let production = match std::env::var("APP_MODE").as_deref() {
            Ok("production") => true,
            Err(_) | Ok("sandbox") => false,
            _ => return Err("APP_MODE must be sandbox or production".into()),
        };
        if live != production {
            return Err(
                "Stripe production requires a live key; sandbox requires a test key".into(),
            );
        }
        let webhook_secret = secret("STRIPE_WEBHOOK_SECRET_FILE").await?;
        if !webhook_secret.starts_with("whsec_") {
            return Err("Invalid Stripe webhook secret format".into());
        }
        let country = std::env::var("STRIPE_ACCOUNT_COUNTRY")
            .map_err(|_| "STRIPE_ACCOUNT_COUNTRY is required")?;
        if country.len() != 2 || !country.bytes().all(|v| v.is_ascii_uppercase()) {
            return Err("STRIPE_ACCOUNT_COUNTRY must be an ISO 3166-1 alpha-2 code".into());
        }
        Ok(Some(Self {
            http,
            key,
            webhook_secret,
            origin: public_origin()?,
            country,
            live,
            #[cfg(test)]
            endpoint: None,
        }))
    }
    pub fn is_live(&self) -> bool {
        self.live
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        account: Option<&str>,
        fields: &[(String, String)],
        key: Option<&str>,
    ) -> Result<Value> {
        #[cfg(not(test))]
        let endpoint = "https://api.stripe.com";
        #[cfg(test)]
        let endpoint = self.endpoint.as_deref().unwrap_or("https://api.stripe.com");
        let mut request = self
            .http
            .request(method.clone(), format!("{endpoint}/v1{path}"))
            .bearer_auth(&self.key)
            .header("Stripe-Version", STRIPE_VERSION);
        if let Some(account) = account {
            id(&json!(account), "acct_")?;
            request = request.header("Stripe-Account", account);
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if method == reqwest::Method::GET {
            request = request.query(fields);
        } else {
            request = request.form(fields);
        }
        response_json(request.send().await.map_err(|_| unavailable())?).await
    }
    async fn get(&self, path: &str, account: Option<&str>) -> Result<Value> {
        self.request(reqwest::Method::GET, path, account, &[], None)
            .await
    }
    async fn post(
        &self,
        path: &str,
        account: Option<&str>,
        fields: Vec<(String, String)>,
        key: &str,
    ) -> Result<Value> {
        self.request(reqwest::Method::POST, path, account, &fields, Some(key))
            .await
    }
    fn mode(&self, v: &Value) -> Result<()> {
        if v["livemode"].as_bool() != Some(self.live) {
            Err(unavailable())
        } else {
            Ok(())
        }
    }
}
fn form(items: &[(&str, String)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}
fn uuid(v: &Value) -> Result<Uuid> {
    v.as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(unavailable)
}
fn timestamp(v: &Value) -> Result<DateTime<Utc>> {
    v.as_i64()
        .and_then(|v| DateTime::from_timestamp(v, 0))
        .ok_or_else(unavailable)
}
fn stripe(s: &PlatformState) -> Result<&Stripe> {
    s.providers.stripe.as_deref().ok_or_else(unavailable)
}

async fn creator_user(s: &PlatformState, h: &HeaderMap) -> Result<User> {
    let u = user(s, h).await?;
    let u = authorize_current(s, h, u.id, Assurance::RecentMfa).await?;
    if !u.email_verified {
        return Err(bad("Verify your email before connecting a payout account."));
    }
    mfa_session(s, h).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM creators WHERE user_id=$1)")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !exists {
        return Err(forbidden());
    }
    rate_limit(s, &format!("connect:{}", u.id), 5).await?;
    Ok(u)
}
pub async fn account(State(s): State<PlatformState>, h: HeaderMap) -> Result<Json<Value>> {
    let u = creator_user(&s, &h).await?;
    let p = stripe(&s)?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM provider_accounts WHERE user_id=$1")
            .bind(u.id)
            .fetch_optional(&s.db)
            .await?;
    let account = if let Some(account) = existing {
        account
    } else {
        let v = p
            .post(
                "/accounts",
                None,
                form(&[
                    ("type", "express".into()),
                    ("country", p.country.clone()),
                    ("capabilities[card_payments][requested]", "true".into()),
                    ("capabilities[transfers][requested]", "true".into()),
                    ("metadata[myfans_user]", u.id.to_string()),
                ]),
                &format!("myfans-account:{}", u.id),
            )
            .await?;
        let account = id(&v, "acct_")?;
        sqlx::query("INSERT INTO provider_accounts(user_id,account_id) VALUES($1,$2) ON CONFLICT(user_id) DO NOTHING").bind(u.id).bind(&account).execute(&s.db).await?;
        let stored: String =
            sqlx::query_scalar("SELECT account_id FROM provider_accounts WHERE user_id=$1")
                .bind(u.id)
                .fetch_one(&s.db)
                .await?;
        if stored != account {
            return Err(unavailable());
        }
        account
    };
    authorize_current(&s, &h, u.id, Assurance::RecentMfa).await?;
    // Each explicit action receives a fresh single-use hosted URL. Never persist or log it.
    let v = p
        .post(
            "/account_links",
            None,
            form(&[
                ("account", account),
                ("type", "account_onboarding".into()),
                (
                    "return_url",
                    format!("{}/settings?connect=returned", p.origin),
                ),
                (
                    "refresh_url",
                    format!("{}/settings?connect=expired", p.origin),
                ),
            ]),
            &format!("myfans-link:{}", Uuid::new_v4()),
        )
        .await?;
    authorize_current(&s, &h, u.id, Assurance::RecentMfa).await?;
    Ok(Json(json!({"url":hosted_url(&v,"connect.stripe.com")?})))
}
pub async fn dashboard(State(s): State<PlatformState>, h: HeaderMap) -> Result<Json<Value>> {
    let u = creator_user(&s, &h).await?;
    let account: String =
        sqlx::query_scalar("SELECT account_id FROM provider_accounts WHERE user_id=$1")
            .bind(u.id)
            .fetch_one(&s.db)
            .await?;
    let v = stripe(&s)?
        .post(
            &format!("/accounts/{account}/login_links"),
            None,
            vec![],
            &format!("myfans-dashboard:{}", Uuid::new_v4()),
        )
        .await?;
    authorize_current(&s, &h, u.id, Assurance::RecentMfa).await?;
    Ok(Json(json!({"url":hosted_url(&v,"connect.stripe.com")?})))
}
pub async fn ready_account(s: &PlatformState, owner: Uuid) -> Result<String> {
    let account: String =
        sqlx::query_scalar("SELECT account_id FROM provider_accounts WHERE user_id=$1")
            .bind(owner)
            .fetch_optional(&s.db)
            .await?
            .ok_or(bad("This creator has not connected a payout account."))?;
    let v = stripe(s)?
        .get(&format!("/accounts/{account}"), None)
        .await?;
    if id(&v, "acct_")? != account || v["metadata"]["myfans_user"] != owner.to_string() {
        return Err(unavailable());
    }
    let charges = v["charges_enabled"] == true;
    let payouts = v["payouts_enabled"] == true;
    sqlx::query("UPDATE provider_accounts SET charges_enabled=$2,payouts_enabled=$3,checked_at=now() WHERE user_id=$1").bind(owner).bind(charges).bind(payouts).execute(&s.db).await?;
    if !charges || !payouts {
        return Err(bad("This creator's payment account is not ready."));
    }
    if v["default_currency"] != "usd" {
        return Err(bad(
            "Creator accounts must use USD settlement for this platform.",
        ));
    }
    Ok(account)
}
pub async fn authorized_checkout(
    s: &PlatformState,
    h: &HeaderMap,
    user: Uuid,
    checkout_id: Uuid,
) -> Result<Json<Value>> {
    authorize_current(s, h, user, Assurance::Member).await?;
    let result = checkout(s, checkout_id).await?;
    // Persist successful remote session creation before checking whether its URL
    // can still be returned. Reconciliation must survive an account freeze.
    authorize_current(s, h, user, Assurance::Member).await?;
    Ok(result)
}
pub async fn checkout(s: &PlatformState, checkout: Uuid) -> Result<Json<Value>> {
    let p = stripe(s)?;
    // Commit the uncertainty marker before the network call. A crash after this
    // point must never be interpreted as proof that no payment session exists.
    sqlx::query("UPDATE checkouts SET provider_attempted_at=clock_timestamp() WHERE id=$1 AND provider='stripe' AND status='pending' AND provider_attempted_at IS NULL AND created_at>clock_timestamp()-interval '20 minutes'").bind(checkout).execute(&s.db).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='15s'")
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query("SELECT * FROM checkouts WHERE id=$1 AND provider='stripe' FOR UPDATE")
        .bind(checkout)
        .fetch_one(&mut *tx)
        .await?;
    if row.get::<String, _>("status") != "pending" {
        return Err(bad("This checkout is no longer pending."));
    }
    let created: DateTime<Utc> = row.get("created_at");
    let account: String = row.get("provider_account");
    let amount: i64 = row.get("amount_minor");
    let kind: String = row.get("kind");
    let session = if let Some(session) = row.get::<Option<String>, _>("provider_session") {
        p.get(&format!("/checkout/sessions/{session}"), Some(&account))
            .await?
    } else {
        // Stable absolute expiry keeps retries identical. The worker only looks
        // up old requests; it never creates another session after this deadline.
        if Utc::now() - created > chrono::Duration::minutes(20) {
            return Err(bad(
                "This checkout is being reconciled. Retry after its payment status is confirmed.",
            ));
        }
        let mut fields = form(&[
            (
                "mode",
                if kind == "subscription" {
                    "subscription"
                } else {
                    "payment"
                }
                .into(),
            ),
            ("client_reference_id", checkout.to_string()),
            ("metadata[myfans_checkout]", checkout.to_string()),
            ("line_items[0][price_data][currency]", "usd".into()),
            ("line_items[0][price_data][unit_amount]", amount.to_string()),
            (
                "line_items[0][price_data][product_data][name]",
                format!("MyFans {kind}"),
            ),
            (
                "line_items[0][price_data][tax_behavior]",
                "inclusive".into(),
            ),
            ("line_items[0][quantity]", "1".into()),
            ("allowed_payment_method_types[0]", "card".into()),
            ("automatic_tax[enabled]", "true".into()),
            ("billing_address_collection", "required".into()),
            (
                "success_url",
                format!("{}/settings?checkout=returned", p.origin),
            ),
            ("cancel_url", format!("{}/?checkout=canceled", p.origin)),
            ("expires_at", (created.timestamp() + 3600).to_string()),
        ]);
        if kind == "subscription" {
            fields.extend(form(&[
                (
                    "line_items[0][price_data][recurring][interval]",
                    "month".into(),
                ),
                ("subscription_data[application_fee_percent]", "20".into()),
                (
                    "subscription_data[metadata][myfans_checkout]",
                    checkout.to_string(),
                ),
            ]));
        } else {
            fields.extend(form(&[
                (
                    "payment_intent_data[application_fee_amount]",
                    (amount / 5).to_string(),
                ),
                (
                    "payment_intent_data[metadata][myfans_checkout]",
                    checkout.to_string(),
                ),
            ]));
        }
        let v = p
            .post(
                "/checkout/sessions",
                Some(&account),
                fields,
                &format!("myfans-checkout:{checkout}"),
            )
            .await?;
        let session = id(&v, "cs_")?;
        sqlx::query(
            "UPDATE checkouts SET provider_session=$2 WHERE id=$1 AND provider_session IS NULL",
        )
        .bind(checkout)
        .bind(session)
        .execute(&mut *tx)
        .await?;
        v
    };
    p.mode(&session)?;
    if session["status"] != "open" {
        return Err(bad(
            "This checkout is complete or expired. Refresh your purchases.",
        ));
    }
    if session["client_reference_id"] != checkout.to_string()
        || session["currency"] != "usd"
        || session["amount_total"].as_i64() != Some(amount)
    {
        return Err(unavailable());
    }
    let redirect = hosted_url(&session, "checkout.stripe.com")?;
    tx.commit().await?;
    Ok(Json(
        json!({"checkout":{"id":checkout,"status":"pending","amount_minor":amount,"currency":"USD","mode":"stripe"},"redirect_url":redirect,"confirmation_required":false}),
    ))
}
pub async fn cancel(s: &PlatformState, user: Uuid, subscription: Uuid) -> Result<bool> {
    let row=sqlx::query("SELECT s.provider_subscription_id,a.account_id FROM subscriptions s JOIN creators r ON r.id=s.creator_id JOIN provider_accounts a ON a.user_id=r.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.provider='stripe'").bind(subscription).bind(user).fetch_optional(&s.db).await?;
    let Some(row) = row else { return Ok(false) };
    let sid: String = row.get("provider_subscription_id");
    let account: String = row.get("account_id");
    let v = stripe(s)?
        .post(
            &format!("/subscriptions/{sid}"),
            Some(&account),
            form(&[("cancel_at_period_end", "true".into())]),
            &format!("myfans-cancel:{subscription}"),
        )
        .await?;
    stripe(s)?.mode(&v)?;
    if id(&v, "sub_")? != sid || v["cancel_at_period_end"] != true {
        return Err(unavailable());
    }
    Ok(true)
}
pub async fn balances(s: &PlatformState, user: Uuid) -> Result<(i64, i64)> {
    let account: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM provider_accounts WHERE user_id=$1")
            .bind(user)
            .fetch_optional(&s.db)
            .await?;
    let Some(account) = account else {
        return Ok((0, 0));
    };
    let v = stripe(s)?.get("/balance", Some(&account)).await?;
    stripe(s)?.mode(&v)?;
    let sum = |field: &str| -> Result<i64> {
        v[field]
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .filter(|v| v["currency"] == "usd")
            .try_fold(0_i64, |sum, v| {
                sum.checked_add(v["amount"].as_i64().ok_or_else(unavailable)?)
                    .ok_or_else(unavailable)
            })
    };
    Ok((sum("available")?, sum("pending")?))
}
fn signed_event(secret: &str, header: &str, body: &[u8], now: i64) -> bool {
    if header.len() > 2048 {
        return false;
    }
    let times: Vec<_> = header
        .split(',')
        .filter_map(|v| v.trim().strip_prefix("t="))
        .collect();
    if times.len() != 1 {
        return false;
    }
    let Ok(time) = times[0].parse::<i64>() else {
        return false;
    };
    if now.abs_diff(time) > 300 {
        return false;
    }
    let mut signed = format!("{time}.").into_bytes();
    signed.extend_from_slice(body);
    header
        .split(',')
        .filter_map(|v| v.trim().strip_prefix("v1="))
        .any(|v| verify_signature(secret.as_bytes(), &signed, v))
}
pub async fn webhook(
    State(s): State<PlatformState>,
    h: HeaderMap,
    body: Bytes,
) -> Result<StatusCode> {
    let p = stripe(&s)?;
    if !signed_event(
        &p.webhook_secret,
        h.get("stripe-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        &body,
        Utc::now().timestamp(),
    ) {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Invalid provider signature.",
        ));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|_| bad("Invalid provider event."))?;
    p.mode(&v)?;
    if v["api_version"] != STRIPE_VERSION {
        return Err(bad("Unexpected Stripe event API version."));
    }
    let event = id(&v, "evt_")?;
    let account = id(&v["account"], "acct_")?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_accounts WHERE account_id=$1)")
            .bind(&account)
            .fetch_one(&s.db)
            .await?;
    if !known {
        return Ok(StatusCode::NO_CONTENT);
    }
    let hash = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut tx = s.db.begin().await?;
    let inserted=sqlx::query("INSERT INTO payment_webhook_inbox(provider,event_id,payload_sha256) VALUES('stripe',$1,$2) ON CONFLICT DO NOTHING").bind(&event).bind(&hash).execute(&mut *tx).await?;
    if inserted.rows_affected() == 1 {
        sqlx::query("INSERT INTO jobs(kind,payload) VALUES('stripe_event',$1)")
            .bind(json!({"event_id":event,"account":account}))
            .execute(&mut *tx)
            .await?;
    } else {
        let stored:String=sqlx::query_scalar("SELECT payload_sha256 FROM payment_webhook_inbox WHERE provider='stripe' AND event_id=$1").bind(&event).fetch_one(&mut *tx).await?;
        if stored != hash {
            return Err(bad("Conflicting provider event."));
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn event(s: &PlatformState, payload: &Value) -> Result<()> {
    let p = stripe(s)?;
    let event = id(&payload["event_id"], "evt_")?;
    let account = id(&payload["account"], "acct_")?;
    let v = p.get(&format!("/events/{event}"), Some(&account)).await?;
    p.mode(&v)?;
    if id(&v, "evt_")? != event {
        return Err(unavailable());
    }
    apply_event(s, &account, &v).await?;
    sqlx::query("UPDATE payment_webhook_inbox SET processed_at=now() WHERE provider='stripe' AND event_id=$1").bind(event).execute(&s.db).await?;
    Ok(())
}
async fn apply_event(s: &PlatformState, account: &str, v: &Value) -> Result<()> {
    // Event snapshots retain their creation version. Only event type/object identifiers
    // are consumed; every mutable business object is retrieved with our pinned version.
    stripe(s)?.mode(v)?;
    let object = &v["data"]["object"];
    match v["type"].as_str().unwrap_or_default() {
        "checkout.session.completed"
        | "checkout.session.async_payment_succeeded"
        | "checkout.session.expired" => {
            let sid = id(object, "cs_")?;
            sync_checkout(s, account, &sid).await?;
        }
        "invoice.paid" | "invoice.payment_succeeded" => {
            let invoice = id(object, "in_")?;
            sync_invoice(s, account, &invoice).await?;
        }
        "charge.refunded" => {
            let charge = id(object, "ch_")?;
            sync_reversal(s, account, &charge).await?;
        }
        "charge.dispute.created"
        | "charge.dispute.closed"
        | "charge.dispute.updated"
        | "charge.dispute.funds_reinstated"
        | "charge.dispute.funds_withdrawn" => {
            finance::sync_dispute(s, account, &id(object, "du_")?).await?;
        }
        "customer.subscription.updated" | "customer.subscription.deleted" => {
            let sid = id(object, "sub_")?;
            sync_subscription(s, account, &sid).await?;
        }
        _ => {}
    }
    Ok(())
}
pub async fn sync_checkout(s: &PlatformState, account: &str, session: &str) -> Result<()> {
    let p = stripe(s)?;
    let v = p
        .get(&format!("/checkout/sessions/{session}"), Some(account))
        .await?;
    p.mode(&v)?;
    if id(&v, "cs_")? != session {
        return Err(unavailable());
    }
    let Ok(checkout) = uuid(&v["client_reference_id"]) else {
        return Ok(());
    };
    let known:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM checkouts WHERE id=$1 AND provider_account=$2 AND (provider_session IS NULL OR provider_session=$3))").bind(checkout).bind(account).bind(session).fetch_one(&s.db).await?;
    if !known {
        return Ok(());
    }
    sqlx::query(
        "UPDATE checkouts SET provider_session=$2 WHERE id=$1 AND provider_session IS NULL",
    )
    .bind(checkout)
    .bind(session)
    .execute(&s.db)
    .await?;
    if v["status"] == "expired" {
        sqlx::query("UPDATE checkouts SET status='failed' WHERE id=$1 AND status='pending'")
            .bind(checkout)
            .execute(&s.db)
            .await?;
        return Ok(());
    }
    if v["status"] != "complete" || v["payment_status"] != "paid" {
        return Ok(());
    }
    if v["mode"] == "subscription" {
        let invoice = id(&v["invoice"], "in_")?;
        sync_invoice(s, account, &invoice).await
    } else {
        let intent_id = id(&v["payment_intent"], "pi_")?;
        let intent = p
            .get(&format!("/payment_intents/{intent_id}"), Some(account))
            .await?;
        p.mode(&intent)?;
        if id(&intent, "pi_")? != intent_id {
            return Err(unavailable());
        }
        let charge = id(&intent["latest_charge"], "ch_")?;
        record(s, account, checkout, &charge, None, None, None).await
    }
}
/// Recover stale intents without resubmitting a creation request or releasing
/// an uncertain payment reservation. Each job processes at most one list page.
pub async fn recover_checkout(s: &PlatformState, checkout: Uuid) -> Result<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='15s'")
        .execute(&mut *tx)
        .await?;
    // An unattempted expiration writes a user-linked audit event. Acquire its
    // account locks before the checkout, including when a creation races us.
    let unattempted: bool =
        sqlx::query_scalar("SELECT provider_attempted_at IS NULL FROM checkouts WHERE id=$1")
            .bind(checkout)
            .fetch_one(&mut *tx)
            .await?;
    if unattempted {
        crate::platform::billing::lock_checkout_accounts(&mut tx, checkout, None).await?;
    }
    let row = sqlx::query("SELECT * FROM checkouts WHERE id=$1 AND provider='stripe' FOR UPDATE")
        .bind(checkout)
        .fetch_one(&mut *tx)
        .await?;
    if row.get::<String, _>("status") != "pending" {
        return Ok(());
    }
    let created: DateTime<Utc> = row.get("created_at");
    if Utc::now() - created < chrono::Duration::minutes(25) {
        return Ok(());
    }
    let account: String = row.get("provider_account");
    if let Some(session) = row.get::<Option<String>, _>("provider_session") {
        tx.commit().await?;
        sync_checkout(s, &account, &session).await?;
        sqlx::query(
            "UPDATE checkouts SET provider_recovery_at=now()+interval '5 minutes' WHERE id=$1",
        )
        .bind(checkout)
        .execute(&s.db)
        .await?;
        return Ok(());
    }
    if row
        .get::<Option<DateTime<Utc>>, _>("provider_attempted_at")
        .is_none()
    {
        // Creation uses the same row lock and cannot start after 20 minutes.
        sqlx::query("UPDATE checkouts SET status='failed' WHERE id=$1")
            .bind(checkout)
            .execute(&mut *tx)
            .await?;
        audit(
            &mut tx,
            row.get("user_id"),
            "checkout.expired.unattempted",
            "checkout",
            checkout,
        )
        .await?;
        tx.commit().await?;
        return Ok(());
    }
    let cursor: Option<String> = row.get("provider_recovery_cursor");
    let mut fields = form(&[
        ("limit", "20".into()),
        ("created[gte]", (created.timestamp() - 300).to_string()),
        ("created[lte]", (created.timestamp() + 1500).to_string()),
    ]);
    if let Some(cursor) = &cursor {
        fields.push(("starting_after".into(), cursor.clone()));
    }
    let p = stripe(s)?;
    let sessions = p
        .request(
            reqwest::Method::GET,
            "/checkout/sessions",
            Some(&account),
            &fields,
            None,
        )
        .await?;
    let data = sessions["data"]
        .as_array()
        .filter(|v| v.len() <= 20)
        .ok_or_else(unavailable)?;
    let has_more = sessions["has_more"].as_bool().ok_or_else(unavailable)?;
    let mut found = None;
    for session in data {
        p.mode(session)?;
        if session["client_reference_id"] == checkout.to_string() {
            // The creation idempotency key is stable and all attempts are within
            // its retention window. Multiple identities require investigation.
            if found.is_some() {
                return Err(unavailable());
            }
            found = Some(id(session, "cs_")?);
        }
    }
    if let Some(session) = found {
        sqlx::query("UPDATE checkouts SET provider_session=$2,provider_recovery_cursor=NULL,provider_recovery_at=now()+interval '5 minutes' WHERE id=$1").bind(checkout).bind(&session).execute(&mut *tx).await?;
        tx.commit().await?;
        return sync_checkout(s, &account, &session).await;
    }
    if has_more {
        let next = id(data.last().ok_or_else(unavailable)?, "cs_")?;
        if cursor.as_deref() == Some(&next) {
            return Err(unavailable());
        }
        sqlx::query("UPDATE checkouts SET provider_recovery_cursor=$2,provider_recovery_at=now() WHERE id=$1").bind(checkout).bind(next).execute(&mut *tx).await?;
    } else {
        sqlx::query("UPDATE checkouts SET provider_recovery_cursor=NULL,provider_recovery_at=now()+interval '30 minutes' WHERE id=$1").bind(checkout).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if !has_more {
        // Absence from a provider list is not authoritative evidence of failure.
        // Keep the reservation and surface a failed job for operator review.
        finance::issue(
            s,
            &account,
            &checkout.to_string(),
            "checkout_indeterminate",
            true,
        )
        .await?;
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Checkout outcome is uncertain; provider investigation is required before releasing the purchase reservation.",
        ));
    }
    Ok(())
}
pub async fn sync_invoice(s: &PlatformState, account: &str, invoice: &str) -> Result<()> {
    let p = stripe(s)?;
    let v = p
        .get(&format!("/invoices/{invoice}"), Some(account))
        .await?;
    p.mode(&v)?;
    if id(&v, "in_")? != invoice {
        return Err(unavailable());
    }
    if v["status"] != "paid"
        || v["amount_remaining"] != 0
        || v["amount_paid_off_stripe"].as_i64().unwrap_or(0) != 0
    {
        return Ok(());
    }
    let sid = id(&v["parent"]["subscription_details"]["subscription"], "sub_")?;
    let sub = p
        .get(&format!("/subscriptions/{sid}"), Some(account))
        .await?;
    p.mode(&sub)?;
    if id(&sub, "sub_")? != sid {
        return Err(unavailable());
    }
    let Ok(checkout) = uuid(&sub["metadata"]["myfans_checkout"]) else {
        return Ok(());
    };
    let lines = v["lines"]["data"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or_else(unavailable)?;
    if v["lines"]["has_more"] != false || lines[0]["currency"] != "usd" {
        return Err(unavailable());
    }
    let start = timestamp(&lines[0]["period"]["start"])?;
    let end = timestamp(&lines[0]["period"]["end"])?;
    if end <= start || end - start > chrono::Duration::days(32) {
        return Err(unavailable());
    }
    let payments = v["payments"]["data"].as_array().ok_or_else(unavailable)?;
    let payments: Vec<_> = payments.iter().filter(|v| v["status"] == "paid").collect();
    if payments.len() != 1 || v["payments"]["has_more"] != false {
        return Err(unavailable());
    }
    let intent_id = id(&payments[0]["payment"]["payment_intent"], "pi_")?;
    let intent = p
        .get(&format!("/payment_intents/{intent_id}"), Some(account))
        .await?;
    p.mode(&intent)?;
    if id(&intent, "pi_")? != intent_id {
        return Err(unavailable());
    }
    let charge = id(&intent["latest_charge"], "ch_")?;
    record(
        s,
        account,
        checkout,
        &charge,
        Some(invoice),
        Some(&sid),
        Some((start, end)),
    )
    .await?;
    sync_subscription(s, account, &sid).await
}
async fn record(
    s: &PlatformState,
    account: &str,
    checkout: Uuid,
    charge_id: &str,
    invoice: Option<&str>,
    subscription: Option<&str>,
    period: Option<(DateTime<Utc>, DateTime<Utc>)>,
) -> Result<()> {
    let p = stripe(s)?;
    let charge = p
        .get(&format!("/charges/{charge_id}"), Some(account))
        .await?;
    p.mode(&charge)?;
    if id(&charge, "ch_")? != charge_id {
        return Err(unavailable());
    }
    let mut tx = s.db.begin().await?;
    crate::platform::billing::lock_checkout_accounts(&mut tx, checkout, None).await?;
    let row=sqlx::query("SELECT * FROM checkouts WHERE id=$1 AND provider='stripe' AND provider_account=$2 FOR UPDATE").bind(checkout).bind(account).fetch_one(&mut *tx).await?;
    let amount: i64 = row.get("amount_minor");
    if !matches!(row.get::<String, _>("status").as_str(), "pending" | "paid") {
        return Err(Error(
            StatusCode::CONFLICT,
            "Provider payment conflicts with a terminal checkout; investigation is required.",
        ));
    }
    if charge["paid"] != true
        || charge["status"] != "succeeded"
        || charge["currency"] != "usd"
        || charge["amount"].as_i64() != Some(amount)
        || charge["amount_captured"].as_i64() != Some(amount)
    {
        return Err(unavailable());
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM provider_receipts WHERE account_id=$1 AND charge_id=$2)",
    )
    .bind(account)
    .bind(charge_id)
    .fetch_one(&mut *tx)
    .await?;
    if exists {
        tx.commit().await?;
        return sync_reversal(s, account, charge_id).await;
    }
    let kind: String = row.get("kind");
    let buyer: Uuid = row.get("user_id");
    let creator: Uuid = row.get("creator_id");
    let refunded = charge["amount_refunded"]
        .as_i64()
        .filter(|v| *v >= 0 && *v <= amount)
        .ok_or_else(unavailable)?;
    let disputed = charge["disputed"] == true;
    let mut entitlement: Option<Uuid> = None;
    if kind == "subscription" {
        let sid = subscription.ok_or_else(unavailable)?;
        let (start, end) = period.ok_or_else(unavailable)?;
        // Paid periods extend access; a failed renewal never grants a new period.
        let sub:Uuid=sqlx::query_scalar("INSERT INTO subscriptions(user_id,creator_id,status,provider,provider_subscription_id,current_period_start,current_period_end) VALUES($1,$2,'active','stripe',$3,$4,$5) ON CONFLICT(provider,provider_subscription_id) DO UPDATE SET current_period_start=LEAST(subscriptions.current_period_start,EXCLUDED.current_period_start),current_period_end=GREATEST(subscriptions.current_period_end,EXCLUDED.current_period_end) RETURNING id").bind(buyer).bind(creator).bind(sid).bind(start).bind(end).fetch_one(&mut *tx).await?;
        if refunded == 0 && !disputed {
            entitlement=Some(sqlx::query_scalar("INSERT INTO entitlements(user_id,creator_id,subscription_id,source,starts_at,expires_at) VALUES($1,$2,$3,'subscription',$4,$5) RETURNING id").bind(buyer).bind(creator).bind(sub).bind(start).bind(end).fetch_one(&mut *tx).await?);
        }
    } else if kind == "purchase" && refunded == 0 && !disputed {
        let post: Uuid = row.get("post_id");
        entitlement=Some(sqlx::query_scalar("INSERT INTO entitlements(user_id,post_id,source) VALUES($1,$2,'purchase') RETURNING id").bind(buyer).bind(post).fetch_one(&mut *tx).await?);
    }
    sqlx::query("INSERT INTO provider_receipts(checkout_id,account_id,charge_id,invoice_id,subscription_id,amount_minor,refunded_minor,disputed,currency,period_start,period_end,entitlement_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'USD',$9,$10,$11)").bind(checkout).bind(account).bind(charge_id).bind(invoice).bind(subscription).bind(amount).bind(refunded).bind(disputed).bind(period.map(|v|v.0)).bind(period.map(|v|v.1)).bind(entitlement).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE checkouts SET status='paid',paid_at=now() WHERE id=$1 AND status='pending'",
    )
    .bind(checkout)
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        buyer,
        "payment.verified.stripe",
        "checkout",
        checkout,
    )
    .await?;
    notify(
        &mut tx,
        buyer,
        "Payment confirmed",
        "Your payment has been confirmed by the payment provider.",
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn sync_reversal(s: &PlatformState, account: &str, charge_id: &str) -> Result<()> {
    let p = stripe(s)?;
    let v = p
        .get(&format!("/charges/{charge_id}"), Some(account))
        .await?;
    p.mode(&v)?;
    if id(&v, "ch_")? != charge_id {
        return Err(unavailable());
    }
    let mut tx = s.db.begin().await?;
    let row=sqlx::query("SELECT id,amount_minor,refunded_minor,disputed,entitlement_id FROM provider_receipts WHERE account_id=$1 AND charge_id=$2 FOR UPDATE").bind(account).bind(charge_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else { return Ok(()) };
    let amount: i64 = row.get("amount_minor");
    let refunded = v["amount_refunded"]
        .as_i64()
        .filter(|v| *v >= 0 && *v <= amount)
        .ok_or_else(unavailable)?;
    if v["currency"] != "usd" || v["amount"].as_i64() != Some(amount) {
        return Err(unavailable());
    }
    let disputed = v["disputed"] == true;
    // Stripe's charge.disputed flag is historical. Keep an explicitly restored
    // merchant win open until a new adverse dispute or refund is observed.
    let restored_win: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_financial_revocations f WHERE f.receipt_id=$1 AND f.restored_at IS NOT NULL) AND EXISTS(SELECT 1 FROM provider_disputes d WHERE d.account_id=$2 AND d.charge_id=$3 AND d.status='won') AND NOT EXISTS(SELECT 1 FROM provider_disputes d WHERE d.account_id=$2 AND d.charge_id=$3 AND d.status NOT IN ('won','warning_closed','prevented'))").bind(row.get::<Uuid,_>("id")).bind(account).bind(charge_id).fetch_one(&mut *tx).await?;
    if refunded > 0 || (disputed && !restored_win) {
        let revoked: Option<(Uuid,DateTime<Utc>)> = sqlx::query_as("UPDATE entitlements SET revoked_at=clock_timestamp() WHERE id=$1 AND revoked_at IS NULL RETURNING id,revoked_at")
            .bind(row.get::<Option<Uuid>, _>("entitlement_id"))
            .fetch_optional(&mut *tx)
            .await?;
        if let Some((entitlement, at)) = revoked {
            sqlx::query("INSERT INTO provider_financial_revocations(receipt_id,entitlement_id,revoked_at) VALUES($1,$2,$3) ON CONFLICT(receipt_id) DO UPDATE SET revoked_at=EXCLUDED.revoked_at,restored_at=NULL,restored_by=NULL").bind(row.get::<Uuid,_>("id")).bind(entitlement).bind(at).execute(&mut *tx).await?;
        }
    }
    sqlx::query("UPDATE provider_receipts SET refunded_minor=GREATEST(refunded_minor,$2),disputed=disputed OR $3,checked_at=now() WHERE id=$1").bind(row.get::<Uuid,_>("id")).bind(refunded).bind(disputed).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn sync_subscription(s: &PlatformState, account: &str, sid: &str) -> Result<()> {
    let p = stripe(s)?;
    let v = p
        .get(&format!("/subscriptions/{sid}"), Some(account))
        .await?;
    p.mode(&v)?;
    if id(&v, "sub_")? != sid {
        return Err(unavailable());
    }
    let Ok(checkout) = uuid(&v["metadata"]["myfans_checkout"]) else {
        return Ok(());
    };
    // Provider status never extends access. Entitlements are created only by verified paid invoices.
    let state = match v["status"].as_str() {
        Some("active") => "active",
        Some("past_due") => "past_due",
        Some("canceled" | "unpaid" | "incomplete_expired") => "canceled",
        _ => "pending",
    };
    sqlx::query("UPDATE subscriptions SET status=$3,cancel_at_period_end=$4 WHERE provider='stripe' AND provider_subscription_id=$1 AND EXISTS(SELECT 1 FROM checkouts c WHERE c.id=$2 AND c.provider_account=$5 AND c.user_id=subscriptions.user_id AND c.creator_id=subscriptions.creator_id)").bind(sid).bind(checkout).bind(state).bind(v["cancel_at_period_end"]==true).bind(account).execute(&s.db).await?;
    Ok(())
}
pub async fn refund(
    s: &PlatformState,
    checkout: Uuid,
    actor: Uuid,
    reason: Option<&str>,
) -> Result<Json<Value>> {
    if s.runtime.production && reason.is_none() {
        return Err(bad("A refund audit reason is required."));
    }
    let row=sqlx::query("SELECT account_id,charge_id,amount_minor,refunded_minor FROM provider_receipts WHERE checkout_id=$1 ORDER BY period_start ASC NULLS FIRST,created_at,id LIMIT 1").bind(checkout).fetch_one(&s.db).await?;
    if row.get::<i64, _>("refunded_minor") == row.get::<i64, _>("amount_minor") {
        return Ok(Json(json!({"status":"refunded","mode":"stripe"})));
    }
    let account: String = row.get("account_id");
    let charge: String = row.get("charge_id");
    if let Some(reason) = reason {
        finance::audit_action(s, actor, "refund.request", &checkout.to_string(), reason).await?;
    }
    stripe(s)?
        .post(
            "/refunds",
            Some(&account),
            form(&[
                ("charge", charge.clone()),
                ("refund_application_fee", "true".into()),
            ]),
            &format!("myfans-refund:{charge}"),
        )
        .await?;
    sync_reversal(s, &account, &charge).await?;
    let mut tx = s.db.begin().await?;
    audit(
        &mut tx,
        actor,
        "refund.requested.stripe",
        "checkout",
        checkout,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"processing","mode":"stripe"})))
}
pub async fn reconcile(s: &PlatformState, payload: &Value) -> Result<()> {
    let account = id(&payload["account"], "acct_")?;
    let row=sqlx::query("SELECT account_id,last_completed_at,window_start,window_end,cursor FROM provider_reconciliation WHERE account_id=$1").bind(&account).fetch_optional(&s.db).await?;
    let Some(row) = row else { return Ok(()) };
    let account: String = row.get("account_id");
    let last: DateTime<Utc> = row.get("last_completed_at");
    if Utc::now() - last > chrono::Duration::days(29) {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Provider reconciliation exceeded event retention; operator recovery is required.",
        ));
    }
    let start: i64 = row
        .get::<Option<i64>, _>("window_start")
        .unwrap_or(last.timestamp() - 600);
    let end: i64 = row
        .get::<Option<i64>, _>("window_end")
        .unwrap_or(Utc::now().timestamp());
    let cursor: Option<String> = row.get("cursor");
    let mut fields = form(&[
        ("limit", "5".into()),
        ("created[gte]", start.to_string()),
        ("created[lte]", end.to_string()),
    ]);
    if let Some(cursor) = cursor {
        fields.push(("starting_after".into(), cursor));
    }
    let events = stripe(s)?
        .request(
            reqwest::Method::GET,
            "/events",
            Some(&account),
            &fields,
            None,
        )
        .await?;
    let data = events["data"]
        .as_array()
        .filter(|v| v.len() <= 5)
        .ok_or_else(unavailable)?;
    for event in data {
        apply_event(s, &account, event).await?;
    }
    if events["has_more"] == true {
        let cursor = id(data.last().ok_or_else(unavailable)?, "evt_")?;
        sqlx::query("UPDATE provider_reconciliation SET window_start=$2,window_end=$3,cursor=$4,next_at=now() WHERE account_id=$1").bind(account).bind(start).bind(end).bind(cursor).execute(&s.db).await?;
    } else if events["has_more"] == false {
        sqlx::query("UPDATE provider_reconciliation SET last_completed_at=to_timestamp($2),window_start=NULL,window_end=NULL,cursor=NULL,next_at=now()+interval '5 minutes' WHERE account_id=$1").bind(account).bind(end as f64).execute(&s.db).await?;
    } else {
        return Err(unavailable());
    }
    Ok(())
}
#[cfg(test)]
#[path = "stripe_tests.rs"]
mod contracts;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn signatures_require_exact_raw_bytes_and_a_fresh_unique_timestamp() {
        let secret = "whsec_test_secret";
        let body = b"{\"a\":1}";
        let now = 1791417600;
        let sig = signature(
            secret.as_bytes(),
            format!("{now}.{}", std::str::from_utf8(body).unwrap()).as_bytes(),
        )
        .unwrap();
        let header = format!("t={now},v1={sig}");
        assert!(signed_event(secret, &header, body, now));
        assert!(signed_event(secret, &format!("{header},v1=bad"), body, now));
        assert!(!signed_event(secret, &header, b"{\"a\":2}", now));
        assert!(!signed_event(secret, &header, body, now + 301));
        assert!(!signed_event(
            secret,
            &format!("{header},t={now}"),
            body,
            now
        ));
        assert!(!signed_event("wrong", &header, body, now));
    }
}
