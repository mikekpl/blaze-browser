//! Download orchestration (T047/T048, FR-020..023): validated state machine,
//! worker threads driving the blaze-net transport, persistence via the
//! storage writer, and throttled `DownloadUpdated` events (>=250ms).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use blaze_net::download::{
    CONTROL_CANCEL, CONTROL_PAUSE, CONTROL_RUN, DownloadJob, DownloadOutcome, collision_safe_path,
    run_download, sanitize_filename, suggested_name_from_url,
};
use blaze_storage::downloads as db;
use blaze_storage::downloads::DownloadRow;
use blaze_storage::writer::WriterHandle;

use crate::events::{Dispatcher, Event};
use crate::{BlazeCore, CoreError};

/// Minimum interval between progress persistence/event ticks (T049).
pub const PROGRESS_THROTTLE: Duration = Duration::from_millis(250);

/// Download lifecycle states (data-model.md §Download).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DownloadState {
    Active,
    Paused,
    Completed,
    Interrupted,
    Cancelled,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "interrupted" => Self::Interrupted,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    /// Transition matrix: `Active -> Paused|Completed|Interrupted|Cancelled`,
    /// `Paused -> Active|Cancelled`, `Interrupted -> Active|Cancelled`.
    pub fn can_transition(self, to: Self) -> bool {
        use DownloadState::*;
        matches!(
            (self, to),
            (Active, Paused)
                | (Active, Completed)
                | (Active, Interrupted)
                | (Active, Cancelled)
                | (Paused, Active)
                | (Paused, Cancelled)
                | (Interrupted, Active)
                | (Interrupted, Cancelled)
        )
    }
}

struct RunningJob {
    control: Arc<AtomicU8>,
    handle: Option<JoinHandle<()>>,
}

/// In-process registry of live download workers.
#[derive(Default)]
pub(crate) struct DownloadsRuntime {
    jobs: Arc<Mutex<HashMap<String, RunningJob>>>,
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn part_path(dest: &str) -> PathBuf {
    PathBuf::from(format!("{dest}.part"))
}

/// Expand a leading `~` in the configured download directory.
fn expand_dir(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

impl BlazeCore {
    /// Begin downloading `url` into the configured download directory,
    /// returning the new download id (FR-020).
    pub fn start_download(
        &self,
        url: &str,
        suggested_name: Option<&str>,
    ) -> Result<String, CoreError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| CoreError::InvalidArgument(format!("download url: {e}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CoreError::InvalidArgument(format!(
                "unsupported download scheme: {}",
                parsed.scheme()
            )));
        }

        let dir = expand_dir(&self.profile.settings().download_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Internal(format!("download dir: {e}")))?;
        let name = suggested_name
            .map(sanitize_filename)
            .filter(|n| !n.is_empty())
            .or_else(|| suggested_name_from_url(url))
            .unwrap_or_else(|| "download".to_string());
        let dest = collision_safe_path(&dir, &name);
        // Reserve the final name so concurrent starts cannot collide.
        let _ = std::fs::File::create_new(part_path(&dest.to_string_lossy()));

        let id = uuid::Uuid::new_v4().to_string();
        let row = DownloadRow {
            id: id.clone(),
            source_url: url.to_string(),
            dest_path: dest.to_string_lossy().into_owned(),
            total_bytes: None,
            received_bytes: 0,
            state: DownloadState::Active.as_str().to_string(),
            etag: None,
            last_modified: None,
            created_at: now_epoch(),
            completed_at: None,
        };
        {
            let row = row.clone();
            self.profile.writer().submit(move |c| {
                if let Err(e) = db::upsert(c, &row) {
                    tracing::error!("persist download row: {e}");
                }
            });
        }
        self.emit_download(&id, DownloadState::Active, 0, None);
        self.spawn_worker(&row, 0);
        Ok(id)
    }

    /// Pause an active download (FR-022); takes effect within one chunk read.
    pub fn pause_download(&self, id: &str) -> Result<(), CoreError> {
        let jobs = self.lock_jobs();
        let job = jobs
            .get(id)
            .ok_or_else(|| CoreError::NotFound("active download", id.to_string()))?;
        job.control.store(CONTROL_PAUSE, Ordering::Relaxed);
        Ok(())
    }

    /// Resume a paused or interrupted download with `If-Range` validation.
    pub fn resume_download(&self, id: &str) -> Result<(), CoreError> {
        if self.lock_jobs().contains_key(id) {
            return Err(CoreError::InvalidArgument(format!(
                "download {id} already running"
            )));
        }
        let row = self
            .download_row(id)?
            .ok_or_else(|| CoreError::NotFound("download", id.to_string()))?;
        let state = DownloadState::parse(&row.state)
            .ok_or_else(|| CoreError::Internal(format!("bad download state: {}", row.state)))?;
        if !state.can_transition(DownloadState::Active) {
            return Err(CoreError::InvalidArgument(format!(
                "cannot resume a {} download",
                row.state
            )));
        }
        // Disk is the source of truth for how much we actually have.
        let resume_from = std::fs::metadata(part_path(&row.dest_path))
            .map(|m| m.len())
            .unwrap_or(0);
        self.set_download_state(id, DownloadState::Active, None);
        self.emit_download(id, DownloadState::Active, resume_from, row.total_bytes);
        self.spawn_worker(&row, resume_from);
        Ok(())
    }

    /// Cancel a download in any non-terminal state; deletes the partial file.
    pub fn cancel_download(&self, id: &str) -> Result<(), CoreError> {
        if let Some(job) = self.lock_jobs().get(id) {
            job.control.store(CONTROL_CANCEL, Ordering::Relaxed);
            return Ok(());
        }
        let row = self
            .download_row(id)?
            .ok_or_else(|| CoreError::NotFound("download", id.to_string()))?;
        let state = DownloadState::parse(&row.state)
            .ok_or_else(|| CoreError::Internal(format!("bad download state: {}", row.state)))?;
        if !state.can_transition(DownloadState::Cancelled) {
            return Err(CoreError::InvalidArgument(format!(
                "cannot cancel a {} download",
                row.state
            )));
        }
        let _ = std::fs::remove_file(part_path(&row.dest_path));
        self.set_download_state(id, DownloadState::Cancelled, None);
        self.emit_download(
            id,
            DownloadState::Cancelled,
            row.received_bytes,
            row.total_bytes,
        );
        Ok(())
    }

    /// All downloads, newest first (history list).
    pub fn list_downloads(&self) -> Result<Vec<DownloadRow>, CoreError> {
        self.profile.flush();
        let conn = self.profile.read_conn()?;
        db::list(&conn).map_err(|e| CoreError::Storage(e.into()))
    }

    /// T048: mark downloads left `active` by a crash/quit as interrupted and
    /// auto-resume the ones that carry resume validators. Returns resumed ids.
    pub fn resume_interrupted_downloads(&self) -> Result<Vec<String>, CoreError> {
        self.profile.flush();
        self.profile.writer().submit(|c| {
            if let Err(e) = db::mark_stale_active_as_interrupted(c) {
                tracing::error!("mark stale downloads: {e}");
            }
        });
        self.profile.flush();

        let conn = self.profile.read_conn()?;
        let rows = db::list(&conn).map_err(|e| CoreError::Storage(e.into()))?;
        let mut resumed = Vec::new();
        for row in rows {
            let interrupted = row.state == DownloadState::Interrupted.as_str();
            let has_validator = row.etag.is_some() || row.last_modified.is_some();
            if interrupted && has_validator && self.resume_download(&row.id).is_ok() {
                resumed.push(row.id);
            }
        }
        Ok(resumed)
    }

    /// Block until the worker for `id` (if any) exits. Test/shutdown helper.
    pub fn wait_for_download(&self, id: &str) {
        let handle = self.lock_jobs().get_mut(id).and_then(|j| j.handle.take());
        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    fn download_row(&self, id: &str) -> Result<Option<DownloadRow>, CoreError> {
        self.profile.flush();
        let conn = self.profile.read_conn()?;
        db::get(&conn, id).map_err(|e| CoreError::Storage(e.into()))
    }

    fn set_download_state(&self, id: &str, state: DownloadState, completed_at: Option<i64>) {
        let id = id.to_string();
        self.profile.writer().submit(move |c| {
            if let Err(e) = db::set_state(c, &id, state.as_str(), completed_at) {
                tracing::error!("persist download state: {e}");
            }
        });
    }

    fn emit_download(&self, id: &str, state: DownloadState, received: u64, total: Option<u64>) {
        self.dispatcher.emit(Event::DownloadUpdated {
            download_id: id.to_string(),
            state: state.as_str().to_string(),
            received_bytes: received,
            total_bytes: total,
        });
        self.dispatcher.flush();
    }

    fn lock_jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, RunningJob>> {
        self.downloads.jobs.lock().expect("download jobs poisoned")
    }

    fn spawn_worker(&self, row: &DownloadRow, resume_from: u64) {
        let control = Arc::new(AtomicU8::new(CONTROL_RUN));
        let ctx = WorkerCtx {
            id: row.id.clone(),
            dest_path: row.dest_path.clone(),
            writer: self.profile.writer().handle(),
            dispatcher: Arc::clone(&self.dispatcher),
            jobs: Arc::clone(&self.downloads.jobs),
            control: Arc::clone(&control),
        };
        let job = DownloadJob {
            url: row.source_url.clone(),
            dest_path: part_path(&row.dest_path),
            resume_from,
            etag: row.etag.clone(),
            last_modified: row.last_modified.clone(),
        };
        let handle = std::thread::Builder::new()
            .name(format!("blaze-download-{}", row.id))
            .spawn(move || download_worker(ctx, job))
            .expect("failed to spawn download worker");
        self.lock_jobs().insert(
            row.id.clone(),
            RunningJob {
                control,
                handle: Some(handle),
            },
        );
    }
}

struct WorkerCtx {
    id: String,
    dest_path: String,
    writer: WriterHandle,
    dispatcher: Arc<Dispatcher>,
    jobs: Arc<Mutex<HashMap<String, RunningJob>>>,
    control: Arc<AtomicU8>,
}

fn download_worker(ctx: WorkerCtx, job: DownloadJob) {
    let total = std::cell::Cell::new(None::<u64>);
    let received = std::cell::Cell::new(job.resume_from);
    let last_tick = std::cell::Cell::new(Instant::now() - PROGRESS_THROTTLE); // first tick emits

    let persist_progress = |writer: &WriterHandle,
                            id: &str,
                            received: u64,
                            total: Option<u64>,
                            etag: Option<String>,
                            lm: Option<String>| {
        let id = id.to_string();
        writer.submit(move |c| {
            if let Err(e) =
                db::update_progress(c, &id, received, total, etag.as_deref(), lm.as_deref())
            {
                tracing::error!("persist download progress: {e}");
            }
        });
    };
    let emit = |dispatcher: &Dispatcher, id: &str, state: DownloadState, r: u64, t: Option<u64>| {
        dispatcher.emit(Event::DownloadUpdated {
            download_id: id.to_string(),
            state: state.as_str().to_string(),
            received_bytes: r,
            total_bytes: t,
        });
        dispatcher.flush();
    };

    let outcome = run_download(
        &job,
        &ctx.control,
        |meta| {
            total.set(meta.total_bytes);
            if meta.restarted {
                received.set(0);
            }
            persist_progress(
                &ctx.writer,
                &ctx.id,
                received.get(),
                total.get(),
                meta.etag.clone(),
                meta.last_modified.clone(),
            );
        },
        |bytes| {
            received.set(bytes);
            if last_tick.get().elapsed() >= PROGRESS_THROTTLE {
                last_tick.set(Instant::now());
                persist_progress(&ctx.writer, &ctx.id, bytes, total.get(), None, None);
                emit(
                    &ctx.dispatcher,
                    &ctx.id,
                    DownloadState::Active,
                    bytes,
                    total.get(),
                );
            }
        },
    );

    let (state, completed_at) = match &outcome {
        DownloadOutcome::Completed => {
            if let Err(e) = std::fs::rename(&job.dest_path, &ctx.dest_path) {
                tracing::error!("finalize download {}: {e}", ctx.id);
            }
            (DownloadState::Completed, Some(now_epoch()))
        }
        DownloadOutcome::Paused => (DownloadState::Paused, None),
        DownloadOutcome::Cancelled => (DownloadState::Cancelled, None),
        DownloadOutcome::Interrupted(reason) => {
            tracing::warn!("download {} interrupted: {reason}", ctx.id);
            (DownloadState::Interrupted, None)
        }
    };
    persist_progress(
        &ctx.writer,
        &ctx.id,
        received.get(),
        total.get(),
        None,
        None,
    );
    {
        let id = ctx.id.clone();
        ctx.writer.submit(move |c| {
            if let Err(e) = db::set_state(c, &id, state.as_str(), completed_at) {
                tracing::error!("persist download state: {e}");
            }
        });
    }
    ctx.jobs
        .lock()
        .expect("download jobs poisoned")
        .remove(&ctx.id);
    emit(&ctx.dispatcher, &ctx.id, state, received.get(), total.get());
}

/// Path-traversal guard used by shells before opening/revealing files:
/// the recorded destination must still be inside the download directory.
pub fn is_within_dir(dest: &Path, dir: &Path) -> bool {
    dest.parent() == Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_accept_no_transitions() {
        use DownloadState::*;
        for from in [Completed, Cancelled] {
            for to in [Active, Paused, Completed, Interrupted, Cancelled] {
                assert!(!from.can_transition(to), "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn state_strings_round_trip() {
        use DownloadState::*;
        for s in [Active, Paused, Completed, Interrupted, Cancelled] {
            assert_eq!(DownloadState::parse(s.as_str()), Some(s));
        }
        assert_eq!(DownloadState::parse("bogus"), None);
    }

    #[test]
    fn expand_dir_handles_tilde() {
        let p = expand_dir("~/Downloads");
        assert!(!p.to_string_lossy().starts_with('~'));
        assert_eq!(expand_dir("/tmp/x"), PathBuf::from("/tmp/x"));
    }
}
