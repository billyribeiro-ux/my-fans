use std::{env, net::SocketAddr, time::Duration};

use myfans_api::platform::{connect, router, worker};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let command = env::args().nth(1);
    if command.as_deref().is_some_and(|v| {
        ![
            "migrate",
            "provision-runtime",
            "bootstrap-admin",
            "rebind-storage",
            "rotate-key",
            "privacy-purge",
            "privacy-retention",
            "privacy-export",
            "privacy-replay",
            "seed",
            "worker",
            "verify-storage",
            "verify-media-pipeline",
            "doctor",
        ]
        .contains(&v)
    }) {
        return Err("Usage: myfans-api [migrate|provision-runtime|bootstrap-admin|seed|worker|verify-storage|verify-media-pipeline|doctor]".into());
    }
    // This release diagnostic only creates synthetic media in fresh scratch
    // directories. It needs no application configuration, database or providers.
    if command.as_deref() == Some("verify-media-pipeline") {
        let root =
            env::var_os("MEDIA_ROOT").ok_or("MEDIA_ROOT is required for media verification")?;
        myfans_api::platform::verify_media_pipeline(std::path::Path::new(&root))
            .await
            .map_err(|error| error.1)?;
        tracing::info!("media_pipeline_verified");
        return Ok(());
    }
    if command.as_deref() == Some("verify-storage")
        && env::var("APP_RUN_MIGRATIONS").as_deref() != Ok("false")
    {
        return Err("Storage verification requires APP_RUN_MIGRATIONS=false and an isolated restored database/media pair".into());
    }
    if let Some(
        cmd @ ("migrate" | "provision-runtime" | "bootstrap-admin" | "rebind-storage"
        | "rotate-key" | "privacy-purge" | "privacy-retention" | "privacy-export"
        | "privacy-replay"),
    ) = command.as_deref()
    {
        myfans_api::platform::database_admin(cmd).await?;
        tracing::info!(command = cmd, "database_admin_complete");
        return Ok(());
    }
    let state = if command.as_deref() == Some("verify-storage") {
        myfans_api::platform::connect_for_storage_verification().await?
    } else {
        connect().await?
    };
    if command.as_deref() == Some("doctor") {
        let evidence = myfans_api::platform::doctor(&state).await;
        println!("{evidence}");
        state.db.close().await;
        if [
            "payments",
            "identity",
            "private_object_storage",
            "storage_probe",
            "scanner_probe",
            "media_isolation",
            "smtp_configured",
        ]
        .iter()
        .any(|field| evidence[*field] != true)
            || evidence["failed_jobs"] != 0
        {
            return Err(
                "Provider test environment is incomplete; see the non-secret diagnostic output"
                    .into(),
            );
        }
        return Ok(());
    }
    if command.as_deref() == Some("verify-storage") {
        let (mfa_secrets,mail_bodies)=myfans_api::platform::verify_storage(&state).await.map_err(|_| "Stored ciphertext verification failed; restore the matching key and unmodified data")?;
        tracing::info!(mfa_secrets, mail_bodies, "stored_ciphertext_verified");
        state.db.close().await;
        return Ok(());
    }
    if command.as_deref() == Some("migrate") {
        tracing::info!("database_migrations_complete");
        return Ok(());
    }
    if command.as_deref() == Some("seed") {
        worker::seed(&state)
            .await
            .map_err(|e| format!("Seed failed: {}", e.1))?;
        tracing::info!("sandbox_seed_complete");
        return Ok(());
    }
    let embedded = match env::var("APP_EMBEDDED_WORKER").as_deref() {
        Ok("false") => false,
        Ok("true") | Err(_) => true,
        _ => return Err("APP_EMBEDDED_WORKER must be true or false.".into()),
    };
    let (stop, receiver) = watch::channel(false);
    let signal = tokio::spawn(async move {
        if let Err(error) = shutdown_signal().await {
            tracing::error!(%error, "shutdown_signal_failed");
        }
        let _ = stop.send(true);
    });
    let mut worker_handle = if embedded || command.as_deref() == Some("worker") {
        Some(tokio::spawn(worker::run(state.clone(), receiver.clone())))
    } else {
        None
    };
    if command.as_deref() == Some("worker") {
        tracing::info!("myfans_worker_started");
        let mut receiver = receiver;
        let _ = receiver.changed().await;
    } else {
        let address: SocketAddr = env::var("API_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()?;
        if !address.ip().is_loopback() {
            return Err("The internal API must bind to a loopback address.".into());
        }
        let listener = tokio::net::TcpListener::bind(address).await?;
        tracing::info!(%address, mode=state.runtime.mode(), embedded_worker=embedded, "myfans_api_started");
        let mut receiver = receiver;
        axum::serve(listener, router(state.clone()))
            .with_graceful_shutdown(async move {
                let _ = receiver.changed().await;
            })
            .await?;
    }
    // Let a claimed job finish before closing the pool. A crashed job is recovered by its lease.
    if let Some(ref mut handle) = worker_handle
        && tokio::time::timeout(Duration::from_secs(115), &mut *handle)
            .await
            .is_err()
    {
        tracing::warn!("worker_shutdown_deadline_exceeded");
        handle.abort();
    }
    state.db.close().await;
    signal.abort();
    tracing::info!("myfans_shutdown_complete");
    Ok(())
}
