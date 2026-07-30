//! The native `TaskService`: one `std::thread` per task, keyed by task kind,
//! cancel-on-resubmit, and an optional debounce so rapid resubmissions
//! (key-repeat deletes) only run the last request.
//!
//! Each kind gets its own slot, so the waveform render that follows every edit
//! never cancels a WAV export (or the other way about); only a resubmission of
//! the *same* kind supersedes.
//!
//! Cancellation is cooperative (an `AtomicBool` the shared task runner checks
//! between render chunks), and results carry a generation number so a task
//! that slips past its cancel flag can never overwrite a newer task's result.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Instant;

use core::time::Duration;

use vgms_ui::{TaskKind, TaskRequest, TaskResult, TaskService, run_task};

#[derive(Debug)]
struct Pending {
    due: Instant,
    request: TaskRequest,
    generation: u64,
}

/// One task kind's state: at most one pending and one running task.
#[derive(Debug)]
struct Slot {
    /// The debounced submission waiting for its deadline, if any.
    pending: Option<Pending>,
    /// The cancel flag of the spawned task, if one is running.
    running: Option<Arc<AtomicBool>>,
    /// Spawned-and-not-yet-exited threads of this kind, for `is_busy_kind`.
    ///
    /// Replaced wholesale on cancel rather than decremented: a task that cannot
    /// stop early (a WAV render checks nothing) keeps running to completion, and
    /// it must not keep this kind looking busy after the user moved on. The
    /// orphan then counts down a counter nobody reads.
    live: Arc<AtomicUsize>,
    /// Bumped per submission *and per cancel*; stale results are dropped by
    /// generation.
    generation: u64,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            pending: None,
            running: None,
            live: Arc::new(AtomicUsize::new(0)),
            generation: 0,
        }
    }
}

impl Slot {
    fn is_busy(&self) -> bool {
        self.pending.is_some() || self.live.load(Ordering::Relaxed) > 0
    }

    /// Drops any pending submission, signals the running task to stop, and moves
    /// past both: the generation bump makes an already-queued result stale, and
    /// the fresh counter forgets any thread that ignores its cancel flag.
    fn cancel(&mut self) {
        self.pending = None;
        if let Some(cancelled) = self.running.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
        self.generation += 1;
        self.live = Arc::new(AtomicUsize::new(0));
    }
}

pub struct ThreadTaskService {
    sender: Sender<(TaskKind, u64, TaskResult)>,
    receiver: Receiver<(TaskKind, u64, TaskResult)>,
    /// One slot per kind, created on first use.
    slots: HashMap<TaskKind, Slot>,
    /// Called after a worker posts its result. The GUI passes
    /// `Context::request_repaint`, closing the race where a task finishes
    /// between a frame's poll and its `is_busy` check -- without this, the
    /// result would sit undelivered until the next input event.
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for ThreadTaskService {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            slots: HashMap::new(),
            notify: None,
        }
    }
}

impl fmt::Debug for ThreadTaskService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadTaskService")
            .field("slots", &self.slots)
            .finish_non_exhaustive()
    }
}

impl ThreadTaskService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// As [`Self::new`], with `notify` called whenever a result is posted.
    #[must_use]
    pub fn with_notifier(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Some(Arc::new(notify)),
            ..Self::default()
        }
    }

    fn spawn(&mut self, kind: TaskKind, request: TaskRequest, generation: u64) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let slot = self.slots.entry(kind).or_default();
        slot.running = Some(Arc::clone(&cancelled));

        let sender = self.sender.clone();
        let live = Arc::clone(&slot.live);
        let notify = self.notify.clone();
        live.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            // A task may emit several times (the waveform render streams
            // progressive snapshots); forward each to the channel, tagged with
            // this run's kind and generation so `poll` can drop a superseded
            // run's tail without touching another kind's.
            let is_cancelled = || cancelled.load(Ordering::Relaxed);
            run_task(&request, &is_cancelled, &mut |result| {
                if !cancelled.load(Ordering::Relaxed) {
                    // A closed channel just means the app shut down.
                    let _ = sender.send((kind, generation, result));
                    if let Some(notify) = &notify {
                        notify();
                    }
                }
            });
            live.fetch_sub(1, Ordering::Relaxed);
        });
    }

    /// Starts any debounced submission whose deadline has passed, across every
    /// kind. Driven by `poll`, which the app calls every frame (and it keeps
    /// frames coming while `is_busy`).
    fn promote_due(&mut self) {
        let now = Instant::now();
        let due: Vec<(TaskKind, Pending)> = self
            .slots
            .iter_mut()
            .filter_map(|(&kind, slot)| {
                slot.pending
                    .take_if(|pending| pending.due <= now)
                    .map(|pending| (kind, pending))
            })
            .collect();
        for (kind, pending) in due {
            self.spawn(kind, pending.request, pending.generation);
        }
    }
}

impl TaskService for ThreadTaskService {
    fn submit(&mut self, request: TaskRequest, debounce: Option<Duration>) {
        let kind = request.kind();
        let slot = self.slots.entry(kind).or_default();
        // Supersede this kind only: an export must survive the waveform re-render
        // that every edit queues behind it.
        slot.cancel();
        let generation = slot.generation;
        match debounce {
            Some(delay) => {
                slot.pending = Some(Pending {
                    due: Instant::now() + delay,
                    request,
                    generation,
                });
            }
            None => self.spawn(kind, request, generation),
        }
    }

    fn cancel(&mut self, kind: TaskKind) {
        if let Some(slot) = self.slots.get_mut(&kind) {
            slot.cancel();
        }
    }

    fn poll(&mut self) -> Vec<TaskResult> {
        self.promote_due();
        let mut results = Vec::new();
        while let Ok((kind, generation, result)) = self.receiver.try_recv() {
            // Only the run that is still current for its kind is delivered.
            if self
                .slots
                .get(&kind)
                .is_some_and(|s| s.generation == generation)
            {
                results.push(result);
            }
        }
        results
    }

    fn is_busy(&self) -> bool {
        self.slots.values().any(Slot::is_busy)
    }

    fn is_busy_kind(&self, kind: TaskKind) -> bool {
        self.slots.get(&kind).is_some_and(Slot::is_busy)
    }

    fn shutdown(&mut self) {
        for slot in self.slots.values_mut() {
            slot.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgms_core::{DroDataV1, OplType, Song};

    fn request(ms_length: u32) -> TaskRequest {
        // A trivially short song; the delay's length labels the request so a
        // test can tell whose result arrived.
        let song = Song::dro_v1(
            "task.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01, 0x00, (ms_length - 1) as u8]).unwrap(),
            ms_length,
            OplType::Opl2,
        );
        TaskRequest::RenderWaveform {
            source: vgms_synth::AudioSource::Opl(Arc::new(song)),
            num_buckets: 4,
            sample_rate: 48_000,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
        }
    }

    /// Polls until the run finishes, returning every result it produced.
    ///
    /// A render emits several progressive snapshots plus a final one, and a
    /// poll only drains what has arrived so far, so keep polling until the
    /// service goes idle rather than stopping at the first non-empty batch.
    fn drain_until_idle(service: &mut ThreadTaskService, timeout: Duration) -> Vec<TaskResult> {
        let deadline = Instant::now() + timeout;
        let mut all = Vec::new();
        loop {
            all.extend(service.poll());
            if (!service.is_busy() && !all.is_empty()) || Instant::now() > deadline {
                all.extend(service.poll());
                return all;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_until_idle(service: &ThreadTaskService, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while service.is_busy() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn an_undebounced_task_delivers_its_result() {
        let mut service = ThreadTaskService::new();
        service.submit(request(10), None);
        assert!(service.is_busy());
        let results = drain_until_idle(&mut service, Duration::from_secs(10));
        // The render streams progressive snapshots then the final buckets.
        assert!(!results.is_empty());
        assert!(!service.is_busy());
    }

    #[test]
    fn a_debounced_task_waits_out_its_delay() {
        let mut service = ThreadTaskService::new();
        service.submit(request(10), Some(Duration::from_millis(80)));
        // Not yet started -- polling well before the deadline yields nothing.
        assert!(service.poll().is_empty());
        assert!(service.is_busy(), "a pending task counts as busy");
        let results = drain_until_idle(&mut service, Duration::from_secs(10));
        assert!(!results.is_empty());
    }

    #[test]
    fn resubmitting_supersedes_the_pending_task() {
        let mut service = ThreadTaskService::new();
        // The first submission never starts: its debounce is far away.
        service.submit(request(10), Some(Duration::from_secs(600)));
        service.submit(request(20), None);
        // Only the replacement ran; poll filters to its generation, so anything
        // returned is the replacement's.
        let results = drain_until_idle(&mut service, Duration::from_secs(10));
        assert!(!results.is_empty(), "the replacement produced results");
        assert!(
            service.slots[&TaskKind::RenderWaveform].pending.is_none(),
            "the superseded submission is gone"
        );
    }

    #[test]
    fn stale_results_are_dropped_by_generation() {
        let mut service = ThreadTaskService::new();
        service.submit(request(10), None);
        // Give the first task time to finish and queue its result...
        wait_until_idle(&service, Duration::from_secs(10));
        // ...then supersede it before polling. The queued result is stale.
        service.submit(request(20), Some(Duration::from_secs(600)));
        assert!(service.poll().is_empty(), "the stale result is discarded");
        service.shutdown();
    }

    #[test]
    fn the_notifier_fires_when_a_result_is_posted() {
        let notified = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&notified);
        let mut service = ThreadTaskService::with_notifier(move || {
            flag.store(true, Ordering::Relaxed);
        });
        service.submit(request(10), None);
        let results = drain_until_idle(&mut service, Duration::from_secs(10));
        assert!(!results.is_empty());
        assert!(notified.load(Ordering::Relaxed));
    }

    #[test]
    fn cancel_stops_a_pending_task() {
        let mut service = ThreadTaskService::new();
        service.submit(request(10), Some(Duration::from_secs(600)));
        service.cancel(TaskKind::RenderWaveform);
        assert!(!service.is_busy());
        assert!(service.poll().is_empty());
    }

    /// A task that has already finished has its result sitting in the channel.
    /// Cancelling must discard it: the app cancels when it loads a new song, and
    /// the old song's render is not something to deliver afterwards.
    #[test]
    fn cancel_drops_an_already_queued_result() {
        let mut service = ThreadTaskService::new();
        service.submit(request(10), None);
        wait_until_idle(&service, Duration::from_secs(10));
        service.cancel(TaskKind::RenderWaveform);
        assert!(service.poll().is_empty(), "the queued result was delivered");
    }

    /// A render that ignores its cancel flag keeps running, but must not keep
    /// its kind looking busy once cancelled.
    #[test]
    fn cancelling_forgets_a_thread_that_is_still_running() {
        let mut service = ThreadTaskService::new();
        // A long song, so the render is still going when cancel lands.
        service.submit(request(20_000), None);
        assert!(service.is_busy_kind(TaskKind::RenderWaveform));
        service.cancel(TaskKind::RenderWaveform);
        assert!(
            !service.is_busy_kind(TaskKind::RenderWaveform),
            "a cancelled kind must report itself idle at once"
        );
        assert!(!service.is_busy());
    }

    #[test]
    fn is_busy_kind_only_answers_for_its_own_kind() {
        let mut service = ThreadTaskService::new();
        service.submit(request(10), Some(Duration::from_secs(600)));
        assert!(service.is_busy_kind(TaskKind::RenderWaveform));
        assert!(service.is_busy());
        service.cancel(TaskKind::RenderWaveform);
        assert!(!service.is_busy_kind(TaskKind::RenderWaveform));
    }

    /// An untouched kind is idle, and has no slot to consult.
    #[test]
    fn an_unused_kind_is_never_busy() {
        let service = ThreadTaskService::new();
        assert!(!service.is_busy_kind(TaskKind::RenderWaveform));
        assert!(!service.is_busy());
    }
}
