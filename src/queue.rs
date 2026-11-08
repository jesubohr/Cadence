//! Download queue: job model, scheduling, cancel, retry and persistence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::media::{Entry, Format};
use crate::paths;
use crate::process::Cancel;
use crate::storage;

/// Downloads that run at the same time.
pub const MAX_PARALLEL: usize = 2;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Queued,
    Downloading,
    Converting,
    Completed,
    Failed,
    Cancelled,
}

impl State {
    pub fn is_active(self) -> bool {
        matches!(self, State::Downloading | State::Converting)
    }

    pub fn is_finished(self) -> bool {
        matches!(self, State::Completed | State::Failed | State::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Progress {
    /// 0.0 to 1.0
    pub fraction: f32,
    /// Bytes per second.
    pub speed: Option<f64>,
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub entry: Entry,
    pub format: Format,
    pub dir: PathBuf,
    pub state: State,
    #[serde(skip)]
    pub progress: Progress,
    /// Final file path, chosen when the job starts.
    pub target: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub error: Option<String>,
}

/// A state report from a running download.
#[derive(Debug, Clone, Copy)]
pub enum Update {
    Downloading(Progress),
    Converting,
}

/// Executes one job. The real implementation starts yt-dlp.
pub trait Runner: Send + Sync + 'static {
    fn run(
        &self,
        job: &Job,
        cancel: &Cancel,
        report: &mut dyn FnMut(Update),
    ) -> Result<PathBuf, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueError {
    Duplicate,
}

struct Inner {
    jobs: Vec<Job>,
    next_id: u64,
    running: HashMap<u64, Cancel>,
}

/// Cheap to clone. All clones share one queue.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<Mutex<Inner>>,
    runner: Arc<dyn Runner>,
    /// Wakes the UI. The channel holds one message, so bursts coalesce.
    changed: async_channel::Sender<()>,
    saver: mpsc::Sender<Vec<Job>>,
    store: PathBuf,
}

impl Manager {
    /// Loads the saved queue from `store` and starts the pending jobs.
    pub fn new(
        runner: Arc<dyn Runner>,
        store: PathBuf,
        changed: async_channel::Sender<()>,
    ) -> Self {
        let mut jobs: Vec<Job> = storage::load_json(&store);
        for job in &mut jobs {
            // A job that was running when the app closed starts again.
            if job.state.is_active() {
                job.state = State::Queued;
            }
        }
        let next_id = jobs.iter().map(|j| j.id).max().map_or(1, |id| id + 1);

        let (saver, saved) = mpsc::channel::<Vec<Job>>();
        let writer_store = store.clone();
        thread::spawn(move || {
            while let Ok(mut snapshot) = saved.recv() {
                // Only the newest snapshot matters.
                while let Ok(newer) = saved.try_recv() {
                    snapshot = newer;
                }
                if let Err(err) = storage::save_json(&writer_store, &snapshot) {
                    eprintln!("could not save queue: {err}");
                }
            }
        });

        let manager = Self {
            inner: Arc::new(Mutex::new(Inner {
                jobs,
                next_id,
                running: HashMap::new(),
            })),
            runner,
            changed,
            saver,
            store,
        };
        manager.with_inner(|inner| manager.pump(inner));
        manager
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("queue lock is never poisoned")
    }

    /// Runs `f` on the locked queue, then saves and wakes the UI.
    fn with_inner<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut inner = self.lock();
        let result = f(&mut inner);
        let _ = self.saver.send(inner.jobs.clone());
        drop(inner);
        self.notify();
        result
    }

    fn notify(&self) {
        // A full channel already holds a pending wake-up.
        let _ = self.changed.try_send(());
    }

    pub fn snapshot(&self) -> Vec<Job> {
        self.lock().jobs.clone()
    }

    /// Adds a job unless the same item is already queued, running or done.
    pub fn enqueue(&self, entry: Entry, format: Format, dir: PathBuf) -> Result<u64, EnqueueError> {
        self.with_inner(|inner| {
            let duplicate = inner.jobs.iter().any(|job| {
                job.entry.key() == entry.key()
                    && job.format == format
                    && !matches!(job.state, State::Failed | State::Cancelled)
            });
            if duplicate {
                return Err(EnqueueError::Duplicate);
            }
            let id = inner.next_id;
            inner.next_id += 1;
            inner.jobs.push(Job {
                id,
                entry,
                format,
                dir,
                state: State::Queued,
                progress: Progress::default(),
                target: None,
                output: None,
                error: None,
            });
            self.pump(inner);
            Ok(id)
        })
    }

    /// Stops a running job, or drops a waiting one.
    pub fn cancel(&self, id: u64) {
        self.with_inner(|inner| {
            if let Some(cancel) = inner.running.get(&id) {
                // The worker thread records the final state.
                cancel.cancel();
            } else if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
                if job.state == State::Queued {
                    job.state = State::Cancelled;
                }
            }
        });
    }

    /// Queues a failed or cancelled job again.
    pub fn retry(&self, id: u64) {
        self.with_inner(|inner| {
            let duplicate = inner.jobs.iter().find(|j| j.id == id).is_some_and(|job| {
                inner.jobs.iter().any(|other| {
                    other.id != id
                        && other.entry.key() == job.entry.key()
                        && other.format == job.format
                        && !matches!(other.state, State::Failed | State::Cancelled)
                })
            });
            if duplicate {
                return;
            }
            if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
                if matches!(job.state, State::Failed | State::Cancelled) {
                    job.state = State::Queued;
                    job.error = None;
                    job.progress = Progress::default();
                    job.target = None;
                }
            }
            self.pump(inner);
        });
    }

    /// Removes a finished job from the list. The downloaded file stays.
    pub fn remove(&self, id: u64) {
        self.with_inner(|inner| inner.jobs.retain(|j| j.id != id || !j.state.is_finished()));
    }

    pub fn clear_finished(&self) {
        self.with_inner(|inner| inner.jobs.retain(|j| !j.state.is_finished()));
    }

    /// Stops all running downloads. They return to the queue on next start.
    pub fn shutdown(&self) {
        let inner = self.lock();
        for cancel in inner.running.values() {
            cancel.cancel();
        }
        let mut jobs = inner.jobs.clone();
        drop(inner);
        for job in &mut jobs {
            if job.state.is_active() {
                job.state = State::Queued;
            }
        }
        // Written on this thread so the file exists before the process exits.
        let _ = storage::save_json(&self.store, &jobs);
    }

    /// Starts waiting jobs until the parallel limit is reached.
    fn pump(&self, inner: &mut Inner) {
        while inner.running.len() < MAX_PARALLEL {
            let Some(index) = inner.jobs.iter().position(|j| j.state == State::Queued) else {
                return;
            };
            let target = plan_target(&inner.jobs, index);
            let job = &mut inner.jobs[index];
            job.state = State::Downloading;
            job.progress = Progress::default();
            job.target = Some(target);
            let snapshot = job.clone();

            let cancel = Cancel::default();
            inner.running.insert(snapshot.id, cancel.clone());
            let manager = self.clone();
            thread::spawn(move || manager.work(snapshot, cancel));
        }
    }

    fn work(&self, job: Job, cancel: Cancel) {
        let id = job.id;
        let mut last_report: Option<Instant> = None;
        let mut report = |update: Update| {
            let is_progress = matches!(update, Update::Downloading(_));
            if is_progress && last_report.is_some_and(|t| t.elapsed() < PROGRESS_INTERVAL) {
                return;
            }
            last_report = Some(Instant::now());
            let mut inner = self.lock();
            let state_changed = inner
                .jobs
                .iter_mut()
                .find(|j| j.id == id)
                .is_some_and(|job| {
                    match update {
                        Update::Downloading(progress) => job.progress = progress,
                        Update::Converting if job.state == State::Downloading => {
                            job.state = State::Converting;
                            job.progress.fraction = 1.0;
                            job.progress.speed = None;
                            job.progress.eta_secs = None;
                            return true;
                        }
                        Update::Converting => {}
                    }
                    false
                });
            if state_changed {
                let _ = self.saver.send(inner.jobs.clone());
            }
            drop(inner);
            self.notify();
        };

        let result = self.runner.run(&job, &cancel, &mut report);

        self.with_inner(|inner| {
            inner.running.remove(&id);
            if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
                match result {
                    Ok(path) => {
                        job.state = State::Completed;
                        job.progress.fraction = 1.0;
                        job.output = Some(path);
                        job.error = None;
                    }
                    Err(_) if cancel.is_cancelled() => job.state = State::Cancelled,
                    Err(message) => {
                        job.state = State::Failed;
                        job.error = Some(message);
                    }
                }
            }
            self.pump(inner);
        });
    }
}

/// Picks the final path for the job at `index`. The path must not exist on
/// disk and must not belong to another job that is waiting or running.
fn plan_target(jobs: &[Job], index: usize) -> PathBuf {
    let job = &jobs[index];
    let reserved: Vec<&Path> = jobs
        .iter()
        .enumerate()
        .filter(|(i, other)| {
            *i != index && (other.state.is_active() || other.state == State::Completed)
        })
        .filter_map(|(_, other)| other.target.as_deref())
        .collect();
    let stem = paths::sanitize(&job.entry.title, &job.entry.id);
    paths::unique_path(&job.dir, &stem, job.format.extension(), |path| {
        path.exists() || reserved.contains(&path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Condvar;

    /// Fake runner. Each job waits until the test releases it.
    #[derive(Default)]
    struct Gate {
        released: Mutex<Vec<u64>>,
        wake: Condvar,
        started: Mutex<Vec<u64>>,
        concurrent: AtomicUsize,
        peak: AtomicUsize,
        fail: Mutex<Vec<u64>>,
    }

    impl Gate {
        fn release(&self, id: u64) {
            self.released.lock().unwrap().push(id);
            self.wake.notify_all();
        }
    }

    impl Runner for Arc<Gate> {
        fn run(
            &self,
            job: &Job,
            cancel: &Cancel,
            report: &mut dyn FnMut(Update),
        ) -> Result<PathBuf, String> {
            let now = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            self.started.lock().unwrap().push(job.id);
            report(Update::Downloading(Progress {
                fraction: 0.5,
                speed: None,
                eta_secs: None,
            }));
            let mut released = self.released.lock().unwrap();
            while !released.contains(&job.id) && !cancel.is_cancelled() {
                released = self
                    .wake
                    .wait_timeout(released, Duration::from_millis(20))
                    .unwrap()
                    .0;
            }
            self.concurrent.fetch_sub(1, Ordering::SeqCst);
            if cancel.is_cancelled() {
                return Err("Cancelled".into());
            }
            if self.fail.lock().unwrap().contains(&job.id) {
                return Err("boom".into());
            }
            Ok(job.target.clone().unwrap())
        }
    }

    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            title: format!("Title {id}"),
            url: format!("https://example.com/{id}"),
            channel: None,
            duration_secs: None,
            thumbnail: None,
        }
    }

    const MP3: Format = Format::Mp3 { kbps: 192 };

    struct Fixture {
        manager: Manager,
        gate: Arc<Gate>,
        dir: tempfile::TempDir,
        _events: async_channel::Receiver<()>,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(Gate::default());
        let (tx, rx) = async_channel::bounded(1);
        let manager = Manager::new(Arc::new(gate.clone()), dir.path().join("queue.json"), tx);
        Fixture {
            manager,
            gate,
            dir,
            _events: rx,
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn state_of(manager: &Manager, id: u64) -> State {
        manager
            .snapshot()
            .iter()
            .find(|j| j.id == id)
            .unwrap()
            .state
    }

    #[test]
    fn runs_two_at_a_time_and_starts_the_next_when_one_ends() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        let ids: Vec<u64> = ["a", "b", "c"]
            .iter()
            .map(|n| f.manager.enqueue(entry(n), MP3, dir.clone()).unwrap())
            .collect();

        wait_until(|| f.gate.started.lock().unwrap().len() == 2);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(f.gate.started.lock().unwrap().len(), 2);
        assert_eq!(state_of(&f.manager, ids[2]), State::Queued);

        f.gate.release(ids[0]);
        wait_until(|| state_of(&f.manager, ids[0]) == State::Completed);
        wait_until(|| state_of(&f.manager, ids[2]) == State::Downloading);

        f.gate.release(ids[1]);
        f.gate.release(ids[2]);
        wait_until(|| {
            f.manager
                .snapshot()
                .iter()
                .all(|j| j.state == State::Completed)
        });
        assert_eq!(f.gate.peak.load(Ordering::SeqCst), MAX_PARALLEL);
    }

    #[test]
    fn rejects_duplicates_but_allows_other_formats() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        f.manager.enqueue(entry("a"), MP3, dir.clone()).unwrap();
        assert_eq!(
            f.manager.enqueue(entry("a"), MP3, dir.clone()),
            Err(EnqueueError::Duplicate)
        );
        assert!(f
            .manager
            .enqueue(entry("a"), Format::Mp3 { kbps: 320 }, dir.clone())
            .is_ok());
        assert!(f
            .manager
            .enqueue(entry("a"), Format::Video { max_height: None }, dir)
            .is_ok());
    }

    #[test]
    fn cancel_then_retry() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        let id = f.manager.enqueue(entry("a"), MP3, dir.clone()).unwrap();
        wait_until(|| state_of(&f.manager, id) == State::Downloading);

        f.manager.cancel(id);
        wait_until(|| state_of(&f.manager, id) == State::Cancelled);

        // A cancelled job does not block adding the same item again.
        f.manager.retry(id);
        wait_until(|| state_of(&f.manager, id) == State::Downloading);
        f.gate.release(id);
        wait_until(|| state_of(&f.manager, id) == State::Completed);
    }

    #[test]
    fn cancel_of_a_waiting_job_never_starts_it() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        let ids: Vec<u64> = ["a", "b", "c"]
            .iter()
            .map(|n| f.manager.enqueue(entry(n), MP3, dir.clone()).unwrap())
            .collect();
        wait_until(|| f.gate.started.lock().unwrap().len() == 2);
        f.manager.cancel(ids[2]);
        assert_eq!(state_of(&f.manager, ids[2]), State::Cancelled);
        f.gate.release(ids[0]);
        f.gate.release(ids[1]);
        wait_until(|| state_of(&f.manager, ids[1]) == State::Completed);
        assert!(!f.gate.started.lock().unwrap().contains(&ids[2]));
    }

    #[test]
    fn failures_keep_the_message_and_can_retry() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        let id = f.manager.enqueue(entry("a"), MP3, dir).unwrap();
        f.gate.fail.lock().unwrap().push(id);
        f.gate.release(id);
        wait_until(|| state_of(&f.manager, id) == State::Failed);
        let job = f.manager.snapshot().remove(0);
        assert_eq!(job.error.as_deref(), Some("boom"));

        f.gate.fail.lock().unwrap().clear();
        f.manager.retry(id);
        wait_until(|| state_of(&f.manager, id) == State::Completed);
        assert!(f.manager.snapshot()[0].error.is_none());
    }

    #[test]
    fn colliding_titles_get_distinct_targets() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        // Same title, different videos. One file already exists on disk.
        std::fs::write(dir.join("Same.mp3"), b"x").unwrap();
        let mut a = entry("a");
        a.title = "Same".into();
        let mut b = entry("b");
        b.title = "Same".into();
        f.manager.enqueue(a, MP3, dir.clone()).unwrap();
        f.manager.enqueue(b, MP3, dir.clone()).unwrap();
        wait_until(|| f.gate.started.lock().unwrap().len() == 2);
        let targets: Vec<PathBuf> = f
            .manager
            .snapshot()
            .iter()
            .map(|j| j.target.clone().unwrap())
            .collect();
        assert_eq!(targets[0], dir.join("Same (1).mp3"));
        assert_eq!(targets[1], dir.join("Same (2).mp3"));
    }

    #[test]
    fn clear_finished_keeps_active_jobs() {
        let f = fixture();
        let dir = f.dir.path().to_path_buf();
        let a = f.manager.enqueue(entry("a"), MP3, dir.clone()).unwrap();
        let b = f.manager.enqueue(entry("b"), MP3, dir).unwrap();
        wait_until(|| f.gate.started.lock().unwrap().len() == 2);
        f.gate.release(a);
        wait_until(|| state_of(&f.manager, a) == State::Completed);
        f.manager.clear_finished();
        let ids: Vec<u64> = f.manager.snapshot().iter().map(|j| j.id).collect();
        assert_eq!(ids, vec![b]);
        f.manager.remove(b); // still running, so it stays
        assert_eq!(f.manager.snapshot().len(), 1);
        f.gate.release(b);
    }

    #[test]
    fn queue_survives_restart_and_resumes_active_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("queue.json");
        let jobs = vec![
            Job {
                id: 4,
                entry: entry("a"),
                format: MP3,
                dir: dir.path().to_path_buf(),
                state: State::Converting,
                progress: Progress::default(),
                target: None,
                output: None,
                error: None,
            },
            Job {
                id: 5,
                entry: entry("b"),
                format: MP3,
                dir: dir.path().to_path_buf(),
                state: State::Completed,
                progress: Progress::default(),
                target: None,
                output: Some("x.mp3".into()),
                error: None,
            },
        ];
        storage::save_json(&store, &jobs).unwrap();

        let gate = Arc::new(Gate::default());
        let (tx, _rx) = async_channel::bounded(1);
        let manager = Manager::new(Arc::new(gate.clone()), store, tx);
        // The interrupted job starts again and the finished one stays finished.
        wait_until(|| state_of(&manager, 4) == State::Downloading);
        assert_eq!(state_of(&manager, 5), State::Completed);
        let next = manager
            .enqueue(entry("c"), MP3, dir.path().to_path_buf())
            .unwrap();
        assert_eq!(next, 6);
        gate.release(4);
        gate.release(6);
    }
}
