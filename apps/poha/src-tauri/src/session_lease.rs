use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const SESSION_LEASE_FILE: &str = ".capture.lock";

/// Holds an advisory exclusive lock for the lifetime of a capture/finalization.
///
/// The lock file intentionally remains in the session directory. The operating
/// system releases the lock if Poha crashes, which lets the CLI distinguish a
/// crashed session from one the app is still writing without stale-file cleanup.
pub(crate) struct SessionLease {
    file: File,
    path: PathBuf,
}

impl SessionLease {
    pub(crate) fn acquire(session_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(session_dir).map_err(|error| {
            format!(
                "failed creating session directory {}: {error}",
                session_dir.display()
            )
        })?;
        let path = session_dir.join(SESSION_LEASE_FILE);
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&path)
            .map_err(|error| format!("failed opening session lease {}: {error}", path.display()))?;
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| format!("failed securing session lease {}: {error}", path.display()),
        )?;

        file.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => format!(
                "session is still active in another Poha process: {}",
                session_dir.display()
            ),
            TryLockError::Error(error) => {
                format!("failed locking session lease {}: {error}", path.display())
            }
        })?;

        Ok(Self { file, path })
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            tracing::warn!(path = %self.path.display(), %error, "failed unlocking session lease");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_blocks_concurrent_recovery_and_releases_on_drop() {
        let session = tempfile::tempdir().expect("session");
        let active = SessionLease::acquire(session.path()).expect("active lease");

        let error = SessionLease::acquire(session.path())
            .err()
            .expect("concurrent lease must fail");
        assert!(error.contains("still active"), "{error}");

        drop(active);
        SessionLease::acquire(session.path()).expect("lease after active capture ends");
    }

    #[cfg(unix)]
    #[test]
    fn lease_file_is_private() {
        let session = tempfile::tempdir().expect("session");
        let _lease = SessionLease::acquire(session.path()).expect("lease");
        let mode = std::fs::metadata(session.path().join(SESSION_LEASE_FILE))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
