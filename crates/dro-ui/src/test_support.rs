//! Fake platform services for the headless GUI tests (`app_gui_tests`).
//!
//! Each fake shares an `Rc<RefCell<…Log>>` with the test that built it, so the
//! test can script return values (queued picked files, `is_playing`) and later
//! inspect what the app asked for (play/pause counts, save requests, saved
//! configs). The harness runs single-threaded on the test thread, so `Rc` is
//! enough -- none of the four service traits require `Send`.

use core::time::Duration;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use dro_core::Song;
use dro_core::config::{AppConfig, AudioConfig, ConfigStore};
use dro_synth::Muting;

use crate::platform::{
    AudioService, FileService, OptimizedImage, PickedFile, PickedFolder, RipJobOutcome,
    RipJobRequest, RipService, SaveOutcome, SaveRequest,
};
use crate::tasks::{TaskKind, TaskRequest, TaskResult, TaskService, run_task};

// -- files -------------------------------------------------------------------

/// What a [`FakeFileService`] was told to do, and what it will hand back.
#[derive(Debug, Default)]
pub(crate) struct FileLog {
    /// Fed to `poll_picked`, front first. A test queues an entry to simulate the
    /// picker (or drag-and-drop) delivering a file on the next frame.
    pub picked: VecDeque<Result<PickedFile, String>>,
    /// Fed to `poll_saved`, front first.
    pub save_outcomes: VecDeque<SaveOutcome>,
    pub pick_open_calls: usize,
    pub opened_paths: Vec<PathBuf>,
    pub save_requests: Vec<SaveRequest>,
    /// Fed to `poll_folder`, front first (rip mode).
    pub picked_folders: VecDeque<Result<PickedFolder, String>>,
    pub pick_folder_calls: usize,
    pub opened_folder_paths: Vec<PathBuf>,
    /// Fed to `poll_renamed`, front first.
    pub rename_outcomes: VecDeque<Result<(), String>>,
    pub rename_requests: Vec<(PathBuf, String)>,
}

#[derive(Debug)]
pub(crate) struct FakeFileService(pub(crate) Rc<RefCell<FileLog>>);

impl FileService for FakeFileService {
    fn pick_open(&mut self) {
        self.0.borrow_mut().pick_open_calls += 1;
    }

    fn open_path(&mut self, path: PathBuf) {
        self.0.borrow_mut().opened_paths.push(path);
    }

    fn poll_picked(&mut self) -> Option<Result<PickedFile, String>> {
        self.0.borrow_mut().picked.pop_front()
    }

    fn save(&mut self, request: SaveRequest) {
        self.0.borrow_mut().save_requests.push(request);
    }

    fn poll_saved(&mut self) -> Option<SaveOutcome> {
        self.0.borrow_mut().save_outcomes.pop_front()
    }

    fn pick_folder(&mut self) {
        self.0.borrow_mut().pick_folder_calls += 1;
    }

    fn open_folder_path(&mut self, path: PathBuf) {
        self.0.borrow_mut().opened_folder_paths.push(path);
    }

    fn poll_folder(&mut self) -> Option<Result<PickedFolder, String>> {
        self.0.borrow_mut().picked_folders.pop_front()
    }

    fn rename(&mut self, from: PathBuf, to_name: String) {
        self.0.borrow_mut().rename_requests.push((from, to_name));
    }

    fn poll_renamed(&mut self) -> Option<Result<(), String>> {
        self.0.borrow_mut().rename_outcomes.pop_front()
    }
}

// -- audio -------------------------------------------------------------------

/// Every call the app made to a [`FakeAudioService`], plus the scriptable state
/// (`playing`, `finished`) the app reads back.
#[derive(Debug, Default)]
pub(crate) struct AudioLog {
    pub loaded: Option<Arc<Song>>,
    pub load_count: usize,
    pub play_calls: usize,
    pub pause_calls: usize,
    pub rewind_calls: usize,
    pub unload_calls: usize,
    pub seeks_ms: Vec<u32>,
    pub seeks_pos: Vec<usize>,
    pub mutings: Vec<Muting>,
    pub boosts: Vec<f32>,
    /// Toggled by `play`/`pause`; also directly settable by a test.
    pub playing: bool,
    /// Read by `is_finished`; a test sets it to exercise end-of-song handling.
    pub finished: bool,
}

#[derive(Debug)]
pub(crate) struct FakeAudioService(pub(crate) Rc<RefCell<AudioLog>>);

impl AudioService for FakeAudioService {
    fn load(&mut self, song: Arc<Song>, _config: &AudioConfig) -> Result<(), String> {
        let mut log = self.0.borrow_mut();
        log.loaded = Some(song);
        log.load_count += 1;
        Ok(())
    }

    fn unload(&mut self) {
        let mut log = self.0.borrow_mut();
        log.loaded = None;
        log.playing = false;
        log.unload_calls += 1;
    }

    fn play(&mut self) -> Result<(), String> {
        let mut log = self.0.borrow_mut();
        if log.loaded.is_none() {
            return Err("nothing loaded".to_owned());
        }
        log.play_calls += 1;
        log.playing = true;
        Ok(())
    }

    fn pause(&mut self) {
        let mut log = self.0.borrow_mut();
        log.pause_calls += 1;
        log.playing = false;
    }

    fn seek_ms(&mut self, ms: u32) {
        self.0.borrow_mut().seeks_ms.push(ms);
    }

    fn seek_pos(&mut self, pos: usize) {
        self.0.borrow_mut().seeks_pos.push(pos);
    }

    fn rewind(&mut self) {
        self.0.borrow_mut().rewind_calls += 1;
    }

    fn set_muting(&mut self, muting: Muting) {
        self.0.borrow_mut().mutings.push(muting);
    }

    fn set_boost(&mut self, boost: f32) {
        self.0.borrow_mut().boosts.push(boost);
    }

    fn is_playing(&self) -> bool {
        self.0.borrow().playing
    }

    fn is_finished(&self) -> bool {
        self.0.borrow().finished
    }

    fn position(&self) -> Option<dro_synth::Position> {
        // No test drives a live position; the readout/cursor updates it depends
        // on are exercised by dro-synth's own tests.
        None
    }

    fn take_peaks(&mut self) -> Option<[f32; 2]> {
        // Meter stays at rest, keeping frames still for snapshot determinism.
        None
    }

    fn output_rate(&self) -> Option<u32> {
        None
    }
}

// -- tasks -------------------------------------------------------------------

/// Submissions and cancellations seen by a task service.
#[derive(Debug, Default)]
pub(crate) struct TaskLog {
    pub submitted: Vec<(TaskKind, Option<Duration>)>,
    pub cancelled: Vec<TaskKind>,
}

/// Records submissions but never produces results. The default for interaction
/// tests: no waveform is rendered, so frames are cheap and deterministic.
#[derive(Debug)]
pub(crate) struct NoopTaskService(pub(crate) Rc<RefCell<TaskLog>>);

impl TaskService for NoopTaskService {
    fn submit(&mut self, request: TaskRequest, debounce: Option<Duration>) {
        self.0
            .borrow_mut()
            .submitted
            .push((request.kind(), debounce));
    }

    fn cancel(&mut self, kind: TaskKind) {
        self.0.borrow_mut().cancelled.push(kind);
    }

    fn poll(&mut self) -> Vec<TaskResult> {
        Vec::new()
    }

    fn is_busy(&self) -> bool {
        false
    }
}

/// Runs each request synchronously on submit (via [`run_task`]), keeping only
/// the final emit for the next `poll`. Used by the loaded-song snapshot so the
/// waveform actually has pixels; still records submissions like the noop one.
#[derive(Debug)]
pub(crate) struct InlineTaskService {
    log: Rc<RefCell<TaskLog>>,
    pending: Vec<TaskResult>,
}

impl InlineTaskService {
    pub(crate) fn new(log: Rc<RefCell<TaskLog>>) -> Self {
        Self {
            log,
            pending: Vec::new(),
        }
    }
}

impl TaskService for InlineTaskService {
    fn submit(&mut self, request: TaskRequest, debounce: Option<Duration>) {
        self.log
            .borrow_mut()
            .submitted
            .push((request.kind(), debounce));
        let mut last = None;
        run_task(&request, &|| false, &mut |result| last = Some(result));
        self.pending.extend(last);
    }

    fn cancel(&mut self, kind: TaskKind) {
        self.log.borrow_mut().cancelled.push(kind);
    }

    fn poll(&mut self) -> Vec<TaskResult> {
        std::mem::take(&mut self.pending)
    }

    fn is_busy(&self) -> bool {
        false
    }
}

// -- config ------------------------------------------------------------------

/// An in-memory config store: hands out `initial` on load, and appends every
/// saved config to `saved` so a test can assert on what was persisted. `save`
/// takes `&self`, which is why the store shares its `saved` handle.
#[derive(Debug)]
pub(crate) struct MemoryConfigStore {
    pub(crate) initial: AppConfig,
    pub(crate) saved: Rc<RefCell<Vec<AppConfig>>>,
}

impl ConfigStore for MemoryConfigStore {
    fn load(&self) -> AppConfig {
        self.initial
    }

    fn save(&self, config: &AppConfig) -> dro_core::Result<()> {
        self.saved.borrow_mut().push(*config);
        Ok(())
    }
}

// -- rip ---------------------------------------------------------------------

/// Export jobs submitted to a [`FakeRipService`], the outcomes it will hand
/// back, and the scriptable `today`/`busy` state the app reads.
#[derive(Debug)]
pub(crate) struct RipLog {
    pub submitted: Vec<RipJobRequest>,
    /// Fed to `poll`, front first.
    pub outcomes: VecDeque<RipJobOutcome>,
    /// Screenshot optimisations requested, as `(name, byte length)`.
    pub optimize_requests: Vec<(String, usize)>,
    /// Fed to `poll_optimized`, front first.
    pub optimized_outcomes: VecDeque<Result<OptimizedImage, String>>,
    pub busy: bool,
    /// A fixed date, so prefilled history lines and snapshots are deterministic.
    pub today: Option<(i32, u8, u8)>,
}

impl Default for RipLog {
    fn default() -> Self {
        Self {
            submitted: Vec::new(),
            outcomes: VecDeque::new(),
            optimize_requests: Vec::new(),
            optimized_outcomes: VecDeque::new(),
            busy: false,
            today: Some((2026, 7, 16)),
        }
    }
}

#[derive(Debug)]
pub(crate) struct FakeRipService(pub(crate) Rc<RefCell<RipLog>>);

impl RipService for FakeRipService {
    fn submit(&mut self, request: RipJobRequest) {
        self.0.borrow_mut().submitted.push(request);
    }

    fn poll(&mut self) -> Option<RipJobOutcome> {
        self.0.borrow_mut().outcomes.pop_front()
    }

    fn is_busy(&self) -> bool {
        self.0.borrow().busy
    }

    fn cancel(&mut self) {}

    fn optimize(&mut self, name: String, bytes: Vec<u8>) {
        self.0
            .borrow_mut()
            .optimize_requests
            .push((name, bytes.len()));
    }

    fn poll_optimized(&mut self) -> Option<Result<OptimizedImage, String>> {
        self.0.borrow_mut().optimized_outcomes.pop_front()
    }

    fn today(&self) -> Option<(i32, u8, u8)> {
        self.0.borrow().today
    }
}
