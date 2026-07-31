//! 90-day history retention (clarification #5, FR-026): a daily pruning job
//! runs on its own thread and deletes rows through the single-writer queue.

use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::Connection;

use crate::StorageError;
use crate::writer::WriterHandle;

pub const RETENTION_DAYS: i64 = 90;
const DAY_SECS: i64 = 86_400;

/// Delete history entries whose last visit is older than the retention window.
/// Returns the number of pruned rows.
pub fn prune_history(conn: &Connection, now_secs: i64) -> Result<usize, StorageError> {
    let cutoff = now_secs - RETENTION_DAYS * DAY_SECS;
    let n = conn.execute("DELETE FROM history WHERE last_visit < ?1", [cutoff])?;
    Ok(n)
}

/// Owns the daily prune thread; pruning happens immediately on spawn and
/// then every 24h. Dropping the job stops the thread promptly.
pub struct RetentionJob {
    stop: Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl RetentionJob {
    pub fn spawn(writer: WriterHandle) -> Self {
        // First prune is queued synchronously so callers can flush-and-observe.
        submit_prune(&writer);
        let (stop, rx) = channel::<()>();
        let handle = std::thread::Builder::new()
            .name("blaze-retention".into())
            .spawn(move || {
                // Sleep 24h between prunes, exiting early when the job drops.
                while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    rx.recv_timeout(Duration::from_secs(DAY_SECS as u64))
                {
                    submit_prune(&writer);
                }
            })
            .expect("failed to spawn retention thread");
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

fn submit_prune(writer: &WriterHandle) {
    writer.submit(|conn| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Ok(n) = prune_history(conn, now)
            && n > 0
        {
            tracing::info!(pruned = n, "history retention prune");
        }
    });
}

impl Drop for RetentionJob {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::open_profile_db;
    use crate::writer::StorageWriter;

    fn seed(conn: &Connection, url: &str, last_visit: i64) {
        conn.execute(
            "INSERT INTO history (url, title, last_visit) VALUES (?1, '', ?2)",
            rusqlite::params![url, last_visit],
        )
        .unwrap();
    }

    #[test]
    fn prunes_only_rows_older_than_90_days() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_profile_db(&dir.path().join("p.db")).unwrap();
        let now = 100 * DAY_SECS;
        seed(&conn, "https://old.example", now - 91 * DAY_SECS);
        seed(&conn, "https://edge.example", now - 90 * DAY_SECS); // exactly at edge: kept
        seed(&conn, "https://new.example", now - DAY_SECS);

        assert_eq!(prune_history(&conn, now).unwrap(), 1);
        let left: i64 = conn
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 2);
    }

    #[test]
    fn job_prunes_on_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        {
            let conn = open_profile_db(&path).unwrap();
            seed(&conn, "https://ancient.example", 0);
        }
        let writer = StorageWriter::spawn(open_profile_db(&path).unwrap());
        let _job = RetentionJob::spawn(writer.handle());
        writer.flush();

        let conn = open_profile_db(&path).unwrap();
        let left: i64 = conn
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }
}
