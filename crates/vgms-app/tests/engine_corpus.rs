//! The multi-chip engine, driven over every VGM on this machine.
//!
//! With no cores here, this cannot check what anything *sounds* like, but it
//! checks everything around that against real files: that a stream this app
//! agreed to open always walks to its end, renders for exactly as long as its
//! waits say, and never panics on real data blocks, DAC streams or compressed
//! banks -- the shapes rippers actually produced, including ones the spec did
//! not anticipate.
//!
//! Ignored by default and pointed at a corpus with `VGMSTUDIO_VGMRIPS_CORPUS`:
//!
//! ```text
//! VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM cargo test -p vgms-app --release \
//!     --test engine_corpus -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use vgms_core::vgm::ChipKind;
use vgms_synth::chip::ChipCore;
use vgms_synth::vgm_engine::VgmEngine;

mod common;

/// A core that renders a value derived from its writes, so a file whose
/// commands never reach a chip is distinguishable from one whose do.
struct Counting {
    writes: u64,
}

impl ChipCore for Counting {
    fn reset(&mut self, _clock: u32, _variant: bool) {
        self.writes = 0;
    }

    fn native_rate(&self) -> u32 {
        44_100
    }

    fn write(&mut self, _port: u8, _addr: u16, _data: u16) {
        self.writes += 1;
    }

    fn render(&mut self, out: &mut [i32]) {
        // Non-zero once it has been written to, so the field is not dead and a
        // chip that never receives its writes is audibly (and testably) distinct.
        out.fill(i32::from(self.writes > 0));
    }
}

/// A core that tallies its writes into a shared per-chip map, so a test can
/// prove each chip's writes reached *it* and not a sibling.
struct Recording {
    kind: ChipKind,
    seen: Arc<Mutex<HashMap<ChipKind, u64>>>,
}

impl ChipCore for Recording {
    fn reset(&mut self, _clock: u32, _variant: bool) {}

    fn native_rate(&self) -> u32 {
        44_100
    }

    fn write(&mut self, _port: u8, _addr: u16, _data: u16) {
        *self.seen.lock().unwrap().entry(self.kind).or_insert(0) += 1;
    }

    fn render(&mut self, out: &mut [i32]) {
        out.fill(0);
    }
}

/// A cheap, non-ignored gate: a two-chip VGM must route each chip's writes to
/// that chip. The whole-corpus walk below cannot see this -- its `Counting` core
/// renders regardless of where a write landed -- so misrouting needs its own
/// fixture-scale check that runs in ordinary `cargo test`.
#[test]
fn each_chips_writes_reach_that_chip() {
    fn put(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    // A Mega Drive header: SN76489 + YM2612, with two SN writes and three YM2612
    // writes in the stream.
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put(&mut bytes, 0x08, 0x151);
    put(&mut bytes, 0x34, 0x100 - 0x34);
    put(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
    put(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
    bytes.extend_from_slice(&[
        0x50, 0x9F, // SN76489 write
        0x50, 0x81, // SN76489 write
        0x52, 0x22, 0x08, // YM2612 port 0 write
        0x52, 0x28, 0xF0, // YM2612 port 0 write
        0x52, 0x2A, 0x80, // YM2612 port 0 write
        0x66,
    ]);
    let eof = bytes.len();
    put(&mut bytes, 0x04, (eof - 4) as u32);

    let file = Arc::new(vgms_core::vgm::file::read("md.vgm", &bytes).expect("a walkable VGM"));
    let seen: Arc<Mutex<HashMap<ChipKind, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let seen_for_cores = Arc::clone(&seen);
    let mut engine = VgmEngine::with_cores(file, 44_100, move |kind| {
        Some(Box::new(Recording {
            kind,
            seen: Arc::clone(&seen_for_cores),
        }))
    });

    let mut buffer = vec![0i16; 512 * 2];
    while engine.render(&mut buffer) != 0 {}

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.get(&ChipKind::Sn76489),
        Some(&2),
        "the SN76489 must see its two writes"
    );
    assert_eq!(
        seen.get(&ChipKind::Ym2612),
        Some(&3),
        "the YM2612 must see its three writes"
    );
}

/// Pulls up to `want` frames, in audio-callback-sized chunks, and returns how
/// many actually came back. A short answer means the engine stopped early.
fn drain(engine: &mut VgmEngine, buffer: &mut [i16], want: u64) -> u64 {
    let mut frames = 0u64;
    while frames < want {
        let room = usize::try_from(want - frames).unwrap_or(usize::MAX);
        let take = (buffer.len() / 2).min(room) * 2;
        let rendered = engine.render(&mut buffer[..take]);
        if rendered == 0 {
            break;
        }
        frames += rendered as u64;
    }
    frames
}

#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn the_engine_plays_every_corpus_file_for_exactly_its_own_length() {
    // Loud, not a silent skip: run explicitly (`--ignored`), so an unset corpus
    // is a setup mistake to report, not a reason to pass green.
    let root = std::env::var("VGMSTUDIO_VGMRIPS_CORPUS")
        .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test");
    let songs = common::collect_songs(Path::new(&root));
    assert!(!songs.is_empty(), "no .vgm/.vgz files under {root}");

    const OUTPUT_RATE: u32 = 44_100;
    /// How much of each file to render. A corpus this size is hundreds of hours
    /// of audio, and the property being checked -- the engine renders exactly
    /// what the waits ask for and never stops early -- is as true of a prefix as
    /// of a whole file. Files shorter than this are checked to their end, which
    /// covers the "does it terminate" half too.
    const BUDGET_FRAMES: u64 = OUTPUT_RATE as u64 * 20;

    let mut played = 0usize;
    let mut in_full = 0usize;
    // What the minimum-version computation says about real files, reported
    // rather than asserted: rippers over-claim routinely, and an *under*-claim
    // is the interesting number, because it means either a bad ripper or a gap
    // in the version table.
    let mut over_claimed = 0usize;
    let mut exact = 0usize;
    let mut under_claimed: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    let mut total_frames = 0u64;
    let mut failures: Vec<String> = Vec::new();

    for path in &songs {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(file) = vgms_core::vgm::file::read(&name, &bytes) else {
            skipped += 1;
            continue;
        };
        if file.stream().is_none() {
            skipped += 1;
            continue;
        }
        let file = Arc::new(file);
        let stream = file.stream().expect("just checked");

        let declared = file.header.version();
        let needed = vgms_core::vgm::version::minimum_version(&file.header, file.stream());
        match needed.cmp(&declared) {
            std::cmp::Ordering::Less => over_claimed += 1,
            std::cmp::Ordering::Equal => exact += 1,
            std::cmp::Ordering::Greater => {
                if under_claimed.len() < 20 {
                    under_claimed.push(format!(
                        "{name}: declares {} but needs {}",
                        vgms_core::vgm::header::format_version(declared),
                        vgms_core::vgm::header::format_version(needed)
                    ));
                }
            }
        }

        // What the stream's own waits add up to, in output frames. This is the
        // number the engine must render, and it is derived from the stream
        // rather than the header on purpose: a header that disagrees with its
        // stream is a thing this app reports, not a thing it plays by.
        let samples = stream.total_samples();
        let expected =
            samples * u64::from(OUTPUT_RATE) / u64::from(vgms_core::vgm::VGM_SAMPLE_RATE);

        let mut engine = VgmEngine::with_cores(Arc::clone(&file), OUTPUT_RATE, |_| {
            Some(Box::new(Counting { writes: 0 }))
        });

        // Pulled in chunks, as an audio callback would.
        let mut buffer = vec![0i16; 4096 * 2];
        let want = expected.min(BUDGET_FRAMES);
        let frames = drain(&mut engine, &mut buffer, want);

        if frames != want {
            failures.push(format!(
                "{name}: rendered {frames} of the {want} frames its waits ask for"
            ));
        }
        if expected <= BUDGET_FRAMES {
            in_full += 1;
            // One more pull past the end must be a clean zero -- which is how a
            // caller learns the stream ended, and only then is the engine
            // finished: the commands after the last wait have not run until
            // something asks for the frames that would follow them.
            if engine.render(&mut buffer) != 0 {
                failures.push(format!("{name}: rendered past the end of its stream"));
            }
            if !engine.is_finished() {
                failures.push(format!("{name}: never reached the end of its stream"));
            }
        }
        played += 1;
        total_frames += frames;

        // And a seek must land where it was asked to and play on from there.
        let middle = stream.len() / 2;
        let tail_expected = stream.samples_from(middle) * u64::from(OUTPUT_RATE)
            / u64::from(vgms_core::vgm::VGM_SAMPLE_RATE);
        engine.seek_to_row(middle);
        let want_tail = tail_expected.min(BUDGET_FRAMES);
        let tail = drain(&mut engine, &mut buffer, want_tail);
        if tail != want_tail {
            failures.push(format!(
                "{name}: a seek to row {middle} rendered {tail} of the {want_tail} frames \
                 its waits ask for"
            ));
        }
        total_frames += tail;
    }

    eprintln!("--- engine corpus ---");
    eprintln!("played:        {played}");
    eprintln!("  to the end:  {in_full} (the rest were cut off at the render budget)");
    eprintln!("skipped:       {skipped} (unreadable, or commands that will not walk)");
    eprintln!("version:       {over_claimed} could be stamped lower, {exact} already exact");
    eprintln!(
        "               {} declare less than they need:",
        under_claimed.len()
    );
    for line in &under_claimed {
        eprintln!("                 {line}");
    }
    eprintln!(
        "audio:         {:.1} hours rendered",
        total_frames as f64 / f64::from(OUTPUT_RATE) / 3600.0
    );

    assert!(
        failures.is_empty(),
        "{} file(s) did not play as their own streams describe:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
