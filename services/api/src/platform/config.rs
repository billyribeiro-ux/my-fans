use sqlx::{ConnectOptions, postgres::PgConnectOptions};
use std::{path::Path, str::FromStr};

fn parse_database_url(value: &str) -> Result<PgConnectOptions, &'static str> {
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(
            "Database connection secret must contain one nonempty URL (maximum 4096 bytes)",
        );
    }
    let parsed = url::Url::parse(value).map_err(|_| "Invalid database connection URL")?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql")
        || parsed.host_str().is_none()
        || parsed.username().is_empty()
        || parsed.path().len() <= 1
    {
        return Err(
            "Database connection URL requires a PostgreSQL scheme, host, user, and database",
        );
    }
    // Never propagate parser errors that might include the connection secret.
    PgConnectOptions::from_str(value).map_err(|_| "Invalid database connection options")
}

async fn read_database_secret(path: &Path) -> Result<String, &'static str> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "Cannot open database connection secret file")?;
    let metadata = file
        .metadata()
        .await
        .map_err(|_| "Cannot inspect database connection secret file")?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err("Database connection secret must be a regular file of at most 4096 bytes");
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read database connection secret file")?;
    String::from_utf8(bytes).map_err(|_| "Database connection secret must be UTF-8")
}

pub(super) async fn database_options() -> Result<PgConnectOptions, &'static str> {
    let direct = std::env::var_os("DATABASE_URL");
    let path = std::env::var_os("DATABASE_URL_FILE");
    let value = match (direct, path) {
        (Some(_), Some(_)) => return Err("Set only one of DATABASE_URL and DATABASE_URL_FILE"),
        (Some(value), None) => value
            .into_string()
            .map_err(|_| "DATABASE_URL must be UTF-8")?,
        (None, Some(path)) => read_database_secret(Path::new(&path)).await?,
        (None, None) => "postgresql://myfans:local-development-only@127.0.0.1:54328/myfans".into(),
    };
    if std::env::var("APP_MODE").as_deref() == Ok("production") {
        let url =
            url::Url::parse(value.trim_end()).map_err(|_| "Invalid database connection URL")?;
        if !url
            .query_pairs()
            .any(|(k, v)| k == "sslmode" && v == "verify-full")
            || !url
                .query_pairs()
                .any(|(k, v)| k == "sslrootcert" && !v.is_empty())
        {
            return Err(
                "Production database connections require sslmode=verify-full and sslrootcert",
            );
        }
    }
    parse_database_url(&value).map(|options| options.disable_statement_logging())
}
pub(super) async fn encryption_key(path: &Path) -> Result<[u8; 32], &'static str> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "Cannot open encryption key file")?;
    let info = file
        .metadata()
        .await
        .map_err(|_| "Cannot inspect encryption key file")?;
    if !info.is_file() || info.len() != 32 {
        return Err("Encryption key file must contain exactly 32 raw bytes");
    }
    let mut bytes = Vec::new();
    file.take(33)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read encryption key file")?;
    bytes
        .try_into()
        .map_err(|_| "Encryption key file must contain exactly 32 raw bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn external_encryption_key_requires_exact_binary_material()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("myfans-key-file-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await?;
        let file = root.join("key");
        for size in [0, 31, 33, 4096] {
            tokio::fs::write(&file, vec![7; size]).await?;
            assert!(encryption_key(&file).await.is_err());
        }
        tokio::fs::write(&file, [7; 32]).await?;
        assert_eq!(encryption_key(&file).await?, [7; 32]);
        assert!(encryption_key(&root).await.is_err());
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }

    #[test]
    fn database_url_validation_never_reports_credentials() {
        assert!(parse_database_url("postgresql://runtime:secret@postgres:5432/myfans\n").is_ok());
        for value in [
            "",
            "postgresql://runtime:secret@postgres",
            "https://runtime:secret@postgres/myfans",
            "postgresql://runtime:secret@postgres/myfans\nother",
            "not-a-url:secret",
        ] {
            let Err(error) = parse_database_url(value) else {
                panic!("Invalid connection input accepted");
            };
            assert!(!error.contains("runtime:secret"));
        }
    }

    #[tokio::test]
    async fn database_secret_file_is_bounded_and_required() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = std::env::temp_dir().join(format!("myfans-dsn-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await?;
        let file = root.join("database-url");
        assert!(read_database_secret(&file).await.is_err());
        assert!(read_database_secret(&root).await.is_err());
        tokio::fs::write(&file, b"postgresql://runtime:secret@postgres/myfans\n").await?;
        assert!(parse_database_url(&read_database_secret(&file).await?).is_ok());
        tokio::fs::write(&file, vec![b'x'; 4097]).await?;
        assert!(read_database_secret(&file).await.is_err());
        tokio::fs::write(&file, [255]).await?;
        assert!(read_database_secret(&file).await.is_err());
        tokio::fs::remove_dir_all(root).await?;
        Ok(())
    }
}
