//! Explicit local/provider acceptance check. The EICAR test string is harmless.
use myfans_api::platform::MediaScanner;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scanner = MediaScanner::from_env()?;
    if !scanner.enabled() {
        return Err("Configure MEDIA_SCANNER=clamav and a loopback CLAMAV_ADDRESS".into());
    }
    scanner
        .scan(b"MyFans clean scanner fixture")
        .await
        .map_err(|e| e.1)?;
    let eicar = [
        "X5O!P%@AP[4\\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*",
    ]
    .concat();
    match scanner.scan(eicar.as_bytes()).await {
        Err(error) if error.0 == axum::http::StatusCode::BAD_REQUEST => {}
        _ => return Err("Scanner did not reject the harmless antivirus test fixture".into()),
    }
    println!("Fresh ClamAV database: clean input accepted; antivirus test fixture rejected.");
    Ok(())
}
