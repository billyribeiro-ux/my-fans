//! Independent provider accounting. Webhooks grant verified entitlements; these
//! scans compare provider money, fees, taxes and payouts without granting access.
use super::*;
use crate::platform::pagination::{self, PageQuery};
use axum::extract::Query;
use serde::Deserialize;

const PAGE: usize = 25;
fn parse_currency(v: &Value) -> Result<String> {
    let s = v
        .as_str()
        .filter(|v| v.len() == 3 && v.bytes().all(|b| b.is_ascii_lowercase()))
        .ok_or_else(unavailable)?;
    Ok(s.to_ascii_uppercase())
}
fn minor(v: &Value) -> Result<i64> {
    v.as_i64()
        .filter(|v| (-1_000_000_000_000_000..=1_000_000_000_000_000).contains(v))
        .ok_or_else(unavailable)
}
fn token(v: &Value) -> Result<String> {
    v.as_str()
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 255
                && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(str::to_owned)
        .ok_or_else(unavailable)
}
fn optional_id(v: &Value, prefix: &str) -> Result<Option<String>> {
    if v.is_null() {
        Ok(None)
    } else {
        id(v, prefix).map(Some)
    }
}
pub fn audit_reason(v: &str) -> Result<&str> {
    let v = v.trim();
    if !(5..=500).contains(&v.chars().count()) || v.chars().any(char::is_control) {
        return Err(bad("An audit reason of 5 to 500 characters is required."));
    }
    Ok(v)
}
pub async fn audit_action(
    s: &PlatformState,
    actor: Uuid,
    kind: &str,
    target: &str,
    why: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO provider_finance_actions(actor_id,action,target,reason) VALUES($1,$2,$3,$4)",
    )
    .bind(actor)
    .bind(kind)
    .bind(target)
    .bind(audit_reason(why)?)
    .execute(&s.db)
    .await?;
    Ok(())
}
pub async fn issue(
    s: &PlatformState,
    account: &str,
    object: &str,
    code: &str,
    active: bool,
) -> Result<()> {
    if active {
        sqlx::query("INSERT INTO provider_finance_issues(account_id,object_id,code) VALUES($1,$2,$3) ON CONFLICT(account_id,object_id,code) DO UPDATE SET last_seen_at=now(),resolved_at=NULL,acknowledged_at=CASE WHEN provider_finance_issues.resolved_at IS NULL THEN provider_finance_issues.acknowledged_at END,acknowledged_by=CASE WHEN provider_finance_issues.resolved_at IS NULL THEN provider_finance_issues.acknowledged_by END").bind(account).bind(object).bind(code).execute(&s.db).await?;
    } else {
        sqlx::query("UPDATE provider_finance_issues SET resolved_at=COALESCE(resolved_at,now()) WHERE account_id=$1 AND object_id=$2 AND code=$3").bind(account).bind(object).bind(code).execute(&s.db).await?;
    }
    Ok(())
}
/// Each account and scan gets its own lease: one failing creator cannot starve
/// other creators or stop accounting because the Events API retention elapsed.
pub async fn schedule(s: &PlatformState) -> Result<()> {
    sqlx::query("INSERT INTO provider_finance_scans(account_id,kind,last_completed_at) SELECT account_id,kind,to_timestamp(0) FROM provider_accounts CROSS JOIN unnest(ARRAY['balance','payout','payout_members','dispute','receipt']) AS kind ON CONFLICT DO NOTHING").execute(&s.db).await?;
    sqlx::query("WITH due AS (UPDATE provider_finance_scans SET next_at=now()+interval '30 minutes' WHERE (account_id,kind) IN (SELECT account_id,kind FROM provider_finance_scans WHERE next_at<=now() ORDER BY next_at,account_id,kind FOR UPDATE SKIP LOCKED LIMIT 5) RETURNING account_id,kind) INSERT INTO jobs(kind,payload) SELECT 'stripe_finance',jsonb_build_object('account',account_id,'scan',kind) FROM due ON CONFLICT DO NOTHING").execute(&s.db).await?;
    Ok(())
}
pub async fn reconcile(s: &PlatformState, payload: &Value) -> Result<()> {
    let account = id(&payload["account"], "acct_")?;
    let kind = payload["scan"].as_str().ok_or_else(unavailable)?;
    let row = sqlx::query("SELECT * FROM provider_finance_scans WHERE account_id=$1 AND kind=$2")
        .bind(&account)
        .bind(kind)
        .fetch_optional(&s.db)
        .await?;
    let Some(row) = row else { return Ok(()) };
    if kind == "receipt" {
        check_receipt(s, &account).await?;
        sqlx::query("UPDATE provider_finance_scans SET last_completed_at=now(),next_at=now()+interval '30 seconds' WHERE account_id=$1 AND kind=$2").bind(&account).bind(kind).execute(&s.db).await?;
        return Ok(());
    }
    if kind == "payout_members" {
        payout_members(s, &account).await?;
        sqlx::query("UPDATE provider_finance_scans SET last_completed_at=now(),next_at=now()+interval '30 seconds' WHERE account_id=$1 AND kind=$2").bind(&account).bind(kind).execute(&s.db).await?;
        return Ok(());
    }
    let (path, prefix, overlap) = match kind {
        "balance" => ("/balance_transactions", "txn_", 600),
        "payout" => ("/payouts", "po_", 14 * 86400),
        "dispute" => ("/disputes", "du_", 600),
        _ => return Err(unavailable()),
    };
    // Pending balance transactions and open disputes may change long after
    // creation. Refresh one older mutable object independently on every pass.
    if kind == "balance" {
        let old:Option<String>=sqlx::query_scalar("SELECT transaction_id FROM provider_financial_transactions WHERE account_id=$1 AND status='pending' AND checked_at<now()-interval '5 minutes' ORDER BY checked_at,id LIMIT 1").bind(&account).fetch_optional(&s.db).await?;
        if let Some(old) = old {
            let v = stripe(s)?
                .get(&format!("/balance_transactions/{old}"), Some(&account))
                .await?;
            if id(&v, "txn_")? != old {
                return Err(unavailable());
            };
            record_balance(s, &account, &v).await?;
        }
    }
    if kind == "dispute" {
        let old:Option<String>=sqlx::query_scalar("SELECT dispute_id FROM provider_disputes WHERE account_id=$1 AND status NOT IN ('won','lost','warning_closed','prevented','charge_refunded') AND checked_at<now()-interval '5 minutes' ORDER BY checked_at,id LIMIT 1").bind(&account).fetch_optional(&s.db).await?;
        if let Some(old) = old {
            sync_dispute(s, &account, &old).await?;
        }
    }
    let last: DateTime<Utc> = row.get("last_completed_at");
    let start = row
        .get::<Option<i64>, _>("window_start")
        .unwrap_or((last.timestamp() - overlap).max(0));
    let end = row
        .get::<Option<i64>, _>("window_end")
        .unwrap_or(Utc::now().timestamp());
    let cursor: Option<String> = row.get("cursor");
    let page_limit = if kind == "dispute" { 5 } else { PAGE };
    let mut fields = form(&[
        ("limit", page_limit.to_string()),
        ("created[gte]", start.to_string()),
        ("created[lte]", end.to_string()),
    ]);
    if let Some(cursor) = &cursor {
        fields.push(("starting_after".into(), cursor.clone()));
    }
    let v = stripe(s)?
        .request(reqwest::Method::GET, path, Some(&account), &fields, None)
        .await?;
    let data = v["data"]
        .as_array()
        .filter(|v| v.len() <= PAGE)
        .ok_or_else(unavailable)?;
    let more = v["has_more"].as_bool().ok_or_else(unavailable)?;
    for value in data {
        match kind {
            "balance" => {
                record_balance(s, &account, value).await?;
            }
            "payout" => {
                record_payout(s, &account, value).await?;
            }
            "dispute" => {
                sync_dispute(s, &account, &id(value, "du_")?).await?;
            }
            _ => return Err(unavailable()),
        }
    }
    if more {
        let next = id(data.last().ok_or_else(unavailable)?, prefix)?;
        if cursor.as_deref() == Some(&next) {
            return Err(unavailable());
        };
        sqlx::query("UPDATE provider_finance_scans SET window_start=$3,window_end=$4,cursor=$5,next_at=now() WHERE account_id=$1 AND kind=$2").bind(&account).bind(kind).bind(start).bind(end).bind(next).execute(&s.db).await?;
    } else {
        sqlx::query("UPDATE provider_finance_scans SET last_completed_at=to_timestamp($3),window_start=NULL,window_end=NULL,cursor=NULL,next_at=now()+interval '5 minutes' WHERE account_id=$1 AND kind=$2").bind(&account).bind(kind).bind(end as f64).execute(&s.db).await?;
    }
    Ok(())
}
async fn record_balance(s: &PlatformState, account: &str, v: &Value) -> Result<Uuid> {
    let transaction = id(v, "txn_")?;
    let currency = parse_currency(&v["currency"])?;
    let gross = minor(&v["amount"])?;
    let fee = minor(&v["fee"])?;
    let net = minor(&v["net"])?;
    if gross - fee != net {
        return Err(unavailable());
    };
    let status = v["status"]
        .as_str()
        .filter(|v| matches!(*v, "pending" | "available"))
        .ok_or_else(unavailable)?;
    let category = token(&v["reporting_category"])?;
    let source = if v["source"].is_null() {
        None
    } else {
        Some(token(if v["source"].is_object() {
            &v["source"]["id"]
        } else {
            &v["source"]
        })?)
    };
    let details = v["fee_details"]
        .as_array()
        .filter(|v| v.len() <= 100)
        .ok_or_else(unavailable)?;
    let mut fees = Vec::new();
    let mut total = 0i64;
    for item in details {
        let value = minor(&item["amount"])?;
        if parse_currency(&item["currency"])? != currency {
            return Err(unavailable());
        };
        total = total.checked_add(value).ok_or_else(unavailable)?;
        fees.push(json!({"type":token(&item["type"])?,"amount":value,"currency":currency}));
    }
    if total != fee {
        return Err(unavailable());
    };
    let fees = json!(fees);
    let created = timestamp(&v["created"])?;
    let available = timestamp(&v["available_on"])?;
    let fingerprint = digest(
        &json!([
            transaction,
            source,
            category,
            currency,
            gross,
            fee,
            net,
            fees,
            created.timestamp()
        ])
        .to_string(),
    );
    let row:(Uuid,String)=sqlx::query_as("INSERT INTO provider_financial_transactions(account_id,transaction_id,source_id,reporting_category,currency,gross_minor,fee_minor,net_minor,fee_details,provider_created_at,available_at,status,fingerprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT(account_id,transaction_id) DO UPDATE SET checked_at=now(),status=EXCLUDED.status,available_at=EXCLUDED.available_at RETURNING id,fingerprint").bind(account).bind(&transaction).bind(&source).bind(category).bind(&currency).bind(gross).bind(fee).bind(net).bind(fees).bind(created).bind(available).bind(status).bind(&fingerprint).fetch_one(&s.db).await?;
    issue(
        s,
        account,
        &transaction,
        "journal_changed",
        row.1 != fingerprint,
    )
    .await?;
    if let Some(source) = source.filter(|v| v.starts_with("ch_")) {
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM provider_receipts WHERE account_id=$1 AND charge_id=$2)",
        )
        .bind(account)
        .bind(&source)
        .fetch_one(&s.db)
        .await?;
        issue(s, account, &source, "unknown_charge", !known).await?;
    }
    Ok(row.0)
}
async fn record_payout(s: &PlatformState, account: &str, v: &Value) -> Result<()> {
    stripe(s)?.mode(v)?;
    let payout = id(v, "po_")?;
    let amount = minor(&v["amount"])?;
    if amount <= 0 {
        return Err(unavailable());
    };
    let status = v["status"]
        .as_str()
        .filter(|v| {
            matches!(
                *v,
                "pending" | "in_transit" | "paid" | "failed" | "canceled"
            )
        })
        .ok_or_else(unavailable)?;
    let automatic = v["automatic"].as_bool().ok_or_else(unavailable)?;
    let reconciliation = v["reconciliation_status"]
        .as_str()
        .filter(|v| matches!(*v, "completed" | "in_progress" | "not_applicable"))
        .ok_or_else(unavailable)?;
    let stored:(i64,String)=sqlx::query_as("INSERT INTO provider_payouts(account_id,payout_id,amount_minor,currency,status,automatic,reconciliation_status,balance_transaction_id,failure_balance_transaction_id,arrival_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT(account_id,payout_id) DO UPDATE SET status=EXCLUDED.status,reconciliation_status=EXCLUDED.reconciliation_status,balance_transaction_id=EXCLUDED.balance_transaction_id,failure_balance_transaction_id=EXCLUDED.failure_balance_transaction_id,arrival_at=EXCLUDED.arrival_at,checked_at=now() RETURNING amount_minor,currency").bind(account).bind(&payout).bind(amount).bind(parse_currency(&v["currency"])?).bind(status).bind(automatic).bind(reconciliation).bind(optional_id(&v["balance_transaction"],"txn_")?).bind(optional_id(&v["failure_balance_transaction"],"txn_")?).bind(timestamp(&v["arrival_date"])?).bind(timestamp(&v["created"])?).fetch_one(&s.db).await?;
    issue(
        s,
        account,
        &payout,
        "payout_identity",
        stored != (amount, parse_currency(&v["currency"])?),
    )
    .await?;
    issue(s, account, &payout, "payout_failed", status == "failed").await?;
    Ok(())
}
async fn payout_members(s: &PlatformState, account: &str) -> Result<()> {
    let row=sqlx::query("SELECT * FROM provider_payouts WHERE account_id=$1 AND automatic AND reconciliation_status='completed' AND NOT members_complete ORDER BY created_at,id LIMIT 1").bind(account).fetch_optional(&s.db).await?;
    let Some(row) = row else { return Ok(()) };
    let payout: String = row.get("payout_id");
    let key: Uuid = row.get("id");
    let cursor: Option<String> = row.get("members_cursor");
    let mut fields = form(&[("limit", PAGE.to_string()), ("payout", payout.clone())]);
    if let Some(cursor) = &cursor {
        fields.push(("starting_after".into(), cursor.clone()))
    };
    let v = stripe(s)?
        .request(
            reqwest::Method::GET,
            "/balance_transactions",
            Some(account),
            &fields,
            None,
        )
        .await?;
    let data = v["data"]
        .as_array()
        .filter(|v| v.len() <= PAGE)
        .ok_or_else(unavailable)?;
    for value in data {
        let txid = record_balance(s, account, value).await?;
        sqlx::query("INSERT INTO provider_payout_members(payout_id,transaction_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(key).bind(txid).execute(&s.db).await?;
    }
    match v["has_more"].as_bool().ok_or_else(unavailable)? {
        true => {
            let next = id(data.last().ok_or_else(unavailable)?, "txn_")?;
            if cursor.as_deref() == Some(&next) {
                return Err(unavailable());
            };
            sqlx::query("UPDATE provider_payouts SET members_cursor=$2 WHERE id=$1")
                .bind(key)
                .bind(next)
                .execute(&s.db)
                .await?;
        }
        false => {
            let amount:i64=sqlx::query_scalar("SELECT COALESCE(sum(t.net_minor),0)::bigint FROM provider_payout_members m JOIN provider_financial_transactions t ON t.id=m.transaction_id WHERE m.payout_id=$1 AND t.currency=$2").bind(key).bind(row.get::<String,_>("currency")).fetch_one(&s.db).await?;
            let mismatched:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_payout_members m JOIN provider_financial_transactions t ON t.id=m.transaction_id WHERE m.payout_id=$1 AND t.currency<>$2)").bind(key).bind(row.get::<String,_>("currency")).fetch_one(&s.db).await?;
            issue(
                s,
                account,
                &payout,
                "payout_amount",
                amount != row.get::<i64, _>("amount_minor") || mismatched,
            )
            .await?;
            sqlx::query(
                "UPDATE provider_payouts SET members_cursor=NULL,members_complete=true WHERE id=$1",
            )
            .bind(key)
            .execute(&s.db)
            .await?;
        }
    }
    Ok(())
}
async fn check_receipt(s: &PlatformState, account: &str) -> Result<()> {
    let row=sqlx::query("SELECT r.*,c.provider_session FROM provider_receipts r JOIN checkouts c ON c.id=r.checkout_id LEFT JOIN provider_receipt_finance f ON f.receipt_id=r.id WHERE r.account_id=$1 AND (f.checked_at IS NULL OR f.checked_at<now()-interval '6 hours') ORDER BY f.checked_at NULLS FIRST,r.created_at,r.id LIMIT 1").bind(account).fetch_optional(&s.db).await?;
    let Some(row) = row else { return Ok(()) };
    let receipt: Uuid = row.get("id");
    let charge: String = row.get("charge_id");
    let amount: i64 = row.get("amount_minor");
    // Claim the oldest receipt before I/O so an unavailable historical object
    // cannot starve every subsequent sale. Failed jobs still retry independently.
    sqlx::query("INSERT INTO provider_receipt_finance(receipt_id) VALUES($1) ON CONFLICT(receipt_id) DO UPDATE SET checked_at=now(),verified=false").bind(receipt).execute(&s.db).await?;
    let p = stripe(s)?;
    let v = p.get(&format!("/charges/{charge}"), Some(account)).await?;
    p.mode(&v)?;
    if id(&v, "ch_")? != charge {
        return Err(unavailable());
    };
    let charge_valid = v["amount"].as_i64() == Some(amount)
        && v["amount_captured"].as_i64() == Some(amount)
        && v["currency"] == "usd"
        && v["paid"] == true;
    issue(s, account, &charge, "charge_amount", !charge_valid).await?;
    let balance = id(&v["balance_transaction"], "txn_")?;
    let bt = p
        .get(&format!("/balance_transactions/{balance}"), Some(account))
        .await?;
    if id(&bt, "txn_")? != balance {
        return Err(unavailable());
    };
    record_balance(s, account, &bt).await?;
    let charge_valid = charge_valid
        && bt["source"] == charge
        && bt["amount"].as_i64() == Some(amount)
        && bt["currency"] == "usd";
    issue(s, account, &charge, "charge_amount", !charge_valid).await?;
    let fee_id = id(&v["application_fee"], "fee_")?;
    let fee = p.get(&format!("/application_fees/{fee_id}"), None).await?;
    p.mode(&fee)?;
    if id(&fee, "fee_")? != fee_id {
        return Err(unavailable());
    };
    let actual_fee = minor(&fee["amount"])?;
    let refunded_fee = minor(&fee["amount_refunded"])?;
    let fee_valid = fee["account"] == account
        && fee["charge"] == charge
        && fee["currency"] == "usd"
        && (actual_fee == amount / 5
            || (row.get::<Option<String>, _>("invoice_id").is_some()
                && actual_fee == (amount + 4) / 5))
        && refunded_fee >= 0
        && refunded_fee <= actual_fee;
    issue(s, account, &charge, "application_fee", !fee_valid).await?;
    let (tax, tax_valid) = if let Some(invoice) = row.get::<Option<String>, _>("invoice_id") {
        let v = p
            .get(&format!("/invoices/{invoice}"), Some(account))
            .await?;
        p.mode(&v)?;
        if id(&v, "in_")? != invoice {
            return Err(unavailable());
        };
        let mut tax = 0i64;
        let taxes = v["total_taxes"].as_array().ok_or_else(unavailable)?;
        for item in taxes {
            tax = tax
                .checked_add(minor(&item["amount"])?)
                .ok_or_else(unavailable)?;
        }
        (
            tax,
            v["automatic_tax"]["status"] == "complete"
                && v["currency"] == "usd"
                && v["total"].as_i64() == Some(amount),
        )
    } else if let Some(session) = row.get::<Option<String>, _>("provider_session") {
        let v = p
            .get(&format!("/checkout/sessions/{session}"), Some(account))
            .await?;
        p.mode(&v)?;
        if id(&v, "cs_")? != session {
            return Err(unavailable());
        };
        (
            minor(&v["total_details"]["amount_tax"])?,
            v["automatic_tax"]["status"] == "complete"
                && v["currency"] == "usd"
                && v["amount_total"].as_i64() == Some(amount)
                && v["client_reference_id"] == row.get::<Uuid, _>("checkout_id").to_string(),
        )
    } else {
        (0, false)
    };
    let tax_valid = tax_valid && (0..=amount).contains(&tax);
    issue(s, account, &charge, "tax_unverified", !tax_valid).await?;
    sqlx::query("UPDATE provider_receipt_finance SET balance_transaction_id=$2,application_fee_id=$3,application_fee_minor=$4,refunded_fee_minor=$5,tax_minor=$6,verified=$7,checked_at=now() WHERE receipt_id=$1").bind(receipt).bind(balance).bind(fee_id).bind(actual_fee).bind(refunded_fee).bind(if tax_valid{Some(tax)}else{None}).bind(charge_valid&&fee_valid&&tax_valid).execute(&s.db).await?;
    Ok(())
}
/// Retrieve the current dispute; never decide from an old webhook snapshot.
/// Evidence submission stays in Stripe's hosted dashboard, including deadlines.
pub async fn sync_dispute(s: &PlatformState, account: &str, dispute: &str) -> Result<Value> {
    id(&json!(account), "acct_")?;
    id(&json!(dispute), "du_")?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_accounts WHERE account_id=$1)")
            .bind(account)
            .fetch_one(&s.db)
            .await?;
    if !known {
        return Err(forbidden());
    };
    let p = stripe(s)?;
    let v = p
        .get(&format!("/disputes/{dispute}"), Some(account))
        .await?;
    p.mode(&v)?;
    if id(&v, "du_")? != dispute {
        return Err(unavailable());
    };
    let charge = id(&v["charge"], "ch_")?;
    let amount = minor(&v["amount"])?;
    if amount <= 0 {
        return Err(unavailable());
    };
    let status = v["status"]
        .as_str()
        .filter(|v| {
            matches!(
                *v,
                "warning_needs_response"
                    | "warning_under_review"
                    | "warning_closed"
                    | "needs_response"
                    | "under_review"
                    | "won"
                    | "lost"
                    | "prevented"
                    | "charge_refunded"
            )
        })
        .ok_or_else(unavailable)?;
    let due = if v["evidence_details"]["due_by"].is_null() {
        None
    } else {
        Some(timestamp(&v["evidence_details"]["due_by"])?)
    };
    sqlx::query("INSERT INTO provider_disputes(account_id,dispute_id,charge_id,amount_minor,currency,status,due_by,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(account_id,dispute_id) DO UPDATE SET status=EXCLUDED.status,due_by=EXCLUDED.due_by,checked_at=now() WHERE provider_disputes.charge_id=EXCLUDED.charge_id AND provider_disputes.amount_minor=EXCLUDED.amount_minor AND provider_disputes.currency=EXCLUDED.currency").bind(account).bind(dispute).bind(&charge).bind(amount).bind(parse_currency(&v["currency"])?).bind(status).bind(due).bind(timestamp(&v["created"])?).execute(&s.db).await?;
    issue(
        s,
        account,
        dispute,
        "dispute_response",
        matches!(status, "warning_needs_response" | "needs_response"),
    )
    .await?;
    if let Some(transactions) = v["balance_transactions"].as_array() {
        if transactions.len() > 10 {
            return Err(unavailable());
        };
        for transaction in transactions {
            record_balance(s, account, transaction).await?;
        }
    }
    sync_reversal(s, account, &charge).await?;
    Ok(json!({"status":status,"charge_id":charge}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutResolution {
    pub checkout_id: Uuid,
    pub session_id: String,
    pub reason: String,
}
pub async fn resolve_checkout(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(input): Json<CheckoutResolution>,
) -> Result<Json<Value>> {
    let actor = admin(&s, &h).await?;
    audit_reason(&input.reason)?;
    rate_limit(&s, &format!("finance:{}", actor.id), 10).await?;
    let session = id(&json!(input.session_id), "cs_")?;
    let row=sqlx::query("SELECT provider_account,amount_minor,kind,provider_session FROM checkouts WHERE id=$1 AND provider='stripe'").bind(input.checkout_id).fetch_one(&s.db).await?;
    let account: String = row.get("provider_account");
    let p = stripe(&s)?;
    let v = p
        .get(&format!("/checkout/sessions/{session}"), Some(&account))
        .await?;
    p.mode(&v)?;
    let mode = if row.get::<String, _>("kind") == "subscription" {
        "subscription"
    } else {
        "payment"
    };
    if id(&v, "cs_")? != session
        || v["client_reference_id"] != input.checkout_id.to_string()
        || v["mode"] != mode
        || v["currency"] != "usd"
        || v["amount_total"].as_i64() != Some(row.get("amount_minor"))
        || row
            .get::<Option<String>, _>("provider_session")
            .is_some_and(|s| s != session)
    {
        return Err(bad("The provider session does not match this checkout."));
    }
    // Record intent before the idempotent provider synchronization. A crash can
    // be retried safely and cannot manufacture payment or release uncertainty.
    authorize_current(&s, &h, actor.id, Assurance::Admin).await?;
    audit_action(
        &s,
        actor.id,
        "checkout.attach",
        &input.checkout_id.to_string(),
        &input.reason,
    )
    .await?;
    sync_checkout(&s, &account, &session).await?;
    let status: String = sqlx::query_scalar("SELECT status FROM checkouts WHERE id=$1")
        .bind(input.checkout_id)
        .fetch_one(&s.db)
        .await?;
    issue(
        &s,
        &account,
        &input.checkout_id.to_string(),
        "checkout_indeterminate",
        false,
    )
    .await?;
    Ok(Json(
        json!({"checkout_id":input.checkout_id,"status":status}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisputeResolution {
    pub account_id: String,
    pub dispute_id: String,
    pub restore_access: bool,
    pub reason: String,
}
pub async fn refresh_dispute(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(input): Json<DisputeResolution>,
) -> Result<Json<Value>> {
    let actor = admin(&s, &h).await?;
    audit_reason(&input.reason)?;
    rate_limit(&s, &format!("finance:{}", actor.id), 10).await?;
    let result = sync_dispute(&s, &input.account_id, &input.dispute_id).await?;
    authorize_current(&s, &h, actor.id, Assurance::Admin).await?;
    audit_action(
        &s,
        actor.id,
        if input.restore_access {
            "dispute.restore"
        } else {
            "dispute.refresh"
        },
        &input.dispute_id,
        &input.reason,
    )
    .await?;
    let mut restored = false;
    if input.restore_access {
        if result["status"] != "won" {
            return Err(bad(
                "Access can only be restored after a provider-confirmed merchant win.",
            ));
        }
        let charge = id(&result["charge_id"], "ch_")?;
        // Current charge and all current disputes are rechecked in the restoration
        // helper. Existing refunds and non-financial revocations always win.
        restored = restore_access(&s, &h, &input.account_id, &charge, actor.id).await?;
    }
    Ok(Json(json!({"status":result["status"],"restored":restored})))
}
async fn restore_access(
    s: &PlatformState,
    h: &HeaderMap,
    account: &str,
    charge: &str,
    actor: Uuid,
) -> Result<bool> {
    let p = stripe(s)?;
    let v = p.get(&format!("/charges/{charge}"), Some(account)).await?;
    p.mode(&v)?;
    if id(&v, "ch_")? != charge
        || v["amount_refunded"] != 0
        || v["paid"] != true
        || v["status"] != "succeeded"
    {
        return Err(bad("This payment is not eligible for restored access."));
    }
    let disputes = p
        .request(
            reqwest::Method::GET,
            "/disputes",
            Some(account),
            &form(&[("charge", charge.to_string()), ("limit", "25".into())]),
            None,
        )
        .await?;
    let current = disputes["data"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 25)
        .ok_or_else(unavailable)?;
    if disputes["has_more"] != false {
        return Err(bad(
            "All payment disputes must be verified before restoring access.",
        ));
    }
    let mut won = false;
    for dispute in current {
        p.mode(dispute)?;
        if id(&dispute["charge"], "ch_")? != charge
            || !matches!(
                dispute["status"].as_str(),
                Some("won" | "warning_closed" | "prevented")
            )
        {
            return Err(bad("Other disputes still prevent restored access."));
        }
        won |= dispute["status"] == "won";
    }
    if !won {
        return Err(bad(
            "Access can only be restored after a provider-confirmed merchant win.",
        ));
    }
    let mut tx = s.db.begin().await?;
    let buyer:Option<Uuid>=sqlx::query_scalar("SELECT e.user_id FROM provider_receipts r JOIN entitlements e ON e.id=r.entitlement_id WHERE r.account_id=$1 AND r.charge_id=$2").bind(account).bind(charge).fetch_optional(&mut *tx).await?;
    sqlx::query("SELECT id FROM app_users WHERE id=$1 OR id=$2 ORDER BY id FOR UPDATE")
        .bind(actor)
        .bind(buyer)
        .execute(&mut *tx)
        .await?;
    authorize_tx(&mut tx, h, actor, Assurance::Admin).await?;
    let row=sqlx::query("SELECT r.id,r.amount_minor,r.refunded_minor,r.entitlement_id,f.revoked_at,f.restored_at FROM provider_receipts r JOIN provider_financial_revocations f ON f.receipt_id=r.id WHERE r.account_id=$1 AND r.charge_id=$2 FOR UPDATE OF r,f").bind(account).bind(charge).fetch_optional(&mut *tx).await?;
    let Some(row) = row else { return Ok(false) };
    if row.get::<i64, _>("refunded_minor") != 0
        || v["amount"].as_i64() != Some(row.get("amount_minor"))
        || v["currency"] != "usd"
    {
        return Err(bad("This payment is not eligible for restored access."));
    }
    let can_restore:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_disputes WHERE account_id=$1 AND charge_id=$2 AND status='won') AND NOT EXISTS(SELECT 1 FROM provider_disputes WHERE account_id=$1 AND charge_id=$2 AND status NOT IN ('won','warning_closed','prevented'))").bind(account).bind(charge).fetch_one(&mut *tx).await?;
    if !can_restore {
        return Err(bad("Other disputes still prevent restored access."));
    }
    let count=sqlx::query("UPDATE entitlements e SET revoked_at=NULL WHERE id=$1 AND revoked_at=$2 AND (expires_at IS NULL OR expires_at>now()) AND EXISTS(SELECT 1 FROM app_users u WHERE u.id=e.user_id AND u.disabled_at IS NULL)").bind(row.get::<Option<Uuid>,_>("entitlement_id")).bind(row.get::<DateTime<Utc>,_>("revoked_at")).execute(&mut *tx).await?.rows_affected();
    if count > 0 {
        sqlx::query("UPDATE provider_financial_revocations SET restored_at=now(),restored_by=$2 WHERE receipt_id=$1").bind(row.get::<Uuid,_>("id")).bind(actor).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(count > 0)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledgment {
    pub issue_id: Uuid,
    pub reason: String,
}
pub async fn acknowledge(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Json(input): Json<Acknowledgment>,
) -> Result<Json<Value>> {
    let actor = admin(&s, &h).await?;
    audit_reason(&input.reason)?;
    rate_limit(&s, &format!("finance:{}", actor.id), 10).await?;
    let mut tx = s.db.begin().await?;
    authorize_tx(&mut tx, &h, actor.id, Assurance::Admin).await?;
    let n=sqlx::query("UPDATE provider_finance_issues SET acknowledged_at=now(),acknowledged_by=$2 WHERE id=$1 AND resolved_at IS NULL").bind(input.issue_id).bind(actor.id).execute(&mut *tx).await?.rows_affected();
    if n == 0 {
        return Err(bad("This issue is missing or already resolved."));
    }
    sqlx::query("INSERT INTO provider_finance_actions(actor_id,action,target,reason) VALUES($1,'issue.acknowledge',$2,$3)").bind(actor.id).bind(input.issue_id.to_string()).bind(audit_reason(&input.reason)?).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"status":"acknowledged","issue_id":input.issue_id}),
    ))
}
pub async fn overview(
    State(s): State<PlatformState>,
    h: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>> {
    admin(&s, &h).await?;
    query.validate(&[
        "accounts_cursor",
        "issues_cursor",
        "checkouts_cursor",
        "disputes_cursor",
        "payouts_cursor",
    ])?;
    let (accounts, accounts_next) = pagination::read(
        &s.db,
        "SELECT id,account_id,kind,last_completed_at,next_at FROM provider_finance_scans",
        None,
        query.cursor("accounts_cursor")?,
        "last_completed_at",
        true,
    )
    .await?;
    let (issues,issues_next)=pagination::read(&s.db,"SELECT id,account_id,object_id,code,first_seen_at,last_seen_at,acknowledged_at,resolved_at FROM provider_finance_issues WHERE resolved_at IS NULL",None,query.cursor("issues_cursor")?,"first_seen_at",true).await?;
    let (checkouts,checkouts_next)=pagination::read(&s.db,"SELECT id,provider_account,provider_session,created_at,status FROM checkouts WHERE provider='stripe' AND status='pending' AND created_at<now()-interval '25 minutes'",None,query.cursor("checkouts_cursor")?,"created_at",true).await?;
    let (disputes,disputes_next)=pagination::read(&s.db,"SELECT id,account_id,dispute_id,charge_id,status,amount_minor,currency,due_by,created_at,checked_at FROM provider_disputes",None,query.cursor("disputes_cursor")?,"created_at",false).await?;
    let (payouts,payouts_next)=pagination::read(&s.db,"SELECT id,account_id,payout_id,status,amount_minor,currency,arrival_at,created_at,checked_at,automatic,reconciliation_status,members_complete FROM provider_payouts",None,query.cursor("payouts_cursor")?,"created_at",false).await?;
    let scanner:Value=sqlx::query_scalar("SELECT jsonb_build_object('open_issues',(SELECT count(*) FROM provider_finance_issues WHERE resolved_at IS NULL),'failed_jobs',(SELECT count(*) FROM jobs WHERE kind='stripe_finance' AND status='failed'),'unverified_receipts',(SELECT count(*) FROM provider_receipts r LEFT JOIN provider_receipt_finance f ON f.receipt_id=r.id WHERE NOT COALESCE(f.verified,false)),'next_at',(SELECT min(next_at) FROM provider_finance_scans))").fetch_one(&s.db).await?;
    Ok(Json(
        json!({"mode":if s.providers.stripe.is_some(){"stripe"}else{"sandbox"},"accounts":accounts,"issues":issues,"checkouts":checkouts,"disputes":disputes,"payouts":payouts,"scanner":scanner,"next_cursors":{"accounts":accounts_next,"issues":issues_next,"checkouts":checkouts_next,"disputes":disputes_next,"payouts":payouts_next}}),
    ))
}

#[cfg(test)]
#[path = "stripe_finance_tests.rs"]
mod tests;
