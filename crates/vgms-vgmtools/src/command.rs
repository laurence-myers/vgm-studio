//! The tools as *commands*, whoever runs them.
//!
//! Natively a tool is a child process; on the web it is a `wasm32-wasip1`
//! command module a host instantiates. Either way the interaction is the same
//! -- argv `[tool, in.vgm, out.vgm]`, an exit code, maybe an output file, some
//! printed text -- and so is the interpretation. This module holds that
//! interpretation once, so the native binding, the wasmi parity test and the
//! web worker cannot drift apart on what an exit code means.

use crate::ToolOutcome;

/// Which optimiser a command ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    /// `vgm_cmp` -- drops chip writes that change nothing.
    Compress,
    /// `vgm_sro` -- strips unused regions out of sample ROMs.
    SampleRom,
    /// `optdac` -- collapses long runs of identical YM2612 DAC writes.
    DacRuns,
}

impl ToolId {
    /// The name this tool is known by upstream -- also what it is called on
    /// disk, in module names and in log lines.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compress => "vgm_cmp",
            Self::SampleRom => "vgm_sro",
            Self::DacRuns => "optdac",
        }
    }

    /// Whether `code` is this tool's way of saying "there is nothing here for
    /// me", as opposed to "I broke".
    ///
    /// Every tool uses 0 for a normal run and 1 for a file it could not open.
    /// `vgm_sro` adds two refusals that leave the file untouched and valid:
    ///
    /// - `2` -- the header declares no chip that has a sample ROM
    ///   (vgm_sro.c:157), which is most files.
    /// - `9` -- the stream uses RF5C memory writes or `0x68` PCM RAM writes,
    ///   which it says outright it does not support (vgm_sro.c:512, 551, 557).
    #[must_use]
    pub const fn declines_with(self, code: i32) -> bool {
        matches!(self, Self::SampleRom) && matches!(code, 2 | 9)
    }
}

/// Interprets one finished command run as a [`ToolOutcome`].
///
/// `output` is the bytes of `out.vgm` if the tool wrote it, `None` otherwise;
/// `tail` is the last line or two the tool printed, quoted on failure. The
/// rules mirror the native binding exactly:
///
/// - exit 0 with no output: the tool found nothing to gain -- all three write
///   only when the result is smaller than the input.
/// - exit 0 with output: the output must still look like a VGM; a tool that
///   exits 0 having written garbage must not reach a caller.
/// - a decline code ([`ToolId::declines_with`]): the file is untouched and
///   valid, an answer rather than a fault.
/// - anything else: failed, with the tool's own last words when there are any.
#[must_use]
pub fn command_outcome(
    tool: ToolId,
    exit_code: i32,
    output: Option<Vec<u8>>,
    tail: &str,
) -> ToolOutcome {
    match exit_code {
        0 => match output {
            None => ToolOutcome::Unchanged,
            Some(bytes) => match crate::check_output(&bytes) {
                Ok(()) => ToolOutcome::Smaller(bytes),
                Err(reason) => ToolOutcome::Failed(format!(
                    "{} wrote {reason}{}",
                    tool.name(),
                    suffix(tail)
                )),
            },
        },
        code if tool.declines_with(code) => ToolOutcome::Unchanged,
        code => ToolOutcome::Failed(format!("{} exited with {code}{}", tool.name(), suffix(tail))),
    }
}

fn suffix(tail: &str) -> String {
    if tail.is_empty() {
        String::new()
    } else {
        format!(" ({tail})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_zero_without_output_means_nothing_to_gain() {
        assert_eq!(
            command_outcome(ToolId::Compress, 0, None, ""),
            ToolOutcome::Unchanged
        );
    }

    #[test]
    fn a_valid_output_is_the_smaller_file() {
        let mut vgm = vec![0u8; 0x40];
        vgm[..4].copy_from_slice(b"Vgm ");
        assert_eq!(
            command_outcome(ToolId::Compress, 0, Some(vgm.clone()), ""),
            ToolOutcome::Smaller(vgm)
        );
    }

    #[test]
    fn garbage_output_fails_rather_than_propagates() {
        let outcome = command_outcome(ToolId::DacRuns, 0, Some(vec![0u8; 0x40]), "oops");
        let ToolOutcome::Failed(reason) = outcome else {
            panic!("garbage must not come back as Smaller");
        };
        assert!(reason.contains("optdac") && reason.contains("oops"), "{reason}");
    }

    #[test]
    fn sro_decline_codes_are_answers_not_faults() {
        assert_eq!(
            command_outcome(ToolId::SampleRom, 2, None, "No chips with Sample-ROM used!"),
            ToolOutcome::Unchanged
        );
        assert_eq!(
            command_outcome(ToolId::SampleRom, 9, None, ""),
            ToolOutcome::Unchanged
        );
        // The same codes from any other tool are failures.
        assert!(matches!(
            command_outcome(ToolId::Compress, 2, None, ""),
            ToolOutcome::Failed(_)
        ));
    }

    #[test]
    fn a_failure_quotes_the_tools_last_words() {
        let ToolOutcome::Failed(reason) = command_outcome(ToolId::SampleRom, 1, None, "Error opening the file!")
        else {
            panic!("exit 1 is a failure");
        };
        assert!(
            reason.contains("vgm_sro") && reason.contains("Error opening the file!"),
            "{reason}"
        );
    }
}
