//! Single-instance handoff (ratified Q4: the access outlives the window).
//!
//! While the access is on, a window close only HIDES the window (lib.rs); the
//! process keeps serving the phone. Without a handoff the hidden window would
//! be unreachable — a second launch connects to this socket and asks the
//! running instance to show itself again.
//!
//! `acquire` answers with THREE typed outcomes (quality return R2): «I am the
//! instance», «another instance is running and was asked to show», and «the
//! handoff socket could not be created». The last one is not the same fact as
//! the second one, and collapsing them into `None` hid a broken single-instance
//! guard behind a normal exit.

/// The result of trying to become the single UI instance.
pub enum InstanceAcquire {
    /// This process owns the handoff socket.
    Acquired(InstanceGuard),
    /// Another instance answered and was asked to re-show its window; the
    /// caller must exit quietly.
    AlreadyRunning,
    /// The handoff socket could not be set up; the reason is human-readable
    /// and does NOT contain secrets. The window may still run without it.
    Unavailable(String),
}

#[cfg(all(unix, not(mobile)))]
pub use unix::{InstanceGuard, acquire};

#[cfg(all(unix, not(mobile)))]
mod unix {
    use super::InstanceAcquire;
    use std::path::PathBuf;

    pub struct InstanceGuard {
        path: PathBuf,
        listener: std::os::unix::net::UnixListener,
    }

    /// Take the handoff socket, or report the typed outcome.
    pub fn acquire(path: PathBuf) -> InstanceAcquire {
        use std::os::unix::net::{UnixListener, UnixStream};
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return InstanceAcquire::Unavailable(format!(
                    "не удалось создать каталог {}: {e}",
                    dir.display()
                ));
            }
        }
        if let Ok(mut stream) = UnixStream::connect(&path) {
            use std::io::Write;
            let _ = stream.write_all(b"show\n");
            return InstanceAcquire::AlreadyRunning;
        }
        // A stale socket file from a crashed run is removed; a path that is
        // NOT a stale socket (a directory, an unwritable parent) stays and is
        // reported as the setup failure it is.
        let _ = std::fs::remove_file(&path);
        match UnixListener::bind(&path) {
            Ok(listener) => InstanceAcquire::Acquired(InstanceGuard { path, listener }),
            Err(e) => InstanceAcquire::Unavailable(format!(
                "не удалось открыть сокет {}: {e}",
                path.display()
            )),
        }
    }

    impl InstanceGuard {
        /// Accept handoffs on a background thread and re-show the window.
        pub fn spawn(self, app: tauri::AppHandle) {
            std::thread::Builder::new()
                .name("devai-ui-instance".into())
                .spawn(move || {
                    use tauri::Manager;
                    for stream in self.listener.incoming() {
                        if stream.is_err() {
                            continue;
                        }
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    let _ = std::fs::remove_file(&self.path);
                })
                .ok();
        }
    }
}

#[cfg(not(all(unix, not(mobile))))]
pub struct InstanceGuard;

#[cfg(not(all(unix, not(mobile))))]
pub fn acquire(_path: std::path::PathBuf) -> InstanceAcquire {
    InstanceAcquire::Acquired(InstanceGuard)
}

#[cfg(not(all(unix, not(mobile))))]
impl InstanceGuard {
    pub fn spawn(self, _app: tauri::AppHandle) {}
}

#[cfg(all(test, unix, not(mobile)))]
mod tests {
    use super::*;

    /// R2: «занято» and «сокет не создался» are different answers. The first
    /// must exit quietly, the second must be visible to the operator.
    #[test]
    fn a_second_instance_is_told_apart_from_a_failed_socket() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ui-instance.sock");
        let guard = match acquire(path.clone()) {
            InstanceAcquire::Acquired(guard) => guard,
            other => panic!(
                "the first acquire must own the socket, got {}",
                describe(&other)
            ),
        };
        assert!(matches!(
            acquire(path.clone()),
            InstanceAcquire::AlreadyRunning
        ));

        // A directory at the socket path cannot be removed and cannot be bound:
        // that is a setup failure, not «another instance is running».
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        assert!(matches!(acquire(blocked), InstanceAcquire::Unavailable(_)));
        drop(guard);
    }

    fn describe(outcome: &InstanceAcquire) -> &'static str {
        match outcome {
            InstanceAcquire::Acquired(_) => "acquired",
            InstanceAcquire::AlreadyRunning => "already running",
            InstanceAcquire::Unavailable(_) => "unavailable",
        }
    }
}
