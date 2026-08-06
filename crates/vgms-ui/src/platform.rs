//! The platform-service traits the application is parameterized by.
//!
//! Every difference between the native shell (`vgms-app`) and the web shell
//! (`vgms-web`) lives behind these traits. Anything asynchronous is
//! *polled*, never awaited, from the egui update loop: a native implementation
//! may block inside the call and deliver on the very next poll; a web
//! implementation delivers whenever its future resolves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vgms_core::config::AudioConfig;
use vgms_pack_archive::PackArchive;
use vgms_synth::{
    AudioSource, ChipMuting, ChipPanning, ChipTrims, LoopConfig, Muting, Panning, Position,
};

pub use vgms_core::config::ConfigStore;

/// A file the user picked or dropped: a display name, the bytes, and -- native
/// only -- the path it came from.
///
/// Bytes, not paths: `vgms-core`'s readers take `&[u8]`, and the web has no
/// filesystem.
#[derive(Debug, Clone)]
pub struct PickedFile {
    /// The file name including its extension, without any directory.
    pub name: String,
    /// The full path, for later in-place saves. `None` on the web.
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
}

/// A folder the user opened as a pack project: its name, path (native only), and
/// the relevant files it holds, each with bytes already read.
///
/// The native scan is non-recursive and keeps only `.vgm`/`.vgz`/`.png`/`.txt`,
/// sorted case-insensitively by name -- everything pack mode needs to show a track
/// list, the screenshots, and any existing description.
#[derive(Debug, Clone)]
pub struct PickedFolder {
    /// The folder's own name, without any parent directory.
    pub name: String,
    /// The full path, for saving the description/playlist back. `None` on the web.
    pub path: Option<PathBuf>,
    pub files: Vec<PickedFile>,
}

/// Where a pack's files live, and therefore how edits are persisted.
///
/// A [`Directory`](PackOrigin::Directory) pack writes each edit straight to its
/// folder (a real directory natively, a File System Access handle on Chromium).
/// A [`MemoryZip`](PackOrigin::MemoryZip) pack -- one opened from a `.zip`, the
/// everywhere answer for browsers without a writable-directory API -- holds its
/// files in an [`ArchiveBackend`] in memory; edits are volatile until an explicit
/// **Save Pack** re-exports the archive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PackOrigin {
    /// Backed by a writable directory: edits land immediately.
    #[default]
    Directory,
    /// Backed by an in-memory archive from a `.zip`: edits are volatile until
    /// saved. `source` is the `.zip`'s path where one exists (native), for an
    /// in-place Save Pack; `None` on the web, where saving is a download.
    MemoryZip { source: Option<PathBuf> },
}

impl PackOrigin {
    /// Whether edits are held in memory and need an explicit save.
    #[must_use]
    pub fn is_memory(&self) -> bool {
        matches!(self, PackOrigin::MemoryZip { .. })
    }
}

/// Where a save should go.
#[derive(Debug, Clone)]
pub enum SaveRequest {
    /// Write straight to `path` -- File > Save. Never produced on the web,
    /// which has no paths.
    InPlace { path: PathBuf, bytes: Vec<u8> },
    /// Ask the user where -- File > Save As (and every save, on the web).
    Dialog {
        suggested_name: String,
        bytes: Vec<u8>,
    },
}

/// What became of a [`SaveRequest`].
#[derive(Debug, Clone)]
pub enum SaveOutcome {
    Saved {
        /// The file name actually written, including its extension.
        name: String,
        /// The path written to, so the next Save can reuse it. `None` on the web.
        path: Option<PathBuf>,
    },
    /// The user dismissed the save dialog.
    Cancelled,
    Failed(String),
}

/// Opens and saves files. Results arrive via the `poll_*` methods, called every
/// frame from the update loop.
pub trait FileService {
    /// Opens the platform's file picker.
    fn pick_open(&mut self);

    /// Reads the file at `path` -- the native drag-and-drop case, where egui
    /// delivers a path but no bytes. The web never calls this: its drops carry
    /// the bytes themselves.
    fn open_path(&mut self, path: PathBuf);

    /// The file from the most recent [`Self::pick_open`] / [`Self::open_path`],
    /// or an error message to show. `None` while nothing has arrived (including
    /// after a cancelled picker).
    fn poll_picked(&mut self) -> Option<Result<PickedFile, String>>;

    /// Starts a save.
    ///
    /// Saves are honoured in order, one [`SaveOutcome`] per call: pack mode fires
    /// the description and the playlist back to back and correlates the outcomes
    /// by that FIFO order, so an implementation must not coalesce or drop a
    /// pending outcome when a second save arrives before the first is polled.
    fn save(&mut self, request: SaveRequest);

    /// The next save outcome, oldest first. `None` when none is waiting.
    fn poll_saved(&mut self) -> Option<SaveOutcome>;

    /// Opens a picker for a screenshot to copy into the open pack's folder.
    ///
    /// Distinct from [`Self::pick_open`], which loads a *song* into the editor:
    /// these two run on their own channels so a pending screenshot is never
    /// mistaken for a file to open. Defaulted to nothing at all, because a
    /// browser has no pack folder to copy into.
    fn pick_image(&mut self) {}

    /// The image from the most recent [`Self::pick_image`], or an error to show.
    /// `None` while nothing has arrived (including after a dismissed picker).
    fn poll_picked_image(&mut self) -> Option<Result<PickedFile, String>> {
        None
    }

    /// Removes `path` from disk (a pack `Delete` mutation).
    ///
    /// Only ever issued by the pack file-op executor, whose transaction carries
    /// the inverse `Write` that puts the file back, so this is the one deletion
    /// the app can undo. Defaulted to nothing: the web has no pack folder.
    fn delete(&mut self, path: PathBuf) {
        let _ = path;
    }

    /// The outcome of the most recent [`Self::delete`]. `None` until one lands.
    fn poll_deleted(&mut self) -> Option<Result<(), String>> {
        None
    }

    /// Opens a `.zip`'s bytes as an in-memory pack (wt-8), delivering the result
    /// through the same channel as [`Self::pick_folder`] ([`Self::poll_folder`]).
    ///
    /// The everywhere path: it needs no writable-directory API, so a browser
    /// without File System Access still edits packs. The file service registers
    /// an [`ArchiveBackend`] entry and routes this pack's later save/delete/rename
    /// to it. Defaulted to nothing for a service that does not support it.
    fn open_pack_archive(&mut self, name: String, bytes: Vec<u8>) {
        let _ = (name, bytes);
    }

    /// Opens the platform's file picker, filtered to `.zip`, for a pack archive
    /// (File > Open Pack Zip).
    ///
    /// The picked file arrives through [`Self::poll_picked`], not
    /// [`Self::poll_folder`]: the app recognises a `.zip` there and routes it to
    /// [`Self::open_pack_archive`] *after* its discard-changes prompt, so the
    /// menu path and a dropped `.zip` behave identically. Required rather than
    /// defaulted, so a new service cannot silently answer the menu item with
    /// nothing.
    fn pick_pack_zip(&mut self);

    /// Opens the platform's folder picker (pack mode).
    fn pick_folder(&mut self);

    /// Scans the folder at `path` -- the drag-and-drop / command-line case, where
    /// a directory is handed over instead of a file.
    fn open_folder_path(&mut self, path: PathBuf);

    /// The folder from the most recent [`Self::pick_folder`] /
    /// [`Self::open_folder_path`], or an error to show. `None` until one arrives.
    fn poll_folder(&mut self) -> Option<Result<PickedFolder, String>>;

    /// Renames the file at `from` to the bare name `to_name`, in the same
    /// directory (pack mode's quick-edit). Must fail rather than overwrite an
    /// existing file.
    fn rename(&mut self, from: PathBuf, to_name: String);

    /// The outcome of the most recent [`Self::rename`]. `None` until it arrives.
    fn poll_renamed(&mut self) -> Option<Result<(), String>>;

    /// Asks where to put a batch of new files -- the split's one directory for
    /// its per-channel outputs.
    ///
    /// Distinct from [`Self::pick_folder`], which *reads* a folder in as a pack
    /// project. Defaulted to nothing at all, because a browser has no directory
    /// to write into; a web build offers its outputs some other way.
    fn pick_output_folder(&mut self) {}

    /// The folder from the most recent [`Self::pick_output_folder`]:
    /// `Some(Some(dir))` once one is chosen, `Some(None)` if the picker was
    /// dismissed, `None` while nothing has happened.
    fn poll_output_folder(&mut self) -> Option<Option<PathBuf>> {
        None
    }
}

/// The in-memory backend a file service embeds to serve zip-opened packs (wt-8).
///
/// Each opened `.zip` gets a synthetic `/vgms-zip-N` token folder; the file
/// service routes any save/delete/rename/rescan whose token this backend holds to
/// the [`PackArchive`] instead of the disk (native) or a directory handle (web).
/// The routing is one implementation, embedded in every file service (native,
/// web, and the test fake), so a zip pack behaves identically everywhere. Every
/// archive op is synchronous, so the outcome is ready on the same poll.
#[derive(Debug, Default)]
pub struct ArchiveBackend {
    entries: HashMap<String, ArchiveEntry>,
    next_id: u64,
}

#[derive(Debug)]
struct ArchiveEntry {
    archive: PackArchive,
    /// The pack's display name (the `.zip`'s stem).
    name: String,
}

impl ArchiveBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens `bytes` as a new zip pack under a fresh token, returning the folder
    /// to deliver through `poll_folder`.
    ///
    /// # Errors
    /// If the bytes are not a readable zip.
    pub fn open(&mut self, name: &str, bytes: &[u8]) -> Result<PickedFolder, String> {
        let archive = PackArchive::open(bytes)?;
        let token = format!("vgms-zip-{}", self.next_id);
        self.next_id += 1;
        let display = zip_stem(name);
        let folder = folder_for(&token, &display, &archive);
        self.entries.insert(
            token,
            ArchiveEntry {
                archive,
                name: display,
            },
        );
        Ok(folder)
    }

    /// Whether this backend holds the pack folder at `path`.
    #[must_use]
    pub fn holds_folder(&self, path: &Path) -> bool {
        archive_token(path).is_some_and(|token| self.entries.contains_key(&token))
    }

    /// Whether `path` names a file inside a pack this backend holds.
    #[must_use]
    pub fn holds_file(&self, path: &Path) -> bool {
        path.parent()
            .is_some_and(|parent| self.holds_folder(parent))
    }

    /// Re-lists the pack folder at `path` (the rescan path), if it is one.
    #[must_use]
    pub fn folder(&self, path: &Path) -> Option<PickedFolder> {
        let token = archive_token(path)?;
        let entry = self.entries.get(&token)?;
        Some(folder_for(&token, &entry.name, &entry.archive))
    }

    /// Writes (creating or overwriting) a file in a held pack.
    ///
    /// # Errors
    /// If `path` is not a held pack file.
    pub fn write(&mut self, path: &Path, bytes: Vec<u8>) -> Result<(), String> {
        let (token, name) = self.split(path)?;
        self.entries
            .get_mut(&token)
            .ok_or_else(|| unknown(path))?
            .archive
            .write(&name, bytes);
        Ok(())
    }

    /// Deletes a file from a held pack.
    ///
    /// # Errors
    /// If `path` is not a held pack file, or the file is absent.
    pub fn delete(&mut self, path: &Path) -> Result<(), String> {
        let (token, name) = self.split(path)?;
        self.entries
            .get_mut(&token)
            .ok_or_else(|| unknown(path))?
            .archive
            .delete(&name)
    }

    /// Renames a file in a held pack (the native decision tree lives in the
    /// archive).
    ///
    /// # Errors
    /// If `from` is not a held pack file, the source is absent, or the target
    /// exists.
    pub fn rename(&mut self, from: &Path, to_name: &str) -> Result<(), String> {
        let (token, from_name) = self.split(from)?;
        self.entries
            .get_mut(&token)
            .ok_or_else(|| unknown(from))?
            .archive
            .rename(&from_name, to_name)
    }

    fn split(&self, path: &Path) -> Result<(String, String), String> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| unknown(path))?
            .to_owned();
        let token = path
            .parent()
            .and_then(archive_token)
            .ok_or_else(|| unknown(path))?;
        Ok((token, name))
    }
}

/// Builds the [`PickedFolder`] for a token: the pack's files with `/token/<name>`
/// paths, in the archive's (lowercase-sorted) order.
fn folder_for(token: &str, name: &str, archive: &PackArchive) -> PickedFolder {
    let root = PathBuf::from(format!("/{token}"));
    let files = archive
        .files()
        .into_iter()
        .map(|(file_name, bytes)| PickedFile {
            path: Some(root.join(&file_name)),
            name: file_name,
            bytes,
        })
        .collect();
    PickedFolder {
        name: name.to_owned(),
        path: Some(root),
        files,
    }
}

/// The `vgms-zip-N` token component of a `/vgms-zip-N` path, if it is one. Uses
/// `file_name`, so it is oblivious to the platform path separator.
fn archive_token(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.starts_with("vgms-zip-"))
        .map(str::to_owned)
}

/// A `.zip`'s file name without the extension -- the pack's display name.
fn zip_stem(name: &str) -> String {
    name.rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("zip"))
        .map_or_else(|| name.to_owned(), |(stem, _)| stem.to_owned())
}

fn unknown(path: &Path) -> String {
    format!("{} is not an open zip pack", path.display())
}

/// Owns the platform's audio output and the `PlayerEngine` behind it.
///
/// The native implementation wraps `vgms-audio-native`'s cpal stream; the web
/// implementation talks to an `AudioWorklet`. Errors are strings
/// because the GUI can only show them.
pub trait AudioService {
    /// Prepares `source` for playback, replacing any current song, stopped and
    /// positioned at the start.
    ///
    /// The source is an immutable snapshot: edits made to the editor's copy
    /// afterwards do not reach it. The app reloads before the next play.
    ///
    /// # Errors
    /// If the platform's audio output cannot be opened, or the backend cannot
    /// play this kind of source -- the RetroWave hardware is OPL-only, so a VGM
    /// for other chips is refused there rather than silently ignored.
    fn load(&mut self, source: AudioSource, config: &AudioConfig) -> Result<(), String>;

    /// Drops the current song and closes the output.
    fn unload(&mut self);

    /// Starts or resumes playback.
    ///
    /// # Errors
    /// If nothing is loaded, or the device rejects starting the stream.
    fn play(&mut self) -> Result<(), String>;

    /// Pauses playback, holding the current position.
    fn pause(&mut self);

    /// Seeks to the instruction playing at `ms`.
    fn seek_ms(&mut self, ms: u32);

    /// Seeks to instruction `pos`.
    fn seek_pos(&mut self, pos: usize);

    /// Returns to the start of the song.
    fn rewind(&mut self);

    /// Replaces the channel/percussion muting, live. An OPL idea; the default
    /// no-op is for backends that are not the emulated OPL player.
    fn set_muting(&mut self, muting: Muting);

    /// Replaces the per-channel panning, live.
    fn set_panning(&mut self, panning: Panning);

    /// Replaces the any-chip channel mutes, live -- the generic engine's
    /// counterpart of [`set_muting`](Self::set_muting).
    ///
    /// **Deliberately not defaulted**, unlike almost everything a backend can
    /// decline to do. These two had a `{}` default once, and
    /// [`SwitchingAudioService`] -- the only service the desktop binary builds
    /// -- inherited it while forwarding its OPL siblings, so every channel
    /// mute, chip mute and pan on a non-OPL file was swallowed between the UI
    /// and an engine that applied them perfectly. Nothing failed; the calls
    /// simply stopped. A required method makes a forwarding wrapper say so,
    /// and a backend that truly cannot must write the empty body and its
    /// reason.
    ///
    /// [`SwitchingAudioService`]: https://docs.rs/vgms-app
    fn set_chip_muting(&mut self, muting: ChipMuting);

    /// Replaces the any-chip channel pans, live. Required, for the reason on
    /// [`set_chip_muting`](Self::set_chip_muting).
    fn set_chip_panning(&mut self, panning: ChipPanning);

    /// Replaces the per-chip listening trims, live -- the volume half of the
    /// per-chip mixer, beside the chip mutes and pans. Required, for the same
    /// reason as [`set_chip_muting`](Self::set_chip_muting): a `{}` default
    /// would let a forwarding wrapper swallow it in silence. A backend that
    /// renders no signal to attenuate writes the empty body and its reason, as
    /// it does for [`set_boost`](Self::set_boost).
    fn set_chip_trims(&mut self, trims: ChipTrims);

    /// Sets the live playback volume boost. A limiter keeps the boosted signal
    /// from clipping. Never affects a WAV render or the waveform.
    fn set_boost(&mut self, boost: f32);

    /// Sets (or clears) the region playback loops over, live.
    ///
    /// Build the config with `LoopConfig::for_song`, which precomputes the frame
    /// position the audio callback cannot afford to derive. A region that is
    /// empty or reaches past the song is ignored by the engine, so playback
    /// simply does not loop.
    fn set_loop(&mut self, config: Option<LoopConfig>);

    fn is_playing(&self) -> bool;

    /// Whether the loaded song has played to its end.
    fn is_finished(&self) -> bool;

    /// The current playback position, or `None` when nothing is loaded.
    fn position(&self) -> Option<Position>;

    /// The loudest output peak per channel (left, right) since the last call,
    /// measured after boost and limiting -- what the listener actually hears.
    /// `0.0..=1.0`; `None` when nothing is loaded. A destructive read: each
    /// peak is reported once.
    fn take_peaks(&mut self) -> Option<[f32; 2]>;

    /// The sample rate the output actually runs at, when known. May differ
    /// from the configured frequency if the device rejected it -- positions
    /// report frames at *this* rate.
    fn output_rate(&self) -> Option<u32>;

    /// The lowest boost at which the limiter has engaged since the current song
    /// loaded, or `None` if it has not clipped (or nothing is loaded). Reset when
    /// a new song loads. The app uses it as the volume ceiling -- the cap ratchets
    /// down to the lowest boost that clips, so dropping the volume and still
    /// clipping lowers the cap.
    fn min_engaged_boost(&self) -> Option<f32>;

    /// Whether the limiter engaged since the last call -- a passage that would
    /// have clipped was pulled down. A destructive read, so each clip is
    /// reported once; the meter holds its peak marker to show it.
    ///
    /// Distinct from [`Self::min_engaged_boost`], which is sticky and says only
    /// *that* the song has clipped, never when.
    fn take_limited(&mut self) -> bool {
        false
    }

    /// The output ports hardware playback could use.
    ///
    /// Answered without opening anything, so the settings dialog can offer a
    /// choice while another backend is still playing. Empty on platforms with no
    /// hardware output at all.
    fn list_hardware_ports(&self) -> Vec<HardwarePortInfo> {
        Vec::new()
    }

    /// Takes the last playback failure, if one is waiting. Reported once.
    ///
    /// For faults that happen away from a call the app made -- a device unplugged
    /// mid-song -- which have nowhere else to surface.
    fn last_error(&mut self) -> Option<String> {
        None
    }
}

/// An output port hardware playback could use.
///
/// Plain data: `vgms-ui` stays free of the serial layer, which never builds for
/// the web.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwarePortInfo {
    /// What to store in the config, such as `COM3`.
    pub port_name: String,
    /// What to show in the picker.
    pub label: String,
    /// Whether this looks like the hardware we are after, so the picker can
    /// default to it.
    pub recognised: bool,
}

/// A file bound for the release zip, and what kind it is. Both are defined by
/// [`vgms_pack_archive`] -- the crate that packs them -- and re-exported here so
/// the UI's platform surface still names them.
pub use vgms_pack_archive::{PackEntry, PackEntryKind};

/// A request to build a release zip. The entries are already in final order.
#[derive(Debug, Clone)]
pub struct PackJobRequest {
    /// The suggested file name for the finished archive, e.g. `"Game Name.zip"`.
    pub zip_name: String,
    pub entries: Vec<PackEntry>,
    /// Whether to gzip `.vgm` songs to `.vgz` (renaming the entry).
    pub gzip_vgms: bool,
    /// Whether to strip redundant register writes from each VGM before packing
    /// it (the `vgm_cmp` step of the VGMRips pipeline).
    pub optimize_vgms: bool,
    /// Which optimiser to run when `optimize_vgms`: the built-in pass, the
    /// external tools, or the routing between them (the Settings choice).
    pub optimizer: vgms_core::config::OptimizerChoice,
}

/// What became of a [`PackJobRequest`].
#[derive(Debug, Clone)]
pub enum PackJobOutcome {
    Done {
        zip_name: String,
        bytes: Vec<u8>,
        /// Human-readable notes (per-PNG savings, counts) for the status line.
        log: Vec<String>,
    },
    Failed(String),
}

/// The result of an explicit screenshot optimisation ([`PackService::optimize`]).
#[derive(Debug, Clone)]
pub struct OptimizedImage {
    /// The file name the request carried.
    pub name: String,
    /// The size before optimisation, for the savings report.
    pub original_len: usize,
    /// The optimised bytes (not necessarily smaller; the caller compares).
    pub bytes: Vec<u8>,
}

/// Runs the pack export off the UI thread: optimise PNGs, optionally gzip songs,
/// and build the zip. Kept separate from [`crate::tasks::TaskService`] because
/// its job body needs native-only crates (zip, oxipng) that must not reach the
/// wasm-clean `run_task`.
pub trait PackService {
    /// Starts building a release zip, superseding any job already running.
    fn submit(&mut self, request: PackJobRequest);

    /// The finished archive (or a failure), once ready. `None` until then.
    fn poll(&mut self) -> Option<PackJobOutcome>;

    /// Whether a job is in flight, for the status line.
    fn is_busy(&self) -> bool;

    /// Cancels the running job, if any.
    fn cancel(&mut self);

    /// Losslessly recompresses a screenshot's bytes off the UI thread (oxipng
    /// natively). The result arrives via [`Self::poll_optimized`].
    fn optimize(&mut self, name: String, bytes: Vec<u8>);

    /// The next optimisation result. `None` until one arrives.
    fn poll_optimized(&mut self) -> Option<Result<OptimizedImage, String>>;

    /// Today's local date as `(year, month, day)`, for the prefilled history
    /// line. The default returns `None`, keeping the wasm-clean UI free of a
    /// clock; native shells override it.
    fn today(&self) -> Option<(i32, u8, u8)> {
        None
    }
}
