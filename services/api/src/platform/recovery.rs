use super::*;
use sqlx::{Postgres, Transaction};

pub fn normalize(code: &str) -> Result<String> {
    let code = code.trim();
    if code.len() > 39 {
        return Err(invalid());
    }
    let normalized = code.replace('-', "").to_ascii_lowercase();
    if normalized.len() != 32 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    Ok(normalized)
}
fn invalid() -> Error {
    Error(
        StatusCode::UNAUTHORIZED,
        "Enter a valid, unused recovery code.",
    )
}
pub async fn rotate(tx: &mut Transaction<'_, Postgres>, user: Uuid) -> Result<Vec<String>> {
    sqlx::query("DELETE FROM mfa_recovery_codes WHERE user_id=$1")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    let mut codes = Vec::with_capacity(10);
    for _ in 0..10 {
        // Two independent OS-random UUIDs provide 244 random bits; truncate their digest
        // to a 128-bit recovery secret. Hashes, never printable codes, enter PostgreSQL.
        let raw = digest(&random_token())[..32].to_owned();
        sqlx::query("INSERT INTO mfa_recovery_codes(code_digest,user_id) VALUES($1,$2)")
            .bind(digest(&raw))
            .bind(user)
            .execute(&mut **tx)
            .await?;
        codes.push(
            raw.as_bytes()
                .chunks(4)
                .map(|part| String::from_utf8_lossy(part))
                .collect::<Vec<_>>()
                .join("-"),
        );
    }
    Ok(codes)
}
// The caller must first lock the active account. All code rotation/consumption follows
// this lock order. A recovered account retains MFA but cannot gain privileged assurance
// until a new authenticator has been enrolled and verified.
pub async fn consume(
    s: &PlatformState,
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    code: &str,
) -> Result<()> {
    let changed = sqlx::query("DELETE FROM mfa_recovery_codes WHERE user_id=$1 AND code_digest=$2")
        .bind(user)
        .bind(digest(&normalize(code)?))
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(invalid());
    }
    sqlx::query("UPDATE app_users SET mfa_recovery_required=true,mfa_replacement_pending=false WHERE id=$1 AND mfa_enabled")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    audit(tx, user, "mfa.recovery.used", "user", user).await?;
    notify(tx,user,"Authenticator recovery started","A saved recovery code was used. Existing sessions were revoked. Replace your authenticator in Settings before administrative actions or payouts.").await?;
    let address: String = sqlx::query_scalar("SELECT email FROM app_users WHERE id=$1")
        .bind(user)
        .fetch_one(&mut **tx)
        .await?;
    super::mail::enqueue(s,tx,user,&address,"Authenticator recovery started","A saved recovery code was used and existing sessions were revoked. If this was not you, contact the operator. Replace your authenticator in Settings.",None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_are_normalized_without_accepting_malformed_secrets() {
        let raw = "a1b2c3d4e5f60708a1b2c3d4e5f60708";
        assert_eq!(
            normalize(" A1B2-C3D4-E5F6-0708-A1B2-C3D4-E5F6-0708 ")
                .ok()
                .as_deref(),
            Some(raw)
        );
        for bad in [
            "",
            "123456",
            "a1b2 c3d4e5f60708a1b2c3d4e5f60708",
            "g1b2c3d4e5f60708a1b2c3d4e5f60708",
            "a1b2c3d4e5f60708a1b2c3d4e5f607080",
        ] {
            assert!(normalize(bad).is_err());
        }
    }
}
