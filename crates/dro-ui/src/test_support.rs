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
use dro_synth::{Muting, Panning};

use crate::platform::{
    AudioService, FileService, OptimizedImage, PackJobOutcome, PackJobRequest, PackService,
    PickedFile, PickedFolder, SaveOutcome, SaveRequest,
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
    /// Answers for `pick_output_folder`, front first: `Some(dir)` for a chosen
    /// folder, `None` for a dismissed picker (also the default once empty).
    pub output_folders: VecDeque<Option<PathBuf>>,
    pub pick_output_folder_calls: usize,
    /// The answer the picker gave, waiting to be polled.
    pending_output_folder: Option<Option<PathBuf>>,
    /// Fed to `poll_picked_image`, front first (the pack's Add Screenshot).
    pub picked_images: VecDeque<Result<PickedFile, String>>,
    pub pick_image_calls: usize,
    /// Fed to `poll_folder`, front first (pack mode).
    pub picked_folders: VecDeque<Result<PickedFolder, String>>,
    pub pick_folder_calls: usize,
    pub opened_folder_paths: Vec<PathBuf>,
    /// Fed to `poll_renamed`, front first.
    pub rename_outcomes: VecDeque<Result<(), String>>,
    pub rename_requests: Vec<(PathBuf, String)>,
    /// Answers for `delete`, front first; an empty queue succeeds.
    pub delete_outcomes: VecDeque<Result<(), String>>,
    pub delete_requests: Vec<PathBuf>,
    /// The answer to the last `delete`, waiting to be polled.
    pending_delete: Option<Result<(), String>>,
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

    fn pick_image(&mut self) {
        self.0.borrow_mut().pick_image_calls += 1;
    }

    fn poll_picked_image(&mut self) -> Option<Result<PickedFile, String>> {
        self.0.borrow_mut().picked_images.pop_front()
    }

    fn delete(&mut self, path: PathBuf) {
        let mut log = self.0.borrow_mut();
        log.delete_requests.push(path);
        // Deletes succeed unless a test says otherwise, like renames.
        let outcome = log.delete_outcomes.pop_front().unwrap_or(Ok(()));
        log.pending_delete = Some(outcome);
    }

    fn poll_deleted(&mut self) -> Option<Result<(), String>> {
        self.0.borrow_mut().pending_delete.take()
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

    fn pick_output_folder(&mut self) {
        let mut log = self.0.borrow_mut();
        log.pick_output_folder_calls += 1;
        // As the native service does: the dialog blocks, answers, and its result
        // waits for the next poll -- so a queued answer cannot be drained before
        // anything asked for it.
        let answer = log.output_folders.pop_front().unwrap_or(None);
        log.pending_output_folder = Some(answer);
    }

    fn poll_output_folder(&mut self) -> Option<Option<PathBuf>> {
        self.0.borrow_mut().pending_output_folder.take()
    }
}

// -- audio -------------------------------------------------------------------

/// Every call the app made to a [`FakeAudioService`], plus the scriptable state
/// (`playing`, `finished`) the app reads back.
#[derive(Debug, Default)]
pub(crate) struct AudioLog {
    pub loaded: Option<Arc<Song>>,
    /// The boost from the [`AudioConfig`] the most recent `load` was given, so a
    /// test can check a preview loaded at the track's own volume.
    pub loaded_boost: Option<f32>,
    pub load_count: usize,
    pub play_calls: usize,
    pub pause_calls: usize,
    pub rewind_calls: usize,
    pub unload_calls: usize,
    pub seeks_ms: Vec<u32>,
    pub seeks_pos: Vec<usize>,
    pub mutings: Vec<Muting>,
    pub pannings: Vec<Panning>,
    pub boosts: Vec<f32>,
    /// Every loop region pushed at the service, `None` for "stop looping".
    pub loops: Vec<Option<dro_synth::LoopConfig>>,
    /// Toggled by `play`/`pause`; also directly settable by a test.
    pub playing: bool,
    /// Read by `is_finished`; a test sets it to exercise end-of-song handling.
    pub finished: bool,
    /// Read by `min_engaged_boost`; a test sets the lowest boost that has clipped
    /// to exercise the volume ceiling (the clipping guard that stops the volume
    /// rising past the lowest level that bit the limiter).
    pub min_engaged_boost: Option<f32>,
    /// When set, the next `load` fails (and clears the flag), letting a test
    /// exercise the failed-load paths -- e.g. a pack preview that can't decode.
    pub fail_next_load: bool,
    /// When set, the next `play` fails (and clears the flag), for the
    /// load-succeeds-but-playback-won't-start paths (e.g. no audio device).
    pub fail_next_play: bool,
    /// The stream's real output rate, reported by `output_rate`. `Some` stands
    /// in for a live stream (e.g. after the device rounded the requested rate).
    pub output_rate: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct FakeAudioService(pub(crate) Rc<RefCell<AudioLog>>);

impl AudioService for FakeAudioService {
    fn load(&mut self, song: Arc<Song>, config: &AudioConfig) -> Result<(), String> {
        let mut log = self.0.borrow_mut();
        if core::mem::take(&mut log.fail_next_load) {
            // Mirror `NativeAudioService::load`, which unloads the prior stream
            // before building the new one -- so a failed build leaves the
            // service cleanly empty rather than holding a half-loaded song.
            log.loaded = None;
            log.playing = false;
            return Err("fake load failure".to_owned());
        }
        log.loaded = Some(song);
        log.loaded_boost = Some(config.boost);
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
        if core::mem::take(&mut log.fail_next_play) {
            // Load succeeded but playback won't start (e.g. no output device):
            // leave `playing` false without touching the loaded song.
            return Err("fake play failure".to_owned());
        }
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

    fn set_panning(&mut self, panning: Panning) {
        self.0.borrow_mut().pannings.push(panning);
    }

    fn set_boost(&mut self, boost: f32) {
        self.0.borrow_mut().boosts.push(boost);
    }

    fn set_loop(&mut self, config: Option<dro_synth::LoopConfig>) {
        self.0.borrow_mut().loops.push(config);
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
        self.0.borrow().output_rate
    }

    fn min_engaged_boost(&self) -> Option<f32> {
        self.0.borrow().min_engaged_boost
    }
}

// -- tasks -------------------------------------------------------------------

/// Submissions and cancellations seen by a task service.
#[derive(Debug, Default)]
pub(crate) struct TaskLog {
    pub submitted: Vec<(TaskKind, Option<Duration>)>,
    pub cancelled: Vec<TaskKind>,
    /// Kinds the service should claim to be working on. A test sets this to
    /// exercise the paths that refuse to start a second export, or that name the
    /// running job in the status bar.
    pub busy: Vec<TaskKind>,
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
        !self.0.borrow().busy.is_empty()
    }

    fn is_busy_kind(&self, kind: TaskKind) -> bool {
        self.0.borrow().busy.contains(&kind)
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
        !self.log.borrow().busy.is_empty()
    }

    fn is_busy_kind(&self, kind: TaskKind) -> bool {
        self.log.borrow().busy.contains(&kind)
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
        self.initial.clone()
    }

    fn save(&self, config: &AppConfig) -> dro_core::Result<()> {
        self.saved.borrow_mut().push(config.clone());
        Ok(())
    }
}

// -- pack ---------------------------------------------------------------------

/// Export jobs submitted to a [`FakePackService`], the outcomes it will hand
/// back, and the scriptable `today`/`busy` state the app reads.
#[derive(Debug)]
pub(crate) struct PackLog {
    pub submitted: Vec<PackJobRequest>,
    /// Fed to `poll`, front first.
    pub outcomes: VecDeque<PackJobOutcome>,
    /// Screenshot optimisations requested, as `(name, byte length)`.
    pub optimize_requests: Vec<(String, usize)>,
    /// Fed to `poll_optimized`, front first.
    pub optimized_outcomes: VecDeque<Result<OptimizedImage, String>>,
    pub busy: bool,
    /// A fixed date, so prefilled history lines and snapshots are deterministic.
    pub today: Option<(i32, u8, u8)>,
}

impl Default for PackLog {
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
pub(crate) struct FakePackService(pub(crate) Rc<RefCell<PackLog>>);

impl PackService for FakePackService {
    fn submit(&mut self, request: PackJobRequest) {
        self.0.borrow_mut().submitted.push(request);
    }

    fn poll(&mut self) -> Option<PackJobOutcome> {
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
