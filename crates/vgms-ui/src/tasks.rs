//! Background-task definitions and the `TaskService` trait.
//!
//! The register analysis is `vgms-core`'s synchronous replay cursor, so the
//! waveform render is the only background task left. The task *logic* lives
//! here, shared by every platform; the *scheduling* -- threads natively, Web
//! Workers later -- lives behind [`TaskService`].

use core::time::Duration;
use std::sync::Arc;

use vgms_core::Song;
use vgms_core::io::write_song;
use vgms_core::loopfind::{Candidate, find_loops, rank};
use vgms_core::pack::track_file_name;
use vgms_core::split_songs::{
    detect_segments, detect_segments_in_vgm, materialise, materialise_vgm,
};
use vgms_synth::{
    AudioSource, Peak, RenderMix, SplitData, SplitOptions, VgmSplitOptions, WaveformBucket,
    measure_peak_cancellable, render_wav_cancellable, render_waveform_progressive,
    split_cancellable, split_vgm_cancellable,
};

/// Identifies a task for cancel-on-resubmit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    RenderWaveform,
    /// File > Render to WAV.
    RenderWav,
    /// File > Split Channels.
    Split,
    /// File > Split Songs (one capture into its per-song files).
    SplitSongs,
    /// Measuring a song's peak level, for the volume lever's "Match" button and
    /// the VGM volume-modifier suggestion.
    VolumeScan,
    /// Measuring every pack track's peak, for the pack "Scan Volumes" button.
    PackVolumeScan,
    /// Edit > Find Loop: searching the command stream for loop candidates.
    LoopSearch,
}

/// A unit of background work, with everything it needs captured as an
/// immutable snapshot -- tasks never share the editor's song.
#[derive(Debug, Clone)]
pub enum TaskRequest {
    RenderWaveform {
        source: AudioSource,
        num_buckets: usize,
        sample_rate: u32,
        /// How a non-OPL engine reaches the output rate; ignored by an OPL
        /// source, whose engine renders at the chip's own rate.
        resampling: vgms_synth::resample::ResampleMode,
    },
    RenderWav {
        source: WavSource,
        /// Muting and panning are OPL ideas, so they only reach an OPL source;
        /// the boost applies to either.
        mix: RenderMix,
        sample_rate: u32,
        bit_depth: u16,
        /// As on [`TaskRequest::RenderWaveform`]: the export honours the same
        /// resampling choice playback does.
        resampling: vgms_synth::resample::ResampleMode,
    },
    Split {
        source: SplitTaskSource,
    },
    SplitSongs {
        source: SplitSource,
        /// The gap threshold, in the song's native delay unit (samples for a VGM,
        /// milliseconds for a DRO).
        threshold_native: u32,
        /// One flag per detected segment (in detection order); a `false` drops
        /// that segment from the export.
        included: Vec<bool>,
        /// Decay tail to keep after each piece, in native units.
        trailing_tail: u32,
    },
    /// Measures `song`'s peak level by rendering it internally at `sample_rate`.
    VolumeScan {
        song: Arc<Song>,
        sample_rate: u32,
    },
    /// Measures the peak of every `(file_name, song)` at `sample_rate`, for pack
    /// mode's "Scan Volumes". One background task covers the whole pack so its
    /// many songs never freeze the UI.
    PackVolumeScan {
        tracks: Vec<(String, Arc<Song>)>,
        sample_rate: u32,
    },
    /// Searches the loaded document for loop candidates at least
    /// `min_len_commands` delay-stripped commands long, for the Find Loop
    /// dialog. Runs in the background because a long song's search takes a
    /// moment.
    LoopSearch {
        source: LoopSearchSource,
        min_len_commands: usize,
    },
}

/// What a loop search runs over.
///
/// A loop is a block of commands that recurs, and what those commands *mean*
/// never enters into it -- so the search serves either representation, and only
/// the key-building differs.
#[derive(Debug, Clone)]
pub enum LoopSearchSource {
    Opl(Arc<Song>),
    Vgm(Arc<vgms_core::VgmFile>),
}

/// What a WAV render runs over.
///
/// The two engines take different mixes -- an OPL render can mute and pan, a
/// generic one has no register policy to do it with -- so the choice is made
/// here rather than inside the renderer.
#[derive(Debug, Clone)]
pub enum WavSource {
    Opl(Arc<Song>),
    Vgm(Arc<vgms_core::VgmFile>),
}

impl WavSource {
    /// The file name the render is offered under.
    fn name(&self) -> &str {
        match self {
            Self::Opl(song) => &song.name,
            Self::Vgm(file) => &file.name,
        }
    }
}

/// What a channel split runs over.
///
/// An OPL song splits per OPL channel (to WAV or captured song), reading the
/// register usage to skip untouched channels; a generic VGM splits per chip
/// channel to WAV, soloing each and keeping what sounds. The two take different
/// options, so the choice is made here.
#[derive(Debug, Clone)]
pub enum SplitTaskSource {
    Opl {
        song: Arc<Song>,
        options: SplitOptions,
    },
    Vgm {
        file: Arc<vgms_core::VgmFile>,
        options: VgmSplitOptions,
    },
}

/// What a song split runs over.
///
/// Where a capture goes silent is not an OPL question either, so this serves
/// both representations. The dialog holds one of these too: it re-runs detection
/// on every slider move, and the export re-runs it once more so the flags line
/// up with what was shown.
#[derive(Debug, Clone)]
pub enum SplitSource {
    Opl(Arc<Song>),
    Vgm(Arc<vgms_core::VgmFile>),
}

impl SplitSource {
    /// Delay units per second in this capture's native unit: 44100 for a VGM
    /// (samples), 1000 for a DRO (milliseconds).
    #[must_use]
    pub fn rate(&self) -> u32 {
        match self {
            Self::Opl(song) => vgms_core::split_songs::native_rate(song),
            Self::Vgm(_) => vgms_core::util::VGM_SAMPLE_RATE,
        }
    }

    /// The songs in the capture at `threshold` native units.
    #[must_use]
    pub fn detect(&self, threshold: u32) -> Vec<vgms_core::Segment> {
        match self {
            Self::Opl(song) => detect_segments(song, threshold),
            Self::Vgm(file) => detect_segments_in_vgm(file, threshold),
        }
    }

    /// Whether a piece can be auditioned before exporting. Previewing seeks
    /// playback, which needs a chip this app can actually render.
    #[must_use]
    pub fn can_preview(&self) -> bool {
        matches!(self, Self::Opl(_))
    }

    /// The file name each piece is numbered against, and its extension.
    fn stem_and_extension(&self) -> (&str, &'static str) {
        let (name, extension) = match self {
            Self::Opl(song) => (
                song.name.as_str(),
                if song.is_vgm() { "vgm" } else { "dro" },
            ),
            Self::Vgm(file) => (file.name.as_str(), "vgm"),
        };
        let stem = name
            .rsplit_once('.')
            .map_or(name, |(stem, _extension)| stem);
        (stem, extension)
    }
}

impl TaskRequest {
    #[must_use]
    pub fn kind(&self) -> TaskKind {
        match self {
            Self::RenderWaveform { .. } => TaskKind::RenderWaveform,
            Self::RenderWav { .. } => TaskKind::RenderWav,
            Self::Split { .. } => TaskKind::Split,
            Self::SplitSongs { .. } => TaskKind::SplitSongs,
            Self::VolumeScan { .. } => TaskKind::VolumeScan,
            Self::PackVolumeScan { .. } => TaskKind::PackVolumeScan,
            Self::LoopSearch { .. } => TaskKind::LoopSearch,
        }
    }
}

/// A finished task's product.
#[derive(Debug, Clone)]
pub enum TaskResult {
    Waveform(Vec<WaveformBucket>),
    /// The rendered WAV and the name to offer for it, or why it failed.
    ///
    /// The name is derived inside the task from the snapshot it rendered, so an
    /// edit (or a convert) while the render runs cannot mislabel the save dialog
    /// that follows.
    Wav(Result<(String, Vec<u8>), String>),
    /// One `(name, bytes)` per channel the song uses, ready to write, or why the
    /// split failed. Song-format outputs are serialised inside the task, so the
    /// app never has to know a DRO from a VGM to save them.
    Split(SplitFiles),
    /// One `(name, bytes)` per included song in the capture, ready to write, or
    /// why the split failed. Serialised inside the task, like [`Self::Split`].
    SplitSongs(SplitFiles),
    /// A finished volume scan's peak, for the "Match Volume" button and the VGM
    /// volume-modifier suggestion. A cancelled scan emits nothing.
    Peak(Peak),
    /// One `(file_name, peak)` per pack track measured, for the pack Peak column.
    /// A cancelled scan (the folder changed) emits nothing.
    PackPeaks(Vec<(String, Peak)>),
    /// The loop candidates found so far, best-first. Emitted as a growing ranked
    /// snapshot while the search streams, so the dialog's table fills in live; a
    /// cancelled search emits nothing.
    LoopCandidates(Vec<Candidate>),
}

/// A finished split's files, ready to write, or why it failed.
pub type SplitFiles = Result<Vec<(String, Vec<u8>)>, String>;

/// Schedules [`TaskRequest`]s off the UI thread.
///
/// Semantics: tasks are keyed by [`TaskKind`]; submitting cancels any pending
/// or running task of the same kind **and only that kind**; a debounced
/// submission only starts once the debounce elapses with no resubmission (so
/// holding Delete does not thrash the renderer).
pub trait TaskService {
    fn submit(&mut self, request: TaskRequest, debounce: Option<Duration>);

    fn cancel(&mut self, kind: TaskKind);

    /// Results of tasks that finished since the last poll. Called every frame
    /// from the update loop.
    fn poll(&mut self) -> Vec<TaskResult>;

    /// Whether anything is pending or running -- drives the status-bar
    /// indicator and repaint requests.
    fn is_busy(&self) -> bool;

    /// Whether work of this kind specifically is pending or running.
    ///
    /// Kinds run concurrently, so "busy" is not one thing: the status bar names
    /// what is actually running, and an export refuses to start a second copy of
    /// itself without blocking on the waveform render that always follows an
    /// edit. Required rather than defaulted, so an implementation cannot quietly
    /// answer "never busy" and let both slip through.
    fn is_busy_kind(&self, kind: TaskKind) -> bool;

    /// Cancels everything, for app shutdown.
    fn shutdown(&mut self) {}
}

/// Runs `request`, calling `emit` with each result it produces and checking
/// `is_cancelled` as it goes.
///
/// This is the platform-independent half of every `TaskService`: the native
/// implementation calls it on a `std::thread`, the web implementation inside a
/// Worker. A task may `emit` more than once -- the waveform render
/// emits progressive snapshots as it fills in, then the finished buckets -- and
/// emits nothing more once cancelled.
pub fn run_task(
    request: &TaskRequest,
    is_cancelled: &dyn Fn() -> bool,
    emit: &mut dyn FnMut(TaskResult),
) {
    match request {
        TaskRequest::RenderWaveform {
            source,
            num_buckets,
            sample_rate,
            resampling,
        } => {
            // A waveform is a picture of the audio, so it comes from whichever
            // engine would make that audio.
            match source {
                AudioSource::Opl(song) => {
                    render_waveform_progressive(
                        song,
                        *num_buckets,
                        *sample_rate,
                        &mut || !is_cancelled(),
                        &mut |buckets| emit(TaskResult::Waveform(buckets)),
                    );
                }
                AudioSource::Vgm(file) => {
                    vgms_synth::render_vgm_waveform_progressive(
                        Arc::clone(file),
                        *num_buckets,
                        *sample_rate,
                        *resampling,
                        &mut || !is_cancelled(),
                        &mut |buckets| emit(TaskResult::Waveform(buckets)),
                    );
                }
            }
        }
        TaskRequest::RenderWav {
            source,
            mix,
            sample_rate,
            bit_depth,
            resampling,
        } => {
            // `song.dro` becomes `song.dro.wav`, the name `vgmstudio render`
            // writes -- so the same song exported both ways lands in one place.
            let name = format!("{}.wav", source.name());
            let rendered = match source {
                WavSource::Opl(song) => render_wav_cancellable(
                    Arc::clone(song),
                    *mix,
                    *sample_rate,
                    *bit_depth,
                    &mut |_| {},
                    &mut || !is_cancelled(),
                ),
                WavSource::Vgm(file) => vgms_synth::render_vgm_wav_cancellable(
                    Arc::clone(file),
                    *sample_rate,
                    *bit_depth,
                    mix.boost,
                    *resampling,
                    &mut |_| {},
                    &mut || !is_cancelled(),
                ),
            }
            .map_err(crate::strings::tasks_render_wav_failed);
            // A cancelled render emits nothing at all, like the waveform's.
            match rendered {
                Ok(None) => {}
                Ok(Some(bytes)) => emit(TaskResult::Wav(Ok((name, bytes)))),
                Err(message) => emit(TaskResult::Wav(Err(message))),
            }
        }
        TaskRequest::Split { source } => {
            if let Some(result) = split_to_bytes(source, is_cancelled) {
                emit(TaskResult::Split(result));
            }
        }
        TaskRequest::SplitSongs {
            source,
            threshold_native,
            included,
            trailing_tail,
        } => {
            if let Some(result) = split_songs_to_bytes(
                source,
                *threshold_native,
                included,
                *trailing_tail,
                is_cancelled,
            ) {
                emit(TaskResult::SplitSongs(result));
            }
        }
        TaskRequest::VolumeScan { song, sample_rate } => {
            // A cancelled scan (the song was replaced, or a fresh scan started)
            // emits nothing, like the WAV render.
            if let Some(peak) =
                measure_peak_cancellable(Arc::clone(song), *sample_rate, &mut |_| {}, &mut || {
                    !is_cancelled()
                })
            {
                emit(TaskResult::Peak(peak));
            }
        }
        TaskRequest::PackVolumeScan {
            tracks,
            sample_rate,
        } => {
            let mut peaks = Vec::with_capacity(tracks.len());
            for (name, song) in tracks {
                // Abandon promptly (emitting nothing) if the folder changed under
                // us -- a whole-pack scan is easy to leave stale.
                let Some(peak) = measure_peak_cancellable(
                    Arc::clone(song),
                    *sample_rate,
                    &mut |_| {},
                    &mut || !is_cancelled(),
                ) else {
                    return;
                };
                peaks.push((name.clone(), peak));
            }
            emit(TaskResult::PackPeaks(peaks));
        }
        TaskRequest::LoopSearch {
            source,
            min_len_commands,
        } => {
            // Accumulate as the search streams, emitting a ranked snapshot each
            // time so the dialog's table fills in best-first while it runs. A
            // cancelled search never emits (find_loops stops before the first
            // candidate), like the volume scans above.
            let mut found: Vec<Candidate> = Vec::new();
            let mut on_candidate = |candidate| {
                found.push(candidate);
                let mut snapshot = found.clone();
                rank(&mut snapshot);
                emit(TaskResult::LoopCandidates(snapshot));
            };
            match source {
                LoopSearchSource::Opl(song) => {
                    find_loops(song, *min_len_commands, &mut on_candidate, is_cancelled);
                }
                LoopSearchSource::Vgm(file) => {
                    if let Some(stream) = file.stream() {
                        vgms_core::loopfind::find_loops_in_stream(
                            stream,
                            *min_len_commands,
                            &mut on_candidate,
                            is_cancelled,
                        );
                    }
                }
            }
        }
    }
}

/// Detects the songs in `song`, materialises each included one, and serialises it
/// to `NN <stem>.<ext>` bytes (a running number over the included songs; `ext` is
/// `vgm` for a VGM capture, `dro` for a DRO). `None` if cancelled part-way.
///
/// The detection is re-run here from `threshold_native` (the song's native delay
/// unit), so `included[i]` lines up with the same segment the dialog showed at
/// that threshold; a shorter or absent `included` treats the remaining segments
/// as kept. `trailing_tail` (native units) keeps up to that much decay after each
/// piece; state replay is always on.
fn split_songs_to_bytes(
    source: &SplitSource,
    threshold_native: u32,
    included: &[bool],
    trailing_tail: u32,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<SplitFiles> {
    let (stem, extension) = source.stem_and_extension();

    let mut files = Vec::new();
    let mut number = 1;
    for (index, segment) in source.detect(threshold_native).into_iter().enumerate() {
        if is_cancelled() {
            return None;
        }
        if !included.get(index).copied().unwrap_or(true) {
            continue;
        }
        let bytes = match source {
            SplitSource::Opl(song) => {
                let piece = materialise(song, &segment, true, trailing_tail);
                write_song(&piece).map_err(|error| error.to_string())
            }
            SplitSource::Vgm(file) => match materialise_vgm(file, &segment, true, trailing_tail) {
                Some(piece) => {
                    vgms_core::vgm::file::write(&piece).map_err(|error| error.to_string())
                }
                None => Err(crate::strings::tasks_song_not_extracted(number)),
            },
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => return Some(Err(error)),
        };
        files.push((track_file_name(number, stem, extension), bytes));
        number += 1;
    }
    Some(Ok(files))
}

/// Splits `song` and serialises each output, so what comes back is ready to
/// write wherever the user chose. `None` if the split was cancelled part-way.
fn split_to_bytes(source: &SplitTaskSource, is_cancelled: &dyn Fn() -> bool) -> Option<SplitFiles> {
    let outputs = match source {
        SplitTaskSource::Opl { song, options } => split_cancellable(
            song,
            options,
            &mut |channel| log::info!("split: skipping unused channel {channel:#05X}"),
            &mut |_, _| {},
            &mut || !is_cancelled(),
        ),
        SplitTaskSource::Vgm { file, options } => split_vgm_cancellable(
            file,
            options,
            &mut |name| log::info!("split: skipping silent channel {name}"),
            &mut |_, _| {},
            &mut || !is_cancelled(),
        ),
    };
    let outputs = match outputs {
        Ok(Some(outputs)) => outputs,
        Ok(None) => return None,
        Err(error) => return Some(Err(error.to_string())),
    };

    Some(
        outputs
            .into_iter()
            .map(|output| {
                let bytes = match output.data {
                    SplitData::Wav(bytes) => bytes,
                    SplitData::Song(song) => write_song(&song).map_err(|e| e.to_string())?,
                };
                Ok((output.name, bytes))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_song::tone_song;
    use vgms_synth::{render_wav_mixed, render_waveform};

    fn request(song: Song) -> TaskRequest {
        TaskRequest::RenderWaveform {
            source: AudioSource::Opl(Arc::new(song)),
            num_buckets: 32,
            sample_rate: 48_000,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
        }
    }

    fn collect(request: &TaskRequest, is_cancelled: impl Fn() -> bool) -> Vec<TaskResult> {
        let mut results = Vec::new();
        run_task(request, &is_cancelled, &mut |result| results.push(result));
        results
    }

    #[test]
    fn the_waveform_task_ends_at_the_batch_render() {
        let song = tone_song();
        let expected = render_waveform(&song, 32, 48_000);
        let results = collect(&request(song), || false);
        // Progressive snapshots first, the finished buckets last.
        assert!(!results.is_empty());
        let Some(TaskResult::Waveform(last)) = results.last() else {
            panic!("expected waveform buckets, got {results:?}")
        };
        assert_eq!(*last, expected);
    }

    #[test]
    fn a_cancelled_task_produces_nothing() {
        assert!(collect(&request(tone_song()), || true).is_empty());
    }

    #[test]
    fn the_volume_scan_emits_the_songs_peak() {
        let song = tone_song();
        let expected = vgms_synth::measure_peak(&song, 48_000);
        let scan = TaskRequest::VolumeScan {
            song: Arc::new(song),
            sample_rate: 48_000,
        };
        let results = collect(&scan, || false);
        assert!(
            matches!(results.as_slice(), [TaskResult::Peak(peak)] if *peak == expected),
            "expected one Peak matching the direct measurement, got {results:?}"
        );
        // A cancelled scan emits nothing.
        assert!(collect(&scan, || true).is_empty());
    }

    /// Two copies of one loop body, as a VGM, so the search has a repeat to find.
    fn looping_vgm() -> Song {
        use vgms_core::{OplType, VgmData, VgmMeta};
        let mut stream = Vec::new();
        for _ in 0..2 {
            for (reg, value) in [(0xA0u8, 0x11u8), (0xB0, 0x22), (0xA0, 0x33), (0xC0, 0x44)] {
                stream.extend_from_slice(&[0x5A, reg, value]); // OPL2 write
                stream.extend_from_slice(&[0x61, 0x20, 0x00]); // wait 32 samples
            }
        }
        let data = VgmData::new(stream).expect("valid VGM stream");
        Song::vgm(
            "loop.vgm".to_owned(),
            0x151,
            data,
            OplType::Opl2,
            VgmMeta::new(Vec::new()),
        )
    }

    #[test]
    fn the_loop_search_streams_ranked_candidates() {
        let search = TaskRequest::LoopSearch {
            source: LoopSearchSource::Opl(Arc::new(looping_vgm())),
            min_len_commands: 4,
        };
        let results = collect(&search, || false);
        let Some(TaskResult::LoopCandidates(candidates)) = results.last() else {
            panic!("expected loop candidates, got {results:?}");
        };
        // The body repeats once and runs to the end: the top candidate is "!".
        assert_eq!(candidates.first().map(|c| c.quality_label()), Some("!"));
    }

    #[test]
    fn a_cancelled_loop_search_emits_nothing() {
        let search = TaskRequest::LoopSearch {
            source: LoopSearchSource::Opl(Arc::new(looping_vgm())),
            min_len_commands: 4,
        };
        assert!(collect(&search, || true).is_empty());
    }

    #[test]
    fn the_pack_volume_scan_emits_a_peak_per_track() {
        let song = Arc::new(tone_song());
        let expected = vgms_synth::measure_peak(&*song, 48_000);
        let scan = TaskRequest::PackVolumeScan {
            tracks: vec![
                ("01.vgm".to_owned(), Arc::clone(&song)),
                ("02.vgm".to_owned(), Arc::clone(&song)),
            ],
            sample_rate: 48_000,
        };
        let results = collect(&scan, || false);
        let [TaskResult::PackPeaks(peaks)] = results.as_slice() else {
            panic!("expected one PackPeaks, got {results:?}");
        };
        assert_eq!(peaks.len(), 2);
        assert_eq!(peaks[0].0, "01.vgm");
        assert_eq!(peaks[0].1, expected);
        assert_eq!(peaks[1].0, "02.vgm");
        assert_eq!(peaks[1].1, expected);
        // A cancelled scan emits nothing at all, not a partial list.
        assert!(collect(&scan, || true).is_empty());
    }

    /// An abandoned export must produce nothing at all: its bytes belong to a
    /// song the user has moved on from, and a save dialog for it would be a
    /// surprise.
    #[test]
    fn a_cancelled_export_emits_nothing() {
        let wav = TaskRequest::RenderWav {
            source: WavSource::Opl(Arc::new(tone_song())),
            mix: RenderMix::default(),
            sample_rate: 48_000,
            bit_depth: 16,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
        };
        assert!(collect(&wav, || true).is_empty());

        let split = TaskRequest::Split {
            source: SplitTaskSource::Opl {
                song: Arc::new(tone_song()),
                options: SplitOptions {
                    format: vgms_synth::SplitFormat::Wav,
                    isolate_percussion: false,
                    audio: vgms_core::config::AudioConfig::default(),
                },
            },
        };
        assert!(collect(&split, || true).is_empty());
    }

    #[test]
    fn the_wav_task_renders_the_mix_and_names_the_file() {
        let song = tone_song();
        let expected = render_wav_mixed(&song, RenderMix::default(), 48_000, 16).unwrap();

        let results = collect(
            &TaskRequest::RenderWav {
                source: WavSource::Opl(Arc::new(song)),
                mix: RenderMix::default(),
                sample_rate: 48_000,
                bit_depth: 16,
                resampling: vgms_synth::resample::ResampleMode::Sinc,
            },
            || false,
        );
        let [TaskResult::Wav(Ok((name, bytes)))] = &results[..] else {
            panic!("expected one rendered WAV, got {results:?}")
        };
        // The CLI's own naming: `tone.dro` renders to `tone.dro.wav`.
        assert_eq!(name, "tone.dro.wav");
        assert_eq!(*bytes, expected);
    }

    // -- split songs -------------------------------------------------------

    #[test]
    fn a_song_split_names_a_numbered_vgm_per_song() {
        let song = crate::test_song::multi_song_capture();
        let files = split_songs_to_bytes(
            &SplitSource::Opl(Arc::new(song.clone())),
            33_075,
            &[true, true, true],
            0,
            &|| false,
        )
        .unwrap()
        .unwrap();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["01 capture.vgm", "02 capture.vgm", "03 capture.vgm"]
        );
        // Each piece reads back as a VGM.
        for (name, bytes) in &files {
            let piece = vgms_core::io::read_song(name, bytes).unwrap();
            assert!(piece.is_vgm(), "{name} should be a VGM");
        }
    }

    #[test]
    fn a_song_split_drops_excluded_segments_and_renumbers() {
        let song = crate::test_song::multi_song_capture();
        // Drop the middle song; the numbering must stay contiguous.
        let files = split_songs_to_bytes(
            &SplitSource::Opl(Arc::new(song.clone())),
            33_075,
            &[true, false, true],
            0,
            &|| false,
        )
        .unwrap()
        .unwrap();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["01 capture.vgm", "02 capture.vgm"]);
    }

    #[test]
    fn a_dro_song_split_writes_dro_pieces() {
        // A DRO capture yields `.dro` pieces (threshold and tail in milliseconds).
        let song = crate::test_song::multi_song_capture_dro();
        let files = split_songs_to_bytes(
            &SplitSource::Opl(Arc::new(song.clone())),
            750,
            &[true, true, true],
            0,
            &|| false,
        )
        .unwrap()
        .unwrap();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["01 capture.dro", "02 capture.dro", "03 capture.dro"]
        );
        for (name, bytes) in &files {
            let piece = vgms_core::io::read_song(name, bytes).unwrap();
            assert!(!piece.is_vgm(), "{name} should be a DRO");
        }
    }

    #[test]
    fn a_cancelled_song_split_emits_nothing() {
        let split = TaskRequest::SplitSongs {
            source: SplitSource::Opl(Arc::new(crate::test_song::multi_song_capture())),
            threshold_native: 33_075,
            included: vec![true, true, true],
            trailing_tail: 0,
        };
        assert!(collect(&split, || true).is_empty());
    }
}
