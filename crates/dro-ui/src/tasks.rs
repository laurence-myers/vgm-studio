//! Background-task definitions and the `TaskService` trait (Python's
//! `TaskMaster`, `tasks.py`).
//!
//! The Python ran two tasks: the detailed register analysis and the waveform
//! render. The analysis became `dro-core`'s synchronous replay cursor, so the
//! waveform render is the only background task left. The task *logic* lives
//! here, shared by every platform; the *scheduling* -- threads natively, Web
//! Workers later -- lives behind [`TaskService`].

use core::time::Duration;
use std::sync::Arc;

use dro_core::Song;
use dro_core::io::write_song;
use dro_synth::{
    RenderMix, SplitData, SplitOptions, WaveformBucket, render_wav_cancellable,
    render_waveform_progressive, split_cancellable,
};

/// Identifies a task for cancel-on-resubmit, as the Python keyed its registry
/// by task name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    RenderWaveform,
    /// File > Render to WAV.
    RenderWav,
    /// File > Split Channels.
    Split,
}

/// A unit of background work, with everything it needs captured as an
/// immutable snapshot -- tasks never share the editor's song.
#[derive(Debug, Clone)]
pub enum TaskRequest {
    RenderWaveform {
        song: Arc<Song>,
        num_buckets: usize,
        sample_rate: u32,
    },
    RenderWav {
        song: Arc<Song>,
        mix: RenderMix,
        sample_rate: u32,
        bit_depth: u16,
    },
    Split {
        song: Arc<Song>,
        options: SplitOptions,
    },
}

impl TaskRequest {
    #[must_use]
    pub fn kind(&self) -> TaskKind {
        match self {
            Self::RenderWaveform { .. } => TaskKind::RenderWaveform,
            Self::RenderWav { .. } => TaskKind::RenderWav,
            Self::Split { .. } => TaskKind::Split,
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
}

/// A finished split's files, ready to write, or why it failed.
pub type SplitFiles = Result<Vec<(String, Vec<u8>)>, String>;

/// Schedules [`TaskRequest`]s off the UI thread.
///
/// Semantics ported from the Python `TaskMaster`: tasks are keyed by
/// [`TaskKind`]; submitting cancels any pending or running task of the same
/// kind **and only that kind**; a debounced submission only starts once the
/// debounce elapses with no resubmission (so holding Delete does not thrash the
/// renderer).
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
/// implementation calls it on a `std::thread`, the web implementation (Step 10)
/// inside a Worker. A task may `emit` more than once -- the waveform render
/// emits progressive snapshots as it fills in, then the finished buckets -- and
/// emits nothing more once cancelled.
pub fn run_task(
    request: &TaskRequest,
    is_cancelled: &dyn Fn() -> bool,
    emit: &mut dyn FnMut(TaskResult),
) {
    match request {
        TaskRequest::RenderWaveform {
            song,
            num_buckets,
            sample_rate,
        } => {
            render_waveform_progressive(
                song,
                *num_buckets,
                *sample_rate,
                &mut || !is_cancelled(),
                &mut |buckets| emit(TaskResult::Waveform(buckets)),
            );
        }
        TaskRequest::RenderWav {
            song,
            mix,
            sample_rate,
            bit_depth,
        } => {
            // `song.dro` becomes `song.dro.wav`, the name `drotrim render`
            // writes -- so the same song exported both ways lands in one place.
            let name = format!("{}.wav", song.name);
            let rendered = render_wav_cancellable(
                Arc::clone(song),
                *mix,
                *sample_rate,
                *bit_depth,
                &mut |_| {},
                &mut || !is_cancelled(),
            )
            .map_err(|e| format!("Rendering to WAV failed: {e}"));
            // A cancelled render emits nothing at all, like the waveform's.
            match rendered {
                Ok(None) => {}
                Ok(Some(bytes)) => emit(TaskResult::Wav(Ok((name, bytes)))),
                Err(message) => emit(TaskResult::Wav(Err(message))),
            }
        }
        TaskRequest::Split { song, options } => {
            if let Some(result) = split_to_bytes(song, *options, is_cancelled) {
                emit(TaskResult::Split(result));
            }
        }
    }
}

/// Splits `song` and serialises each output, so what comes back is ready to
/// write wherever the user chose. `None` if the split was cancelled part-way.
fn split_to_bytes(
    song: &Song,
    options: SplitOptions,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<SplitFiles> {
    let outputs = match split_cancellable(
        song,
        &options,
        &mut |channel| log::info!("split: skipping unused channel {channel:#05X}"),
        &mut |_, _| {},
        &mut || !is_cancelled(),
    ) {
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
    use dro_synth::{render_wav_mixed, render_waveform};

    fn request(song: Song) -> TaskRequest {
        TaskRequest::RenderWaveform {
            song: Arc::new(song),
            num_buckets: 32,
            sample_rate: 48_000,
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

    /// An abandoned export must produce nothing at all: its bytes belong to a
    /// song the user has moved on from, and a save dialog for it would be a
    /// surprise.
    #[test]
    fn a_cancelled_export_emits_nothing() {
        let wav = TaskRequest::RenderWav {
            song: Arc::new(tone_song()),
            mix: RenderMix::default(),
            sample_rate: 48_000,
            bit_depth: 16,
        };
        assert!(collect(&wav, || true).is_empty());

        let split = TaskRequest::Split {
            song: Arc::new(tone_song()),
            options: SplitOptions {
                format: dro_synth::SplitFormat::Wav,
                isolate_percussion: false,
                audio: dro_core::config::AudioConfig::default(),
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
                song: Arc::new(song),
                mix: RenderMix::default(),
                sample_rate: 48_000,
                bit_depth: 16,
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
}
