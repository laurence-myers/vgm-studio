//! Corpus validation for the VGM optimiser, run on demand.
//!
//! Needs the local OPL VGM corpus via `DROTRIM_CORPUS`:
//!
//! ```powershell
//! $env:DROTRIM_CORPUS = 'F:\GameMusic\VGM'
//! cargo test -p vgms-app --release --test optimize_corpus -- --ignored --nocapture
//! ```
//!
//! It optimises every readable track -- asserting the delay total is conserved
//! and a second pass is a no-op -- renders a sampled subset through nuked-opl3
//! for byte-for-byte parity, and prints the aggregate size reduction.
//!
//! # Why the render uses *immediate* writes
//!
//! Nuked's buffered write path (what `render_wav` and live playback use) spreads
//! queued writes a couple of samples apart, so removing a redundant write shifts
//! the following writes ~2 samples (~40 us) -- inaudible but byte-visible, and a
//! property of the emulator's scheduler, not the optimisation. Immediate writes
//! isolate the latched-state audio the optimiser preserves, giving a byte-exact
//! oracle. The harness also reports the peak buffered-path difference to confirm
//! it stays a local phase shift, not a state change.

use std::path::{Path, PathBuf};

use vgms_core::optimize::optimize;
use vgms_core::util::VGM_SAMPLE_RATE;
use vgms_core::{Bank, Instruction, Song};
use vgms_synth::{FrameClock, NATIVE_SAMPLE_RATE, NukedOpl3, OplChip, render_wav};

/// Renders a VGM through the chip with *immediate* register writes (no write
/// buffer), so a same-value strip is a true no-op and the render is a byte-exact
/// oracle for the optimisation. See the module docs.
fn render_immediate(song: &Song, rate: u32) -> Vec<i16> {
    let mut chip = NukedOpl3::new(rate);
    let mut clock = FrameClock::new(rate, VGM_SAMPLE_RATE);
    let mut out = Vec::new();
    let mut scratch = vec![0i16; 8192];
    let mut bank = Bank::Low;
    for index in 0..song.len() {
        match song.instruction(index).unwrap() {
            Instruction::Register {
                reg,
                value,
                bank: written,
            } => {
                if let Some(written) = written {
                    bank = written;
                }
                chip.write_reg(bank.register_offset() | u16::from(reg), value);
            }
            Instruction::DelaySamples { samples, .. } => {
                let mut frames = clock.frames_for(samples);
                while frames > 0 {
                    let n = frames.min((scratch.len() / 2) as u64) as usize;
                    chip.generate_samples(&mut scratch[..n * 2]);
                    out.extend_from_slice(&scratch[..n * 2]);
                    frames -= n as u64;
                }
            }
            Instruction::BankSwitch(_) | Instruction::DelayMs { .. } => {}
        }
    }
    out
}

/// The peak absolute sample difference between two 16-bit WAV renders.
fn peak_diff(a: &[u8], b: &[u8]) -> i32 {
    a.chunks_exact(2)
        .zip(b.chunks_exact(2))
        .map(|(x, y)| {
            let x = i16::from_le_bytes([x[0], x[1]]);
            let y = i16::from_le_bytes([y[0], y[1]]);
            (i32::from(x) - i32::from(y)).abs()
        })
        .max()
        .unwrap_or(0)
}

/// Run the expensive render-parity check on every Nth *optimised* track (the ones
/// that actually changed), capped at this many renders in total.
const PARITY_STRIDE: usize = 3;
const PARITY_MAX: usize = 60;

fn collect_songs(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_songs(&path, out);
        } else if matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("vgm") | Some("vgz")
        ) {
            out.push(path);
        }
    }
}

#[test]
#[ignore = "needs the local corpus via DROTRIM_CORPUS"]
fn optimise_the_whole_corpus() {
    let Ok(root) = std::env::var("DROTRIM_CORPUS") else {
        eprintln!("DROTRIM_CORPUS not set; skipping corpus validation");
        return;
    };
    let mut songs = Vec::new();
    collect_songs(Path::new(&root), &mut songs);
    songs.sort();
    assert!(!songs.is_empty(), "no .vgm/.vgz files under {root}");

    let mut scanned = 0usize;
    let mut unreadable = 0usize;
    let mut readable = 0usize;
    let mut optimised = 0usize;
    let mut total_before = 0u64; // stream bytes across readable tracks
    let mut total_after = 0u64;
    let mut parity_checked = 0usize;
    let mut parity_failed = 0usize;
    let mut peak_buffered_diff = 0i32;
    let mut biggest: Vec<(f64, String)> = Vec::new();

    for path in &songs {
        scanned += 1;
        let Ok(bytes) = std::fs::read(path) else {
            unreadable += 1;
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy();
        let song = match vgms_core::io::read_song(&name, &bytes) {
            Ok(song) => song,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        readable += 1;

        let before = song.data().raw().len() as u64;
        total_before += before;

        let Some(outcome) = optimize(&song) else {
            total_after += before; // already optimal: unchanged
            continue;
        };

        let after = outcome.data.raw().len() as u64;
        let mut opt = song.clone();
        outcome.install(&mut opt);

        // Cheap invariants on every optimised track.
        assert_eq!(
            opt.total_delay_samples(),
            song.total_delay_samples(),
            "delay total changed for {}",
            path.display()
        );
        assert!(
            optimize(&opt).is_none(),
            "not idempotent: {}",
            path.display()
        );

        total_after += after;
        optimised += 1;
        let reduction = 100.0 * (1.0 - after as f64 / before as f64);
        biggest.push((reduction, name.to_string()));

        // Deep render-parity on a spread-out, capped sample of the changed tracks.
        if optimised.is_multiple_of(PARITY_STRIDE) && parity_checked < PARITY_MAX {
            parity_checked += 1;
            // Byte-exact through the immediate path -- the real invariant.
            if render_immediate(&song, NATIVE_SAMPLE_RATE)
                != render_immediate(&opt, NATIVE_SAMPLE_RATE)
            {
                parity_failed += 1;
                eprintln!("IMMEDIATE PARITY FAILED: {}", path.display());
            }
            // The buffered path (live playback) differs only by an inaudible
            // sub-sample write-scheduler shift; track its peak to confirm.
            let wa = render_wav(&song, NATIVE_SAMPLE_RATE, 16).unwrap();
            let wb = render_wav(&opt, NATIVE_SAMPLE_RATE, 16).unwrap();
            peak_buffered_diff = peak_buffered_diff.max(peak_diff(&wa, &wb));
        }
    }

    biggest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let overall = if total_before == 0 {
        0.0
    } else {
        100.0 * (1.0 - total_after as f64 / total_before as f64)
    };

    println!("\n--- VGM optimiser corpus report ({root}) ---");
    println!("scanned:    {scanned} files");
    println!("readable:   {readable}  (unreadable/non-OPL: {unreadable})");
    println!("optimised:  {optimised} / {readable} readable tracks shrank");
    println!("stream size: {total_before} -> {total_after} bytes  ({overall:.1}% smaller overall)",);
    println!(
        "render-parity: {parity_checked} sampled, {parity_failed} mismatched \
         (immediate path, byte-exact)"
    );
    println!(
        "buffered-path peak diff on the sample: {peak_buffered_diff} \
         (local ~2-sample write-timing phase shift, not a state change)"
    );
    println!("largest reductions:");
    for (reduction, name) in biggest.iter().take(10) {
        println!("  {reduction:5.1}%  {name}");
    }

    assert_eq!(parity_failed, 0, "render parity failed on some tracks");
}
