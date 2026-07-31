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
//! **A wholly-OPL file skips the tools entirely.** `vgms_core` covers the OPL
//! family and its output is pinned byte-for-byte against the corpus; the
//! bypass keeps those pins meaningful rather than re-spelling every OPL file
//! through a second implementation.

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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sample_roms: true,
            dac_runs: true,
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
pub struct Optimised {
    /// The optimised file -- or the original bytes, unchanged, if no stage
    /// gained anything.
    pub bytes: Vec<u8>,
    /// Every stage in the order it ran, for a log or a report.
    pub stages: Vec<Stage>,
    /// What the file weighed before any of it.
    pub original_len: usize,
}

impl Optimised {
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
pub fn optimize_vgm_with(vgm: &[u8], options: Options, tools: &dyn Tools) -> Optimised {
    let original_len = vgm.len();
    let mut bytes = vgm.to_vec();
    let mut stages = Vec::new();

    let facts = Facts::read(vgm);

    if facts.is_opl {
        // The one case where running the tools would be a step backwards --
        // see the module note.
        stages.push(Stage {
            name: "vgmtools",
            outcome: StageOutcome::Skipped("an OPL file: the built-in optimiser covers it"),
        });
    } else {
        if options.dac_runs {
            let skip = (!facts.has_ym2612).then_some("no YM2612 to have a DAC");
            run_stage("optdac", skip, &mut bytes, &mut stages, |b| {
                tools.clean_dac_runs(b)
            });
        }
        // Before vgm_cmp, on the wiki's order: the trim reads the ROM out of
        // the write history, and that history should be the file's own.
        if options.sample_roms {
            run_stage("vgm_sro", facts.rom_trim_denied, &mut bytes, &mut stages, |b| {
                tools.trim_sample_roms(b)
            });
        }
        let skip = facts.has_saa1099.then_some(SAA1099_HELD_BACK);
        run_stage("vgm_cmp", skip, &mut bytes, &mut stages, |b| {
            tools.optimize_writes(b)
        });
    }

    built_in(&mut bytes, &mut stages);

    Optimised {
        bytes,
        stages,
        original_len,
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
    let outcome = match vgms_core::vgm::file::read("optimising.vgm", bytes) {
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
    is_opl: bool,
    has_ym2612: bool,
    has_saa1099: bool,
    /// Why the sample-ROM trim must not run, if it must not.
    rom_trim_denied: Option<&'static str>,
}

impl Facts {
    fn read(vgm: &[u8]) -> Self {
        let Ok(file) = vgms_core::vgm::file::read("optimising.vgm", vgm) else {
            // Unreadable here does not mean unusable to the tools -- they have
            // their own reader. Let the stages try, but keep the hold-backs:
            // they exist to prevent a wrong answer, not to save work, and a
            // file we cannot read is the last one to take a chance on.
            return Self {
                is_opl: false,
                has_ym2612: true,
                has_saa1099: true,
                rom_trim_denied: Some("the header could not be read, so its chips are unknown"),
            };
        };
        let declares = |kind| file.header.chips().iter().any(|chip| chip.kind == kind);
        Self {
            is_opl: file.is_opl(),
            has_ym2612: declares(ChipKind::Ym2612),
            has_saa1099: declares(ChipKind::Saa1099),
            // The bottomless-ROM guard takes precedence: it prevents a hang, not
            // merely a wrong answer, so it is the more urgent reason to skip.
            rom_trim_denied: if declares_bottomless_rom(&file) {
                Some(BOTTOMLESS_ROM)
            } else {
                ROM_TRIM_DENIED
                    .iter()
                    .find(|(kind, _)| declares(*kind))
                    .map(|(_, reason)| *reason)
            },
        }
    }
}

/// Whether any `0x67` ROM-image block (types `0x80`-`0xBF`, whose payload begins
/// `[UINT32 total ROM size][UINT32 start address]`) declares a total size at or
/// above [`ROM_SIZE_CEILING`] -- the size that spins `chip_srom.c`'s mask.
///
/// `raw_command` hands back the block's whole byte run, so the declared size is
/// the little-endian `u32` at offset 7 (past `0x67 0x66 <type> <u32 length>`).
fn declares_bottomless_rom(file: &vgms_core::vgm::file::VgmFile) -> bool {
    let Some(stream) = file.stream() else {
        return false;
    };
    (0..stream.len()).any(|index| {
        let is_rom_image = matches!(
            stream.get(index),
            Some(VgmCommand::DataBlock { kind, .. }) if (0x80..=0xBF).contains(&kind)
        );
        is_rom_image
            && stream
                .raw_command(index)
                .and_then(|raw| raw.get(7..11))
                .is_some_and(|size| {
                    u32::from_le_bytes([size[0], size[1], size[2], size[3]]) >= ROM_SIZE_CEILING
                })
    })
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
    fn a_readable_synthetic_segapcm_file_is_not_opl() {
        // Guards the fixture itself: if this file stopped being readable, the
        // two guard tests below would pass for the wrong reason (the unreadable
        // fallback also denies the trim).
        let facts = Facts::read(&segapcm_vgm(0x0006_0000));
        assert!(!facts.is_opl, "a SegaPCM file must not read as OPL");
    }

    #[test]
    fn a_bottomless_rom_block_denies_the_sample_rom_trim() {
        let facts = Facts::read(&segapcm_vgm(ROM_SIZE_CEILING));
        assert_eq!(
            facts.rom_trim_denied,
            Some(BOTTOMLESS_ROM),
            "a 2 GiB ROM must deny vgm_sro, ahead of the SegaPCM chip reason"
        );
    }

    #[test]
    fn a_normal_rom_size_does_not_trip_the_bottomless_guard() {
        let facts = Facts::read(&segapcm_vgm(0x0006_0000));
        assert_ne!(
            facts.rom_trim_denied,
            Some(BOTTOMLESS_ROM),
            "a 0x60000 ROM is normal; the bottomless guard must stay quiet"
        );
    }
}
