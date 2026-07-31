//! Single writer thread: the UI/core never blocks on disk (bottleneck B5).
//! All mutations are queued; readers use separate WAL connections.

use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;

use rusqlite::Connection;

type WriteFn = Box<dyn FnOnce(&mut Connection) + Send>;

enum Op {
    Write(WriteFn),
    Flush(SyncSender<()>),
}

pub struct StorageWriter {
    tx: Sender<Op>,
    handle: Option<JoinHandle<()>>,
}

/// Cloneable submit-only handle for background threads (e.g. download workers).
#[derive(Clone)]
pub struct WriterHandle {
    tx: Sender<Op>,
}

impl WriterHandle {
    /// Queue a write; returns immediately. Discarded if the writer shut down.
    pub fn submit(&self, f: impl FnOnce(&mut Connection) + Send + 'static) {
        let _ = self.tx.send(Op::Write(Box::new(f)));
    }
}

impl StorageWriter {
    pub fn spawn(mut conn: Connection) -> Self {
        let (tx, rx) = channel::<Op>();
        let handle = std::thread::Builder::new()
            .name("blaze-storage-writer".into())
            .spawn(move || {
                while let Ok(op) = rx.recv() {
                    match op {
                        Op::Write(f) => f(&mut conn),
                        Op::Flush(ack) => {
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .expect("failed to spawn storage writer thread");
        Self {
            tx,
            handle: Some(handle),
        }
    }

    /// Queue a write; returns immediately.
    pub fn submit(&self, f: impl FnOnce(&mut Connection) + Send + 'static) {
        // Receiver only drops on shutdown; late submits are safely discarded.
        let _ = self.tx.send(Op::Write(Box::new(f)));
    }

    /// Cloneable handle for threads that outlive the borrow of `self`.
    pub fn handle(&self) -> WriterHandle {
        WriterHandle {
            tx: self.tx.clone(),
        }
    }

    /// Block until every previously queued write has been applied.
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = sync_channel(1);
        if self.tx.send(Op::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }
}

impl Drop for StorageWriter {
    fn drop(&mut self) {
        self.flush();
        // Closing the channel ends the thread loop.
        let (tx, _) = channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::open_profile_db;

    #[test]
    fn writes_apply_in_order_and_flush_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.db");
        let writer = StorageWriter::spawn(open_profile_db(&path).unwrap());

        for i in 0..100i64 {
            writer.submit(move |c| {
                c.execute(
                    "INSERT INTO history (url, title, last_visit) VALUES (?1, '', ?2)",
                    rusqlite::params![format!("https://example.com/{i}"), i],
                )
                .unwrap();
            });
        }
        writer.flush();

        let read = Connection::open(&path).unwrap();
        let n: i64 = read
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 100);
    }

    #[test]
    fn drop_flushes_pending_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.db");
        {
            let writer = StorageWriter::spawn(open_profile_db(&path).unwrap());
            writer.submit(|c| {
                c.execute(
                    "INSERT INTO history (url, title, last_visit) VALUES ('https://x.com', '', 0)",
                    [],
                )
                .unwrap();
            });
        } // drop
        let read = Connection::open(&path).unwrap();
        let n: i64 = read
            .query_row("SELECT count(*) FROM history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
