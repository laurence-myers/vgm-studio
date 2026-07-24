//! The native `PackService`: one `std::thread` per export job, superseded on
//! resubmit. Simpler than [`super::task::ThreadTaskService`] -- a single job at a
//! time, no debounce -- but the same shape: a cooperative cancel flag, a
//! generation number so a superseded job's late result is dropped, and a repaint
//! notifier to close the finish-between-frames race.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use chrono::{Datelike as _, Local};
use dro_ui::{OptimizedImage, PackJobOutcome, PackJobRequest, PackService};

use crate::pack_zip::{build_pack_zip, png_options};

pub struct NativePackService {
    sender: Sender<(u64, PackJobOutcome)>,
    receiver: Receiver<(u64, PackJobOutcome)>,
    /// Screenshot optimisations report on their own channel; they never
    /// supersede one another, so no generation is needed.
    optimize_sender: Sender<Result<OptimizedImage, String>>,
    optimize_receiver: Receiver<Result<OptimizedImage, String>>,
    /// The running job's cancel flag, if any.
    cancelled: Option<Arc<AtomicBool>>,
    /// Spawned-and-not-yet-exited threads, for `is_busy`.
    live: Arc<AtomicUsize>,
    /// Bumped per submission; stale results are dropped by generation.
    generation: u64,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for NativePackService {
    fn default() -> Self {
        let (sender, receiver) = channel();
        let (optimize_sender, optimize_receiver) = channel();
        Self {
            sender,
            receiver,
            optimize_sender,
            optimize_receiver,
            cancelled: None,
            live: Arc::new(AtomicUsize::new(0)),
            generation: 0,
            notify: None,
        }
    }
}

impl fmt::Debug for NativePackService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativePackService")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl NativePackService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// As [`Self::new`], with `notify` called whenever a job posts its result.
    #[must_use]
    pub fn with_notifier(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Some(Arc::new(notify)),
            ..Self::default()
        }
    }

    fn spawn(&mut self, request: PackJobRequest, generation: u64) {
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled = Some(Arc::clone(&cancelled));

        let sender = self.sender.clone();
        let live = Arc::clone(&self.live);
        let notify = self.notify.clone();
        live.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let is_cancelled = || cancelled.load(Ordering::Relaxed);
            let outcome = match build_pack_zip(
                &request.entries,
                request.gzip_vgms,
                request.optimize_vgms,
                &is_cancelled,
            ) {
                Ok(Some(output)) => Some(PackJobOutcome::Done {
                    zip_name: request.zip_name,
                    bytes: output.bytes,
                    log: output.log,
                }),
                Ok(None) => None, // cancelled partway through
                Err(error) => Some(PackJobOutcome::Failed(format!("{error:#}"))),
            };
            // A superseded job's cancel flag is set; drop its late result rather
            // than deliver it (the generation filter in `poll` is the backstop).
            if let Some(outcome) = outcome
                && !cancelled.load(Ordering::Relaxed)
            {
                // A closed channel just means the app shut down.
                let _ = sender.send((generation, outcome));
                if let Some(notify) = &notify {
                    notify();
                }
            }
            live.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

impl PackService for NativePackService {
    fn submit(&mut self, request: PackJobRequest) {
        self.cancel();
        self.generation += 1;
        let generation = self.generation;
        self.spawn(request, generation);
    }

    fn poll(&mut self) -> Option<PackJobOutcome> {
        let latest = self.generation;
        let mut outcome = None;
        while let Ok((generation, result)) = self.receiver.try_recv() {
            if generation == latest {
                outcome = Some(result);
            }
        }
        outcome
    }

    fn is_busy(&self) -> bool {
        self.live.load(Ordering::Relaxed) > 0
    }

    fn cancel(&mut self) {
        if let Some(flag) = self.cancelled.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    fn optimize(&mut self, name: String, bytes: Vec<u8>) {
        let sender = self.optimize_sender.clone();
        let notify = self.notify.clone();
        let live = Arc::clone(&self.live);
        live.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let result = match oxipng::optimize_from_memory(&bytes, &png_options()) {
                Ok(optimised) => Ok(OptimizedImage {
                    name,
                    original_len: bytes.len(),
                    bytes: optimised,
                }),
                Err(error) => Err(format!("{name}: {error}")),
            };
            // A closed channel just means the app shut down.
            let _ = sender.send(result);
            if let Some(notify) = &notify {
                notify();
            }
            live.fetch_sub(1, Ordering::Relaxed);
        });
    }

    fn poll_optimized(&mut self) -> Option<Result<OptimizedImage, String>> {
        self.optimize_receiver.try_recv().ok()
    }

    fn today(&self) -> Option<(i32, u8, u8)> {
        let today = Local::now().date_naive();
        Some((
            today.year(),
            u8::try_from(today.month()).expect("month is 1..=12"),
            u8::try_from(today.day()).expect("day is 1..=31"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use dro_ui::{PackEntry, PackEntryKind};

    fn doc_job(name: &str) -> PackJobRequest {
        PackJobRequest {
            zip_name: format!("{name}.zip"),
            entries: vec![PackEntry {
                name: format!("{name}.txt"),
                bytes: b"description".to_vec(),
                kind: PackEntryKind::Doc,
            }],
            gzip_vgms: false,
            optimize_vgms: false,
        }
    }

    /// Polls until an outcome arrives or the deadline passes.
    fn wait_for_outcome(
        service: &mut NativePackService,
        timeout: Duration,
    ) -> Option<PackJobOutcome> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(outcome) = service.poll() {
                return Some(outcome);
            }
            if Instant::now() > deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_submitted_job_builds_and_delivers_a_zip() {
        let mut service = NativePackService::new();
        service.submit(doc_job("Game"));
        assert!(service.is_busy());
        match wait_for_outcome(&mut service, Duration::from_secs(10)) {
            Some(PackJobOutcome::Done {
                zip_name, bytes, ..
            }) => {
                assert_eq!(zip_name, "Game.zip");
                assert_eq!(&bytes[..2], b"PK", "a zip archive");
            }
            other => panic!("expected a finished zip, got {other:?}"),
        }
        // The thread has exited; the service is idle again.
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.is_busy() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!service.is_busy());
    }

    #[test]
    fn the_notifier_fires_when_a_job_finishes() {
        let fired = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&fired);
        let mut service =
            NativePackService::with_notifier(move || flag.store(true, Ordering::Relaxed));
        service.submit(doc_job("Game"));
        assert!(wait_for_outcome(&mut service, Duration::from_secs(10)).is_some());
        assert!(fired.load(Ordering::Relaxed));
    }

    #[test]
    fn today_reports_a_plausible_date() {
        let (year, month, day) = NativePackService::new().today().unwrap();
        assert!(year >= 2024, "year was {year}");
        assert!((1..=12).contains(&month), "month was {month}");
        assert!((1..=31).contains(&day), "day was {day}");
    }

    /// Polls until an optimisation result arrives or the deadline passes.
    fn wait_for_optimized(
        service: &mut NativePackService,
        timeout: Duration,
    ) -> Result<OptimizedImage, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(result) = service.poll_optimized() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the optimiser"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn optimize_shrinks_the_deliberately_suboptimal_fixture() {
        const PNG: &[u8] = include_bytes!("../../../../tests/screenshot.png");
        let mut service = NativePackService::new();
        service.optimize("shot.png".to_owned(), PNG.to_vec());
        let optimized = wait_for_optimized(&mut service, Duration::from_secs(30)).unwrap();
        assert_eq!(optimized.name, "shot.png");
        assert_eq!(optimized.original_len, PNG.len());
        assert!(optimized.bytes.len() < PNG.len(), "the fixture shrinks");
        assert_eq!(&optimized.bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn optimize_reports_a_corrupt_png_as_an_error() {
        let mut service = NativePackService::new();
        service.optimize("bad.png".to_owned(), b"not a png".to_vec());
        assert!(wait_for_optimized(&mut service, Duration::from_secs(30)).is_err());
    }
}
