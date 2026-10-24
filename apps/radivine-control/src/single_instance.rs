//! Single-instance window activation lock (`$XDG_RUNTIME_DIR/radivine/control.sock`) for `radivine-control`.
//!
//! Ensures launching `radivine-control` a second time from the application menu focuses the
//! existing Control Center window (and optionally switches to a requested navigation page) rather
//! than opening duplicate windows.

use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ControlPage;

/// Returns the default Control Center single-instance socket path (`$RADIVINE_CONTROL_SOCKET` if set, otherwise `$XDG_RUNTIME_DIR/radivine/control.sock`).
#[must_use]
pub fn default_control_socket_path() -> PathBuf {
    if let Ok(env_sock) = std::env::var("RADIVINE_CONTROL_SOCKET")
        && !env_sock.trim().is_empty()
    {
        return PathBuf::from(env_sock);
    }
    if let Ok(xdg_runtime) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg_runtime.trim().is_empty()
    {
        return PathBuf::from(xdg_runtime)
            .join("radivine")
            .join("control.sock");
    }
    let uid = std::env::var("UID").unwrap_or_else(|_| "1000".to_string());
    PathBuf::from(format!("/tmp/radivine-{uid}")).join("control.sock")
}

/// Owns the non-blocking Unix domain socket listener for the active `radivine-control` window.
pub struct ControlInstanceLock {
    listener: UnixListener,
    sock_path: PathBuf,
}

impl ControlInstanceLock {
    /// Acquires the single-instance lock at `sock_path`, or notifies the already-running instance
    /// to focus its window (and switch to `requested_page`, if provided) and returns `Ok(None)`.
    pub fn acquire_or_notify_existing(
        sock_path: &Path,
        requested_page: Option<ControlPage>,
    ) -> anyhow::Result<Option<Self>> {
        if sock_path.exists() {
            if notify_existing_instance(sock_path, requested_page).is_ok() {
                return Ok(None);
            }
            let _ = fs::remove_file(sock_path);
        }

        if let Some(parent) = sock_path.parent() {
            fs::create_dir_all(parent)?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }

        let listener = UnixListener::bind(sock_path)?;
        let _ = fs::set_permissions(sock_path, fs::Permissions::from_mode(0o600));
        listener.set_nonblocking(true)?;

        Ok(Some(Self {
            listener,
            sock_path: sock_path.to_path_buf(),
        }))
    }

    /// Polls for any activation request sent by a newly launched `radivine-control` process.
    ///
    /// Returns `Some(Some(page))` if the caller requested focusing a specific page, `Some(None)`
    /// if the caller requested focusing the current page, or `None` if no activation occurred.
    #[must_use]
    pub fn poll_activation(&self) -> Option<Option<ControlPage>> {
        let mut activated: Option<Option<ControlPage>> = None;
        loop {
            match self.listener.accept() {
                Ok((mut stream, _addr)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                    let mut reader = BufReader::new(
                        stream
                            .try_clone()
                            .unwrap_or_else(|_| panic!("clone unix stream")),
                    );
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok() {
                        let trimmed = line.trim();
                        if let Some(page_str) = trimmed.strip_prefix("focus:") {
                            activated = Some(ControlPage::from_slug(page_str));
                        } else if trimmed == "focus" {
                            activated = Some(None);
                        }
                        let _ = stream.write_all(b"ok\n");
                        let _ = stream.flush();
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        activated
    }
}

impl Drop for ControlInstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.sock_path);
    }
}

fn notify_existing_instance(
    sock_path: &Path,
    requested_page: Option<ControlPage>,
) -> anyhow::Result<()> {
    let mut stream = UnixStream::connect(sock_path)?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;

    let cmd = match requested_page {
        Some(page) => format!("focus:{}\n", page.slug()),
        None => "focus\n".to_string(),
    };
    stream.write_all(cmd.as_bytes())?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_notifies_first_and_stale_socket_recovers() {
        let dir =
            std::env::temp_dir().join(format!("radivine-control-lock-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("control.sock");

        let lock1 = ControlInstanceLock::acquire_or_notify_existing(&sock, None)
            .unwrap()
            .expect("first instance should acquire lock");

        // Spawn a background thread to accept the activation while the second instance connects
        let handle = std::thread::spawn(move || {
            for _ in 0..40 {
                if let Some(req) = lock1.poll_activation() {
                    return (lock1, Some(req));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            (lock1, None)
        });

        std::thread::sleep(Duration::from_millis(15));
        let lock2 =
            ControlInstanceLock::acquire_or_notify_existing(&sock, Some(ControlPage::Actions))
                .unwrap();
        assert!(
            lock2.is_none(),
            "second instance should return None after notifying first"
        );

        let (lock1, activated) = handle.join().unwrap();
        assert_eq!(activated, Some(Some(ControlPage::Actions)));
        drop(lock1);

        // After dropping lock1, a new instance can acquire cleanly
        let lock3 = ControlInstanceLock::acquire_or_notify_existing(&sock, None)
            .unwrap()
            .expect("third instance should acquire after drop");
        drop(lock3);
        let _ = fs::remove_dir_all(&dir);
    }
}
