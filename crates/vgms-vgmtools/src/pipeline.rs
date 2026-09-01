//! The three tools and the built-in optimiser, run as one pass.
//!
//! Order is upstream's, from the VGMRips wiki's "Optimizing VGMs" page, which
//! runs the sample-ROM trim *before* the write dedup:
//!
//! 1. **`optdac`** first, so collapsing a run of identical DAC writes leaves
//!    fewer commands for everything after it to walk.
//! 2. **`vgm_sro`**, while every write is still present. It decides which
//!    sample-ROM bytes are reachable by replaying the register writes through
//!    its own chip models -- not `vgm_cmp`'s. Running it second would mean
//!    asking it to find the ROM from a write history another tool had already
//!    pruned, on rules it does not share.
//! 3. **`vgm_cmp`**: the per-chip redundancy table this whole crate exists to
//!    borrow.
//! 4. **`vgms_core`'s own optimiser** to finish. Its redundancy pass is
//!    subsumed by `vgm_cmp`, but its delay re-encoder is provably byte-minimal
//!    where `vgm_cmp`'s writer is not, so it reliably shaves a little more.
//!
//! **A file every chip of which the built-in covers skips `vgm_cmp`.**
//! `vgms_core` is the primary write-dedup optimiser; `vgm_cmp` is the fallback
//! for a file carrying a chip it does not have redundancy rules for -- which,
//! since the coverage widened to every chip the format defines, is no file at
//! all under `Auto`. When the built-in covers the whole file its output is what
//! ships -- pinned byte-for-byte against the corpus for OPL, and gated on a
//! render-parity check for every chip -- so `vgm_cmp`'s per-chip bugs never
//! touch a file the built-in can do itself.
//!
//! That bypass is **`vgm_cmp`'s alone**. `optdac` and `vgm_sro` do work the
//! built-in does not do at all -- collapsing DAC runs, trimming sample ROMs --
//! so they are gated on whether the *file* has anything for them (a YM2612, a
//! ROM-image block), never on chip coverage. Making them share the write-dedup
//! bypass would have silently retired both the day the last chip earned a rule.
//! See `docs/optimizer-2026-08/PLAN.md`.

use vgms_core::vgm::{ChipKind, VgmCommand};

use crate::ToolOutcome;

/// Runs one optimiser over a buffer of VGM bytes.
///
/// The pipeline owns the order, the bypasses and the guards; a `Tools`
/// implementation owns only *how a single tool runs* -- the desktop spawns a
/// child process ([`NativeTools`](crate::NativeTools)), the web instantiates a
/// wasm module. Each method returns [`ToolOutcome::Unchanged`] when the tool
/// declined or gained nothing, and never panics: a failure is a
/// [`ToolOutcome::Failed`] the pipeline records and steps past.
pub trait Tools {
    /// `vgm_cmp` -- drop chip writes that change nothing.
    fn optimize_writes(&self, vgm: &[u8]) -> ToolOutcome;
    /// `vgm_sro` -- strip unused regions out of sample ROMs.
    fn trim_sample_roms(&self, vgm: &[u8]) -> ToolOutcome;
    /// `optdac` -- collapse long runs of identical YM2612 DAC writes.
    fn clean_dac_runs(&self, vgm: &[u8]) -> ToolOutcome;
}

/// Why a file naming an SAA1099 does not go through `vgm_cmp` yet.
///
/// `vgm_cmp.c:537` is missing a `break`:
///
/// ```text
/// case 0xBD:  // SAA1099 write
///     SetChipSet((VGMPnt[0x01] & 0x80) >> 7);
/// case 0x51:  // YM2413 write
///     WriteEvent = ym2413_write(VGMPnt[0x01], VGMPnt[0x02]);
/// ```
///
/// So SAA1099 register writes are judged by the YM2413's rules, which dedupe
/// every register with no exceptions. The SAA1099 has some: writing `0x18` or
/// `0x19` *reloads* an envelope rather than latching, so dropping a repeat of
/// the same byte is audible. A fallthrough, not a considered rule, so it does
/// not get the benefit of the doubt the rest of the table has earned.
const SAA1099_HELD_BACK: &str =
    "names an SAA1099, whose writes vgm_cmp judges with the YM2413's rules (a missing `break`)";

/// Chips whose sample ROMs `vgm_sro` must not be let near.
///
/// The trim keeps only the ROM bytes its own chip models say a register write
/// can reach. A model that misreads a chip throws away samples that do get
/// played -- and the file still parses and keeps its timing, so only a render
/// catches it.
///
/// - **QSound** -- measured here: running `vgm_sro` alone over the corpus and
///   rendering both sides, 12 of the 23 QSound files it changed came back
///   playing something different. (Whether the fault is ours or upstream's is
///   open -- see `vgms-app/tests/optimize_parity.rs`. Held back either way.)
/// - **K053260** -- upstream's own wiki: *"It will still incorrectly strip
///   K053260 PCM roms."*
/// - **SegaPCM** -- upstream again: *"SegaPCM support isn't 100% safe. That
///   means there may be samples stripped off despite them being used."*
///
/// Everything else is unmeasured rather than cleared: the trim never fired on
/// another chip in this corpus.
const ROM_TRIM_DENIED: &[(ChipKind, &str)] = &[
    (
        ChipKind::QSound,
        "QSound: measured to change what 12 of 23 corpus files play",
    ),
    (
        ChipKind::K053260,
        "K053260: upstream says the trim strips its PCM ROMs incorrectly",
    ),
    (
        ChipKind::SegaPcm,
        "SegaPCM: upstream says the trim is not 100% safe on it",
    ),
];

/// The declared sample-ROM size `vgm_sro` cannot survive.
///
/// `chip_srom.c:3268` rounds a ROM-image block's declared total size up to a
/// power of two by doubling a `UINT32`: `for (m = 1; m < ROMSize; m *= 2);`. At
/// or above this value the mask overflows to zero and the loop never ends. No
/// real chip carries 2 GiB of sample ROM, so a block declaring one is a broken
/// header, not a file to optimise -- refused here rather than run into the loop.
///
/// This is defence in depth, not the only stop: on native a 120 s timeout kills
/// the hung child, and on wasm the optimiser's worker is terminated (ow-6). But
/// this fixes the loop we *know* about, on both targets, before it starts --
/// which on wasm, where a run cannot be pre-empted, is the difference between a
/// skipped file and a dead worker.
const ROM_SIZE_CEILING: u32 = 0x8000_0000;

/// Why a file naming a 2-GiB-or-larger sample ROM does not go through `vgm_sro`.
const BOTTOMLESS_ROM: &str =
    "declares a sample ROM of 2 GiB or more, which vgm_sro's size rounding cannot terminate on";

/// Which stages to run. Write dedup is not optional -- it is the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Run `vgm_sro`, trimming unused sample-ROM regions.
    pub sample_roms: bool,
    /// Run `optdac`, collapsing long runs of identical YM2612 DAC writes.
    pub dac_runs: bool,
    /// Which optimiser to use: the built-in, the external tools, or the
    /// per-file routing between them. The `sample_roms`/`dac_runs` flags only
    /// bite when the tools run.
    pub optimizer: vgms_core::config::OptimizerChoice,
    /// Run the per-chip hold-backs speculatively (D-orw-8): let `vgm_sro` run on
    /// the chips it is otherwise denied (QSound / K053260 / SegaPCM), and
    /// `vgm_cmp` on an SAA1099. Only ever set by a caller that renders and
    /// verifies the output afterwards ([`optimize_verified`](../../vgms_ui) sets
    /// it) -- the blanket denials are there because *some* files corrupt, and the
    /// render gate turns a blanket denial into try-and-keep-if-it-matches, per
    /// file. The bottomless-ROM guard is **not** lifted: it prevents a hang, not
    /// a wrong answer, so it stands whatever this says.
    pub speculative: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sample_roms: true,
            dac_runs: true,
            optimizer: vgms_core::config::OptimizerChoice::Auto,
            // Off by default: the hold-backs deny a stage unless a caller has a
            // render gate to catch a corruption.
            speculative: false,
        }
    }
}

/// What one stage did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// It was not run, for the reason given.
    Skipped(&'static str),
    /// It ran and found nothing to gain.
    Unchanged,
    /// It ran and the file got smaller.
    Shrank { from: usize, to: usize },
    /// It could not run, or would not finish. The file is untouched.
    Failed(String),
}

/// One step of the pass, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// The tool's own name, as upstream calls it.
    pub name: &'static str,
    pub outcome: StageOutcome,
}

/// The result of a whole pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Optimized {
    /// The optimised file -- or the original bytes, unchanged, if no stage
    /// gained anything.
    pub bytes: Vec<u8>,
    /// Every stage in the order it ran, for a log or a report.
    pub stages: Vec<Stage>,
    /// What the file weighed before any of it.
    pub original_len: usize,
}

impl Optimized {
    /// Whether the file actually got smaller.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.bytes.len() < self.original_len
    }

    /// How many bytes the pass saved.
    #[must_use]
    pub const fn saved(&self) -> usize {
        self.original_len.saturating_sub(self.bytes.len())
    }

    /// Anything that went wrong, for a caller that logs rather than fails.
    ///
    /// A failed stage never costs the file: the pass carries on from the bytes
    /// that stage was given.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        self.stages
            .iter()
            .filter_map(|stage| match &stage.outcome {
                StageOutcome::Failed(reason) => Some(format!("{}: {reason}", stage.name)),
                _ => None,
            })
            .collect()
    }
}

/// Runs every applicable optimiser over `vgm`, in the order described above,
/// using `tools` to run each one -- child processes on the desktop
/// ([`NativeTools`](crate::NativeTools)), wasm modules on the web. The desktop
/// [`optimize_vgm`](crate::optimize_vgm) is this with the native runner.
///
/// Never fails as a whole: a stage that cannot run is recorded and the pass
/// continues from the bytes it was handed, so the worst case is the original
/// file back with an explanation.
#[must_use]
pub fn optimize_vgm_with(vgm: &[u8], options: Options, tools: &dyn Tools) -> Optimized {
    let original_len = vgm.len();
    let mut bytes = vgm.to_vec();
    let mut stages = Vec::new();

    let facts = Facts::read(vgm);

    use vgms_core::config::OptimizerChoice;
    // The built-in only: never spawn a tool, whatever the chips.
    let spawn_tools = options.optimizer != OptimizerChoice::BuiltInOnly;

    if spawn_tools {
        if options.dac_runs {
            let skip = (!facts.has_ym2612).then_some("no YM2612 to have a DAC");
            run_stage("optdac", skip, &mut bytes, &mut stages, |b| {
                tools.clean_dac_runs(b)
            });
        }
        // Before vgm_cmp, on the wiki's order: the trim reads the ROM out of
        // the write history, and that history should be the file's own. The
        // bottomless-ROM guard always denies; the per-chip hold-back is lifted
        // when the caller will verify the render (D-orw-8).
        if options.sample_roms {
            let rom_skip = (!facts.has_rom_image)
                .then_some("no sample ROM to trim")
                .or(facts.rom_trim_bottomless)
                .or(if options.speculative {
                    None
                } else {
                    facts.rom_trim_chip_denied
                });
            run_stage("vgm_sro", rom_skip, &mut bytes, &mut stages, |b| {
                tools.trim_sample_roms(b)
            });
        }
        // The write dedup, which the built-in pass below does itself for every
        // chip it has rules for. `Tools` runs it regardless -- it is the A/B
        // control, and re-spells even a covered file. The SAA1099 hold-back is
        // lifted under a render gate.
        let skip = if options.optimizer == OptimizerChoice::Auto && facts.built_in_covers_all {
            Some("the built-in optimizer covers every chip here")
        } else if !options.speculative && facts.has_saa1099 {
            Some(SAA1099_HELD_BACK)
        } else {
            None
        };
        run_stage("vgm_cmp", skip, &mut bytes, &mut stages, |b| {
            tools.optimize_writes(b)
        });
    } else {
        stages.push(Stage {
            name: "vgmtools",
            outcome: StageOutcome::Skipped("built-in optimizer selected in Settings"),
        });
    }

    built_in(&mut bytes, &mut stages);

    Optimized {
        bytes,
        stages,
        original_len,
    }
}

/// Optimises one pack song and narrates it: the whole pass over `bytes`, with
/// every line a pack export's log wants -- the shrink summary, each stage worth
/// a line, and the passthrough chips named so a rip that comes back byte for
/// byte does not look unreadable.
///
/// This is the *one* copy of that narration; the desktop pack, the web pack
/// worker and anything else with a `log` call it with their own [`Tools`].
/// Never fatal: a DRO, an unreadable file, or a failed stage passes the
/// original bytes through with a note.
///
/// Returns the optimised bytes when the pass shrank the file, or `bytes`
/// exactly as given (possibly still a `.vgz`) when it did not -- so an entry
/// the tools cannot improve keeps its original spelling, compression included.
pub fn optimize_song_logged(
    name: &str,
    bytes: &[u8],
    options: Options,
    tools: &dyn Tools,
    log: &mut Vec<String>,
) -> Vec<u8> {
    let Ok(file) = vgms_core::vgm::file::read(name, bytes) else {
        // A DRO, or something unreadable. Either way it passes through.
        log.push(format!("{name}: kept as-is (not a readable VGM)"));
        return bytes.to_vec();
    };
    // The tools take plain bytes, and a pack entry may already be a `.vgz`.
    let Ok(plain) = vgms_core::vgm::file::write(&file) else {
        log.push(format!("{name}: kept as-is (could not be prepared)"));
        return bytes.to_vec();
    };

    let result = optimize_vgm_with(&plain, options, tools);

    if result.changed() {
        log.push(format!(
            "{name}: {} -> {} bytes (optimized, {} saved)",
            bytes.len(),
            result.bytes.len(),
            result.saved()
        ));
    }
    // Only the stages worth a line: "nothing to gain" is the common case and
    // would bury the rest.
    for stage in &result.stages {
        match &stage.outcome {
            StageOutcome::Shrank { from, to } => {
                log.push(format!("{name}:   {} {from} -> {to} bytes", stage.name));
            }
            StageOutcome::Failed(reason) => {
                log.push(format!("{name}:   {} failed: {reason}", stage.name));
            }
            StageOutcome::Skipped(reason) => {
                log.push(format!("{name}:   {} skipped: {reason}", stage.name));
            }
            StageOutcome::Unchanged => {}
        }
    }

    // Only worth saying when `vgm_cmp` is what handled the writes: these are
    // the chips *it* copies through, and the built-in has rules for all of
    // them. Naming them after a pass the built-in did would be a lie.
    let ran_vgm_cmp = result
        .stages
        .iter()
        .any(|stage| stage.name == "vgm_cmp" && !matches!(stage.outcome, StageOutcome::Skipped(_)));
    let untouched: Vec<&str> = file
        .header
        .chips()
        .iter()
        .filter(|chip| passthrough_chips().contains(&chip.kind))
        .map(|chip| chip.kind.name())
        .collect();
    if ran_vgm_cmp && !untouched.is_empty() {
        log.push(format!(
            "{name}: {} not optimized by vgm_cmp -- its table has no handler for them",
            untouched.join(", ")
        ));
    }

    if result.changed() {
        result.bytes
    } else {
        bytes.to_vec()
    }
}

/// Runs one tool, or records why it did not.
fn run_stage(
    name: &'static str,
    skip: Option<&'static str>,
    bytes: &mut Vec<u8>,
    stages: &mut Vec<Stage>,
    tool: impl FnOnce(&[u8]) -> ToolOutcome,
) {
    if let Some(reason) = skip {
        stages.push(Stage {
            name,
            outcome: StageOutcome::Skipped(reason),
        });
        return;
    }

    let before = bytes.len();
    let outcome = match tool(bytes) {
        ToolOutcome::Smaller(smaller) => {
            let to = smaller.len();
            *bytes = smaller;
            StageOutcome::Shrank { from: before, to }
        }
        ToolOutcome::Unchanged => StageOutcome::Unchanged,
        ToolOutcome::Failed(reason) => StageOutcome::Failed(reason),
    };
    stages.push(Stage { name, outcome });
}

/// The finishing pass: `vgms_core`'s own optimiser, mostly for its byte-minimal
/// delay re-encoder.
fn built_in(bytes: &mut Vec<u8>, stages: &mut Vec<Stage>) {
    let before = bytes.len();
    let outcome = match vgms_core::vgm::file::read("optimizing.vgm", bytes) {
        Err(error) => StageOutcome::Failed(format!("could not re-read the file: {error}")),
        Ok(mut file) => {
            if file.optimize().is_none() {
                StageOutcome::Unchanged
            } else {
                match vgms_core::vgm::file::write(&file) {
                    Err(error) => StageOutcome::Failed(format!("could not write back: {error}")),
                    Ok(written) => {
                        // The built-in optimiser gates on its own body bytes,
                        // which is not quite the whole file; if the file did
                        // not actually shrink, keep what we had.
                        if written.len() < before {
                            let to = written.len();
                            *bytes = written;
                            StageOutcome::Shrank { from: before, to }
                        } else {
                            StageOutcome::Unchanged
                        }
                    }
                }
            }
        }
    };
    stages.push(Stage {
        name: "built-in",
        outcome,
    });
}

/// What the header says, for the decisions the pass has to make before it runs
/// anything.
struct Facts {
    /// Every chip the file declares has a built-in redundancy rule, so the
    /// built-in optimiser covers the whole file and the tools add nothing. The
    /// generalisation of the OPL bypass: the built-in is the primary path, and
    /// the external tools are the fallback for a file carrying any chip it does
    /// not yet cover. See `docs/optimizer-2026-08/PLAN.md`.
    built_in_covers_all: bool,
    has_ym2612: bool,
    has_saa1099: bool,
    /// The file carries at least one `0x67` ROM-image block, so there is a
    /// sample ROM for `vgm_sro` to trim. Without one the tool has nothing to
    /// do, and since the write dedup stopped gating the whole tool stage that
    /// is most files.
    has_rom_image: bool,
    /// Why the sample-ROM trim can never run on this file: a bottomless ROM the
    /// trim's size rounding cannot terminate on. Not lifted by the speculative
    /// mode -- it prevents a hang, not merely a wrong answer.
    rom_trim_bottomless: Option<&'static str>,
    /// Why the sample-ROM trim is held back for this file's chips (QSound /
    /// K053260 / SegaPCM). Lifted by the speculative mode, where a render gate
    /// catches a bad trim per file.
    rom_trim_chip_denied: Option<&'static str>,
}

impl Facts {
    fn read(vgm: &[u8]) -> Self {
        let Ok(file) = vgms_core::vgm::file::read("optimizing.vgm", vgm) else {
            // Unreadable here does not mean unusable to the tools -- they have
            // their own reader. Let the stages try, but keep the hold-backs:
            // they exist to prevent a wrong answer, not to save work, and a
            // file we cannot read is the last one to take a chance on. Charge
            // the denial to the chip category, so the render gate can still try
            // it speculatively -- an unreadable header is a wrong-answer risk,
            // not a hang.
            return Self {
                built_in_covers_all: false,
                has_ym2612: true,
                has_saa1099: true,
                has_rom_image: true,
                rom_trim_bottomless: None,
                rom_trim_chip_denied: Some(
                    "the header could not be read, so its chips are unknown",
                ),
            };
        };
        let declares = |kind| file.header.chips().iter().any(|chip| chip.kind == kind);
        let chips = file.header.chips();
        let roms = rom_images(&file);
        Self {
            built_in_covers_all: !chips.is_empty()
                && chips
                    .iter()
                    .all(|chip| vgms_core::redundancy::has_latch_rules(chip.kind)),
            has_ym2612: declares(ChipKind::Ym2612),
            has_saa1099: declares(ChipKind::Saa1099),
            has_rom_image: roms.any,
            rom_trim_bottomless: roms.bottomless.then_some(BOTTOMLESS_ROM),
            rom_trim_chip_denied: ROM_TRIM_DENIED
                .iter()
                .find(|(kind, _)| declares(*kind))
                .map(|(_, reason)| *reason),
        }
    }
}

/// What the file's `0x67` ROM-image blocks (types `0x80`-`0xBF`) amount to.
struct RomImages {
    /// There is at least one, so `vgm_sro` has something to trim.
    any: bool,
    /// One of them declares a total size at or above [`ROM_SIZE_CEILING`] --
    /// the size that spins `chip_srom.c`'s mask forever.
    bottomless: bool,
}

/// Reads them off the stream.
///
/// A ROM-image block's payload begins `[UINT32 total ROM size][UINT32 start
/// address]`, and `raw_command` hands back the block's whole byte run, so the
/// declared size is the little-endian `u32` at offset 7 (past
/// `0x67 0x66 <type> <u32 length>`).
fn rom_images(file: &vgms_core::vgm::file::VgmFile) -> RomImages {
    let Some(stream) = file.stream() else {
        return RomImages {
            any: false,
            bottomless: false,
        };
    };
    let mut images = RomImages {
        any: false,
        bottomless: false,
    };
    for index in 0..stream.len() {
        let is_rom_image = matches!(
            stream.get(index),
            Some(VgmCommand::DataBlock { kind, .. }) if (0x80..=0xBF).contains(&kind)
        );
        if !is_rom_image {
            continue;
        }
        images.any = true;
        images.bottomless |= stream
            .raw_command(index)
            .and_then(|raw| raw.get(7..11))
            .is_some_and(|size| {
                u32::from_le_bytes([size[0], size[1], size[2], size[3]]) >= ROM_SIZE_CEILING
            });
    }
    images
}

/// The chips `vgm_cmp` copies through untouched.
///
/// Read off the handler list at the top of `vgm_cmp.c`: the MultiPCM and
/// K053260 handlers exist but are commented out (`vgm_cmp.c:715`, `:763`); the
/// PWM, GA20, ES5505 and Mikey commands have no case at all and fall to the
/// switch's default, which copies them.
///
/// A file made only of these comes back unchanged however redundant it looks;
/// worth saying out loud in an export log.
///
/// The SAA1099 is deliberately *not* here: it is not passed through, it is
/// deduped by the wrong chip's rules -- see [`SAA1099_HELD_BACK`].
#[must_use]
pub fn passthrough_chips() -> &'static [ChipKind] {
    &[
        ChipKind::MultiPcm,
        ChipKind::K053260,
        ChipKind::Pwm,
        ChipKind::Ga20,
        ChipKind::Es5505,
        ChipKind::Mikey,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, readable v1.51 VGM declaring a SegaPCM (so it is not OPL, and
    /// the tools run) whose one `0x67` type-`0x80` ROM-image block declares
    /// `rom_size` as its total sample-ROM size.
    fn segapcm_vgm(rom_size: u32) -> Vec<u8> {
        let mut vgm = vec![0u8; 0x40];
        vgm[0x00..0x04].copy_from_slice(b"Vgm ");
        vgm[0x08..0x0C].copy_from_slice(&0x0000_0151u32.to_le_bytes()); // version 1.51
        vgm[0x34..0x38].copy_from_slice(&0x0000_000Cu32.to_le_bytes()); // data at 0x40
        vgm[0x38..0x3C].copy_from_slice(&4_000_000u32.to_le_bytes()); // SegaPCM clock
        vgm[0x3C..0x40].copy_from_slice(&0u32.to_le_bytes()); // SegaPCM interface

        // Data: one ROM-image block, then end-of-data.
        let mut payload = Vec::new();
        payload.extend_from_slice(&rom_size.to_le_bytes()); // declared total ROM size
        payload.extend_from_slice(&0u32.to_le_bytes()); // start address
        payload.extend_from_slice(&[0xAB; 4]); // a little ROM data
        vgm.push(0x67);
        vgm.push(0x66);
        vgm.push(0x80); // SegaPCM ROM image
        vgm.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        vgm.extend_from_slice(&payload);
        vgm.push(0x66); // end of sound data

        let eof = u32::try_from(vgm.len()).unwrap() - 4;
        vgm[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
        vgm
    }

    #[test]
    fn a_readable_synthetic_segapcm_file_reads_as_itself() {
        // Guards the fixture itself: a readable SegaPCM declares no YM2612, so
        // `has_ym2612` is false -- the unreadable fallback would set it true (and
        // deny the trim) for the wrong reason, which the ROM tests below rely on
        // not happening.
        let facts = Facts::read(&segapcm_vgm(0x0006_0000));
        assert!(
            !facts.has_ym2612,
            "the fixture must be readable, not the fallback"
        );
        assert!(
            facts.built_in_covers_all,
            "every chip the format defines now has built-in rules"
        );
        assert!(facts.has_rom_image, "it carries a type-0x80 ROM image");
    }

    /// The write dedup is the only stage the built-in's coverage bypasses. A
    /// file it covers end to end still goes to `vgm_sro` for its sample ROM --
    /// work the built-in does not do at all, and would have lost silently had
    /// the coverage test gated the whole tool stage.
    #[test]
    fn full_coverage_bypasses_vgm_cmp_and_nothing_else() {
        use crate::ToolOutcome;

        /// A `Tools` that records which stages were asked to run.
        #[derive(Default)]
        struct Tap {
            deduped: std::cell::Cell<bool>,
            trimmed: std::cell::Cell<bool>,
        }
        impl Tools for Tap {
            fn optimize_writes(&self, _vgm: &[u8]) -> ToolOutcome {
                self.deduped.set(true);
                ToolOutcome::Unchanged
            }
            fn trim_sample_roms(&self, _vgm: &[u8]) -> ToolOutcome {
                self.trimmed.set(true);
                ToolOutcome::Unchanged
            }
            fn clean_dac_runs(&self, _vgm: &[u8]) -> ToolOutcome {
                ToolOutcome::Unchanged
            }
        }

        let vgm = segapcm_vgm(0x0006_0000);
        let tap = Tap::default();
        let options = Options {
            // SegaPCM's ROM trim is a chip hold-back; lift it so the question
            // under test is coverage, not the hold-back.
            speculative: true,
            ..Options::default()
        };
        let result = optimize_vgm_with(&vgm, options, &tap);

        assert!(!tap.deduped.get(), "the built-in covers every chip here");
        assert!(tap.trimmed.get(), "but the sample ROM still wants trimming");
        assert!(
            result.stages.iter().any(|stage| stage.name == "vgm_cmp"
                && matches!(stage.outcome, StageOutcome::Skipped(_))),
            "and the bypass is reported as a skipped vgm_cmp, not a missing stage"
        );
    }

    /// A file with no sample ROM does not pay to start `vgm_sro`.
    #[test]
    fn a_file_without_a_rom_image_skips_the_trim() {
        let mut vgm = segapcm_vgm(0x0006_0000);
        // Truncate to the header plus a bare end-of-data marker.
        vgm.truncate(0x40);
        vgm.push(0x66);
        let eof = u32::try_from(vgm.len()).unwrap() - 4;
        vgm[0x04..0x08].copy_from_slice(&eof.to_le_bytes());

        assert!(!Facts::read(&vgm).has_rom_image);
    }

    #[test]
    fn a_bottomless_rom_block_denies_the_sample_rom_trim() {
        let facts = Facts::read(&segapcm_vgm(ROM_SIZE_CEILING));
        assert_eq!(
            facts.rom_trim_bottomless,
            Some(BOTTOMLESS_ROM),
            "a 2 GiB ROM must deny vgm_sro through the hang guard"
        );
    }

    #[test]
    fn a_normal_rom_size_does_not_trip_the_bottomless_guard() {
        let facts = Facts::read(&segapcm_vgm(0x0006_0000));
        assert_eq!(
            facts.rom_trim_bottomless, None,
            "a 0x60000 ROM is normal; the bottomless guard must stay quiet"
        );
        // But SegaPCM is a chip hold-back -- denied normally, tried speculatively.
        assert!(
            facts.rom_trim_chip_denied.is_some(),
            "SegaPCM's ROM trim is held back for the chip's sake"
        );
    }

    /// The speculative mode lifts the per-chip ROM hold-back but never the
    /// bottomless-ROM hang guard.
    #[test]
    fn speculative_lifts_the_chip_holdback_but_not_the_hang_guard() {
        use crate::ToolOutcome;

        /// A `Tools` that records whether `vgm_sro` was actually invoked.
        struct RomTap {
            trimmed: std::cell::Cell<bool>,
        }
        impl Tools for RomTap {
            fn optimize_writes(&self, _vgm: &[u8]) -> ToolOutcome {
                ToolOutcome::Unchanged
            }
            fn trim_sample_roms(&self, _vgm: &[u8]) -> ToolOutcome {
                self.trimmed.set(true);
                ToolOutcome::Unchanged
            }
            fn clean_dac_runs(&self, _vgm: &[u8]) -> ToolOutcome {
                ToolOutcome::Unchanged
            }
        }

        let speculative = Options {
            optimizer: vgms_core::config::OptimizerChoice::Tools,
            speculative: true,
            ..Options::default()
        };

        // SegaPCM: a chip hold-back. Normally denied; run under speculative.
        let tap = RomTap {
            trimmed: std::cell::Cell::new(false),
        };
        let _ = optimize_vgm_with(&segapcm_vgm(0x0006_0000), speculative, &tap);
        assert!(
            tap.trimmed.get(),
            "the chip hold-back is lifted speculatively"
        );

        // A bottomless ROM: the hang guard denies it even speculatively.
        let tap = RomTap {
            trimmed: std::cell::Cell::new(false),
        };
        let _ = optimize_vgm_with(&segapcm_vgm(ROM_SIZE_CEILING), speculative, &tap);
        assert!(
            !tap.trimmed.get(),
            "the bottomless-ROM guard stands even under the render gate"
        );
    }
}
