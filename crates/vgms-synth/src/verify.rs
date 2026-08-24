// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rendering two VGMs and proving they play the same samples.
//!
//! This is the runtime twin of `vgms-app`'s `optimize_parity` corpus test: the
//! same oracle (`VgmEngine` applies writes immediately at wait-boundaries, so a
//! sample difference is a real state change, never a phase artifact), moved out
//! of the test harness so an interactive per-track optimise can gate on it
//! before rewriting a user's file. See `docs/optimizer-rework-2026-08/PLAN.md`.
//!
//! # Why two threads, never one (D-orw-2)
//!
//! Stage 0 made the vendored cores' `rand`/`srand` draw from a *thread-local*
//! LCG, reseeded at every chip reset (`vgms-cores-libvgm/src/rng.rs`). Two
//! engines alternating chunk-by-chunk on a single thread would draw from that
//! one shared stream and desynchronise each other -- a false "differs" by
//! construction. So each side renders on its own thread: its own stream,
//! identically seeded when the engine is *constructed on that thread*. The
//! chunks meet at a bounded channel, which also caps memory at a few chunks
//! rather than two whole songs.
//!
//! # Coverage (D-orw-3)
//!
//! The intro plus one *extra* loop pass, not a fixed few seconds: a dropped
//! write can matter only on the second approach to the loop body, when the
//! chip's state differs from the first. So a looping file is rendered with its
//! loop region played [`VerifyOptions::loop_passes`] times (two by default),
//! and an unlooped file is rendered to its natural end. A pathological header
//! (an absurd delay total) is bounded by [`VerifyOptions::max_frames`] per
//! side, logged when it bites -- no silent caps.
//!
//! # Native only (D-orw-7)
//!
//! Verification doubles render time, and the web pack path has no per-track
//! action yet, so this module is not built for wasm at all.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

use vgms_core::VgmFile;
use vgms_core::vgm::VGM_SAMPLE_RATE;

use crate::clock::{LoopConfig, LoopCount};
use crate::vgm_engine::VgmEngine;

/// How many frames each render chunk carries. Large enough that the per-chunk
/// channel and comparison overhead is noise against the emulation, small enough
/// that a difference near the start bails almost immediately.
const CHUNK_FRAMES: usize = 8_192;

/// How many chunks may sit in a side's channel before its worker blocks. This
/// is the memory cap: a few chunks per side, never the whole song.
const CHANNEL_DEPTH: usize = 4;

/// The rate both sides render at by default. Parity is a comparison against
/// ourselves, so the value only has to be the same on both sides.
const DEFAULT_RATE: u32 = 44_100;

/// The default loop-region play count: the intro, then the body a second time
/// (D-orw-3). One would render only the first approach, which cannot expose a
/// second-pass-only regression.
const DEFAULT_LOOP_PASSES: u32 = 2;

/// The default per-side render ceiling, in seconds of audio -- 30 minutes. A
/// well-formed song reaches its own end long before this; the ceiling only
/// bounds a broken header whose delays never stop.
const DEFAULT_CEILING_SECS: u64 = 30 * 60;

/// How a verification is scoped: the rate to render at, how many times to play
/// a loop region, and the hard per-side frame ceiling.
#[derive(Debug, Clone, Copy)]
pub struct VerifyOptions {
    /// The output rate both sides render at.
    pub output_rate: u32,
    /// How many times a loop region is played in total (>= 1). `2` renders the
    /// body once from the intro and once from the wrap; `1` disables looping.
    pub loop_passes: u32,
    /// The hard ceiling on frames rendered per side, guarding a pathological
    /// header. Logged when it bites.
    pub max_frames: u64,
}

impl VerifyOptions {
    /// Options for `output_rate`, with the default loop passes and a 30-minute
    /// ceiling scaled to that rate.
    #[must_use]
    pub fn new(output_rate: u32) -> Self {
        let rate = output_rate.max(1);
        Self {
            output_rate: rate,
            loop_passes: DEFAULT_LOOP_PASSES,
            max_frames: DEFAULT_CEILING_SECS * u64::from(rate),
        }
    }
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self::new(DEFAULT_RATE)
    }
}

/// Whether two renders agreed, and where they first parted if not.
///
/// `sample`/`of` are indices into the interleaved-stereo `i16` streams (two per
/// frame), matching `optimize_parity`'s "differs at sample X of Y" message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every sample matched, to the end of both renders.
    Identical,
    /// The renders first disagreed at interleaved sample `sample`; `of` is the
    /// full length of the comparison, for context.
    DiffersAt { sample: u64, of: u64 },
}

impl Verdict {
    /// Whether the two files render to the same samples.
    #[must_use]
    pub const fn is_identical(self) -> bool {
        matches!(self, Self::Identical)
    }
}

/// Renders `original` and `candidate` through the real engine and reports
/// whether they play the same samples.
///
/// Each side renders on its own thread (D-orw-2), meeting the comparator at a
/// bounded channel; the comparison streams sample-by-sample and bails at the
/// first difference. Rendering uses the ambient core registry (as playback
/// does), so a caller must have installed cores -- with none, every chip
/// renders silence and every file trivially "matches", which is why this is
/// only ever called from a shell that has installed them.
#[must_use]
pub fn renders_identically(
    original: &Arc<VgmFile>,
    candidate: &Arc<VgmFile>,
    opts: VerifyOptions,
) -> Verdict {
    let rate = opts.output_rate.max(1);
    let cfg_a = verify_loop(original, rate, opts.loop_passes);
    let cfg_b = verify_loop(candidate, rate, opts.loop_passes);
    // The expected total is read off the original's timing, which the optimiser
    // preserves -- cheap context for the report without rendering to the end.
    let of = expected_interleaved(original, rate, cfg_a).min(opts.max_frames.saturating_mul(2));

    let a_file = Arc::clone(original);
    let b_file = Arc::clone(candidate);
    render_and_compare(
        move || configured(VgmEngine::new(a_file, rate), cfg_a),
        move || configured(VgmEngine::new(b_file, rate), cfg_b),
        opts.max_frames,
        of,
    )
}

/// Applies a loop config to a freshly-built engine, if the file has one.
fn configured(mut engine: VgmEngine, cfg: Option<LoopConfig>) -> VgmEngine {
    if let Some(cfg) = cfg {
        engine.set_loop(Some(cfg));
    }
    engine
}

/// The loop config that plays `file`'s loop region `passes` times, or `None`
/// when the file does not loop (or `passes <= 1`, i.e. one forward pass, which
/// needs no wrap).
fn verify_loop(file: &VgmFile, rate: u32, passes: u32) -> Option<LoopConfig> {
    if passes <= 1 {
        return None;
    }
    let start = file.loop_index()?;
    // A loop that runs to the end has no explicit end index; the region is then
    // [start, len), which the engine wraps at.
    let end = file.loop_end_index().unwrap_or_else(|| file.len());
    if start >= end || end > file.len() {
        return None;
    }
    let stream = file.stream()?;
    let start_frames = stream.samples_before(start) * u64::from(rate) / u64::from(VGM_SAMPLE_RATE);
    Some(LoopConfig {
        start,
        end,
        count: LoopCount::Times(passes),
        start_frames,
    })
}

/// The interleaved-sample length a full verification of `file` will cover: one
/// forward pass to the end, plus one loop body per extra pass, in frames × 2.
fn expected_interleaved(file: &VgmFile, rate: u32, cfg: Option<LoopConfig>) -> u64 {
    let Some(stream) = file.stream() else {
        return 0;
    };
    let samples = match cfg {
        None => stream.total_samples(),
        Some(cfg) => {
            let body = stream
                .samples_before(cfg.end)
                .saturating_sub(stream.samples_before(cfg.start));
            let extra = u64::from(cfg.count.wraps().unwrap_or(0));
            stream.total_samples() + body * extra
        }
    };
    (samples * u64::from(rate) / u64::from(VGM_SAMPLE_RATE)) * 2
}

/// Spawns a worker per side, streams their chunks through bounded channels, and
/// compares sample-by-sample with a first-difference early bail.
fn render_and_compare<A, B>(build_a: A, build_b: B, max_frames: u64, of: u64) -> Verdict
where
    A: FnOnce() -> VgmEngine + Send + 'static,
    B: FnOnce() -> VgmEngine + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let (tx_a, rx_a) = sync_channel::<Vec<i16>>(CHANNEL_DEPTH);
    let (tx_b, rx_b) = sync_channel::<Vec<i16>>(CHANNEL_DEPTH);

    let handle_a = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || render_side(build_a, &tx_a, &stop, max_frames))
    };
    let handle_b = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || render_side(build_b, &tx_b, &stop, max_frames))
    };

    let mut a = Side::new(rx_a);
    let mut b = Side::new(rx_b);
    let mut compared: u64 = 0;
    let verdict = loop {
        a.fill();
        b.fill();
        let sa = a.avail();
        let sb = b.avail();
        if sa.is_empty() || sb.is_empty() {
            // One side ran out. Equal lengths and no earlier difference is a
            // match; one side outliving the other is a difference at the point
            // the shorter stopped.
            break if sa.is_empty() && sb.is_empty() {
                Verdict::Identical
            } else {
                Verdict::DiffersAt {
                    sample: compared,
                    of: of.max(compared),
                }
            };
        }
        let n = sa.len().min(sb.len());
        let first_diff = sa[..n].iter().zip(&sb[..n]).position(|(x, y)| x != y);
        if let Some(k) = first_diff {
            break Verdict::DiffersAt {
                sample: compared + k as u64,
                of: of.max(compared + n as u64),
            };
        }
        compared += n as u64;
        a.consume(n);
        b.consume(n);
    };

    // Let the workers finish: set the flag, drain what they have already queued
    // so a blocked send unblocks, then join. Without the drain a worker parked
    // in `send` (its channel full) would never see the flag.
    stop.store(true, Ordering::Relaxed);
    a.drain();
    b.drain();
    let _ = handle_a.join();
    let _ = handle_b.join();
    verdict
}

/// Renders one side into `tx` until the engine ends, the ceiling bites, or the
/// comparator asks it to stop.
fn render_side(
    build: impl FnOnce() -> VgmEngine,
    tx: &SyncSender<Vec<i16>>,
    stop: &AtomicBool,
    max_frames: u64,
) {
    // Constructed here, on this thread, so the reset that seeds the thread-local
    // RNG happens on the thread that will draw from it (D-orw-2).
    let mut engine = build();
    let mut remaining = max_frames;
    while remaining > 0 && !stop.load(Ordering::Relaxed) {
        let want = remaining.min(CHUNK_FRAMES as u64) as usize;
        let mut buf = vec![0i16; want * 2];
        let rendered = engine.render(&mut buf);
        if rendered == 0 {
            return; // the song ended
        }
        buf.truncate(rendered * 2);
        remaining -= rendered as u64;
        if tx.send(buf).is_err() {
            return; // the comparator has stopped listening
        }
    }
    if remaining == 0 && !engine.is_finished() {
        log::warn!(
            "verify: hit the {max_frames}-frame render ceiling before the song ended; \
             the comparison is bounded to that length"
        );
    }
}

/// One side's incoming chunk stream, with a cursor into the current chunk.
struct Side {
    rx: Receiver<Vec<i16>>,
    cur: Vec<i16>,
    off: usize,
    closed: bool,
}

impl Side {
    fn new(rx: Receiver<Vec<i16>>) -> Self {
        Self {
            rx,
            cur: Vec::new(),
            off: 0,
            closed: false,
        }
    }

    /// Ensures [`Self::avail`] is non-empty if any data remains, pulling chunks
    /// until one has samples or the channel disconnects.
    fn fill(&mut self) {
        while self.off >= self.cur.len() && !self.closed {
            match self.rx.recv() {
                Ok(chunk) if chunk.is_empty() => {}
                Ok(chunk) => {
                    self.cur = chunk;
                    self.off = 0;
                }
                Err(_) => self.closed = true,
            }
        }
    }

    /// The unconsumed tail of the current chunk.
    fn avail(&self) -> &[i16] {
        &self.cur[self.off..]
    }

    /// Marks `n` samples of the current chunk consumed.
    fn consume(&mut self, n: usize) {
        self.off += n;
    }

    /// Discards everything still queued, so a worker blocked in `send` unblocks
    /// and can see the stop flag.
    fn drain(&mut self) {
        self.off = self.cur.len();
        while self.rx.recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::chip::ChipCore;
    use crate::testing::install_registry_with_stub;
    use vgms_core::vgm::ChipKind;

    /// A minimal walkable VGM declaring an SN76489, with `stream` as its body
    /// and an optional loop point at byte offset `loop_at` into the data.
    fn sn_vgm(stream: &[u8], loop_at: Option<usize>) -> Arc<VgmFile> {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        const DATA_START: usize = 0x100;
        let mut bytes = vec![0u8; DATA_START];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x171); // version
        put_u32(&mut bytes, 0x34, (DATA_START - 0x34) as u32); // data offset
        put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        if let Some(loop_at) = loop_at {
            // The loop points at an absolute byte; the header stores it relative
            // to 0x1C, and a big loop-sample count means "loops to the end".
            put_u32(&mut bytes, 0x1C, (DATA_START + loop_at - 0x1C) as u32);
            put_u32(&mut bytes, 0x20, u32::MAX);
        }
        bytes.extend_from_slice(stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        Arc::new(vgms_core::vgm::file::read("verify-test.vgm", &bytes).expect("a walkable VGM"))
    }

    /// A wait command for `samples` VGM samples (`0x61 llll`).
    fn wait(samples: u16) -> [u8; 3] {
        [0x61, samples as u8, (samples >> 8) as u8]
    }

    #[test]
    fn an_identical_file_verifies_as_identical() {
        install_registry_with_stub();
        // Channel 0 on, a wait, off, a wait.
        let mut body = vec![0x50, 0x90];
        body.extend_from_slice(&wait(20_000));
        body.extend_from_slice(&[0x50, 0x9F]);
        body.extend_from_slice(&wait(20_000));
        body.push(0x66);
        let a = sn_vgm(&body, None);
        let b = sn_vgm(&body, None);
        assert_eq!(
            renders_identically(&a, &b, VerifyOptions::default()),
            Verdict::Identical
        );
    }

    #[test]
    fn a_changed_register_is_caught() {
        install_registry_with_stub();
        let mut on = vec![0x50, 0x90]; // channel 0 to full volume (sounds)
        on.extend_from_slice(&wait(20_000));
        on.push(0x66);
        let mut off = vec![0x50, 0x9F]; // channel 0 silenced -- a different render
        off.extend_from_slice(&wait(20_000));
        off.push(0x66);

        let a = sn_vgm(&on, None);
        let b = sn_vgm(&off, None);
        let verdict = renders_identically(&a, &b, VerifyOptions::default());
        assert!(
            matches!(verdict, Verdict::DiffersAt { .. }),
            "a silenced channel must not verify as identical: {verdict:?}"
        );
    }

    #[test]
    fn a_difference_only_on_the_second_loop_pass_is_caught() {
        install_registry_with_stub();
        // Intro turns channel 0 on. The loop body re-asserts "on" (redundant on
        // the first pass, since the intro already left it on) and then, at the
        // end of the body, turns it off. On the wrap the body's re-assert is
        // what brings it back -- so a candidate that drops that re-assert as
        // "redundant" plays the body silent on the *second* pass only.
        let mut original = vec![0x50, 0x90]; // intro: channel 0 on
        original.extend_from_slice(&wait(5_000));
        let loop_at = original.len(); // loop starts here
        original.extend_from_slice(&[0x50, 0x90]); // body: re-assert on
        original.extend_from_slice(&wait(20_000));
        original.extend_from_slice(&[0x50, 0x9F]); // body end: off
        original.extend_from_slice(&wait(5_000));
        original.push(0x66);

        // The candidate drops the body's re-assert (bytes `0x50 0x90` at
        // `loop_at`), keeping the loop point on the following wait.
        let mut candidate = original.clone();
        candidate.drain(loop_at..loop_at + 2);
        let candidate_loop_at = loop_at; // now points at the wait

        let a = sn_vgm(&original, Some(loop_at));
        let b = sn_vgm(&candidate, Some(candidate_loop_at));

        // One pass would miss it (both play the body on the first approach);
        // two passes expose the divergence on the wrap.
        let one_pass = VerifyOptions {
            loop_passes: 1,
            ..VerifyOptions::default()
        };
        assert_eq!(
            renders_identically(&a, &b, one_pass),
            Verdict::Identical,
            "the first pass alone cannot see a second-pass regression"
        );
        let verdict = renders_identically(&a, &b, VerifyOptions::default());
        assert!(
            matches!(verdict, Verdict::DiffersAt { .. }),
            "the second loop pass must expose the dropped re-assert: {verdict:?}"
        );
    }

    // A core whose output depends on a thread-local stream reseeded at reset --
    // the shape of every vendored core that reaches for `rand` (NES noise, DMG
    // wave RAM). Two of these interleaved on one thread would desynchronise;
    // one per thread is deterministic. It models `vgms-cores-libvgm`'s rng.rs
    // without depending on that GPL crate.
    thread_local! {
        static DRIFT: Cell<u64> = const { Cell::new(0) };
    }

    #[derive(Debug)]
    struct DriftCore;

    impl ChipCore for DriftCore {
        fn reset(&mut self, _clock: u32, _variant: bool) {
            DRIFT.with(|state| state.set(0x1234_5678));
        }
        fn native_rate(&self) -> u32 {
            44_100
        }
        fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
        fn render(&mut self, out: &mut [i32]) {
            let value = DRIFT.with(|state| {
                let stepped = state
                    .get()
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                state.set(stepped);
                ((stepped >> 40) as i32 & 0x7FFF) - 0x4000
            });
            out.fill(value);
        }
    }

    /// The seam's own regression: with a core drawing from a thread-local stream
    /// reseeded at reset, two renders of the *same* file must verify identical.
    /// They can only do so because each side runs on its own thread with its own
    /// stream -- interleaving them on one thread (see below) desynchronises them.
    #[test]
    fn each_side_draws_from_its_own_thread_local_rng() {
        let mut body = vec![0x50u8, 0x90];
        body.extend_from_slice(&wait(30_000));
        body.push(0x66);
        let file = sn_vgm(&body, None);

        let build = || {
            let file = Arc::clone(&file);
            move || VgmEngine::with_cores(file, 44_100, |_| Some(Box::new(DriftCore)))
        };
        assert_eq!(
            render_and_compare(build(), build(), u64::from(u32::MAX), 0),
            Verdict::Identical,
            "each side must have its own thread-local RNG stream"
        );

        // And the trap the seam avoids: two engines interleaved on *this* thread
        // share the one stream, so identical files render differently. This is
        // exactly the false positive the two-thread design exists to prevent.
        let mut ea =
            VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| Some(Box::new(DriftCore)));
        let mut eb =
            VgmEngine::with_cores(Arc::clone(&file), 44_100, |_| Some(Box::new(DriftCore)));
        let (mut buf_a, mut buf_b) = (vec![0i16; 512], vec![0i16; 512]);
        let mut diverged = false;
        for _ in 0..8 {
            let na = ea.render(&mut buf_a);
            let nb = eb.render(&mut buf_b);
            if na != nb || buf_a[..na * 2] != buf_b[..nb * 2] {
                diverged = true;
                break;
            }
        }
        assert!(
            diverged,
            "interleaving on one thread must desynchronise the shared stream -- \
             if it does not, this test no longer proves the seam is load-bearing"
        );
    }

    #[test]
    fn the_ceiling_bounds_the_comparison() {
        install_registry_with_stub();
        // A very long silence: a wait far past the ceiling we set.
        let mut body = vec![0x50u8, 0x9F];
        for _ in 0..64 {
            body.extend_from_slice(&wait(60_000));
        }
        body.push(0x66);
        let a = sn_vgm(&body, None);
        let b = sn_vgm(&body, None);
        // A tiny ceiling: both sides are cut to it, and still compare identical.
        let opts = VerifyOptions {
            max_frames: 4_096,
            ..VerifyOptions::default()
        };
        assert_eq!(
            renders_identically(&a, &b, opts),
            Verdict::Identical,
            "the ceiling caps length; identical stays identical"
        );
    }
}
