//! Download + transcribe queue: one episode at a time, independent of the UI.
//!
//! Whisper already uses every CPU core, so jobs run strictly in order. A later
//! change could prefetch the next download during a transcription; that is
//! left out for now because cancel, the child-process slot, and the progress
//! bar are all global.

use crate::config::Conf;
use crate::store::{self, State};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One episode in the queue, identified by its show and guid.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId {
    pub show: u32,
    pub guid: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Downloading,
    Transcribing,
    Done,
    Failed(String),
    Cancelled,
}

impl JobStatus {
    pub fn is_pending(&self) -> bool {
        matches!(
            self,
            JobStatus::Queued | JobStatus::Downloading | JobStatus::Transcribing
        )
    }

    pub fn is_active(&self) -> bool {
        matches!(self, JobStatus::Downloading | JobStatus::Transcribing)
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            JobStatus::Done | JobStatus::Failed(_) | JobStatus::Cancelled
        )
    }

    /// Tag drawn in `[.....]` in a list, if this status should override the
    /// episode's own state tag. Finished jobs fall back to the episode state.
    pub fn list_tag(&self) -> Option<&'static str> {
        match self {
            JobStatus::Queued => Some("QUEUED"),
            JobStatus::Downloading => Some("FETCH"),
            JobStatus::Transcribing => Some("TRANS"),
            JobStatus::Failed(_) => Some("FAIL "),
            JobStatus::Done | JobStatus::Cancelled => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub title: String,
    pub show_title: String,
    pub status: JobStatus,
}

impl Job {
    /// One line for a queue list: `[TRANS] Show — Episode`.
    pub fn list_label(&self) -> String {
        let tag = self.status.list_tag().unwrap_or(match self.status {
            JobStatus::Done => "DONE ",
            JobStatus::Cancelled => "CANCL",
            _ => "     ",
        });
        if self.show_title.is_empty() {
            format!("[{tag}] {}", self.title)
        } else {
            format!("[{tag}] {} — {}", self.show_title, self.title)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddError {
    AlreadyQueued,
    InProgress,
    NotEligible,
}

impl AddError {
    pub fn as_str(self) -> &'static str {
        match self {
            AddError::AlreadyQueued => "already in the queue",
            AddError::InProgress => "already being processed",
            AddError::NotEligible => "already transcribed",
        }
    }
}

/// How the queue is shown in the window. Picked at startup from the
/// `PODCAST_TRIM_QUEUE_UI` environment variable or `[secret] queue-ui` in
/// configuration.conf (`inline`, `tab`, or `strip`). Default is inline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueUi {
    #[default]
    Inline,
    Tab,
    Strip,
}

impl QueueUi {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "inline" | "a" => Some(QueueUi::Inline),
            "tab" | "b" => Some(QueueUi::Tab),
            "strip" | "c" => Some(QueueUi::Strip),
            _ => None,
        }
    }

    pub fn from_env_and_conf(conf: &Conf) -> Self {
        if let Ok(v) = std::env::var("PODCAST_TRIM_QUEUE_UI") {
            if let Some(u) = Self::parse(&v) {
                return u;
            }
        }
        conf.get("secret", "queue-ui")
            .and_then(|v| Self::parse(&v))
            .unwrap_or(QueueUi::Inline)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            QueueUi::Inline => "inline",
            QueueUi::Tab => "tab",
            QueueUi::Strip => "strip",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    jobs: Vec<Job>,
}

impl Queue {
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn get(&self, id: &JobId) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == *id)
    }

    pub fn can_enqueue(state: State) -> bool {
        matches!(state, State::New | State::Downloaded)
    }

    /// Drop finished jobs when nothing is still pending, so a later add starts
    /// a fresh "1 of N" batch.
    fn start_new_batch_if_idle(&mut self) {
        if self.jobs.iter().all(|j| j.status.is_finished()) {
            self.jobs.clear();
        }
    }

    /// Add a new job, or re-queue a Failed or Cancelled one.
    pub fn add(
        &mut self,
        id: JobId,
        title: String,
        show_title: String,
        state: State,
    ) -> Result<(), AddError> {
        if !Self::can_enqueue(state) {
            return Err(AddError::NotEligible);
        }
        if let Some(j) = self.jobs.iter().find(|j| j.id == id) {
            match j.status {
                JobStatus::Queued => return Err(AddError::AlreadyQueued),
                JobStatus::Downloading | JobStatus::Transcribing => {
                    return Err(AddError::InProgress)
                }
                JobStatus::Done => return Err(AddError::NotEligible),
                JobStatus::Failed(_) | JobStatus::Cancelled => {}
            }
        }
        self.start_new_batch_if_idle();
        if let Some(i) = self.jobs.iter().position(|j| j.id == id) {
            // Still present, so the batch was not cleared: re-queue at the end.
            let mut job = self.jobs.remove(i);
            job.title = title;
            job.show_title = show_title;
            job.status = JobStatus::Queued;
            self.jobs.push(job);
            return Ok(());
        }
        self.jobs.push(Job {
            id,
            title,
            show_title,
            status: JobStatus::Queued,
        });
        Ok(())
    }

    /// Add each eligible episode. Returns (added, skipped).
    pub fn add_many<I>(&mut self, items: I) -> (usize, usize)
    where
        I: IntoIterator<Item = (JobId, String, String, State)>,
    {
        let mut added = 0;
        let mut skipped = 0;
        for (id, title, show_title, state) in items {
            match self.add(id, title, show_title, state) {
                Ok(()) => added += 1,
                Err(_) => skipped += 1,
            }
        }
        (added, skipped)
    }

    /// Remove a job that is not currently running. Returns whether it was removed.
    pub fn remove(&mut self, id: &JobId) -> bool {
        match self.jobs.iter().position(|j| j.id == *id) {
            Some(i) if !self.jobs[i].status.is_active() => {
                self.jobs.remove(i);
                true
            }
            _ => false,
        }
    }

    /// Drop every job that is not running. The active job, if any, is kept.
    pub fn clear_inactive(&mut self) {
        self.jobs.retain(|j| j.status.is_active());
    }

    /// Move a queued job up or down among the other queued jobs. `dir` is -1
    /// (up) or +1 (down). The active and finished jobs stay where they are.
    pub fn move_queued(&mut self, id: &JobId, dir: i32) -> bool {
        if dir != -1 && dir != 1 {
            return false;
        }
        let Some(i) = self.jobs.iter().position(|j| j.id == *id) else {
            return false;
        };
        if self.jobs[i].status != JobStatus::Queued {
            return false;
        }
        let j = i as i32 + dir;
        if j < 0 || j >= self.jobs.len() as i32 {
            return false;
        }
        let j = j as usize;
        if self.jobs[j].status != JobStatus::Queued {
            return false;
        }
        self.jobs.swap(i, j);
        true
    }

    pub fn active(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| j.status.is_active())
    }

    pub fn pending_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.status.is_pending()).count()
    }

    pub fn queued_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.status == JobStatus::Queued)
            .count()
    }

    pub fn is_idle(&self) -> bool {
        !self.jobs.iter().any(|j| j.status.is_pending())
    }

    /// 1-based position of the active (or first queued) job in this batch,
    /// and the batch length, for "Queue 2 of 5".
    pub fn progress_position(&self) -> Option<(usize, usize)> {
        let total = self.jobs.len();
        if total == 0 {
            return None;
        }
        let idx = self
            .jobs
            .iter()
            .position(|j| j.status.is_active() || j.status == JobStatus::Queued)?;
        Some((idx + 1, total))
    }

    /// Mark the first queued job as Downloading and return it. None if a job
    /// is already running or nothing is waiting.
    pub fn begin_next(&mut self) -> Option<&Job> {
        if self.jobs.iter().any(|j| j.status.is_active()) {
            return None;
        }
        let i = self
            .jobs
            .iter()
            .position(|j| j.status == JobStatus::Queued)?;
        self.jobs[i].status = JobStatus::Downloading;
        Some(&self.jobs[i])
    }

    pub fn set_status(&mut self, id: &JobId, status: JobStatus) -> bool {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == *id) {
            j.status = status;
            true
        } else {
            false
        }
    }

    /// Mark the running job Cancelled and leave the rest Queued.
    pub fn cancel_active(&mut self) {
        for j in &mut self.jobs {
            if j.status.is_active() {
                j.status = JobStatus::Cancelled;
            }
        }
    }

    /// Mark the running job and every still-queued job Cancelled.
    pub fn cancel_all_pending(&mut self) {
        for j in &mut self.jobs {
            if j.status.is_pending() {
                j.status = JobStatus::Cancelled;
            }
        }
    }

    pub fn done_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.status == JobStatus::Done)
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Failed(_)))
            .count()
    }

    /// Status line while a queue job is running, e.g.
    /// `Queue 2 of 5: Episode title — transcribing 35%`.
    pub fn status_line(&self, stage: &str, percent: u32) -> Option<String> {
        let job = self.active()?;
        let (n, total) = self.progress_position()?;
        let stage = stage.trim().trim_end_matches('.').to_ascii_lowercase();
        Some(format!(
            "Queue {n} of {total}: {} — {stage} {percent}%",
            job.title
        ))
    }

    /// Pending jobs only, with any in-progress job stored as Queued so it can
    /// be started again after a restart.
    pub fn snapshot_pending(&self) -> Queue {
        Queue {
            jobs: self
                .jobs
                .iter()
                .filter(|j| j.status.is_pending())
                .map(|j| {
                    let mut j = j.clone();
                    j.status = JobStatus::Queued;
                    j
                })
                .collect(),
        }
    }

    pub fn load(path: &Path) -> Queue {
        store::load_json::<Queue>(path).snapshot_pending()
    }

    /// Load jobs as saved, including in-progress statuses. For screenshots
    /// and tests; normal startup uses `load`.
    pub fn load_as_saved(path: &Path) -> Queue {
        store::load_json(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        store::save_json(path, &self.snapshot_pending())
    }

    /// Write the live jobs (including in-progress) — used by tests and the
    /// preview helper, not by the running app.
    pub fn save_as_is(&self, path: &Path) -> Result<(), String> {
        store::save_json(path, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn id(show: u32, g: &str) -> JobId {
        JobId {
            show,
            guid: g.into(),
        }
    }

    fn add(q: &mut Queue, show: u32, g: &str, title: &str, state: State) -> Result<(), AddError> {
        q.add(id(show, g), title.into(), "Show".into(), state)
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("pt-queue-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn add_skips_duplicates_and_ineligible() {
        let mut q = Queue::default();
        assert!(add(&mut q, 1, "a", "A", State::New).is_ok());
        assert_eq!(
            add(&mut q, 1, "a", "A", State::New),
            Err(AddError::AlreadyQueued)
        );
        assert_eq!(
            add(&mut q, 1, "b", "B", State::Transcribed),
            Err(AddError::NotEligible)
        );
        assert_eq!(
            add(&mut q, 1, "c", "C", State::Ready),
            Err(AddError::NotEligible)
        );
        assert_eq!(
            add(&mut q, 1, "d", "D", State::Delivered),
            Err(AddError::NotEligible)
        );
        assert!(add(&mut q, 1, "e", "E", State::Downloaded).is_ok());
        assert_eq!(q.jobs().len(), 2);
        assert_eq!(q.queued_count(), 2);
    }

    #[test]
    fn add_many_and_dedup() {
        let mut q = Queue::default();
        let (added, skipped) = q.add_many([
            (id(1, "a"), "A".into(), "S".into(), State::New),
            (id(1, "a"), "A".into(), "S".into(), State::New),
            (id(1, "b"), "B".into(), "S".into(), State::Transcribed),
            (id(2, "c"), "C".into(), "T".into(), State::Downloaded),
        ]);
        assert_eq!((added, skipped), (2, 2));
        assert_eq!(q.jobs()[0].id.guid, "a");
        assert_eq!(q.jobs()[1].id.show, 2);
    }

    #[test]
    fn begin_next_one_at_a_time() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "A", State::New).unwrap();
        add(&mut q, 1, "b", "B", State::New).unwrap();
        let first = q.begin_next().unwrap().id.guid.clone();
        assert_eq!(first, "a");
        assert_eq!(q.active().unwrap().status, JobStatus::Downloading);
        assert!(q.begin_next().is_none(), "must not start a second job");
        q.set_status(&id(1, "a"), JobStatus::Transcribing);
        assert_eq!(q.active().unwrap().status, JobStatus::Transcribing);
        assert!(q.begin_next().is_none());
        q.set_status(&id(1, "a"), JobStatus::Done);
        assert!(!q.is_idle());
        let second = q.begin_next().unwrap().id.guid.clone();
        assert_eq!(second, "b");
        q.set_status(&id(1, "b"), JobStatus::Failed("whisper died".into()));
        assert!(q.is_idle());
        assert_eq!(q.done_count(), 1);
        assert_eq!(q.failed_count(), 1);
        assert_eq!(q.progress_position(), None);
    }

    #[test]
    fn progress_position_and_status_line() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "Alpha", State::New).unwrap();
        add(&mut q, 1, "b", "Beta long title", State::New).unwrap();
        add(&mut q, 1, "c", "Gamma", State::New).unwrap();
        assert_eq!(q.progress_position(), Some((1, 3)));
        q.begin_next();
        q.set_status(&id(1, "a"), JobStatus::Done);
        q.begin_next();
        assert_eq!(q.progress_position(), Some((2, 3)));
        let line = q.status_line("Transcribing", 35).unwrap();
        assert_eq!(line, "Queue 2 of 3: Beta long title — transcribing 35%");
        q.set_status(&id(1, "b"), JobStatus::Done);
        assert!(q.status_line("Transcribing", 10).is_none());
    }

    #[test]
    fn continue_after_failure_and_cancel() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "A", State::New).unwrap();
        add(&mut q, 1, "b", "B", State::New).unwrap();
        add(&mut q, 1, "c", "C", State::New).unwrap();
        q.begin_next();
        q.set_status(&id(1, "a"), JobStatus::Failed("network".into()));
        assert_eq!(q.begin_next().unwrap().id.guid, "b");
        q.cancel_all_pending();
        assert!(q.jobs().iter().all(|j| j.status.is_finished()));
        assert_eq!(q.jobs()[0].status, JobStatus::Failed("network".into()));
        assert_eq!(q.jobs()[1].status, JobStatus::Cancelled);
        assert_eq!(q.jobs()[2].status, JobStatus::Cancelled);
        assert!(q.is_idle());
    }

    #[test]
    fn cancel_active_leaves_the_rest_queued() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "A", State::New).unwrap();
        add(&mut q, 1, "b", "B", State::New).unwrap();
        q.begin_next();
        q.cancel_active();
        assert_eq!(q.jobs()[0].status, JobStatus::Cancelled);
        assert_eq!(q.jobs()[1].status, JobStatus::Queued);
        assert_eq!(q.pending_count(), 1);
        assert_eq!(q.begin_next().unwrap().id.guid, "b");
    }

    #[test]
    fn remove_reorder_clear() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "A", State::New).unwrap();
        add(&mut q, 1, "b", "B", State::New).unwrap();
        add(&mut q, 1, "c", "C", State::New).unwrap();
        q.begin_next();
        assert!(!q.remove(&id(1, "a")), "cannot remove the running job");
        assert!(q.remove(&id(1, "b")));
        assert_eq!(
            q.jobs()
                .iter()
                .map(|j| j.id.guid.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        add(&mut q, 1, "d", "D", State::New).unwrap();
        add(&mut q, 1, "e", "E", State::New).unwrap();
        // order: a (active), c, d, e
        assert!(q.move_queued(&id(1, "e"), -1));
        assert_eq!(q.jobs()[2].id.guid, "e");
        assert_eq!(q.jobs()[3].id.guid, "d");
        assert!(
            !q.move_queued(&id(1, "c"), -1),
            "cannot move past the active job"
        );
        assert!(!q.move_queued(&id(1, "a"), 1), "cannot move the active job");
        q.clear_inactive();
        assert_eq!(q.jobs().len(), 1);
        assert_eq!(q.jobs()[0].id.guid, "a");
    }

    #[test]
    fn requeue_failed_and_new_batch_when_idle() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "A", State::New).unwrap();
        q.begin_next();
        q.set_status(&id(1, "a"), JobStatus::Failed("x".into()));
        assert!(q
            .add(id(1, "a"), "A".into(), "Show".into(), State::Downloaded)
            .is_ok());
        assert_eq!(q.jobs().len(), 1, "idle finished batch is cleared first");
        assert_eq!(q.jobs()[0].status, JobStatus::Queued);

        // A finished batch plus a new episode starts fresh.
        q.set_status(&id(1, "a"), JobStatus::Done);
        assert!(add(&mut q, 1, "b", "B", State::New).is_ok());
        assert_eq!(q.jobs().len(), 1);
        assert_eq!(q.jobs()[0].id.guid, "b");
        assert_eq!(q.progress_position(), Some((1, 1)));
    }

    #[test]
    fn persist_only_pending_and_reset_active() {
        let dir = tmp("save");
        let path = dir.join("queue.json");
        let mut q = Queue::default();
        add(&mut q, 1, "a", "A", State::New).unwrap();
        add(&mut q, 1, "b", "B", State::New).unwrap();
        add(&mut q, 1, "c", "C", State::New).unwrap();
        q.begin_next();
        q.set_status(&id(1, "a"), JobStatus::Transcribing);
        q.set_status(&id(1, "c"), JobStatus::Failed("nope".into()));
        q.save(&path).unwrap();
        let back = Queue::load(&path);
        assert_eq!(back.jobs().len(), 2, "failed job is not pending");
        assert!(back.jobs().iter().all(|j| j.status == JobStatus::Queued));
        assert_eq!(back.jobs()[0].id.guid, "a");
        assert_eq!(back.jobs()[1].id.guid, "b");
        // As-saved keeps the in-progress status for a preview.
        q.save_as_is(&path).unwrap();
        let live = Queue::load_as_saved(&path);
        assert_eq!(live.jobs()[0].status, JobStatus::Transcribing);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn list_labels_and_tags() {
        let mut q = Queue::default();
        add(&mut q, 1, "a", "Bees", State::New).unwrap();
        q.begin_next();
        q.set_status(&id(1, "a"), JobStatus::Transcribing);
        assert_eq!(q.jobs()[0].list_label(), "[TRANS] Show — Bees");
        assert_eq!(JobStatus::Queued.list_tag(), Some("QUEUED"));
        assert_eq!(JobStatus::Done.list_tag(), None);
        assert!(Queue::can_enqueue(State::New));
        assert!(!Queue::can_enqueue(State::Ready));
    }

    #[test]
    fn queue_ui_parse() {
        assert_eq!(QueueUi::parse("Tab"), Some(QueueUi::Tab));
        assert_eq!(QueueUi::parse("c"), Some(QueueUi::Strip));
        assert_eq!(QueueUi::parse("inline"), Some(QueueUi::Inline));
        assert_eq!(QueueUi::parse("nope"), None);
        let c = Conf::parse("[secret]\nqueue-ui = strip\n");
        std::env::remove_var("PODCAST_TRIM_QUEUE_UI");
        assert_eq!(QueueUi::from_env_and_conf(&c), QueueUi::Strip);
        assert_eq!(
            QueueUi::from_env_and_conf(&Conf::parse("")),
            QueueUi::Inline
        );
    }
}
