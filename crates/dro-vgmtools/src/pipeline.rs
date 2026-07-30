//! The three tools and the built-in optimiser, run as one pass.
//!
//! Order matters, and each step has a reason to be where it is:
//!
//! 1. **`optdac`** first, because collapsing a run of identical DAC writes
//!    leaves fewer commands for everything after it to walk.
//! 2. **`vgm_cmp`**, the main event: the per-chip redundancy table this whole
//!    crate exists to borrow.
//! 3. **`vgm_sro`** last of the tools, because it decides which sample-ROM
//!    bytes are reachable by replaying the register writes -- and the fewer
//!    writes remain, the less ROM it has to keep.
//! 4. **`dro_core`'s own optimiser** to finish. Its redundancy pass is
//!    subsumed by `vgm_cmp` and finds nothing, but its delay re-encoder is
//!    provably byte-minimal where `vgm_cmp`'s writer is not, so it reliably
//!    shaves a little more.
//!
//! **A wholly-OPL file skips the tools entirely.** `dro_core` has covered the
//! OPL family since before any of this, its output is pinned byte-for-byte
//! against 3933 corpus files, and nothing here would improve on that -- so the
//! bypass keeps those pins meaningful rather than quietly re-spelling every
//! OPL file through a second implementation.

use dro_core::vgm::ChipKind;

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
/// `0x19` *reloads* an envelope rather than latching a value, so a repeat of
/// the same byte is a retrigger and dropping it is audible.
///
/// This is a fallthrough, not a considered rule -- nobody decided the SAA1099
/// should be read as a YM2413 -- so it does not get the benefit of the doubt
/// that the rest of the table has earned. Held back until ot-7's corpus
/// render-parity run says otherwise, which is cheap: SAA1099 rips are rare, and
/// the alternative is a smaller file that plays wrong.
const SAA1099_HELD_BACK: &str =
    "names an SAA1099, whose writes vgm_cmp judges with the YM2413's rules (a missing `break`)";

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
        let skip = facts.has_saa1099.then_some(SAA1099_HELD_BACK);
        run_stage("vgm_cmp", skip, &mut bytes, &mut stages, optimize_writes);
        if options.sample_roms {
            run_stage("vgm_sro", None, &mut bytes, &mut stages, trim_sample_roms);
        }
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

/// The finishing pass: `dro_core`'s own optimiser, mostly for its byte-minimal
/// delay re-encoder.
fn built_in(bytes: &mut Vec<u8>, stages: &mut Vec<Stage>) {
    let before = bytes.len();
    let outcome = match dro_core::vgm::file::read("optimising.vgm", bytes) {
        Err(error) => StageOutcome::Failed(format!("could not re-read the file: {error}")),
        Ok(mut file) => {
            if file.optimize().is_none() {
                StageOutcome::Unchanged
            } else {
                match dro_core::vgm::file::write(&file) {
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
}

impl Facts {
    fn read(vgm: &[u8]) -> Self {
        let Ok(file) = dro_core::vgm::file::read("optimising.vgm", vgm) else {
            // Unreadable here does not mean unusable to the tools -- they have
            // their own reader. Let the stages try, but keep the one hold-back
            // that exists to prevent a wrong answer rather than to save work.
            return Self {
                is_opl: false,
                has_ym2612: true,
                has_saa1099: true,
            };
        };
        let declares = |kind| file.header.chips().iter().any(|chip| chip.kind == kind);
        Self {
            is_opl: file.is_opl(),
            has_ym2612: declares(ChipKind::Ym2612),
            has_saa1099: declares(ChipKind::Saa1099),
        }
    }
}

/// The chips `vgm_cmp` copies through untouched.
///
/// Read off the handler list at the top of `vgm_cmp.c`: every chip with a
/// `*_write` prototype there has rules, and these are what is left over. The
/// MultiPCM and K053260 handlers exist but are commented out (`vgm_cmp.c:715`,
/// `:763`) with a `TODO: K053260, K054539 (for mega size reduction)` at
/// `chip_cmp.c:10`; the PWM, GA20, ES5505 and Mikey commands have no case at
/// all and fall to the switch's default, which copies them.
///
/// A file made only of these comes back unchanged however redundant it looks.
/// Worth saying out loud in an export log, for the same reason
/// `dro_core::VgmFile::unoptimised_chips` exists: "the K053260 is not
/// optimised" is a better answer than silence, and a much better one than a
/// smaller file that plays wrong.
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
