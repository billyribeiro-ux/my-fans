#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use myfans_api::platform::{self, PlatformState, worker};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, PgPool};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn creator_progress_is_private_and_rejected_review_can_be_resubmitted(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let other = login(&app, "studioluna@myfans.test").await;
    let reviewer = admin(&app).await;
    let (id, owner): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,user_id FROM creators WHERE handle='ellawild'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    sqlx::query("UPDATE creators SET verification_status='pending',verified_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO provider_accounts(user_id,account_id,charges_enabled,payouts_enabled,checked_at) VALUES($1,'acct_PrivateOnboarding',true,false,now())")
        .bind(owner).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO identity_checks(creator_id,applicant_id,status,checked_at) VALUES($1,'private-applicant-identifier','verified',now())")
        .bind(id).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO moderation_evidence(actor_id,resource_type,resource_id,decision,reason,policy_version,snapshot,snapshot_sha256) VALUES($1,'creator',$2,'reject','Historical internal investigation detail','sandbox-1','{}',repeat('a',64))")
        .bind(owner).bind(id).execute(&s.db).await.unwrap();
    let (_, before_review) = call(&app, "GET", "/v1/studio", Some(&creator), json!({})).await;
    assert!(before_review["onboarding"]["review"]["reason"].is_null());
    assert!(
        !before_review
            .to_string()
            .contains("Historical internal investigation detail")
    );
    let original_reason = "Please clarify the profile offering before approval.";
    let (status, response) = call(
        &app,
        "POST",
        &format!("/v1/admin/reviews/{id}"),
        Some(&reviewer),
        json!({"kind":"creator","decision":"reject","reason":original_reason}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let (_, studio) = call(&app, "GET", "/v1/studio", Some(&creator), json!({})).await;
    assert_eq!(studio["onboarding"]["viewer_id"], owner.to_string());
    assert_eq!(studio["onboarding"]["review"]["reason"], original_reason);
    assert_eq!(
        studio["onboarding"]["review"]["resubmission_available"],
        true
    );
    assert_eq!(studio["onboarding"]["identity"]["status"], "verified");
    assert_eq!(studio["onboarding"]["payments"]["charges_enabled"], true);
    assert_eq!(studio["onboarding"]["payments"]["payouts_enabled"], false);
    assert!(!studio.to_string().contains("acct_PrivateOnboarding"));
    assert!(!studio.to_string().contains("private-applicant-identifier"));
    let (_, account) = call(&app, "GET", "/v1/account", Some(&creator), json!({})).await;
    assert_eq!(account["onboarding"], studio["onboarding"]);
    let (_, other_studio) = call(&app, "GET", "/v1/studio", Some(&other), json!({})).await;
    assert_ne!(other_studio["onboarding"]["viewer_id"], owner.to_string());
    assert!(!other_studio.to_string().contains(original_reason));
    let (_, guest) = call(&app, "GET", "/v1/account", None, json!({})).await;
    assert!(guest["onboarding"].is_null());
    assert_eq!(
        call(&app, "GET", "/v1/studio", None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    let request =
        json!({"reason":"Updated the offering description to explain subscriber benefits."});
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/creators/resubmit",
            Some(&creator),
            request.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    enroll_mfa(&app, &creator).await;
    sqlx::query("UPDATE app_users SET email_verified=false WHERE id=$1")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/creators/resubmit",
            Some(&creator),
            request.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE app_users SET email_verified=true WHERE id=$1")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        call(
            &app,
            "POST",
            "/v1/creators/resubmit",
            Some(&creator),
            request.clone()
        ),
        call(
            &app,
            "POST",
            "/v1/creators/resubmit",
            Some(&creator),
            request.clone()
        )
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(a.1["status"], "pending");
    assert_eq!(b.1["status"], "pending");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM moderation_evidence WHERE resource_type='creator' AND resource_id=$1 AND decision='resubmit'").bind(id).fetch_one(&s.db).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM audit_events WHERE resource_id=$1 AND action='creator.review.resubmitted'").bind(id).fetch_one(&s.db).await.unwrap(), 1);
    let (_, reviews) = call(&app, "GET", "/v1/admin/reviews", Some(&reviewer), json!({})).await;
    let row = reviews["creators"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id.to_string())
        .unwrap();
    assert_eq!(row["review_context"]["decision"], "resubmit");
    assert_eq!(row["review_context"]["reason"], request["reason"]);
    let (status, response) = call(&app, "POST", &format!("/v1/admin/reviews/{id}"), Some(&reviewer), json!({"kind":"creator","decision":"approve","reason":"Updated offering is clear and approved."})).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/creators/resubmit",
            Some(&creator),
            request.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE app_users SET disabled_at=now() WHERE id=$1")
        .bind(owner)
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/creators/resubmit",
            Some(&creator),
            request
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn verification_resend_invalidates_old_links_and_is_account_scoped(db: PgPool) {
    let (s, app) = setup(db).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/verification/resend",
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status,registered)=call(&app,"POST","/v1/auth/register",None,json!({"email":"verify@test.example","password":"StrongPassword!2026","display_name":"Verification"})).await;
    assert_eq!(status, StatusCode::OK);
    let token = registered["token"].as_str().unwrap();
    let id = Uuid::parse_str(registered["user"]["id"].as_str().unwrap()).unwrap();
    let (_, old_mail) = call(&app, "GET", "/v1/account/mail", Some(token), json!({})).await;
    let old = old_mail["deliveries"][0]["body"]
        .as_str()
        .unwrap()
        .strip_prefix("/auth?verify=")
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/verification/resend",
            Some(token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/verification/resend",
            Some(token),
            json!({})
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        call(&app, "POST", "/v1/auth/verify", None, json!({"token":old}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, mail) = call(&app, "GET", "/v1/account/mail", Some(token), json!({})).await;
    let new = mail["deliveries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            m["body"]
                .as_str()
                .unwrap()
                .strip_prefix("/auth?verify=")
                .unwrap()
        })
        .find(|v| *v != old)
        .unwrap();
    let payload = json!({"token":new});
    let (a, b) = tokio::join!(
        call(&app, "POST", "/v1/auth/verify", None, payload.clone()),
        call(&app, "POST", "/v1/auth/verify", None, payload)
    );
    assert!(
        (a.0 == StatusCode::OK && b.0 == StatusCode::BAD_REQUEST)
            || (a.0 == StatusCode::BAD_REQUEST && b.0 == StatusCode::OK)
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM recovery_tokens WHERE user_id=$1 AND kind='verify' AND used_at IS NULL").bind(id).fetch_one(&s.db).await.unwrap(),0);
    sqlx::query("DELETE FROM rate_limits WHERE bucket=$1")
        .bind(platform::digest(&format!("verification_resend:{id}")))
        .execute(&s.db)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_deliveries WHERE user_id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/verification/resend",
            Some(token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM email_deliveries WHERE user_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await
            .unwrap(),
        count
    );
    let (_, account) = call(&app, "GET", "/v1/account", Some(token), json!({})).await;
    assert_eq!(account["user"]["email_verified"], true);
    assert_eq!(account["mail_transport"], "sandbox");
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn recovery_code_is_single_use_and_requires_fresh_enrollment_before_privileges(db: PgPool) {
    let (s, app) = setup(db).await;
    let token = login(&app, "admin@myfans.test").await;
    let (codes, old_totp) = enroll_codes(&app, &token, "admin@myfans.test").await;
    assert_eq!(platform::verify_storage(&s).await.ok(), Some((1, 0)));
    assert!(
        platform::verify_storage(&PlatformState {
            key: Arc::new([6; 32]),
            ..s.clone()
        })
        .await
        .is_err()
    );
    let printable = codes[0].as_str().unwrap();
    let stored: Vec<String> = sqlx::query_scalar("SELECT code_digest FROM mfa_recovery_codes")
        .fetch_all(&s.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 10);
    assert!(
        stored
            .iter()
            .all(|v| v.len() == 64 && !v.contains(printable))
    );
    let payload = json!({"email":"admin@myfans.test","password":"DevelopmentOnly!2026","recovery_code":printable.to_uppercase()});
    let (a, b) = tokio::join!(
        call(&app, "POST", "/v1/auth/login", None, payload.clone()),
        call(&app, "POST", "/v1/auth/login", None, payload)
    );
    let successful = if a.0 == StatusCode::OK {
        assert_eq!(b.0, StatusCode::UNAUTHORIZED);
        a.1
    } else {
        assert_eq!(a.0, StatusCode::UNAUTHORIZED);
        assert_eq!(b.0, StatusCode::OK);
        b.1
    };
    assert_eq!(successful["user"]["mfa_enabled"], true);
    assert_eq!(successful["user"]["mfa_recovery_required"], true);
    let recovered = successful["token"].as_str().unwrap();
    assert_eq!(
        call(&app, "GET", "/v1/account/export", Some(&token), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, account) = call(&app, "GET", "/v1/account", Some(recovered), json!({})).await;
    assert_eq!(account["recovery_codes_remaining"], 9);
    assert_eq!(
        call(&app, "GET", "/v1/admin/reviews", Some(recovered), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Even a stale/recent assurance timestamp cannot bypass the account recovery flag.
    sqlx::query("UPDATE sessions SET mfa_verified_at=now() WHERE token_digest=$1")
        .bind(platform::digest(recovered))
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/v1/admin/reviews", Some(recovered), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let future = old_totp
        .generate((chrono::Utc::now().timestamp() + 30) as u64)
        .to_string();
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/mfa",
            Some(recovered),
            json!({"action":"enable","code":future})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (replacement, _) = enroll_codes(&app, recovered, "admin@myfans.test").await;
    assert_eq!(replacement.as_array().unwrap().len(), 10);
    let (_, account) = call(&app, "GET", "/v1/account", Some(recovered), json!({})).await;
    assert_eq!(account["user"]["mfa_recovery_required"], false);
    assert_eq!(account["recovery_codes_remaining"], 10);
    assert_eq!(
        call(&app, "GET", "/v1/admin/reviews", Some(recovered), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(call(&app,"POST","/v1/auth/login",None,json!({"email":"admin@myfans.test","password":"DevelopmentOnly!2026","recovery_code":codes[1]})).await.0,StatusCode::UNAUTHORIZED);
    let (status, export) = call(
        &app,
        "GET",
        "/v1/account/export",
        Some(recovered),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for code in codes
        .as_array()
        .unwrap()
        .iter()
        .chain(replacement.as_array().unwrap())
    {
        assert!(!export.to_string().contains(code.as_str().unwrap()));
    }
    assert!(!export.to_string().contains("code_digest"));
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn recovery_rotation_and_password_reset_revoke_codes_and_sessions(db: PgPool) {
    let (s, app) = setup(db).await;
    let token = login(&app, "demo@myfans.test").await;
    let (codes, totp) = enroll_codes(&app, &token, "demo@myfans.test").await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM app_users WHERE email='demo@myfans.test'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let other = platform::random_token();
    sqlx::query("INSERT INTO sessions(user_id,token_digest) VALUES($1,$2)")
        .bind(id)
        .bind(platform::digest(&other))
        .execute(&s.db)
        .await
        .unwrap();
    let fresh = totp
        .generate((chrono::Utc::now().timestamp() + 30) as u64)
        .to_string();
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/mfa",
            Some(&token),
            json!({"action":"recovery_codes","current_password":"WrongPassword!2026","code":fresh})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mfa_recovery_codes WHERE user_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await
            .unwrap(),
        10
    );
    let (status, rotation) = call(
        &app,
        "POST",
        "/v1/account/mfa",
        Some(&token),
        json!({"action":"recovery_codes","current_password":"DevelopmentOnly!2026","code":fresh}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotation}");
    let replacement = rotation["recovery_codes"].as_array().unwrap();
    assert_eq!(replacement.len(), 10);
    assert_eq!(
        call(&app, "GET", "/v1/account/export", Some(&other), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(call(&app,"POST","/v1/auth/login",None,json!({"email":"demo@myfans.test","password":"DevelopmentOnly!2026","recovery_code":codes[0]})).await.0,StatusCode::UNAUTHORIZED);
    let reset = platform::random_token();
    sqlx::query("INSERT INTO recovery_tokens(user_id,token_digest,kind) VALUES($1,$2,'reset')")
        .bind(id)
        .bind(platform::digest(&reset))
        .execute(&s.db)
        .await
        .unwrap();
    let (status, result) = call(
        &app,
        "POST",
        "/v1/auth/reset",
        None,
        json!({"token":reset,"password":"NewRecoveryPassword!2026","recovery_code":replacement[0]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("another unused")
    );
    assert_eq!(
        call(&app, "GET", "/v1/account/export", Some(&token), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(call(&app,"POST","/v1/auth/login",None,json!({"email":"demo@myfans.test","password":"NewRecoveryPassword!2026","recovery_code":replacement[0]})).await.0,StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/auth/reset",
            None,
            json!({"token":reset,"password":"ReplayPassword!2026","recovery_code":replacement[1]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(call(&app,"POST","/v1/auth/login",None,json!({"email":"demo@myfans.test","password":"DevelopmentOnly!2026","recovery_code":replacement[1]})).await.0,StatusCode::UNAUTHORIZED);
    let (status,result)=call(&app,"POST","/v1/auth/login",None,json!({"email":"demo@myfans.test","password":"NewRecoveryPassword!2026","recovery_code":replacement[1]})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["user"]["mfa_recovery_required"], true);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mfa_recovery_codes WHERE user_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await
            .unwrap(),
        8
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}
async fn enroll_codes(app: &Router, token: &str, email: &str) -> (Value, totp_rs::Totp) {
    let (status, setup) = call(
        app,
        "POST",
        "/v1/account/mfa",
        Some(token),
        json!({"action":"setup"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    let secret = totp_rs::Secret::try_from_base32(setup["secret"].as_str().unwrap()).unwrap();
    let totp = totp_rs::Builder::new()
        .with_secret(secret)
        .with_account_name(email)
        .build()
        .unwrap();
    let (status, result) = call(
        app,
        "POST",
        "/v1/account/mfa",
        Some(token),
        json!({"action":"enable","code":totp.generate_current().to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    (result["recovery_codes"].clone(), totp)
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn recovery_and_mfa_transitions_are_serialized(db: PgPool) {
    let (s, app) = setup(db).await;
    let fan = login(&app, "demo@myfans.test").await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/mfa",
            Some(&fan),
            json!({"action":"enable","code":"000000"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (_, setup) = call(
        &app,
        "POST",
        "/v1/account/mfa",
        Some(&fan),
        json!({"action":"setup"}),
    )
    .await;
    let secret = totp_rs::Secret::try_from_base32(setup["secret"].as_str().unwrap()).unwrap();
    let totp = totp_rs::Builder::new()
        .with_secret(secret)
        .with_account_name("demo@myfans.test")
        .build()
        .unwrap();
    let code = totp.generate_current().to_string();
    let (enable, replace) = tokio::join!(
        call(
            &app,
            "POST",
            "/v1/account/mfa",
            Some(&fan),
            json!({"action":"enable","code":code})
        ),
        call(
            &app,
            "POST",
            "/v1/account/mfa",
            Some(&fan),
            json!({"action":"setup"})
        )
    );
    let enabled: bool =
        sqlx::query_scalar("SELECT mfa_enabled FROM app_users WHERE email='demo@myfans.test'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    if enable.0 == StatusCode::OK {
        assert!(enabled);
        assert_eq!(replace.0, StatusCode::BAD_REQUEST);
        let encrypted: Vec<u8> =
            sqlx::query_scalar("SELECT mfa_secret FROM app_users WHERE email='demo@myfans.test'")
                .fetch_one(&s.db)
                .await
                .unwrap();
        let nonce: &[u8] = &encrypted[..12];
        use aes_gcm::aead::{Aead, KeyInit};
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(s.key.as_ref()).unwrap();
        let plain = cipher
            .decrypt(nonce.try_into().unwrap(), &encrypted[12..])
            .unwrap();
        assert_eq!(
            totp_rs::Secret::new(plain.into_boxed_slice()).to_base32(),
            setup["secret"].as_str().unwrap()
        );
        assert_ne!(
            call(
                &app,
                "POST",
                "/v1/account/mfa",
                Some(&fan),
                json!({"action":"enable","code":code})
            )
            .await
            .0,
            StatusCode::OK
        );
    } else {
        assert_eq!(enable.0, StatusCode::UNAUTHORIZED);
        assert_eq!(replace.0, StatusCode::OK);
        assert!(!enabled);
    }
    // Different outstanding links lock their common account before either token,
    // so exactly one reset commits and consumes all sibling reset links.
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM app_users WHERE email='marcostable@myfans.test'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let tokens = [platform::random_token(), platform::random_token()];
    for token in &tokens {
        sqlx::query("INSERT INTO recovery_tokens(token_digest,user_id,kind) VALUES($1,$2,'reset')")
            .bind(platform::digest(token))
            .bind(id)
            .execute(&s.db)
            .await
            .unwrap();
    }
    let (a, b) = tokio::join!(
        call(
            &app,
            "POST",
            "/v1/auth/reset",
            None,
            json!({"token":tokens[0],"password":"ResetPasswordOne!2026"})
        ),
        call(
            &app,
            "POST",
            "/v1/auth/reset",
            None,
            json!({"token":tokens[1],"password":"ResetPasswordTwo!2026"})
        )
    );
    assert!(
        (a.0 == StatusCode::OK && b.0 == StatusCode::BAD_REQUEST)
            || (b.0 == StatusCode::OK && a.0 == StatusCode::BAD_REQUEST),
        "{a:?}/{b:?}"
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM recovery_tokens WHERE user_id=$1 AND kind='reset' AND used_at IS NULL").bind(id).fetch_one(&s.db).await.unwrap(),0);
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn collection_pages_preserve_ownership_and_timestamp_ties(db: PgPool) {
    let (s, app) = setup(db).await;
    let fan = login(&app, "demo@myfans.test").await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let administrator = admin(&app).await;
    let fan_id: Uuid =
        sqlx::query_scalar("SELECT id FROM app_users WHERE email='demo@myfans.test'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM creators WHERE handle='ellawild'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let creator_id: Uuid = sqlx::query_scalar("SELECT id FROM creators WHERE handle='ellawild'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let public_post: Uuid = sqlx::query_scalar(
        "SELECT id FROM posts WHERE creator_id=$1 AND visibility='public' LIMIT 1",
    )
    .bind(creator_id)
    .fetch_one(&s.db)
    .await
    .unwrap();
    sqlx::raw_sql(r#"
      INSERT INTO app_users(email,display_name) SELECT 'paging'||n||'@fixtures.test','Paging fixture '||n FROM generate_series(1,65)n;
      INSERT INTO creators(user_id,handle,subscription_price_minor,currency,display_name)
        SELECT id,'paging'||split_part(email,'@',1),100,'USD',display_name FROM app_users WHERE email LIKE 'paging%@fixtures.test';
      INSERT INTO jobs(kind,payload,status) SELECT 'paging','{}','failed' FROM generate_series(1,65);
      INSERT INTO audit_events(action,resource_type) SELECT 'paging','test' FROM generate_series(1,65);
    "#).execute(&s.db).await.unwrap();
    for (query, id) in [
        (
            "INSERT INTO notifications(user_id,title,body,created_at) SELECT $1,'Paging','Fixture','2026-01-01'::timestamptz FROM generate_series(1,65)",
            fan_id,
        ),
        (
            "INSERT INTO email_deliveries(user_id,recipient,subject,body,created_at) SELECT $1,'fixture@test.example','Paging','Fixture','2026-01-01'::timestamptz FROM generate_series(1,65)",
            fan_id,
        ),
        (
            "INSERT INTO payouts(user_id,amount_minor,idempotency_key,created_at) SELECT $1,100,gen_random_uuid(),'2026-01-01'::timestamptz FROM generate_series(1,65)",
            owner,
        ),
        (
            "INSERT INTO media_assets(creator_id,storage_key,size_bytes,preview_key,created_at) SELECT $1,'paging/'||n,1,'paging/'||n||'.preview','2026-01-01'::timestamptz FROM generate_series(1,65)n",
            creator_id,
        ),
        (
            "INSERT INTO posts(creator_id,caption,visibility,state,moderation_blocked,created_at) SELECT $1,'Paging','public','removed',true,'2026-01-01'::timestamptz FROM generate_series(1,65)",
            creator_id,
        ),
        (
            "INSERT INTO user_blocks(user_id,blocked_user_id,created_at) SELECT $1,id,'2026-01-01'::timestamptz FROM app_users WHERE email LIKE 'paging%@fixtures.test'",
            fan_id,
        ),
        (
            "INSERT INTO subscriptions(user_id,creator_id,status,provider,provider_subscription_id,created_at) SELECT $1,id,'expired','sandbox',id::text,'2026-01-01'::timestamptz FROM creators WHERE handle LIKE 'paging%'",
            fan_id,
        ),
        (
            "INSERT INTO conversations(user_a,user_b,created_at) SELECT LEAST($1,id),GREATEST($1,id),'2026-01-01'::timestamptz FROM app_users WHERE email LIKE 'paging%@fixtures.test'",
            fan_id,
        ),
    ] {
        sqlx::query(query).bind(id).execute(&s.db).await.unwrap();
    }
    sqlx::query("INSERT INTO moderation_appeals(user_id,post_id,reason,created_at) SELECT $1,id,'Paging appeal','2026-01-01'::timestamptz FROM posts WHERE creator_id=$2 AND moderation_blocked").bind(owner).bind(creator_id).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO reports(user_id,post_id,reason,created_at) SELECT $1,$2,'Paging report','2026-01-01'::timestamptz FROM generate_series(1,65)").bind(fan_id).bind(public_post).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO comments(user_id,post_id,body,created_at) SELECT $1,$2,'Paging comment','2026-01-01'::timestamptz FROM generate_series(1,65)").bind(owner).bind(public_post).execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO checkouts(user_id,creator_id,kind,amount_minor,currency,status,idempotency_key,request_fingerprint,paid_at) SELECT $1,$2,'tip',100,'USD','paid',gen_random_uuid(),'fixture','2026-01-01'::timestamptz FROM generate_series(1,65)").bind(fan_id).bind(creator_id).execute(&s.db).await.unwrap();
    let conversation_id: Uuid =
        sqlx::query_scalar("SELECT id FROM conversations WHERE user_a=$1 OR user_b=$1 LIMIT 1")
            .bind(fan_id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    sqlx::query("INSERT INTO messages(conversation_id,sender_id,body,created_at) SELECT $1,$2,'Paging message','2026-01-01'::timestamptz FROM generate_series(1,65)").bind(conversation_id).bind(fan_id).execute(&s.db).await.unwrap();
    // A transfer has two viewer-owned postings. Their transaction_id is the same, but
    // their independently paginated row ids must remain distinct.
    sqlx::query("INSERT INTO ledger_accounts(owner_id,kind,currency) VALUES($1,'creator_pending','USD'),($1,'creator_available','USD') ON CONFLICT DO NOTHING").bind(owner).execute(&s.db).await.unwrap();
    sqlx::raw_sql("INSERT INTO ledger_transactions(external_key,currency) SELECT 'paging:'||n,'USD' FROM generate_series(1,65)n").execute(&s.db).await.unwrap();
    sqlx::query("INSERT INTO ledger_postings(transaction_id,account_id,currency,amount_minor) SELECT t.id,a.id,'USD',CASE a.kind WHEN 'creator_pending' THEN -100 ELSE 100 END FROM ledger_transactions t CROSS JOIN ledger_accounts a WHERE t.external_key LIKE 'paging:%' AND a.owner_id=$1").bind(owner).execute(&s.db).await.unwrap();
    sqlx::raw_sql(
        "UPDATE ledger_transactions SET status='posted' WHERE external_key LIKE 'paging:%'",
    )
    .execute(&s.db)
    .await
    .unwrap();
    for (path, token, collection, parameter) in [
        ("/v1/studio", &creator, "posts", "posts_cursor"),
        ("/v1/studio", &creator, "media", "media_cursor"),
        (
            "/v1/earnings",
            &creator,
            "transactions",
            "transactions_cursor",
        ),
        ("/v1/earnings", &creator, "payouts", "payouts_cursor"),
        ("/v1/notifications", &fan, "notifications", "cursor"),
        ("/v1/account/mail", &fan, "deliveries", "cursor"),
        ("/v1/account", &fan, "subscriptions", "subscriptions_cursor"),
        ("/v1/conversations", &fan, "conversations", "cursor"),
        ("/v1/account/safety", &fan, "blocks", "blocks_cursor"),
        ("/v1/account/safety", &creator, "appeals", "appeals_cursor"),
        (
            "/v1/account/safety",
            &creator,
            "blocked_posts",
            "blocked_posts_cursor",
        ),
        ("/v1/admin/appeals", &administrator, "appeals", "cursor"),
        (
            "/v1/admin/reviews",
            &administrator,
            "creators",
            "creators_cursor",
        ),
        ("/v1/admin/reviews", &administrator, "media", "media_cursor"),
        (
            "/v1/admin/reviews",
            &administrator,
            "reports",
            "reports_cursor",
        ),
        (
            "/v1/admin/reviews",
            &administrator,
            "payouts",
            "payouts_cursor",
        ),
        (
            "/v1/admin/reviews",
            &administrator,
            "checkouts",
            "checkouts_cursor",
        ),
        (
            "/v1/admin/operations",
            &administrator,
            "jobs",
            "jobs_cursor",
        ),
        (
            "/v1/admin/operations",
            &administrator,
            "audit",
            "audit_cursor",
        ),
        (
            "/v1/admin/operations",
            &administrator,
            "users",
            "users_cursor",
        ),
    ] {
        assert_pages(&app, path, token, collection, parameter).await;
    }
    assert_pages(
        &app,
        &format!("/v1/posts/{public_post}/comments"),
        &fan,
        "comments",
        "cursor",
    )
    .await;
    assert_pages(
        &app,
        &format!("/v1/conversations/{conversation_id}/messages"),
        &fan,
        "messages",
        "cursor",
    )
    .await;
    // Scope is applied before pagination; a valid foreign cursor confers no access.
    let (_, safety) = call(&app, "GET", "/v1/account/safety", Some(&creator), json!({})).await;
    assert!(
        safety["blocked_posts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|post| post["appeal_pending"] == true),
        "Pending appeal status must not depend on the separately paginated appeal list"
    );
    let (_, owned) = call(&app, "GET", "/v1/studio", Some(&creator), json!({})).await;
    let cursor = owned["next_cursors"]["media"]
        .as_str()
        .unwrap()
        .replace('+', "%2B")
        .replace('|', "%7C");
    let (status, foreign) = call(
        &app,
        "GET",
        &format!("/v1/studio?media_cursor={cursor}"),
        Some(&fan),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(foreign["media"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/conversations/{conversation_id}/messages"),
            Some(&creator),
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert!(
        call(&app, "GET", "/v1/notifications", Some(&creator), json!({}))
            .await
            .1["notifications"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/notifications?cursor=invalid",
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/studio?unknown_cursor=invalid",
            Some(&creator),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

async fn assert_pages(app: &Router, path: &str, token: &str, collection: &str, parameter: &str) {
    let mut next = None;
    let mut ids = std::collections::HashSet::new();
    let mut count = 0;
    loop {
        let uri = next.as_ref().map_or_else(
            || path.to_owned(),
            |cursor: &String| {
                format!(
                    "{path}?{parameter}={}",
                    cursor.replace('+', "%2B").replace('|', "%7C")
                )
            },
        );
        let (status, page) = call(app, "GET", &uri, Some(token), json!({})).await;
        assert_eq!(status, StatusCode::OK, "{path}: {page}");
        let rows = page[collection].as_array().unwrap();
        assert!(rows.len() <= 50, "{path}/{collection}");
        if ["messages", "comments"].contains(&collection) {
            for pair in rows.windows(2) {
                let tuple = |row: &Value| {
                    (
                        row["created_at"]
                            .as_str()
                            .unwrap()
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .unwrap(),
                        row["id"].as_str().unwrap().parse::<Uuid>().unwrap(),
                    )
                };
                assert!(
                    tuple(&pair[0]) < tuple(&pair[1]),
                    "Readable pages must preserve timestamp/UUID ordering"
                );
            }
        }
        for row in rows {
            assert!(
                ids.insert(row["id"].as_str().unwrap().to_owned()),
                "duplicate {path}/{collection}: {row}"
            );
            count += 1;
        }
        next = if parameter == "cursor" {
            page["next_cursor"].as_str()
        } else {
            page["next_cursors"][collection].as_str()
        }
        .map(str::to_owned);
        if next.is_none() {
            break;
        }
        assert_eq!(rows.len(), 50);
        assert!(count < 500);
    }
    assert!(count >= 65, "lost records {path}/{collection}: {count}");
    if collection == "transactions" {
        assert_eq!(
            count, 130,
            "Every posting of every transfer must be reachable"
        );
    }
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn runtime_role_can_purchase_but_cannot_destroy_financial_records(db: PgPool) {
    let (s, _) = setup(db).await;
    sqlx::raw_sql(include_str!("../../../db/roles/runtime.sql"))
        .execute(&s.db)
        .await
        .unwrap();
    let db = sqlx::postgres::PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE myfans_runtime")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(s.db.connect_options().as_ref().clone())
        .await
        .unwrap();
    let runtime = PlatformState { db, ..s.clone() };
    let app = platform::router(runtime.clone());
    let buyer = login(&app, "demo@myfans.test").await;
    let(status,v)=call(&app,"POST","/v1/checkout",Some(&buyer),json!({"kind":"tip","creator_id":"a1111111-1111-4111-8111-111111111111","amount_minor":100,"idempotency_key":Uuid::new_v4()})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!(
                "/v1/checkout/{}/confirm",
                v["checkout"]["id"].as_str().unwrap()
            ),
            Some(&buyer),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        sqlx::query("TRUNCATE audit_events")
            .execute(&runtime.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM ledger_postings")
            .execute(&runtime.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("CREATE TABLE unauthorized_table(id integer)")
            .execute(&runtime.db)
            .await
            .is_err()
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn privacy_export_omits_secrets_and_deactivation_revokes_sessions(db: PgPool) {
    let (s, app) = setup(db).await;
    let fan = login(&app, "demo@myfans.test").await;
    let (status, v) = call(&app, "GET", "/v1/account/export", Some(&fan), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let json = v.to_string();
    for field in [
        "password_hash",
        "mfa_secret",
        "token_digest",
        "request_fingerprint",
    ] {
        assert!(!json.contains(field));
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/deactivate",
            Some(&fan),
            json!({"current_password":"wrong password"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/account/deactivate",
            Some(&fan),
            json!({"current_password":"DevelopmentOnly!2026"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/v1/account", Some(&fan), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/auth/login",
            None,
            json!({"email":"demo@myfans.test","password":"DevelopmentOnly!2026"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn moderation_blocks_republication_and_suspension_revokes_sessions(db: PgPool) {
    let (s, app) = setup(db).await;
    let fan = login(&app, "demo@myfans.test").await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let admin = admin(&app).await;
    let (_, v) = call(
        &app,
        "POST",
        "/v1/posts",
        Some(&creator),
        json!({"caption":"Review this post","visibility":"public","state":"published"}),
    )
    .await;
    let post = v["post"]["id"].as_str().unwrap();
    let (_, v) = call(
        &app,
        "POST",
        "/v1/reports",
        Some(&fan),
        json!({"post_id":post,"reason":"This content needs review"}),
    )
    .await;
    let report = v["report"]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/reviews/{report}"),
            Some(&admin),
            json!({"kind":"report","decision":"approve","reason":"Removed after sandbox review"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/v1/posts/{post}"),
            Some(&creator),
            json!({"state":"published"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/posts/{post}/restore"),
            Some(&admin),
            json!({"reason":"Appeal accepted in sandbox"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/v1/posts/{post}"),
            Some(&creator),
            json!({"state":"published"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let id: Uuid =
        sqlx::query_scalar("SELECT id FROM app_users WHERE email='ellawild@myfans.test'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/accounts/{id}"),
            Some(&admin),
            json!({"action":"suspend","reason":"Sandbox suspension test"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/v1/studio", Some(&creator), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, feed) = call(&app, "GET", "/v1/feed", None, json!({})).await;
    assert!(
        feed["creators"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["handle"] != "ellawild")
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/accounts/{id}"),
            Some(&admin),
            json!({"action":"restore","reason":"Sandbox account restored"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let creator = login(&app, "ellawild@myfans.test").await;
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/v1/posts/{post}"),
            Some(&creator),
            json!({"state":"published"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, ops) = call(&app, "GET", "/v1/admin/operations", Some(&admin), json!({})).await;
    assert_eq!(ops["ledger_imbalance_count"], 0);
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn feed_cursor_search_and_saved_filters_cover_older_posts(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator = Uuid::parse_str("a1111111-1111-4111-8111-111111111111").unwrap();
    sqlx::query("INSERT INTO posts(creator_id,caption,visibility,state,published_at) SELECT $1,CASE WHEN n=1 THEN 'older unique needle 100%_literal' ELSE 'cursor test '||n END,'public','published',now()-interval '2 days'+n*interval '1 minute' FROM generate_series(1,60)n").bind(creator).execute(&s.db).await.unwrap();
    let (status, first) = call(&app, "GET", "/v1/feed", None, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["posts"].as_array().unwrap().len(), 50);
    let cursor = first["next_cursor"].as_str().unwrap();
    let (status, second) = call(
        &app,
        "GET",
        &format!(
            "/v1/feed?cursor={}",
            cursor.replace('+', "%2B").replace('|', "%7C")
        ),
        None,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["posts"].as_array().unwrap().len(), 15);
    assert!(second["next_cursor"].is_null());
    let mut ids = std::collections::HashSet::new();
    for page in [&first, &second] {
        for post in page["posts"].as_array().unwrap() {
            assert!(ids.insert(post["id"].as_str().unwrap()));
            assert!(
                page["creators"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["id"] == post["creator_id"])
            );
        }
    }
    let (status, search) = call(&app, "GET", "/v1/feed?q=100%25_literal", None, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{search}");
    assert_eq!(search["posts"].as_array().unwrap().len(), 1);
    let fan = login(&app, "demo@myfans.test").await;
    let post = search["posts"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/v1/posts/{post}/save"),
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, saved) = call(&app, "GET", "/v1/feed?tab=saved", Some(&fan), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["posts"].as_array().unwrap().len(), 1);
    assert_eq!(saved["posts"][0]["id"], post);
    assert_eq!(
        call(&app, "GET", "/v1/feed?cursor=invalid", None, json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, "GET", "/v1/feed?category=Invalid", None, json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, "GET", "/v1/feed?tab=Invalid", None, json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn managed_migration_startup_requires_matching_versions_and_checksums(db: PgPool) {
    let (s, app) = setup(db).await;
    let _ = admin(&app).await;
    tokio::fs::write(s.media_root.join(".mfa-key"), [7; 32])
        .await
        .unwrap();
    let run = || {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_myfans-api"));
        command
            .arg("verify-storage")
            .env(
                "DATABASE_URL",
                s.db.connect_options().to_url_lossy().as_str(),
            )
            .env("MEDIA_ROOT", &s.media_root)
            .env("APP_MODE", "sandbox")
            .env("APP_RUN_MIGRATIONS", "false");
        command
    };
    assert!(run().output().await.unwrap().status.success());
    tokio::fs::write(s.media_root.join(".mfa-key"), [8; 32])
        .await
        .unwrap();
    let wrong_key = run().output().await.unwrap();
    assert!(!wrong_key.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_key.stderr)
            .contains("Stored ciphertext verification failed")
    );
    tokio::fs::write(s.media_root.join(".mfa-key"), [7; 32])
        .await
        .unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=7")
        .execute(&s.db)
        .await
        .unwrap();
    let failed = run().output().await.unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("migration versions and checksums"));
    let invalid = run()
        .env("APP_RUN_MIGRATIONS", "invalid")
        .output()
        .await
        .unwrap();
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr)
            .contains("Storage verification requires APP_RUN_MIGRATIONS=false")
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn blocks_prevent_contact_and_discovery_without_reversing_paid_access(db: PgPool) {
    let (s, app) = setup(db).await;
    let fan = login(&app, "demo@myfans.test").await;
    let creator_token = login(&app, "ellawild@myfans.test").await;
    let (_, feed) = call(&app, "GET", "/v1/feed", Some(&fan), json!({})).await;
    let creator = &feed["creators"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["handle"] == "ellawild")
        .unwrap();
    let owner = creator["user_id"].as_str().unwrap();
    let creator_id = creator["id"].as_str().unwrap();
    let fan_id = feed["viewer"]["id"].as_str().unwrap();
    let post = feed["posts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["creator_id"] == creator_id && p["visibility"] == "public")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let (_, checkout) = call(
        &app,
        "POST",
        "/v1/checkout",
        Some(&fan),
        json!({"kind":"subscription","creator_id":creator_id,"idempotency_key":Uuid::new_v4()}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!(
                "/v1/checkout/{}/confirm",
                checkout["checkout"]["id"].as_str().unwrap()
            ),
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, conv) = call(
        &app,
        "POST",
        "/v1/conversations",
        Some(&fan),
        json!({"recipient_id":owner}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{conv}");
    let conv = conv["conversation"]["id"].as_str().unwrap();
    let message_path = format!("/v1/conversations/{conv}/messages");
    assert_eq!(
        call(
            &app,
            "POST",
            &message_path,
            Some(&fan),
            json!({"body":"Before blocking"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/v1/users/{owner}/block"),
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    for token in [&fan, &creator_token] {
        assert_eq!(
            call(
                &app,
                "POST",
                &message_path,
                Some(token),
                json!({"body":"Contact after blocking"})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&app, "GET", &message_path, Some(token), json!({}))
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/conversations",
            Some(&fan),
            json!({"recipient_id":owner})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/v1/posts/{post}/like"),
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/posts/{post}/comments"),
            Some(&fan),
            json!({"body":"Blocked comment"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, hidden) = call(&app, "GET", "/v1/feed", Some(&fan), json!({})).await;
    assert!(
        !hidden["creators"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == creator_id)
    );
    assert!(
        !hidden["posts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["creator_id"] == creator_id)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM entitlements WHERE user_id=$1 AND revoked_at IS NULL"
        )
        .bind(Uuid::parse_str(fan_id).unwrap())
        .fetch_one(&s.db)
        .await
        .unwrap(),
        1
    );
    let (_, history) = call(&app, "GET", "/v1/conversations", Some(&fan), json!({})).await;
    assert_eq!(history["conversations"][0]["blocked"], true);
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/v1/users/{fan_id}/block"),
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/v1/users/{owner}/block"),
            Some(&fan),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &message_path,
            Some(&fan),
            json!({"body":"Contact restored"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}
#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn appeal_review_is_owned_audited_and_returns_posts_only_to_drafts(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let fan = login(&app, "demo@myfans.test").await;
    let moderator = admin(&app).await;
    let post: Uuid = sqlx::query_scalar(
        "SELECT id FROM posts WHERE creator_id='a1111111-1111-4111-8111-111111111111' LIMIT 1",
    )
    .fetch_one(&s.db)
    .await
    .unwrap();
    sqlx::query("UPDATE posts SET state='removed',moderation_blocked=true WHERE id=$1")
        .bind(post)
        .execute(&s.db)
        .await
        .unwrap();
    let payload =
        json!({"post_id":post,"reason":"Please review this removal against the original context."});
    assert_eq!(
        call(&app, "POST", "/v1/appeals", Some(&fan), payload.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, a) = call(&app, "POST", "/v1/appeals", Some(&creator), payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{a}");
    let id = a["appeal"]["id"].as_str().unwrap();
    assert_eq!(
        call(&app, "POST", "/v1/appeals", Some(&creator), payload)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let path = format!("/v1/admin/appeals/{id}");
    let decision = json!({"decision":"accept","reason":"Reviewed evidence supports restoring this post to draft."});
    assert_eq!(
        call(&app, "POST", &path, Some(&fan), decision.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &path, Some(&moderator), decision.clone())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", &path, Some(&moderator), decision)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let state: (String, bool) =
        sqlx::query_as("SELECT state,moderation_blocked FROM posts WHERE id=$1")
            .bind(post)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(state, ("draft".into(), false));
    let (_, safety) = call(&app, "GET", "/v1/account/safety", Some(&creator), json!({})).await;
    assert_eq!(safety["appeals"][0]["status"], "accepted");
    assert!(
        safety["appeals"][0]["decision_reason"]
            .as_str()
            .unwrap()
            .contains("Reviewed evidence")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events WHERE resource_id=$1 AND action='appeal.accepted'"
        )
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&s.db)
        .await
        .unwrap(),
        1
    );
    assert!(
        sqlx::query("UPDATE moderation_appeals SET reason='Changed after decision' WHERE id=$1")
            .bind(Uuid::parse_str(id).unwrap())
            .execute(&s.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM moderation_appeals WHERE id=$1")
            .bind(Uuid::parse_str(id).unwrap())
            .execute(&s.db)
            .await
            .is_err()
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

async fn setup(db: PgPool) -> (PlatformState, Router) {
    let root = std::env::temp_dir().join(format!("myfans-test-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let state = PlatformState {
        runtime: Default::default(),
        db,
        storage: platform::MediaStore::local(root.clone()),
        scanner: Default::default(),
        providers: Default::default(),
        media_root: root,
        key: Arc::new([7; 32]),
        dummy_hash: Arc::new("invalid".into()),
        mail: Default::default(),
    };
    worker::seed(&state).await.unwrap();
    (state.clone(), platform::router(state))
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn login(app: &Router, email: &str) -> String {
    let (status, value) = call(
        app,
        "POST",
        "/v1/auth/login",
        None,
        json!({"email":email,"password":"DevelopmentOnly!2026"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value["token"].as_str().unwrap().into()
}
async fn admin(app: &Router) -> String {
    let token = login(app, "admin@myfans.test").await;
    enroll_mfa(app, &token).await;
    token
}
async fn enroll_mfa(app: &Router, token: &str) {
    let (status, v) = call(
        app,
        "POST",
        "/v1/account/mfa",
        Some(token),
        json!({"action":"setup"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let secret = totp_rs::Secret::try_from_base32(v["secret"].as_str().unwrap()).unwrap();
    let totp = totp_rs::Builder::new()
        .with_secret(secret)
        .with_account_name("admin@myfans.test")
        .build()
        .unwrap();
    let code = totp.generate_current().to_string();
    let (status, v) = call(
        app,
        "POST",
        "/v1/account/mfa",
        Some(token),
        json!({"action":"enable","code":code}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn checkout_is_durable_idempotent_and_refund_revokes_access(db: PgPool) {
    let (s, app) = setup(db).await;
    let buyer = login(&app, "demo@myfans.test").await;
    let creator = "a1111111-1111-4111-8111-111111111111";
    let key = Uuid::new_v4();
    let payload = json!({"kind":"subscription","creator_id":creator,"idempotency_key":key});
    let (a, b) = tokio::join!(
        call(&app, "POST", "/v1/checkout", Some(&buyer), payload.clone()),
        call(&app, "POST", "/v1/checkout", Some(&buyer), payload.clone())
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(a.1["checkout"]["id"], b.1["checkout"]["id"]);
    let id = a.1["checkout"]["id"].as_str().unwrap();
    let path = format!("/v1/checkout/{id}/confirm");
    let (a, b) = tokio::join!(
        call(&app, "POST", &path, Some(&buyer), json!({})),
        call(&app, "POST", &path, Some(&buyer), json!({}))
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    let restarted = platform::router(s.clone());
    let (_, feed) = call(&restarted, "GET", "/v1/feed", Some(&buyer), json!({})).await;
    assert!(
        !feed["posts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["visibility"] == "subscribers" && p["creator_id"] == creator)
            .unwrap()["locked"]
            .as_bool()
            .unwrap()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ledger_transactions WHERE external_key=$1")
            .bind(format!("checkout:{id}"))
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let (_, guest) = call(&app, "GET", "/v1/feed", None, json!({})).await;
    assert!(
        guest["posts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["locked"] == true)
            .all(|p| p["media_url"].is_null())
    );
    let admin = admin(&app).await;
    let (status, v) = call(
        &app,
        "POST",
        &format!("/v1/admin/refunds/{id}"),
        Some(&admin),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (_, feed) = call(&app, "GET", "/v1/feed", Some(&buyer), json!({})).await;
    assert!(
        feed["posts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["visibility"] == "subscribers" && p["creator_id"] == creator)
            .unwrap()["locked"]
            .as_bool()
            .unwrap()
    );
    let sum: i64 =
        sqlx::query_scalar("SELECT COALESCE(sum(amount_minor),0)::bigint FROM ledger_postings")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(sum, 0);
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn authorization_and_session_revocation(db: PgPool) {
    let (s, app) = setup(db).await;
    let fan = login(&app, "demo@myfans.test").await;
    let creator = login(&app, "ellawild@myfans.test").await;
    assert_eq!(
        call(&app, "GET", "/v1/admin/reviews", Some(&fan), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let admin_password = login(&app, "admin@myfans.test").await;
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/admin/reviews",
            Some(&admin_password),
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let elevated = admin(&app).await;
    assert_eq!(
        call(&app, "GET", "/v1/admin/reviews", Some(&elevated), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/admin/reviews",
            Some(&admin_password),
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, feed) = call(&app, "GET", "/v1/feed", None, json!({})).await;
    let post = feed["posts"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/v1/posts/{post}"),
            Some(&fan),
            json!({"state":"removed"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let recipient = feed["creators"][0]["user_id"].as_str().unwrap();
    let (status, v) = call(
        &app,
        "POST",
        "/v1/conversations",
        Some(&fan),
        json!({"recipient_id":recipient}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let path = format!(
        "/v1/conversations/{}/messages",
        v["conversation"]["id"].as_str().unwrap()
    );
    let third = login(&app, "marcostable@myfans.test").await;
    assert_eq!(
        call(&app, "GET", &path, Some(&third), json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            Some(&fan),
            json!({"body":"Hello creator"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, v) = call(
        &app,
        "POST",
        "/v1/account/password",
        Some(&creator),
        json!({"current_password":"DevelopmentOnly!2026","password":"ChangedPassword!2026"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        call(&app, "GET", "/v1/account", Some(&creator), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn private_media_requires_approval_and_entitlement(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let admin = admin(&app).await;
    let image = image::RgbImage::from_pixel(32, 32, image::Rgb([120, 140, 160]));
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut bytes)
        .encode_image(&image)
        .unwrap();
    let boundary = "testboundary";
    let mut upload=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").into_bytes();
    upload.extend(bytes);
    upload.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/media/upload")
                .header("authorization", format!("Bearer {creator}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(upload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let v: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    let id = v["media"]["id"].as_str().unwrap();
    worker::tick(&s).await.unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/media/{id}/full"),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call(&app,"POST","/v1/posts",Some(&creator),json!({"caption":"Private image","visibility":"subscribers","state":"published","media_id":id})).await.0,StatusCode::BAD_REQUEST);
    let (status, v) = call(
        &app,
        "POST",
        &format!("/v1/admin/reviews/{id}"),
        Some(&admin),
        json!({"kind":"media","decision":"approve","reason":"Approved sandbox image"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let(status,v)=call(&app,"POST","/v1/posts",Some(&creator),json!({"caption":"Private image","visibility":"subscribers","state":"published","media_id":id})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/media/{id}/preview"),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/media/{id}/full"),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let buyer = login(&app, "demo@myfans.test").await;
    let(_,v)=call(&app,"POST","/v1/checkout",Some(&buyer),json!({"kind":"subscription","creator_id":"a1111111-1111-4111-8111-111111111111","idempotency_key":Uuid::new_v4()})).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!(
                "/v1/checkout/{}/confirm",
                v["checkout"]["id"].as_str().unwrap()
            ),
            Some(&buyer),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/media/{id}/full"),
            Some(&buyer),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn payout_reservations_prevent_overspending_and_refunds_are_balanced(db: PgPool) {
    let (s, app) = setup(db).await;
    let buyer = login(&app, "demo@myfans.test").await;
    let creator = login(&app, "ellawild@myfans.test").await;
    enroll_mfa(&app, &creator).await;
    let admin = admin(&app).await;
    let(_,v)=call(&app,"POST","/v1/checkout",Some(&buyer),json!({"kind":"tip","creator_id":"a1111111-1111-4111-8111-111111111111","amount_minor":1200,"idempotency_key":Uuid::new_v4()})).await;
    let checkout = v["checkout"]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/checkout/{checkout}/confirm"),
            Some(&buyer),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE jobs SET run_at=now() WHERE kind='settle'")
        .execute(&s.db)
        .await
        .unwrap();
    worker::tick(&s).await.unwrap();
    let (_, v) = call(&app, "GET", "/v1/earnings", Some(&creator), json!({})).await;
    assert_eq!(v["available_minor"], 960);
    assert_eq!(v["pending_minor"], 0);
    let (a, b) = tokio::join!(
        call(
            &app,
            "POST",
            "/v1/payouts",
            Some(&creator),
            json!({"amount_minor":600,"idempotency_key":Uuid::new_v4()})
        ),
        call(
            &app,
            "POST",
            "/v1/payouts",
            Some(&creator),
            json!({"amount_minor":600,"idempotency_key":Uuid::new_v4()})
        )
    );
    assert_eq!(
        [a.0, b.0].iter().filter(|s| **s == StatusCode::OK).count(),
        1
    );
    let payout = if a.0 == StatusCode::OK { a.1 } else { b.1 };
    let id = payout["payout"]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/reviews/{id}"),
            Some(&admin),
            json!({"kind":"payout","decision":"approve","reason":"Sandbox payout approved"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, v) = call(&app, "GET", "/v1/earnings", Some(&creator), json!({})).await;
    assert_eq!(v["available_minor"], 360);
    assert_eq!(v["reserved_minor"], 0);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/refunds/{checkout}"),
            Some(&admin),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/refunds/{checkout}"),
            Some(&admin),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, v) = call(&app, "GET", "/v1/earnings", Some(&creator), json!({})).await;
    assert_eq!(v["available_minor"], -600);
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/payouts",
            Some(&creator),
            json!({"amount_minor":100,"idempotency_key":Uuid::new_v4()})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let unbalanced:i64=sqlx::query_scalar("SELECT count(*) FROM (SELECT transaction_id FROM ledger_postings GROUP BY transaction_id HAVING sum(amount_minor)<>0)x").fetch_one(&s.db).await.unwrap();
    assert_eq!(unbalanced, 0);
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn purchase_prices_are_server_owned_and_scheduled_posts_are_reviewed(db: PgPool) {
    let (s, app) = setup(db).await;
    let buyer = login(&app, "demo@myfans.test").await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let creator_id = "a1111111-1111-4111-8111-111111111111";
    let(status,v)=call(&app,"POST","/v1/posts",Some(&creator),json!({"caption":"Exclusive lesson","visibility":"purchase","price_minor":700,"state":"published"})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let post = v["post"]["id"].as_str().unwrap();
    let key = Uuid::new_v4();
    let payload = json!({"kind":"purchase","creator_id":creator_id,"post_id":post,"amount_minor":100,"idempotency_key":key});
    let (status, v) = call(&app, "POST", "/v1/checkout", Some(&buyer), payload.clone()).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["checkout"]["amount_minor"], 700);
    let checkout = v["checkout"]["id"].as_str().unwrap();
    let mut changed = payload;
    changed["amount_minor"] = json!(200);
    assert_eq!(
        call(&app, "POST", "/v1/checkout", Some(&buyer), changed)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/checkout/{checkout}/confirm"),
            Some(&buyer),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/posts/{post}/comments"),
            Some(&buyer),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/posts/{post}/comments"),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let(status,v)=call(&app,"POST","/v1/posts",Some(&creator),json!({"caption":"Scheduled post","visibility":"public","state":"scheduled","scheduled_at":(chrono::Utc::now()+chrono::Duration::days(1)).to_rfc3339()})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let scheduled = Uuid::parse_str(v["post"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE posts SET scheduled_at=now() WHERE id=$1")
        .bind(scheduled)
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET run_at=now() WHERE kind='publish'")
        .execute(&s.db)
        .await
        .unwrap();
    worker::tick(&s).await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM posts WHERE id=$1")
        .bind(scheduled)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(state, "published");
    worker::tick(&s).await.unwrap();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action='post.scheduled.published' AND resource_id=$1").bind(scheduled).fetch_one(&s.db).await.unwrap();
    assert_eq!(count, 1);
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL and ffmpeg: pnpm test:integration"]
async fn videos_are_transcoded_captioned_and_range_protected(db: PgPool) {
    let (s, app) = setup(db).await;
    let creator = login(&app, "ellawild@myfans.test").await;
    let admin = admin(&app).await;
    let input = s.media_root.join("fixture.mp4");
    let result = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x240:d=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&input)
        .output()
        .await
        .unwrap();
    assert!(result.status.success());
    let bytes = tokio::fs::read(input).await.unwrap();
    let mut upload=b"--video\r\nContent-Disposition: form-data; name=\"file\"; filename=\"video.mp4\"\r\nContent-Type: video/mp4\r\n\r\n".to_vec();
    upload.extend(bytes);
    upload.extend(b"\r\n--video--\r\n");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/media/upload")
                .header("authorization", format!("Bearer {creator}"))
                .header("content-type", "multipart/form-data; boundary=video")
                .body(Body::from(upload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let v: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
    let id = v["media"]["id"].as_str().unwrap();
    worker::tick(&s).await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM media_assets WHERE id=$1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(state, "quarantine");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/admin/reviews/{id}"),
            Some(&admin),
            json!({"kind":"media","decision":"approve","reason":"Reviewed sandbox video"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let post = json!({"caption":"Blue video","visibility":"subscribers","state":"published","media_id":id});
    assert_eq!(
        call(&app, "POST", "/v1/posts", Some(&creator), post.clone())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let captions = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nA blue screen.\n";
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/v1/media/{id}/captions"),
            Some(&creator),
            json!({"vtt":captions})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", "/v1/posts", Some(&creator), post)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/v1/media/{id}/captions"),
            None,
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/media/{id}/full"))
                .header("authorization", format!("Bearer {creator}"))
                .header("range", "bytes=0-15")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-type"], "video/mp4");
    assert!(
        response.headers()["content-range"]
            .to_str()
            .unwrap()
            .starts_with("bytes 0-15/")
    );
    assert_eq!(to_bytes(response.into_body(), 100).await.unwrap().len(), 16);
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn production_registration_records_exact_policy_consent(db: PgPool) {
    let (mut state, _) = setup(db).await;
    Arc::make_mut(&mut state.runtime).production = true;
    let app = platform::router(state.clone());
    let base = json!({"email":"consent@test.example","password":"StrongPassword!2026","display_name":"Consent"});
    assert_eq!(
        call(&app, "POST", "/v1/auth/register", None, base.clone())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut accepted = base;
    accepted["adult_confirmed"] = json!(true);
    accepted["country_code"] = json!("US");
    accepted["terms_version"] = json!("sandbox-1");
    accepted["privacy_version"] = json!("sandbox-1");
    accepted["content_policy_version"] = json!("outdated");
    assert_eq!(
        call(&app, "POST", "/v1/auth/register", None, accepted.clone())
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    accepted["content_policy_version"] = json!("sandbox-1");
    let (status, body) = call(&app, "POST", "/v1/auth/register", None, accepted).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "production");
    let user = Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM policy_acceptances WHERE user_id=$1 AND country_code='US' AND minimum_age=18").bind(user).fetch_one(&state.db).await.unwrap(),1);
    assert!(
        sqlx::query("UPDATE policy_acceptances SET terms_version='changed' WHERE user_id=$1")
            .bind(user)
            .execute(&state.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM policy_acceptances WHERE user_id=$1")
            .bind(user)
            .execute(&state.db)
            .await
            .is_err()
    );
    let _ = tokio::fs::remove_dir_all(state.media_root).await;
}

#[sqlx::test(migrations = "../../db/migrations")]
#[ignore = "requires disposable PostgreSQL: pnpm test:integration"]
async fn discovery_cursors_find_unfeatured_creators_and_enforce_blocks(db: PgPool) {
    let (s, app) = setup(db).await;
    let mut ids = vec![];
    for n in 0..53 {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO app_users(id,email,display_name) VALUES($1,$2,$3)")
            .bind(id)
            .bind(format!("directory{n}@test.example"))
            .bind(format!("Directory{n}"))
            .execute(&s.db)
            .await
            .unwrap();
        let creator:Uuid=sqlx::query_scalar("INSERT INTO creators(user_id,handle,display_name,category,verification_status,subscription_price_minor,currency) VALUES($1,$2,$3,'Art','approved',1000,'USD') RETURNING id").bind(id).bind(format!("directory{n}")).bind(format!("Directory{n}")).fetch_one(&s.db).await.unwrap();
        ids.push((id, creator));
    }
    let token = login(&app, "demo@myfans.test").await;
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/v1/users/{}/block", ids[0].0),
            Some(&token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, first) = call(
        &app,
        "GET",
        "/v1/creators?q=Directory&category=Art",
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["creators"].as_array().unwrap().len(), 50);
    let cursor = first["next_cursor"].as_str().unwrap();
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", "Directory")
        .append_pair("category", "Art")
        .append_pair("cursor", cursor)
        .finish();
    let (status, last) = call(
        &app,
        "GET",
        &format!("/v1/creators?{query}"),
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{last}");
    assert_eq!(last["creators"].as_array().unwrap().len(), 2);
    let all: std::collections::HashSet<_> = first["creators"]
        .as_array()
        .unwrap()
        .iter()
        .chain(last["creators"].as_array().unwrap())
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(all.len(), 52);
    assert!(!all.contains(ids[0].1.to_string().as_str()));
    assert_eq!(
        call(&app, "GET", "/v1/creators?q=%25", None, json!({}))
            .await
            .1["creators"],
        json!([])
    );
    let _ = tokio::fs::remove_dir_all(s.media_root).await;
}
