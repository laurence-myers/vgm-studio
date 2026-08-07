//! Background-task definitions and the `TaskService` trait.
//!
//! The register analysis is `vgms-core`'s synchronous replay cursor, so the
//! waveform render is the only background task left. The task *logic* lives
//! here, shared by every platform; the *scheduling* -- threads natively, Web
//! Workers later -- lives behind [`TaskService`].

use core::time::Duration;
use std::sync::Arc;

#[cfg(test)]
use vgms_core::DroSong;
use vgms_core::VgmFile;
use vgms_core::convert::opl_song_to_vgm_file;
use vgms_core::io::write_song;
use vgms_core::loopfind::{Candidate, find_loops, rank};
use vgms_core::pack::naming::track_file_name;
use vgms_core::split_songs::{materialise, materialise_vgm};
use vgms_synth::{
    AudioSource, CoreChoices, Peak, SplitData, VgmRenderMix, VgmSplitOptions, WaveformBucket,
    measure_vgm_peak_cancellable, render_vgm_waveform_progressive, split_vgm_cancellable,
};

/// Projects a source to the VGM the one engine plays: a DRO becomes its
/// primed OPL VGM ([`opl_song_to_vgm_file`]), a VGM is taken as is. So every
/// background render/scan runs the same code path live playback does, and a
/// DRO's export matches what it sounds like. `None` only if a (valid) DRO
/// somehow fails to project.
fn as_vgm(source: &AudioSource) -> Option<Arc<VgmFile>> {
    match source {
        AudioSource::Vgm(file) => Some(Arc::clone(file)),
        AudioSource::Dro(song) => opl_song_to_vgm_file(song).ok().map(Arc::new),
    }
}

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
        /// The per-chip mix to bake in -- the channel mutes and pan positions the
        /// cores apply. A DRO's panel produces this keyed to its projection
        /// chips, so one vocabulary serves both source arms.
        mix: VgmRenderMix,
        sample_rate: u32,
        bit_depth: u16,
        /// As on [`TaskRequest::RenderWaveform`]: the export honours the same
        /// resampling choice playback does.
        resampling: vgms_synth::resample::ResampleMode,
        /// The per-render core choices (slot slug -> core short-name), seeded
        /// from Settings but never persisted: the render is wrapped in
        /// [`with_render_choices`](vgms_synth::with_render_choices) so a picked
        /// core is used without disturbing playback. Empty means the configured
        /// cores, keeping the default render byte-identical.
        core_choices: CoreChoices,
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
    /// Measures `source`'s peak level by rendering it internally at
    /// `sample_rate`. Either representation is measured, through its own engine.
    VolumeScan {
        source: AudioSource,
        sample_rate: u32,
        /// As on [`TaskRequest::RenderWaveform`]: how a non-OPL engine reaches
        /// the output rate; ignored by an OPL source.
        resampling: vgms_synth::resample::ResampleMode,
    },
    /// Measures the peak of every `(file_name, source)` at `sample_rate`, for
    /// pack mode's "Scan Volumes". One background task covers the whole pack so
    /// its many tracks never freeze the UI.
    PackVolumeScan {
        tracks: Vec<(String, AudioSource)>,
        sample_rate: u32,
        /// As on [`TaskRequest::VolumeScan`].
        resampling: vgms_synth::resample::ResampleMode,
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

/// What a loop search runs over: the loaded document, either shape. A loop is a
/// block of commands that recurs, and what those commands *mean* never enters
/// into it -- so the search serves either representation, and only the
/// key-building differs.
pub type LoopSearchSource = vgms_core::DocSource;

/// What a WAV render runs over: the loaded document, either shape. The two
/// engines take different mixes -- an OPL render mutes and pans by register
/// policy, a generic one by per-chip masks the cores apply -- so the choice is
/// made here rather than inside the renderer.
pub type WavSource = vgms_core::DocSource;

/// What a channel split runs over.
///
/// A split runs over a VGM: a multichip rip directly, or an OPL document over a
/// VGM projection of its register stream (ou-4). The app resolves an OPL
/// document to its file -- an OPL VGM keeps its own header, a DRO projects -- and
/// translates the OPL mixer's mutes/pans before this point.
#[derive(Debug, Clone)]
pub enum SplitTaskSource {
    Vgm {
        file: Arc<VgmFile>,
        options: VgmSplitOptions,
    },
}

/// What a song split runs over: the loaded document, either shape. Where a
/// capture goes silent is not an OPL question either, so this serves both. The
/// dialog holds one too: it re-runs detection on every slider move, and the
/// export re-runs it once more so the flags line up with what was shown.
///
/// `rate`, `detect` and `stem_and_extension` are on [`vgms_core::DocSource`];
/// `can_preview` stays here (below), being UI policy about what a core can play.
pub type SplitSource = vgms_core::DocSource;

/// Whether a split piece can be auditioned before exporting. Previewing seeks
/// playback, which needs something this app can render -- so this tracks
/// renderability, not OPL-ness, mirroring `Editor::renderable` exactly (Split
/// routes OPL VGMs down the `Vgm` arm too). A VGM renders if it projects to an
/// OPL stream *or* its chips have a core; an OPL projection always plays. It is a
/// free function, not a `DocSource` method, because it needs `vgms-synth` -- core
/// policy stays in `vgms-core`, UI policy here. Called once at dialog
/// construction, so the projection it may do is not a per-frame cost.
#[must_use]
pub(crate) fn can_preview(source: &vgms_core::DocSource) -> bool {
    use vgms_core::DocSource;
    match source {
        DocSource::Dro(_) => true,
        DocSource::Vgm(file) => {
            // An OPL VGM plays through the same VgmEngine path as any VGM now, so
            // its chips answer `playability` like the rest -- no projection probe.
            let chips: Vec<_> = file.header.chips().iter().map(|chip| chip.kind).collect();
            vgms_synth::playability(&chips).can_play()
        }
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
    /// split failed. DroSong-format outputs are serialised inside the task, so the
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
/// Measures a peak through the one engine that renders `source` -- a DRO via
/// its projection ([`as_vgm`]), a VGM directly. `None` iff the scan was
/// cancelled, or a DRO fails to project. Shared by the single and pack scans.
fn measure_source(
    source: &AudioSource,
    sample_rate: u32,
    resampling: vgms_synth::resample::ResampleMode,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<Peak> {
    measure_vgm_peak_cancellable(
        as_vgm(source)?,
        sample_rate,
        resampling,
        &mut |_| {},
        &mut || !is_cancelled(),
    )
}

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
            // A waveform is a picture of the audio, so it comes from the one
            // engine that makes that audio -- a DRO through its projection.
            let Some(file) = as_vgm(source) else { return };
            render_vgm_waveform_progressive(
                file,
                *num_buckets,
                *sample_rate,
                *resampling,
                &mut || !is_cancelled(),
                &mut |buckets| emit(TaskResult::Waveform(buckets)),
            );
        }
        TaskRequest::RenderWav {
            source,
            mix,
            sample_rate,
            bit_depth,
            resampling,
            core_choices,
        } => {
            // `song.dro` becomes `song.dro.wav`, the name `vgmstudio render`
            // writes -- so the same song exported both ways lands in one place.
            let name = format!("{}.wav", source.name());
            // The chosen cores are active for this render only, on this thread
            // only (renders run off the UI thread), so playback and Settings are
            // untouched. An empty map behaves exactly as the configured cores.
            let rendered = vgms_synth::with_render_choices(Some(core_choices.clone()), || {
                // One engine, like playback: a DRO renders through its projection,
                // a VGM directly, both taking the per-chip mix.
                let file = as_vgm(source)?;
                Some(vgms_synth::render_vgm_wav_mixed_cancellable(
                    file,
                    *sample_rate,
                    *bit_depth,
                    mix,
                    *resampling,
                    &mut |_| {},
                    &mut || !is_cancelled(),
                ))
            });
            // A DRO that fails to project renders nothing at all.
            let Some(rendered) = rendered else { return };
            let rendered = rendered.map_err(crate::strings::tasks_render_wav_failed);
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
        TaskRequest::VolumeScan {
            source,
            sample_rate,
            resampling,
        } => {
            // A cancelled scan (the song was replaced, or a fresh scan started)
            // emits nothing, like the WAV render.
            if let Some(peak) = measure_source(source, *sample_rate, *resampling, is_cancelled) {
                emit(TaskResult::Peak(peak));
            }
        }
        TaskRequest::PackVolumeScan {
            tracks,
            sample_rate,
            resampling,
        } => {
            let mut peaks = Vec::with_capacity(tracks.len());
            for (name, source) in tracks {
                // Abandon promptly (emitting nothing) if the folder changed under
                // us -- a whole-pack scan is easy to leave stale.
                let Some(peak) = measure_source(source, *sample_rate, *resampling, is_cancelled)
                else {
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
            // Accumulate as the search streams, emitting a ranked snapshot so the
            // dialog's table fills in best-first while it runs. Cloning and ranking
            // the whole set on *every* candidate is O(n^2) and floods the result
            // channel, so throttle to at most one emit per EMIT_STRIDE candidates
            // -- the strided pacing the progressive waveform render uses -- with a
            // final emit for the tail. A cancelled search never emits (find_loops
            // stops before the first candidate), like the volume scans above.
            const EMIT_STRIDE: usize = 16;
            let mut found: Vec<Candidate> = Vec::new();
            let mut emitted_len = 0usize;
            let mut on_candidate = |candidate| {
                found.push(candidate);
                if found.len() - emitted_len >= EMIT_STRIDE {
                    emitted_len = found.len();
                    let mut snapshot = found.clone();
                    rank(&mut snapshot);
                    emit(TaskResult::LoopCandidates(snapshot));
                }
            };
            match source {
                LoopSearchSource::Dro(song) => {
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
            // A final ranked snapshot so the last (fewer than a stride) candidates
            // show. Skipped when cancelled, so a superseded search emits nothing
            // new, and when nothing remains unshown.
            if !is_cancelled() && found.len() != emitted_len {
                rank(&mut found);
                emit(TaskResult::LoopCandidates(found));
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
            SplitSource::Dro(song) => {
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
    // Each arm's per-render core choices ride its options; wrapping the whole
    // split in `with_render_choices` makes every channel's render honour them,
    // on this thread only, without touching playback or Settings. An empty map
    // renders exactly as the configured cores would.
    let SplitTaskSource::Vgm { file, options } = source;
    let outputs = vgms_synth::with_render_choices(Some(options.core_choices.clone()), || {
        split_vgm_cancellable(
            file,
            options,
            &mut |name| log::info!("split: skipping silent channel {name}"),
            &mut |_, _| {},
            &mut || !is_cancelled(),
        )
    });
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
                    SplitData::Vgm(file) => {
                        vgms_core::vgm::file::write(&file).map_err(|e| e.to_string())?
                    }
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
    use vgms_synth::{measure_vgm_peak, render_vgm_waveform};

    fn request(song: DroSong) -> TaskRequest {
        TaskRequest::RenderWaveform {
            source: AudioSource::Dro(Arc::new(song)),
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
        // The task projects the DRO and renders through the one engine, so the
        // oracle is that same projected waveform.
        let file = Arc::new(opl_song_to_vgm_file(&song).unwrap());
        let expected =
            render_vgm_waveform(file, 32, 48_000, vgms_synth::resample::ResampleMode::Sinc);
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
        let file = Arc::new(opl_song_to_vgm_file(&song).unwrap());
        let expected = measure_vgm_peak(file, 48_000, vgms_synth::resample::ResampleMode::Sinc);
        let scan = TaskRequest::VolumeScan {
            source: AudioSource::Dro(Arc::new(song)),
            sample_rate: 48_000,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
        };
        let results = collect(&scan, || false);
        assert!(
            matches!(results.as_slice(), [TaskResult::Peak(peak)] if *peak == expected),
            "expected one Peak matching the direct measurement, got {results:?}"
        );
        // A cancelled scan emits nothing.
        assert!(collect(&scan, || true).is_empty());
    }

    /// A minimal SN76489 VGM held as a file, for the generic scan arm.
    fn sms_vgm_file() -> Arc<VgmFile> {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let stream: &[u8] = &[
            0x50, 0x8E, 0x50, 0x0F, 0x50, 0x90, // tone 0, full volume
            0x61, 0x44, 0xAC, // a second
            0x66,
        ];
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x171);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        put_u32(
            &mut bytes,
            vgms_core::ChipKind::Sn76489.clock_offset(),
            3_579_545,
        );
        bytes.extend_from_slice(stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        Arc::new(vgms_core::vgm::file::read("test.vgm", &bytes).expect("a walkable VGM"))
    }

    #[test]
    fn the_volume_scan_measures_a_vgm_through_the_generic_engine() {
        // The VGM arm is wired through the multichip engine: one Peak comes back
        // (its level depending on whether a core is installed in this process),
        // and a cancelled scan emits nothing, exactly like the OPL arm.
        let scan = TaskRequest::VolumeScan {
            source: AudioSource::Vgm(sms_vgm_file()),
            sample_rate: 44_100,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
        };
        let results = collect(&scan, || false);
        assert!(
            matches!(results.as_slice(), [TaskResult::Peak(_)]),
            "expected one Peak from the generic engine, got {results:?}"
        );
        assert!(collect(&scan, || true).is_empty());
    }

    /// Two copies of one loop body, as an OPL VGM `VgmFile`, so the search has a
    /// repeat to find.
    fn looping_vgm() -> VgmFile {
        use vgms_core::vgm::io::synthesise_header;
        let mut stream = Vec::new();
        for _ in 0..2 {
            for (reg, value) in [(0xA0u8, 0x11u8), (0xB0, 0x22), (0xA0, 0x33), (0xC0, 0x44)] {
                stream.extend_from_slice(&[0x5A, reg, value]); // OPL2 write
                stream.extend_from_slice(&[0x61, 0x20, 0x00]); // wait 32 samples
            }
        }
        let mut bytes = synthesise_header();
        bytes[0x50..0x54].copy_from_slice(&3_579_545u32.to_le_bytes());
        bytes.extend_from_slice(&stream);
        bytes.push(0x66);
        let eof = (bytes.len() - 0x04) as u32;
        bytes[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
        vgms_core::vgm::file::read("loop.vgm", &bytes).expect("a walkable OPL VGM")
    }

    #[test]
    fn the_loop_search_streams_ranked_candidates() {
        let search = TaskRequest::LoopSearch {
            source: LoopSearchSource::Vgm(Arc::new(looping_vgm())),
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
            source: LoopSearchSource::Vgm(Arc::new(looping_vgm())),
            min_len_commands: 4,
        };
        assert!(collect(&search, || true).is_empty());
    }

    /// Throttling the streamed snapshots must not drop the tail: the final emit
    /// holds every candidate a direct, un-throttled search finds (sw-11).
    #[test]
    fn the_final_loop_snapshot_holds_every_candidate() {
        let file = looping_vgm();
        let stream = file.stream().expect("a walkable OPL VGM");
        let mut expected = Vec::new();
        vgms_core::loopfind::find_loops_in_stream(stream, 4, &mut |c| expected.push(c), &|| false);
        assert!(!expected.is_empty(), "the fixture has candidates to lose");

        let search = TaskRequest::LoopSearch {
            source: LoopSearchSource::Vgm(Arc::new(file)),
            min_len_commands: 4,
        };
        let results = collect(&search, || false);
        let Some(TaskResult::LoopCandidates(candidates)) = results.last() else {
            panic!("expected loop candidates, got {results:?}");
        };
        assert_eq!(
            candidates.len(),
            expected.len(),
            "no candidate lost to throttling"
        );
    }

    #[test]
    fn the_pack_volume_scan_emits_a_peak_per_track() {
        let song = Arc::new(tone_song());
        let file = Arc::new(opl_song_to_vgm_file(&song).unwrap());
        let expected = measure_vgm_peak(file, 48_000, vgms_synth::resample::ResampleMode::Sinc);
        let scan = TaskRequest::PackVolumeScan {
            tracks: vec![
                ("01.vgm".to_owned(), AudioSource::Dro(Arc::clone(&song))),
                ("02.vgm".to_owned(), AudioSource::Dro(Arc::clone(&song))),
            ],
            sample_rate: 48_000,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
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
            source: WavSource::Dro(Arc::new(tone_song())),
            mix: VgmRenderMix {
                muting: vgms_synth::ChipMuting::new(),
                panning: vgms_synth::ChipPanning::new(),
                boost: 1.0,
            },
            sample_rate: 48_000,
            bit_depth: 16,
            resampling: vgms_synth::resample::ResampleMode::Sinc,
            core_choices: CoreChoices::new(),
        };
        assert!(collect(&wav, || true).is_empty());

        let file = opl_song_to_vgm_file(&tone_song()).unwrap();
        let split = TaskRequest::Split {
            source: SplitTaskSource::Vgm {
                file: Arc::new(file),
                options: VgmSplitOptions {
                    format: vgms_synth::SplitFormat::Wav,
                    audio: vgms_core::config::AudioConfig::default(),
                    resampling: vgms_synth::resample::ResampleMode::Sinc,
                    panning: vgms_synth::ChipPanning::new(),
                    boost: 1.0,
                    skip_muted: None,
                    core_choices: CoreChoices::new(),
                },
            },
        };
        assert!(collect(&split, || true).is_empty());
    }

    #[test]
    fn the_wav_task_renders_the_mix_and_names_the_file() {
        let song = tone_song();
        // The task renders the projected VGM through the one engine.
        let file = Arc::new(opl_song_to_vgm_file(&song).unwrap());
        let mix = VgmRenderMix {
            muting: vgms_synth::ChipMuting::new(),
            panning: vgms_synth::ChipPanning::new(),
            boost: 1.0,
        };
        let expected = vgms_synth::render_vgm_wav_mixed_cancellable(
            file,
            48_000,
            16,
            &mix,
            vgms_synth::resample::ResampleMode::Sinc,
            &mut |_| {},
            &mut || true,
        )
        .unwrap()
        .unwrap();

        let results = collect(
            &TaskRequest::RenderWav {
                source: WavSource::Dro(Arc::new(song)),
                mix,
                sample_rate: 48_000,
                bit_depth: 16,
                resampling: vgms_synth::resample::ResampleMode::Sinc,
                core_choices: CoreChoices::new(),
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
            &SplitSource::Vgm(Arc::new(song.clone())),
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
            assert!(
                vgms_core::vgm::file::read(name, bytes).is_ok(),
                "{name} should be a VGM"
            );
        }
    }

    #[test]
    fn a_song_split_drops_excluded_segments_and_renumbers() {
        let song = crate::test_song::multi_song_capture();
        // Drop the middle song; the numbering must stay contiguous.
        let files = split_songs_to_bytes(
            &SplitSource::Vgm(Arc::new(song.clone())),
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
            &SplitSource::Dro(Arc::new(song.clone())),
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
        // `read_song` accepts only DROs, so a piece reading back at all is the proof.
        for (name, bytes) in &files {
            vgms_core::io::read_song(name, bytes)
                .unwrap_or_else(|e| panic!("{name} should be a DRO: {e}"));
        }
    }

    #[test]
    fn a_cancelled_song_split_emits_nothing() {
        let split = TaskRequest::SplitSongs {
            source: SplitSource::Vgm(Arc::new(crate::test_song::multi_song_capture())),
            threshold_native: 33_075,
            included: vec![true, true, true],
            trailing_tail: 0,
        };
        assert!(collect(&split, || true).is_empty());
    }
}
