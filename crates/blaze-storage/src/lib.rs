//! Portable persistence: one SQLite `profile.db` (WAL) + `settings.json`.
//! Contract: specs/001-lightweight-adblock-browser/contracts/storage-schema.md.

pub mod bookmarks;
pub mod downloads;
pub mod exceptions;
pub mod paths;
pub mod retention;
pub mod schema;
pub mod session;
pub mod settings;
pub mod writer;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use thiserror::Error;

use settings::Settings;
use writer::StorageWriter;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("profile directory unavailable")]
    NoProfileDir,
    #[error("settings: {0}")]
    Settings(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

/// An open user profile: owns the writer thread and the settings document.
/// The DB file plus settings.json constitute the complete portable profile (FR-032).
pub struct Profile {
    dir: PathBuf,
    /// Daily 90-day history prune (T062); stops when the profile closes.
    /// Declared before `writer`: its thread holds a `WriterHandle`, so it must
    /// drop first or `StorageWriter::drop` would join forever (fields drop in
    /// declaration order).
    _retention: retention::RetentionJob,
    writer: StorageWriter,
    settings: Mutex<Settings>,
}

impl Profile {
    /// Open (creating if needed) the profile at `dir`.
    pub fn open(dir: &Path) -> Result<Self, StorageError> {
        std::fs::create_dir_all(dir)?;
        let db_path = dir.join("profile.db");
        let conn = schema::open_profile_db(&db_path)?;
        let writer = StorageWriter::spawn(conn);
        let retention = retention::RetentionJob::spawn(writer.handle());
        let settings = Settings::load_lossy(&dir.join("settings.json"));
        Ok(Self {
            dir: dir.to_owned(),
            writer,
            settings: Mutex::new(settings),
            _retention: retention,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn writer(&self) -> &StorageWriter {
        &self.writer
    }

    /// New read-only connection (WAL allows concurrent readers).
    pub fn read_conn(&self) -> Result<rusqlite::Connection, StorageError> {
        let conn = rusqlite::Connection::open_with_flags(
            self.dir.join("profile.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        Ok(conn)
    }

    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .expect("settings lock poisoned")
            .clone()
    }

    /// Apply a mutation and persist atomically.
    pub fn update_settings(
        &self,
        mutate: impl FnOnce(&mut Settings),
    ) -> Result<Settings, StorageError> {
        let mut guard = self.settings.lock().expect("settings lock poisoned");
        mutate(&mut guard);
        guard
            .save(&self.dir.join("settings.json"))
            .map_err(|e| StorageError::Settings(e.to_string()))?;
        Ok(guard.clone())
    }

    /// Flush all pending writes (shutdown / tests).
    pub fn flush(&self) {
        self.writer.flush();
    }

    /// Run a mutation on the writer thread and wait for its result.
    /// For low-frequency operations that need a synchronous answer (e.g.
    /// bookmark inserts returning the new row id).
    pub fn write_sync<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> R + Send + 'static,
    ) -> Result<R, StorageError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.writer.submit(move |c| {
            let _ = tx.send(f(c));
        });
        rx.recv()
            .map_err(|_| StorageError::Invalid("storage writer unavailable".into()))
    }
}
