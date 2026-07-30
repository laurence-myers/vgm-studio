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

use vgms_core::vgm::ChipKind;

use crate::{ToolOutcome, clean_dac_runs, optimize_writes, trim_sample_roms};

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

/// Runs every applicable optimiser over `vgm`, in the order described above.
///
/// Never fails as a whole: a stage that cannot run is recorded and the pass
/// continues from the bytes it was handed, so the worst case is the original
/// file back with an explanation.
#[must_use]
pub fn optimize_vgm(vgm: &[u8], options: Options) -> Optimised {
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
            run_stage("optdac", skip, &mut bytes, &mut stages, clean_dac_runs);
        }
        // Before vgm_cmp, on the wiki's order: the trim reads the ROM out of
        // the write history, and that history should be the file's own.
        if options.sample_roms {
            run_stage(
                "vgm_sro",
                facts.rom_trim_denied,
                &mut bytes,
                &mut stages,
                trim_sample_roms,
            );
        }
        let skip = facts.has_saa1099.then_some(SAA1099_HELD_BACK);
        run_stage("vgm_cmp", skip, &mut bytes, &mut stages, optimize_writes);
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
    tool: fn(&[u8]) -> ToolOutcome,
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
            rom_trim_denied: ROM_TRIM_DENIED
                .iter()
                .find(|(kind, _)| declares(*kind))
                .map(|(_, reason)| *reason),
        }
    }
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
