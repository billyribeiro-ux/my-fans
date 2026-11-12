//! Bounded subprocess interface. Linux always uses the fail-closed OS launcher;
//! macOS development uses the native deny-by-default sandbox. No decoder runs in
//! the API or worker's address space or inherits application environment values.
use super::*;
use std::path::Path;
use tokio::{io::AsyncReadExt, process::Command};
const OUTPUT_LIMIT: u64 = 64 * 1024;
fn unavailable() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "The isolated media processor is unavailable.",
    )
}
fn local_binary(name: &str) -> Result<PathBuf> {
    if cfg!(target_os = "linux") {
        return Ok(PathBuf::from(format!("/usr/local/bin/{name}")));
    }
    if name == "myfans-media" {
        let executable = std::env::current_exe().map_err(|_| unavailable())?;
        let base = executable.parent().ok_or_else(unavailable)?;
        let base = if base.file_name().is_some_and(|v| v == "deps") {
            base.parent().ok_or_else(unavailable)?
        } else {
            base
        };
        return Ok(base.join(name));
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join(name))
                .find(|p| p.is_file())
        })
        .ok_or_else(unavailable)
}
pub async fn execute(work: &Path, name: &str, args: &[String], seconds: u64) -> Result<Vec<u8>> {
    if !matches!(name, "ffmpeg" | "ffprobe" | "myfans-media") || seconds > 300 {
        return Err(unavailable());
    };
    let work = tokio::fs::canonicalize(work)
        .await
        .map_err(|_| unavailable())?;
    let binary = local_binary(name)?;
    let mut command;
    #[cfg(target_os = "linux")]
    {
        command = Command::new("/usr/local/bin/myfans-media-sandbox");
        command.arg(&work).arg(&binary).args(args);
    }
    #[cfg(target_os = "macos")]
    {
        // The profile is passed directly, never evaluated by a shell. Paths are
        // quoted with JSON's escaped string syntax, compatible with SBPL strings.
        let quote = |path: &Path| {
            serde_json::to_string(&path.to_string_lossy().as_ref()).map_err(|_| unavailable())
        };
        let profile = format!(
            "(version 1)(deny default)(allow process-exec)(allow sysctl-read)(allow file-read-metadata)(allow file-read* (literal \"/\") (literal {}) (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/opt/homebrew/Cellar\") (literal \"/dev/urandom\") (literal \"/dev/null\"))(allow file-read* file-write* (subpath {}))",
            quote(&binary)?,
            quote(&work)?
        );
        command = Command::new("/usr/bin/sandbox-exec");
        command.arg("-p").arg(profile).arg(&binary).args(args);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        return Err(unavailable());
    }
    command
        .current_dir(&work)
        .env_clear()
        .env("PATH", "/usr/local/bin")
        .env("LANG", "C")
        .env("TZ", "UTC")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| unavailable())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(unavailable)?
        .take(OUTPUT_LIMIT + 1);
    let result = tokio::select! {
      result = tokio::time::timeout(Duration::from_secs(seconds), async {
        let mut output = Vec::new();
        stdout
            .read_to_end(&mut output)
            .await
            .map_err(|_| unavailable())?;
        if output.len() as u64 > OUTPUT_LIMIT {
            return Err(bad("Media output exceeded processing limits."));
        }
        let status = child.wait().await.map_err(|_| unavailable())?;
        if status.code() == Some(125) {
            return Err(unavailable());
        };
        if !status.success() {
            return Err(bad(
                "The media could not be processed within resource limits.",
            ));
        }
        Ok(output)
      }) => result,
      error = scratch_limit(&work) => Ok(Err(error)),
    };
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(bad("Media processing exceeded its time limit."))
        }
    }
}
async fn scratch_limit(work: &Path) -> Error {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        interval.tick().await;
        let Ok(mut entries) = tokio::fs::read_dir(work).await else {
            return unavailable();
        };
        let mut bytes = 0u64;
        let mut count = 0usize;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => return unavailable(),
            };
            count += 1;
            if count > 400 {
                return bad("Media output exceeded workspace limits.");
            };
            let metadata = match tokio::fs::symlink_metadata(entry.path()).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return unavailable(),
            };
            if !metadata.is_file() {
                return bad("Media output exceeded workspace limits.");
            };
            bytes = bytes.saturating_add(metadata.len());
            if bytes > 768 * 1024 * 1024 {
                return bad("Media output exceeded workspace limits.");
            };
        }
    }
}
pub async fn render_image(work: &Path, input: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let input = tokio::fs::canonicalize(input)
        .await
        .map_err(|_| unavailable())?;
    execute(
        work,
        "myfans-media",
        &[
            "image".into(),
            input.to_string_lossy().into_owned(),
            "full.jpg".into(),
            "preview.jpg".into(),
        ],
        30,
    )
    .await?;
    let full = read_output(&work.join("full.jpg"), 32 * 1024 * 1024).await?;
    let preview = read_output(&work.join("preview.jpg"), 1024 * 1024).await?;
    Ok((full, preview))
}
async fn read_output(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| unavailable())?;
    if !metadata.is_file() {
        return Err(bad("Invalid media output."));
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| unavailable())?;
    let metadata = file.metadata().await.map_err(|_| unavailable())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err(bad("Media output exceeded processing limits."));
    }
    let mut output = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut output)
        .await
        .map_err(|_| unavailable())?;
    if output.len() as u64 > limit {
        return Err(bad("Media output exceeded processing limits."));
    }
    Ok(output)
}
pub async fn verify_isolation(root: &Path) -> Result<()> {
    let id = Uuid::new_v4();
    let work = root.join(format!(".work-probe-{id}"));
    let canary = root.join(format!(".isolation-canary-{id}"));
    let mut builder = tokio::fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&work).await.map_err(|_| unavailable())?;
    let outcome = async {
        tokio::fs::write(&canary, b"not-a-secret-isolation-canary")
            .await
            .map_err(|_| unavailable())?;
        let canary = tokio::fs::canonicalize(&canary)
            .await
            .map_err(|_| unavailable())?;
        let output = execute(
            &work,
            "myfans-media",
            &["self-test".into(), canary.to_string_lossy().into_owned()],
            10,
        )
        .await?;
        if output != b"media_sandbox_verified\n"
            || tokio::fs::read(&canary).await.map_err(|_| unavailable())?
                != b"not-a-secret-isolation-canary"
        {
            return Err(unavailable());
        };
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(canary).await;
    let _ = tokio::fs::remove_dir_all(work).await;
    outcome
}
