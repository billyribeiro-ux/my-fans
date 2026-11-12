use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Default)]
pub struct MediaScanner {
    address: Option<std::net::SocketAddr>,
}
impl MediaScanner {
    pub fn from_env() -> std::result::Result<Self, &'static str> {
        match std::env::var("MEDIA_SCANNER").as_deref() {
            Err(_) | Ok("disabled") => {
                if std::env::var_os("CLAMAV_ADDRESS").is_some() {
                    return Err("CLAMAV_ADDRESS requires MEDIA_SCANNER=clamav");
                }
                Ok(Self::default())
            }
            Ok("clamav") => {
                let address: std::net::SocketAddr = std::env::var("CLAMAV_ADDRESS")
                    .map_err(|_| "CLAMAV_ADDRESS is required")?
                    .parse()
                    .map_err(|_| "Invalid ClamAV address")?;
                if !address.ip().is_loopback() || address.port() == 0 {
                    return Err(
                        "ClamAV must use a loopback sidecar; the protocol is not encrypted",
                    );
                }
                Ok(Self {
                    address: Some(address),
                })
            }
            _ => Err("MEDIA_SCANNER must be disabled or clamav"),
        }
    }
    pub fn enabled(&self) -> bool {
        self.address.is_some()
    }
    pub async fn probe(&self) -> Result<()> {
        let Some(address) = self.address else {
            return Ok(());
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut socket = tokio::net::TcpStream::connect(address)
                .await
                .map_err(|_| unavailable())?;
            socket
                .write_all(b"zVERSION\0")
                .await
                .map_err(|_| unavailable())?;
            let mut bytes = Vec::new();
            loop {
                let byte = socket.read_u8().await.map_err(|_| unavailable())?;
                if byte == 0 {
                    break;
                }
                if bytes.len() >= 4096 {
                    return Err(unavailable());
                }
                bytes.push(byte);
            }
            let value = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
            if !fresh_database(value, chrono::Utc::now()) {
                return Err(unavailable());
            }
            Ok(())
        })
        .await
        .map_err(|_| unavailable())?
    }
    pub async fn scan(&self, bytes: &[u8]) -> Result<()> {
        let Some(address) = self.address else {
            return Ok(());
        };
        if bytes.is_empty() || bytes.len() > storage::MAX_UPLOAD_BYTES {
            return Err(bad("Invalid scan input."));
        }
        self.probe().await?;
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut socket = tokio::net::TcpStream::connect(address)
                .await
                .map_err(|_| unavailable())?;
            socket
                .write_all(b"zINSTREAM\0")
                .await
                .map_err(|_| unavailable())?;
            for chunk in bytes.chunks(64 * 1024) {
                socket
                    .write_all(&(chunk.len() as u32).to_be_bytes())
                    .await
                    .map_err(|_| unavailable())?;
                socket.write_all(chunk).await.map_err(|_| unavailable())?;
            }
            socket.write_all(&[0; 4]).await.map_err(|_| unavailable())?;
            // Clamd ends a z-command response with NUL. Never include its signature
            // names, paths, or raw responses in a customer response or logs.
            let mut response = Vec::new();
            loop {
                let byte = socket.read_u8().await.map_err(|_| unavailable())?;
                if byte == 0 {
                    break;
                }
                if response.len() >= 4096 {
                    return Err(unavailable());
                }
                response.push(byte);
            }
            if response == b"stream: OK" {
                Ok(())
            } else if response.starts_with(b"stream: ") && response.ends_with(b" FOUND") {
                Err(bad("Upload rejected by malware screening."))
            } else {
                Err(unavailable())
            }
        })
        .await
        .map_err(|_| unavailable())?
    }
}
fn fresh_database(version: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    let fields: Vec<_> = version.split('/').collect();
    if fields.len() != 3 || !fields[0].starts_with("ClamAV ") || fields[1].parse::<u64>().is_err() {
        return false;
    }
    let Ok(date) = chrono::NaiveDateTime::parse_from_str(fields[2], "%a %b %e %H:%M:%S %Y") else {
        return false;
    };
    let age = now - date.and_utc();
    age >= chrono::Duration::minutes(-5) && age <= chrono::Duration::hours(48)
}
fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Media scanning is unavailable; the upload remains held.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn scanner_preserves_binary_input_and_fails_closed()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for (reply, expected) in [
            ("stream: OK\0", None),
            (
                "stream: Eicar-Signature FOUND\0",
                Some(StatusCode::BAD_REQUEST),
            ),
            (
                "stream: size limit exceeded ERROR\0",
                Some(StatusCode::SERVICE_UNAVAILABLE),
            ),
            ("unexpected\0", Some(StatusCode::SERVICE_UNAVAILABLE)),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let scanner = MediaScanner {
                address: Some(listener.local_addr()?),
            };
            let data = vec![42_u8; 100_000];
            let original = data.clone();
            let server = tokio::spawn(async move {
                let (mut version, _) = listener.accept().await?;
                let mut command = [0; 9];
                version.read_exact(&mut command).await?;
                assert_eq!(&command, b"zVERSION\0");
                version
                    .write_all(
                        format!(
                            "ClamAV 1.5.1/28000/{}\0",
                            chrono::Utc::now().format("%a %b %e %H:%M:%S %Y")
                        )
                        .as_bytes(),
                    )
                    .await?;
                let (mut stream, _) = listener.accept().await?;
                let mut command = [0; 10];
                stream.read_exact(&mut command).await?;
                assert_eq!(&command, b"zINSTREAM\0");
                let mut received = Vec::new();
                loop {
                    let length = stream.read_u32().await?;
                    if length == 0 {
                        break;
                    }
                    assert!(length <= 64 * 1024);
                    let mut chunk = vec![0; length as usize];
                    stream.read_exact(&mut chunk).await?;
                    received.extend(chunk);
                }
                assert_eq!(original, received);
                stream.write_all(reply.as_bytes()).await?;
                Ok::<_, std::io::Error>(())
            });
            assert_eq!(scanner.scan(&data).await.err().map(|e| e.0), expected);
            server.await??;
        }
        Ok(())
    }
    #[test]
    fn stale_unknown_and_future_databases_are_rejected() {
        let now = chrono::Utc::now();
        for delta in [-180000, 600] {
            let date = now + chrono::Duration::seconds(delta);
            assert!(!fresh_database(
                &format!("ClamAV 1.5.1/28000/{}", date.format("%a %b %e %H:%M:%S %Y")),
                now
            ));
        }
        assert!(!fresh_database("unknown", now));
    }
}
