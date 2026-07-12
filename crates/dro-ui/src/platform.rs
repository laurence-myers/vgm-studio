//! The platform-service traits the application is parameterized by.
//!
//! Every difference between the native shell (`dro-trimmer`) and the web shell
//! (`dro-web`, Step 8) lives behind these traits. Anything asynchronous is
//! *polled*, never awaited, from the egui update loop: a native implementation
//! may block inside the call and deliver on the very next poll; a web
//! implementation delivers whenever its future resolves.

use std::path::PathBuf;
use std::sync::Arc;

use dro_core::Song;
use dro_core::config::AudioConfig;
use dro_synth::{Muting, Position};

pub use dro_core::config::ConfigStore;

/// A file the user picked or dropped: a display name, the bytes, and -- native
/// only -- the path it came from.
///
/// Bytes, not paths: `dro-core`'s readers take `&[u8]`, and the web has no
/// filesystem.
#[derive(Debug, Clone)]
pub struct PickedFile {
    /// The file name including its extension, without any directory.
    pub name: String,
    /// The full path, for later in-place saves. `None` on the web.
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
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
    fn save(&mut self, request: SaveRequest);

    /// The outcome of the most recent [`Self::save`].
    fn poll_saved(&mut self) -> Option<SaveOutcome>;
}

/// Owns the platform's audio output and the `PlayerEngine` behind it.
///
/// The native implementation wraps `dro-audio-native`'s cpal stream; the web
/// implementation (Step 9) talks to an `AudioWorklet`. Errors are strings
/// because the GUI can only show them.
pub trait AudioService {
    /// Prepares `song` for playback, replacing any current song, stopped and
    /// positioned at the start.
    ///
    /// The song is an immutable snapshot: edits made to the editor's copy
    /// afterwards do not reach it. The app reloads before the next play.
    ///
    /// # Errors
    /// If the platform's audio output cannot be opened.
    fn load(&mut self, song: Arc<Song>, config: &AudioConfig) -> Result<(), String>;

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

    /// Replaces the channel/percussion muting, live.
    fn set_muting(&mut self, muting: Muting);

    fn is_playing(&self) -> bool;

    /// Whether the loaded song has played to its end.
    fn is_finished(&self) -> bool;

    /// The current playback position, or `None` when nothing is loaded.
    fn position(&self) -> Option<Position>;

    /// The sample rate the output actually runs at, when known. May differ
    /// from the configured frequency if the device rejected it -- positions
    /// report frames at *this* rate.
    fn output_rate(&self) -> Option<u32>;
}
