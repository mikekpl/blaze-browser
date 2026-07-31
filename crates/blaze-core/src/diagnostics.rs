//! Local-only crash logging (T066, FR-035): panics are captured to files under
//! the profile's `crashes/` directory and NEVER transmitted anywhere — this
//! crate has no network access and the browser has zero telemetry.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();
const MAX_LOGS: usize = 20;

/// Install the process-wide panic hook once. Subsequent calls are no-ops.
/// The previous hook (default stderr print) is preserved and chained.
pub fn install(crash_dir: &Path) {
    if CRASH_DIR.set(crash_dir.to_owned()).is_err() {
        return; // already installed
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = CRASH_DIR.get() {
            let _ = write_crash_log(dir, &format_panic(info));
        }
        previous(info);
    }));
}

fn format_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_owned());
    let thread = std::thread::current();
    format!(
        "blaze crash report (local only, never transmitted)\n\
         version: {}\n\
         thread: {}\n\
         location: {}\n\
         message: {}\n",
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("<unnamed>"),
        location,
        message,
    )
}

/// Write one crash log and prune the directory to the newest `MAX_LOGS`.
/// Must be infallible-ish: called from a panic hook, so it swallows errors.
fn write_crash_log(dir: &Path, body: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("blaze-crash-{stamp}.log"));
    let mut f = std::fs::File::create(&path)?;
    f.write_all(body.as_bytes())?;
    prune_old_logs(dir);
    Ok(())
}

fn prune_old_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("blaze-crash-") && n.ends_with(".log"))
        })
        .collect();
    if logs.len() <= MAX_LOGS {
        return;
    }
    logs.sort(); // timestamped names sort chronologically
    for old in &logs[..logs.len() - MAX_LOGS] {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_log_written_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(MAX_LOGS + 5) {
            write_crash_log(dir.path(), &format!("crash {i}")).unwrap();
            // Distinct millisecond stamps so file names don't collide.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let count = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(count, MAX_LOGS);
    }

    #[test]
    fn panic_in_spawned_thread_produces_log() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path());
        let _ = std::thread::Builder::new()
            .name("crash-test".into())
            .spawn(|| panic!("intentional test panic"))
            .unwrap()
            .join();
        let logs: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(logs.len(), 1);
        let body = std::fs::read_to_string(logs[0].as_ref().unwrap().path()).unwrap();
        assert!(body.contains("intentional test panic"));
        assert!(body.contains("crash-test"));
        assert!(body.contains("never transmitted"));
    }
}
