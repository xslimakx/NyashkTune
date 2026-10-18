#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "linux")]
mod x11;

mod clipboard;
mod event;
mod keyboard;
mod mouse_cursor;
mod window;
mod window_info;
mod window_open_options;

#[cfg(feature = "opengl")]
pub mod gl;

pub use clipboard::*;
pub use event::*;
pub use mouse_cursor::MouseCursor;
pub use window::*;
pub use window_info::*;
pub use window_open_options::*;

/// NyashkTune diagnostics: append a line to `<tmp>/nyashktune.log` (best effort, never panics).
pub fn nylog(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("nyashktune.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let _ = writeln!(f, "[{t:.3}] [{:?}] {msg}", std::thread::current().id());
    }
}

static LIVE_WINDOWS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts a running window thread; decrements when dropped (also on panic).
pub struct LiveWindowGuard;
impl LiveWindowGuard {
    pub fn new() -> Self {
        LIVE_WINDOWS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        LiveWindowGuard
    }
}
impl Drop for LiveWindowGuard {
    fn drop(&mut self) {
        LIVE_WINDOWS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// NyashkTune: wait (at most `timeout`) until all previously opened GUI window threads have
/// ended, so two GL windows of the same plugin never run at the same time.
pub fn wait_for_windows(timeout: std::time::Duration) {
    let start = std::time::Instant::now();
    while LIVE_WINDOWS.load(std::sync::atomic::Ordering::SeqCst) > 0 && start.elapsed() < timeout {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let left = LIVE_WINDOWS.load(std::sync::atomic::Ordering::SeqCst);
    if left > 0 {
        nylog(&format!("wait_for_windows: timed out, {left} window thread(s) still alive"));
    }
}
