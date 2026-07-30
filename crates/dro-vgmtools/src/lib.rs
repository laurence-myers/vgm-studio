//! The vgmtools optimisers, callable on a buffer of VGM bytes.
//!
//! # Why this crate exists
//!
//! `dro_core` optimises three chips; each redundancy rule has to be checked,
//! because a register that *triggers* on write rather than latching makes the
//! generic "same value, drop it" rule silently, audibly wrong. vgmtools has
//! spent two decades accumulating that table for some thirty chips, along with
//! `vgm_sro`'s sample-ROM decoders. So this crate runs the original instead,
//! and equivalence stops being something to test.
//!
//! # Licence
//!
//! vgmtools is GPL-2.0. This crate is therefore GPL-2.0-or-later and only the
//! copyleft half of the workspace links it: `dro-ui` and `dro-trimmer`.
//! `dro-core` and `dro-synth` stay MIT OR Apache-2.0 and never depend on it,
//! which also keeps the wasm build clear of it -- this crate spawns processes
//! and has no place there.
//!
//! # Shape
//!
//! Each call writes the bytes to a temporary file, runs the tool as a **child
//! process**, and reads back what it produced. The process boundary is the
//! design: these are programs that assume they own the process and exit when
//! done. `chip_srom.c` reallocs some fifty sample-ROM buffers and frees none;
//! a ROM size taken straight from a data block can spin a `UINT32` mask
//! forever. As children those cost a reclaimed page table and a timeout; linked
//! in, they would be an unbounded leak in a long-lived GUI and a freeze with no
//! way out.
//!
//! # Input
//!
//! Uncompressed VGM bytes only. `.vgz` is unpacked and repacked above this
//! layer, so gzip arriving here means a caller skipped that, and is refused
//! rather than misread.

mod exe;
mod pipeline;
mod run;
mod strip;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use exe::Tool;

pub use pipeline::{Optimised, Options, Stage, StageOutcome, optimize_vgm, passthrough_chips};
pub use strip::{strip_unused_chips, unused_chips};

/// What a tool did with the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// It found something to remove. Carries the whole new file.
    Smaller(Vec<u8>),
    /// It ran, and left the file alone -- there was nothing to gain.
    ///
    /// The common answer on a published pack, most of which have been through
    /// these same tools already.
    Unchanged,
    /// It could not be run, or did not finish, or produced something that is
    /// not a VGM. Never fatal to a caller: the right response is to keep the
    /// original bytes and say so.
    Failed(String),
}

impl ToolOutcome {
    /// The optimised bytes, or `original` when nothing changed or the run
    /// failed.
    #[must_use]
    pub fn bytes_or<'a>(&'a self, original: &'a [u8]) -> &'a [u8] {
        match self {
            Self::Smaller(bytes) => bytes,
            Self::Unchanged | Self::Failed(_) => original,
        }
    }

    /// Whether the file actually shrank.
    #[must_use]
    pub const fn is_smaller(&self) -> bool {
        matches!(self, Self::Smaller(_))
    }
}

/// Drops chip writes that change nothing -- vgmtools' `vgm_cmp`.
///
/// Covers about thirty chips, each with hand-written rules for the registers
/// that must survive: key-ons that re-attack, counters that reload, addresses
/// the chip itself moves during playback. Runs repeatedly until a pass stops
/// shrinking the file.
#[must_use]
pub fn optimize_writes(vgm: &[u8]) -> ToolOutcome {
    run_tool(Tool::Compress, vgm)
}

/// Strips unused regions out of sample ROMs -- vgmtools' `vgm_sro`.
///
/// Works by replaying the register writes through cut-down decoders for some
/// twenty-six chips and marking which ROM bytes are actually reached, then
/// re-emitting the `0x67` blocks as the used regions only. The declared total
/// ROM size is preserved, so a core still sizes its memory the same way.
#[must_use]
pub fn trim_sample_roms(vgm: &[u8]) -> ToolOutcome {
    run_tool(Tool::SampleRom, vgm)
}

/// Collapses long runs of identical YM2612 DAC writes -- vgmtools' `optdac`.
///
/// Only fires on 128 or more consecutive writes of the same value to port 0
/// register `0x2A`, which is silence held by a driver that keeps feeding the
/// DAC. The delays the removed writes carried are kept.
#[must_use]
pub fn clean_dac_runs(vgm: &[u8]) -> ToolOutcome {
    run_tool(Tool::DacRuns, vgm)
}

fn run_tool(tool: Tool, vgm: &[u8]) -> ToolOutcome {
    if let Err(reason) = check_input(vgm) {
        return ToolOutcome::Failed(reason);
    }

    let workspace = match Workspace::new(tool) {
        Ok(workspace) => workspace,
        Err(error) => return ToolOutcome::Failed(format!("no working directory: {error}")),
    };

    let input = workspace.dir.join("in.vgm");
    let output = workspace.dir.join("out.vgm");
    let log = workspace.dir.join("tool.log");

    if let Err(error) = std::fs::write(&input, vgm) {
        return ToolOutcome::Failed(format!("could not stage the file: {error}"));
    }

    match run::run(tool, &input, &output, &log) {
        Err(reason) => ToolOutcome::Failed(reason),
        Ok(run::Ended::TimedOut) => ToolOutcome::Failed(format!(
            "{} did not finish within {}s and was stopped",
            tool.name(),
            run::TIMEOUT.as_secs()
        )),
        Ok(run::Ended::Exited(code)) => match code {
            Some(0) => collect(tool, &output, &log),
            // A refusal is an answer, not a fault: the file is untouched and
            // still valid, and the reason belongs in the log rather than in
            // front of the user.
            Some(code) if tool.declines_with(code) => {
                log::debug!("{} left the file alone: {}", tool.name(), run::tail(&log));
                ToolOutcome::Unchanged
            }
            Some(code) => ToolOutcome::Failed(format!(
                "{} exited with {code}{}",
                tool.name(),
                suffix(&run::tail(&log))
            )),
            None => ToolOutcome::Failed(format!("{} was terminated", tool.name())),
        },
    }
}

/// Reads back what the tool wrote, if it wrote anything.
///
/// No output file is not a failure: all three tools write only when the result
/// is smaller than the input, so silence means "nothing to gain".
fn collect(tool: Tool, output: &Path, log: &Path) -> ToolOutcome {
    if !output.exists() {
        return ToolOutcome::Unchanged;
    }
    match std::fs::read(output) {
        Err(error) => {
            ToolOutcome::Failed(format!("could not read {}'s output: {error}", tool.name()))
        }
        Ok(bytes) => {
            // A tool that exits 0 having written something that is not a VGM
            // has gone wrong in a way the caller must not propagate to disk.
            if let Err(reason) = check_output(&bytes) {
                return ToolOutcome::Failed(format!(
                    "{} wrote {reason}{}",
                    tool.name(),
                    suffix(&run::tail(log))
                ));
            }
            ToolOutcome::Smaller(bytes)
        }
    }
}

fn suffix(tail: &str) -> String {
    if tail.is_empty() {
        String::new()
    } else {
        format!(" ({tail})")
    }
}

pub(crate) fn check_input(vgm: &[u8]) -> Result<(), String> {
    if vgm.len() >= 2 && vgm[0] == 0x1F && vgm[1] == 0x8B {
        return Err("the bytes are gzip; unpack a .vgz before optimising it".to_owned());
    }
    check_output(vgm).map_err(|reason| format!("the bytes are {reason}"))
}

pub(crate) fn check_output(vgm: &[u8]) -> Result<(), String> {
    if vgm.len() < 0x40 {
        return Err(format!("too short to be a VGM ({} bytes)", vgm.len()));
    }
    if &vgm[..4] != b"Vgm " {
        return Err("not a VGM (no `Vgm ` signature)".to_owned());
    }
    Ok(())
}

/// A private directory for one run, removed when the run ends.
pub(crate) struct Workspace {
    pub(crate) dir: PathBuf,
}

impl Workspace {
    pub(crate) fn new(tool: Tool) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "dro-vgmtools-run-{}-{}-{serial}",
            std::process::id(),
            tool.name()
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_is_refused_by_name_rather_than_misread() {
        let outcome = optimize_writes(&[0x1F, 0x8B, 0x08, 0x00]);
        let ToolOutcome::Failed(reason) = outcome else {
            panic!("gzip should not be accepted");
        };
        assert!(reason.contains("gzip"), "unhelpful message: {reason}");
    }

    #[test]
    fn something_that_is_not_a_vgm_is_refused_before_a_process_starts() {
        let outcome = trim_sample_roms(&[0u8; 0x80]);
        let ToolOutcome::Failed(reason) = outcome else {
            panic!("a non-VGM should not be accepted");
        };
        assert!(reason.contains("Vgm"), "unhelpful message: {reason}");
    }

    #[test]
    fn bytes_or_falls_back_for_everything_but_a_shrink() {
        let original = b"original".as_slice();
        assert_eq!(ToolOutcome::Unchanged.bytes_or(original), original);
        assert_eq!(
            ToolOutcome::Failed("nope".to_owned()).bytes_or(original),
            original
        );
        assert_eq!(
            ToolOutcome::Smaller(b"small".to_vec()).bytes_or(original),
            b"small".as_slice()
        );
    }
}
