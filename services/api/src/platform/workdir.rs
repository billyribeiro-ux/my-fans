//! Private scratch storage whose lifetime follows its processing future.
use super::*;
use std::path::Path;

pub struct WorkDir(PathBuf);
impl WorkDir {
    pub fn create(root: &Path) -> Result<Self> {
        let path = root.join(format!(".work-{}", Uuid::new_v4()));
        // A single synchronous mkdir prevents cancellation between creation and
        // installing the guard. Subsequent bulk I/O remains asynchronous.
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| providers::unavailable())?;
        Ok(Self(path))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for WorkDir {
    fn drop(&mut self) {
        // Bounded flat directories (at most 400 files) are removed even when a
        // job future is cancelled or unwinds. No asynchronous task can outlive
        // its worker and leave scratch contents behind during normal shutdown.
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::error!("media_scratch_cleanup_failed");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[tokio::test]
    async fn cancellation_removes_private_scratch() {
        let root = std::env::temp_dir();
        let (send, receive) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let work = WorkDir::create(&root).unwrap();
            tokio::fs::write(work.path().join("source.upload"), b"private")
                .await
                .unwrap();
            send.send(work.path().to_path_buf()).unwrap();
            std::future::pending::<()>().await;
        });
        let path = receive.await.unwrap();
        assert!(path.exists());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!path.exists());
    }
}
