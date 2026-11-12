use super::*;
use axum::extract::{Path, Query};
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};

pub async fn account_id(
    tx: &mut Transaction<'_, Postgres>,
    owner: Option<Uuid>,
    kind: &str,
) -> Result<Uuid> {
    sqlx::query("INSERT INTO ledger_accounts(owner_id,kind,currency) VALUES($1,$2,'USD') ON CONFLICT DO NOTHING").bind(owner).bind(kind).execute(&mut **tx).await?;
    Ok(sqlx::query_scalar("SELECT id FROM ledger_accounts WHERE owner_id IS NOT DISTINCT FROM $1 AND kind=$2 AND currency='USD'").bind(owner).bind(kind).fetch_one(&mut **tx).await?)
}
pub async fn balance(tx: &mut Transaction<'_, Postgres>, account: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COALESCE(sum(p.amount_minor),0)::bigint FROM ledger_postings p JOIN ledger_transactions t ON t.id=p.transaction_id WHERE p.account_id=$1 AND t.status='posted'").bind(account).fetch_one(&mut **tx).await?)
}
pub async fn post_ledger(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    entries: &[(Uuid, i64)],
) -> Result<Uuid> {
    if entries.len() < 2 || entries.iter().map(|e| i128::from(e.1)).sum::<i128>() != 0 {
        return Err(bad("The transaction must balance."));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO ledger_transactions(external_key,currency) VALUES($1,'USD') RETURNING id",
    )
    .bind(key)
    .fetch_one(&mut **tx)
    .await?;
    for (account, amount) in entries.iter().filter(|e| e.1 != 0) {
        sqlx::query("INSERT INTO ledger_postings(transaction_id,account_id,currency,amount_minor) VALUES($1,$2,'USD',$3)").bind(id).bind(account).bind(amount).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE ledger_transactions SET status='posted' WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}
#[derive(Deserialize)]
pub struct Checkout {
    kind: String,
    creator_id: Uuid,
    post_id: Option<Uuid>,
    amount_minor: Option<i64>,
    idempotency_key: Uuid,
}
pub async fn checkout(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(p): Json<Checkout>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    if !u.email_verified {
        return Err(bad("Verify your email before making purchases."));
    }
    rate_limit(&s, &format!("checkout:{}", u.id), 30).await?;
    if !["subscription", "purchase", "tip"].contains(&p.kind.as_str()) {
        return Err(bad("Invalid purchase type."));
    }
    let mut tx = s.db.begin().await?;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM creators WHERE id=$1")
        .bind(p.creator_id)
        .fetch_one(&mut *tx)
        .await?;
    community::lock_pair(&mut tx, u.id, owner).await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    let fingerprint=digest(&json!({"kind":p.kind,"creator_id":p.creator_id,"post_id":p.post_id,"amount_minor":p.amount_minor}).to_string());
    if let Some(row)=sqlx::query("SELECT id,status,amount_minor,currency,request_fingerprint,provider FROM checkouts WHERE user_id=$1 AND idempotency_key=$2").bind(u.id).bind(p.idempotency_key).fetch_optional(&mut *tx).await? {
  if row.get::<String,_>("request_fingerprint")!=fingerprint{return Err(Error(StatusCode::CONFLICT,"This idempotency key was used for a different request."))}
  if row.get::<String,_>("provider")=="stripe" {let id=row.get("id");tx.commit().await?;return providers::stripe::authorized_checkout(&s,&h,u.id,id).await;}
  return Ok(Json(json!({"checkout":{"id":row.get::<Uuid,_>("id"),"status":row.get::<String,_>("status"),"amount_minor":row.get::<i64,_>("amount_minor"),"currency":"USD","mode":"sandbox"},"confirmation_required":row.get::<String,_>("status")=="pending"})))
 }
    let creator=sqlx::query("SELECT user_id,subscription_price_minor FROM creators WHERE id=$1 AND verification_status='approved' FOR SHARE").bind(p.creator_id).fetch_one(&mut *tx).await?;
    if creator.get::<Uuid, _>("user_id") == u.id {
        return Err(bad("You cannot purchase your own content."));
    }
    let amount = match p.kind.as_str() {
        "subscription" => {
            if p.post_id.is_some() {
                return Err(bad("Subscriptions cannot target a post."));
            }
            let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM subscriptions WHERE user_id=$1 AND creator_id=$2 AND status IN ('active','pending','past_due'))").bind(u.id).bind(p.creator_id).fetch_one(&mut *tx).await?;
            if active {
                return Err(Error(
                    StatusCode::CONFLICT,
                    "You already have an active subscription.",
                ));
            }
            creator.get("subscription_price_minor")
        }
        "purchase" => {
            let post = p.post_id.ok_or(bad("Choose a post to purchase."))?;
            let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM entitlements WHERE user_id=$1 AND post_id=$2 AND revoked_at IS NULL)").bind(u.id).bind(post).fetch_one(&mut *tx).await?;
            if owned {
                return Err(Error(StatusCode::CONFLICT, "You already own this post."));
            }
            sqlx::query_scalar("SELECT purchase_price_minor FROM posts WHERE id=$1 AND creator_id=$2 AND visibility='purchase' AND state='published'").bind(post).bind(p.creator_id).fetch_one(&mut *tx).await?
        }
        _ => {
            if p.post_id.is_some() {
                return Err(bad("Tips cannot target a post."));
            }
            p.amount_minor.ok_or(bad("Choose a tip amount."))?
        }
    };
    if !(100..=10000000).contains(&amount) {
        return Err(bad("Amount must be between $1 and $100,000."));
    }
    if s.providers.stripe.is_some() && p.kind != "tip" {
        // A reload/new browser key should resume the already reserved purchase,
        // not produce a unique-index error or create a second provider session.
        let existing:Option<Uuid>=sqlx::query_scalar("SELECT id FROM checkouts WHERE user_id=$1 AND creator_id=$2 AND post_id IS NOT DISTINCT FROM $3 AND kind=$4 AND provider='stripe' AND status='pending'").bind(u.id).bind(p.creator_id).bind(p.post_id).bind(&p.kind).fetch_optional(&mut *tx).await?;
        if let Some(id) = existing {
            tx.commit().await?;
            return providers::stripe::authorized_checkout(&s, &h, u.id, id).await;
        }
    }
    let provider_account = if s.providers.stripe.is_some() {
        Some(providers::stripe::ready_account(&s, creator.get("user_id")).await?)
    } else {
        None
    };
    let provider = if provider_account.is_some() {
        "stripe"
    } else {
        "sandbox"
    };
    let id:Uuid=sqlx::query_scalar("INSERT INTO checkouts(user_id,creator_id,post_id,kind,amount_minor,idempotency_key,request_fingerprint,provider,provider_account) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id").bind(u.id).bind(p.creator_id).bind(p.post_id).bind(&p.kind).bind(amount).bind(p.idempotency_key).bind(fingerprint).bind(provider).bind(provider_account).fetch_one(&mut *tx).await?;
    audit(&mut tx, u.id, "checkout.created", "checkout", id).await?;
    tx.commit().await?;
    if provider == "stripe" {
        return providers::stripe::authorized_checkout(&s, &h, u.id, id).await;
    }
    Ok(Json(
        json!({"checkout":{"id":id,"status":"pending","amount_minor":amount,"currency":"USD","mode":"sandbox"},"confirmation_required":true}),
    ))
}
pub async fn confirm(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let mut tx = s.db.begin().await?;
    let owner:Uuid=sqlx::query_scalar("SELECT r.user_id FROM checkouts c JOIN creators r ON r.id=c.creator_id WHERE c.id=$1 AND c.user_id=$2").bind(id).bind(u.id).fetch_one(&mut *tx).await?;
    community::lock_pair(&mut tx, u.id, owner).await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    let row=sqlx::query("SELECT c.*,r.user_id AS owner,r.verification_status FROM checkouts c JOIN creators r ON r.id=c.creator_id WHERE c.id=$1 AND c.user_id=$2 FOR UPDATE OF c").bind(id).bind(u.id).fetch_one(&mut *tx).await?;
    if row.get::<String, _>("provider") != "sandbox" || s.providers.stripe.is_some() {
        return Err(bad(
            "Provider payments can only be confirmed by the payment provider.",
        ));
    }
    let status: String = row.get("status");
    if status == "paid" {
        return Ok(Json(json!({"status":"paid","mode":"sandbox"})));
    }
    if status != "pending" {
        return Err(bad("This checkout cannot be confirmed."));
    }
    if row.get::<String, _>("verification_status") != "approved" || !u.email_verified {
        return Err(forbidden());
    }
    let kind: String = row.get("kind");
    let creator: Uuid = row.get("creator_id");
    let owner: Uuid = row.get("owner");
    let amount: i64 = row.get("amount_minor");
    let created: chrono::DateTime<chrono::Utc> = row.get("created_at");
    if created < chrono::Utc::now() - chrono::Duration::minutes(30) {
        return Err(bad("This checkout expired. Start a new purchase."));
    }
    if kind == "subscription" {
        let subscription:Uuid=sqlx::query_scalar("INSERT INTO subscriptions(user_id,creator_id,status,provider,provider_subscription_id,current_period_start,current_period_end,cancel_at_period_end) VALUES($1,$2,'active','sandbox',$3,now(),now()+interval '30 days',true) RETURNING id").bind(u.id).bind(creator).bind(id.to_string()).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO entitlements(user_id,creator_id,subscription_id,source,expires_at) VALUES($1,$2,$3,'subscription',now()+interval '30 days')").bind(u.id).bind(creator).bind(subscription).execute(&mut *tx).await?;
    } else if kind == "purchase" {
        let post: Uuid = row.get("post_id");
        let visible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM posts WHERE id=$1 AND state='published' AND visibility='purchase')").bind(post).fetch_one(&mut *tx).await?;
        if !visible {
            return Err(bad("This post is no longer available."));
        }
        let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM entitlements WHERE user_id=$1 AND post_id=$2 AND revoked_at IS NULL)").bind(u.id).bind(post).fetch_one(&mut *tx).await?;
        if owned {
            return Err(Error(StatusCode::CONFLICT, "You already own this post."));
        }
        sqlx::query("INSERT INTO entitlements(user_id,post_id,source) VALUES($1,$2,'purchase')")
            .bind(u.id)
            .bind(post)
            .execute(&mut *tx)
            .await?;
    }
    let clearing = account_id(&mut tx, None, "processor_clearing").await?;
    let pending = account_id(&mut tx, Some(owner), "creator_pending").await?;
    let revenue = account_id(&mut tx, None, "platform_revenue").await?;
    let fee = amount / 5;
    post_ledger(
        &mut tx,
        &format!("checkout:{id}"),
        &[(clearing, -amount), (pending, amount - fee), (revenue, fee)],
    )
    .await?;
    sqlx::query("UPDATE checkouts SET status='paid',paid_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO jobs(kind,payload,run_at) VALUES('settle',$1,now()+interval '1 minute')",
    )
    .bind(json!({"checkout_id":id}))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO outbox_events(topic,aggregate_id,payload) VALUES('checkout.paid',$1,$2)",
    )
    .bind(id)
    .bind(json!({"mode":"sandbox","amount_minor":amount}))
    .execute(&mut *tx)
    .await?;
    notify(
        &mut tx,
        u.id,
        "Purchase confirmed",
        "Your sandbox purchase is complete. No real money moved.",
    )
    .await?;
    notify(
        &mut tx,
        owner,
        "New support",
        "A supporter completed a sandbox purchase.",
    )
    .await?;
    audit(&mut tx, u.id, "checkout.confirmed.sandbox", "checkout", id).await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"paid","mode":"sandbox"})))
}
pub async fn cancel(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let u = user(&s, &h).await?;
    let row: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT provider,status,cancel_at_period_end FROM subscriptions WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(u.id)
    .fetch_optional(&s.db)
    .await?;
    let (provider, status, canceled) =
        row.ok_or(Error(StatusCode::NOT_FOUND, "Subscription not found."))?;
    if canceled {
        return Ok(Json(json!({"cancel_at_period_end":true})));
    }
    if !matches!(status.as_str(), "active" | "past_due") {
        return Err(bad(
            "Only an active or past-due subscription can be canceled. Contact support for an unresolved payment.",
        ));
    }
    authorize_current(&s, &h, u.id, Assurance::Member).await?;
    // Stripe's stable idempotency key makes concurrent requests safe without
    // holding a database transaction across external network I/O.
    if !providers::stripe::cancel(&s, u.id, id).await? && provider == "stripe" {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "The payment provider could not confirm cancellation.",
        ));
    }
    let mut tx = s.db.begin().await?;
    if provider != "stripe" {
        authorize_tx(&mut tx, &h, u.id, Assurance::Member).await?;
    } else {
        // Preserve the provider result for inactive accounts, with the same
        // user-before-subscription order as receipt and refund processing.
        sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
            .bind(u.id)
            .execute(&mut *tx)
            .await?;
    }
    // A provider webhook may have advanced the status while Stripe responded.
    // Record nonrenewal without changing that status, the paid period or access.
    let result=sqlx::query("UPDATE subscriptions SET cancel_at_period_end=true WHERE id=$1 AND user_id=$2 AND NOT cancel_at_period_end").bind(id).bind(u.id).execute(&mut *tx).await?;
    if result.rows_affected() == 1 {
        audit(&mut tx, u.id, "subscription.canceled", "subscription", id).await?;
    } else if !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE id=$1 AND user_id=$2 AND cancel_at_period_end)",
    )
    .bind(id)
    .bind(u.id)
    .fetch_one(&mut *tx)
    .await?
    {
        return Err(Error(StatusCode::NOT_FOUND, "Subscription not found."));
    }
    tx.commit().await?;
    if provider == "stripe" {
        // The remote cancellation already happened. Retain that verified result
        // even if recovery/freeze revoked the caller while the provider replied.
        authorize_current(&s, &h, u.id, Assurance::Member).await?;
    }
    Ok(Json(json!({"cancel_at_period_end":true})))
}
pub async fn earnings(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(q): Query<pagination::PageQuery>,
) -> Result<Json<Value>> {
    q.validate(&["transactions_cursor", "payouts_cursor"])?;
    let u = user(&s, &h).await?;
    let mut tx = s.db.begin().await?;
    let mut amounts = [0; 3];
    for (i, kind) in ["creator_available", "creator_pending", "reserve"]
        .iter()
        .enumerate()
    {
        let id = account_id(&mut tx, Some(u.id), kind).await?;
        amounts[i] = balance(&mut tx, id).await?;
    }
    let (transactions, transactions_next) = pagination::read(&mut *tx, "SELECT p.id,t.id AS transaction_id,t.external_key,t.created_at,a.kind,p.amount_minor FROM ledger_postings p JOIN ledger_accounts a ON a.id=p.account_id JOIN ledger_transactions t ON t.id=p.transaction_id WHERE a.owner_id=$1 AND t.status='posted'", Some(u.id), q.cursor("transactions_cursor")?, "created_at", false).await?;
    let (payouts, payouts_next) = pagination::read(
        &mut *tx,
        "SELECT id,amount_minor,status,created_at FROM payouts WHERE user_id=$1",
        Some(u.id),
        q.cursor("payouts_cursor")?,
        "created_at",
        false,
    )
    .await?;
    tx.commit().await?;
    let mode = if s.providers.stripe.is_some() {
        let (available, pending) = providers::stripe::balances(&s, u.id).await?;
        amounts = [available, pending, 0];
        "stripe"
    } else {
        "sandbox"
    };
    Ok(Json(
        json!({"next_cursors":{"transactions":transactions_next,"payouts":payouts_next},"available_minor":amounts[0],"pending_minor":amounts[1],"reserved_minor":amounts[2],"total_minor":amounts.iter().sum::<i64>(),"currency":"USD","transactions":transactions,"payouts":payouts,"mode":mode}),
    ))
}
#[derive(Deserialize)]
pub struct Payout {
    amount_minor: i64,
    idempotency_key: Uuid,
}
pub async fn payout(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(p): Json<Payout>,
) -> Result<Json<Value>> {
    if s.providers.stripe.is_some() {
        return Err(bad(
            "Manage bank payouts in your connected Stripe dashboard.",
        ));
    }
    let u = user(&s, &h).await?;
    if !u.mfa_enabled || !u.email_verified {
        return Err(bad(
            "Verify your email and enable two-factor authentication before requesting a payout.",
        ));
    }
    mfa_session(&s, &h).await?;
    if p.amount_minor < 100 {
        return Err(bad("Minimum payout is $1."));
    }
    let mut tx = s.db.begin().await?;
    let u = authorize_tx(&mut tx, &h, u.id, Assurance::RecentMfa).await?;
    if !u.email_verified {
        return Err(forbidden());
    }
    if let Some(row) = sqlx::query(
        "SELECT id,amount_minor,status FROM payouts WHERE user_id=$1 AND idempotency_key=$2",
    )
    .bind(u.id)
    .bind(p.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if row.get::<i64, _>("amount_minor") != p.amount_minor {
            return Err(Error(StatusCode::CONFLICT, "Idempotency key already used."));
        }
        return Ok(Json(
            json!({"payout":{"id":row.get::<Uuid,_>("id"),"status":row.get::<String,_>("status")},"mode":"sandbox"}),
        ));
    }
    let approved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM creators WHERE user_id=$1 AND verification_status='approved')",
    )
    .bind(u.id)
    .fetch_one(&mut *tx)
    .await?;
    if !approved {
        return Err(forbidden());
    }
    let available = account_id(&mut tx, Some(u.id), "creator_available").await?;
    let reserve = account_id(&mut tx, Some(u.id), "reserve").await?;
    if balance(&mut tx, available).await? < p.amount_minor {
        return Err(bad("Insufficient available balance."));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO payouts(user_id,amount_minor,idempotency_key) VALUES($1,$2,$3) RETURNING id",
    )
    .bind(u.id)
    .bind(p.amount_minor)
    .bind(p.idempotency_key)
    .fetch_one(&mut *tx)
    .await?;
    post_ledger(
        &mut tx,
        &format!("payout-reserve:{id}"),
        &[(available, -p.amount_minor), (reserve, p.amount_minor)],
    )
    .await?;
    audit(&mut tx, u.id, "payout.requested.sandbox", "payout", id).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"payout":{"id":id,"status":"pending"},"mode":"sandbox"}),
    ))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefundRequest {
    reason: Option<String>,
}
pub(super) async fn lock_checkout_accounts(
    tx: &mut Transaction<'_, Postgres>,
    checkout: Uuid,
    actor: Option<Uuid>,
) -> Result<()> {
    let (buyer, owner):(Uuid,Uuid)=sqlx::query_as("SELECT c.user_id,r.user_id FROM checkouts c JOIN creators r ON r.id=c.creator_id WHERE c.id=$1").bind(checkout).fetch_one(&mut **tx).await?;
    sqlx::query("SELECT id FROM app_users WHERE id=$1 OR id=$2 OR id=$3 ORDER BY id FOR UPDATE")
        .bind(buyer)
        .bind(owner)
        .bind(actor)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub async fn refund(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    body: Option<Json<RefundRequest>>,
) -> Result<Json<Value>> {
    let u = admin(&s, &h).await?;
    let input = body.map(|v| v.0).unwrap_or_default();
    let reason = input
        .reason
        .as_deref()
        .map(providers::stripe::finance::audit_reason)
        .transpose()?;
    if s.runtime.production && reason.is_none() {
        return Err(bad("A refund reason of 5 to 500 characters is required."));
    }
    let mut tx = s.db.begin().await?;
    // Include the buyer: refund notifications take a user foreign-key lock.
    // Refunds and provider observations remain valid for inactive accounts.
    lock_checkout_accounts(&mut tx, id, Some(u.id)).await?;
    authorize_tx(&mut tx, &h, u.id, Assurance::Admin).await?;
    let row=sqlx::query("SELECT c.*,r.user_id AS owner FROM checkouts c JOIN creators r ON r.id=c.creator_id WHERE c.id=$1 FOR UPDATE OF c").bind(id).fetch_one(&mut *tx).await?;
    if row.get::<String, _>("provider") == "stripe" {
        tx.commit().await?;
        return providers::stripe::refund(&s, id, u.id, reason).await;
    }
    if row.get::<String, _>("status") == "refunded" {
        return Ok(Json(json!({"status":"refunded","mode":"sandbox"})));
    }
    if row.get::<String, _>("status") != "paid" {
        return Err(bad("Only paid checkouts can be refunded."));
    }
    let owner: Uuid = row.get("owner");
    sqlx::query("SELECT id FROM app_users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *tx)
        .await?;
    let settled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ledger_transactions WHERE external_key=$1)",
    )
    .bind(format!("settle:{id}"))
    .fetch_one(&mut *tx)
    .await?;
    let earnings = account_id(
        &mut tx,
        Some(owner),
        if settled {
            "creator_available"
        } else {
            "creator_pending"
        },
    )
    .await?;
    let revenue = account_id(&mut tx, None, "platform_revenue").await?;
    let clearing = account_id(&mut tx, None, "processor_clearing").await?;
    let amount: i64 = row.get("amount_minor");
    let fee = amount / 5;
    // A reversal may make the creator balance negative; new payouts remain blocked until recovered.
    post_ledger(
        &mut tx,
        &format!("refund:{id}"),
        &[
            (earnings, -(amount - fee)),
            (revenue, -fee),
            (clearing, amount),
        ],
    )
    .await?;
    let buyer: Uuid = row.get("user_id");
    let creator: Uuid = row.get("creator_id");
    let post: Option<Uuid> = row.get("post_id");
    let kind: String = row.get("kind");
    if kind == "subscription" {
        sqlx::query("UPDATE entitlements SET revoked_at=now() WHERE subscription_id IN (SELECT id FROM subscriptions WHERE provider='sandbox' AND provider_subscription_id=$1)").bind(id.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE subscriptions SET status='canceled' WHERE provider='sandbox' AND provider_subscription_id=$1").bind(id.to_string()).execute(&mut *tx).await?;
    } else if kind == "purchase" {
        sqlx::query("UPDATE entitlements SET revoked_at=now() WHERE user_id=$1 AND post_id=$2 AND source='purchase' AND revoked_at IS NULL").bind(buyer).bind(post).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE checkouts SET status='refunded',refunded_minor=amount_minor WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit(&mut tx, u.id, "checkout.refunded.sandbox", "checkout", id).await?;
    if let Some(reason) = reason {
        sqlx::query("INSERT INTO provider_finance_actions(actor_id,action,target,reason) VALUES($1,'refund.request',$2,$3)").bind(u.id).bind(id.to_string()).bind(reason).execute(&mut *tx).await?;
    }
    notify(
        &mut tx,
        buyer,
        "Purchase refunded",
        "Your sandbox purchase was refunded and its access was revoked.",
    )
    .await?;
    let _ = creator;
    tx.commit().await?;
    Ok(Json(json!({"status":"refunded","mode":"sandbox"})))
}
