//! Removing chips a rip declares but never writes to -- vgmtools' `vgm_ptch`.
//!
//! Opt-in, never part of the default pass: removing a chip changes what the
//! file *declares* rather than only how it is spelled, and that is the user's
//! call. (Across 1500 published VGMRips files not one declares an unwritten
//! chip, but those have already been through submission review; the case
//! belongs to *fresh* rips, which the corpus cannot contain.)
//!
//! Detection is ours, removal is theirs: deciding which chips are unwritten is
//! a question about the command stream that `dro_core` already models, while
//! actually removing one -- chip config bytes, the v1.70 extra header's
//! per-chip clock and volume entries, a header whose size and version can come
//! down afterwards -- is what `vgm_ptch` knows.

use std::collections::BTreeSet;

use dro_core::vgm::{ChipKind, VgmCommand};

use crate::exe::Tool;
use crate::{ToolOutcome, Workspace, check_input, run};

/// What `vgm_ptch` calls each chip on a `-Strip:` command line.
///
/// Transcribed from its own parser (`vgm_ptch.c:665`-`865`), not from its
/// `-StripList` help text -- the two disagree, and the parser is what runs.
///
/// The chips missing here are missing from `vgm_ptch`: the SCSP, WonderSwan,
/// VSU, SAA1099, ES5503, ES5505, X1-010, C352, GA20 and Mikey have no name it
/// accepts (*"Stripping does not (yet) work with all chips."*). A file whose
/// only unused chip is one of those comes back unchanged.
const STRIP_NAMES: &[(ChipKind, &str)] = &[
    (ChipKind::Sn76489, "SN76496"),
    (ChipKind::Ym2413, "YM2413"),
    (ChipKind::Ym2612, "YM2612"),
    (ChipKind::Ym2151, "YM2151"),
    (ChipKind::SegaPcm, "SegaPCM"),
    (ChipKind::Rf5c68, "RF5C68"),
    (ChipKind::Ym2203, "YM2203"),
    (ChipKind::Ym2608, "YM2608"),
    (ChipKind::Ym2610, "YM2610"),
    (ChipKind::Ym3812, "YM3812"),
    (ChipKind::Ym3526, "YM3526"),
    (ChipKind::Y8950, "Y8950"),
    (ChipKind::Ymf262, "YMF262"),
    (ChipKind::Ymf278b, "YMF278B"),
    (ChipKind::Ymf271, "YMF271"),
    (ChipKind::Ymz280b, "YMZ280B"),
    (ChipKind::Rf5c164, "RF5C164"),
    (ChipKind::Pwm, "PWM"),
    (ChipKind::Ay8910, "AY8910"),
    (ChipKind::GameBoyDmg, "GBDMG"),
    (ChipKind::NesApu, "NESAPU"),
    (ChipKind::MultiPcm, "MultiPCM"),
    (ChipKind::Upd7759, "UPD7759"),
    (ChipKind::Okim6258, "OKIM6258"),
    (ChipKind::Okim6295, "OKIM6295"),
    (ChipKind::K051649, "K051649"),
    (ChipKind::K054539, "K054539"),
    (ChipKind::HuC6280, "HuC6280"),
    (ChipKind::C140, "C140"),
    (ChipKind::K053260, "K053260"),
    (ChipKind::Pokey, "Pokey"),
    (ChipKind::QSound, "QSound"),
    // `vgm_ptch` also accepts K007232, K005289 and DacCtrl. The first two are
    // chips `dro_core::ChipKind` does not model, so nothing here can name them;
    // DacCtrl is a stream, not a chip at all.
];

/// The chips `vgm` declares that no command in it ever writes to.
///
/// Conservative on purpose. A chip is reported only when **no** `Write` targets
/// it *and* the stream carries nothing that could feed a chip without one --
/// no data blocks, no DAC streams, no PCM RAM writes, no `0x8n` DAC writes. A
/// sample ROM handed to a chip is a use of it that no register write need
/// record, so a file carrying any of those is left entirely alone rather than
/// half-analysed.
#[must_use]
pub fn unused_chips(vgm: &[u8]) -> Vec<ChipKind> {
    let Ok(file) = dro_core::vgm::file::read("checking.vgm", vgm) else {
        return Vec::new();
    };
    let Some(stream) = file.stream() else {
        return Vec::new();
    };

    let mut written: BTreeSet<ChipKind> = BTreeSet::new();
    for index in 0..stream.len() {
        match stream.get(index) {
            Some(VgmCommand::Write { target, .. }) => {
                written.insert(target.kind);
            }
            Some(
                VgmCommand::DataBlock { .. }
                | VgmCommand::DacStream { .. }
                | VgmCommand::PcmRamWrite { .. }
                | VgmCommand::DacWrite { .. },
            ) => return Vec::new(),
            _ => {}
        }
    }

    file.header
        .chips()
        .iter()
        .map(|chip| chip.kind)
        .filter(|kind| !written.contains(kind))
        .collect()
}

/// Strips every chip `vgm` declares but never writes to.
///
/// [`ToolOutcome::Unchanged`] when there are none, or when the only ones are
/// chips `vgm_ptch` cannot name.
#[must_use]
pub fn strip_unused_chips(vgm: &[u8]) -> ToolOutcome {
    if let Err(reason) = check_input(vgm) {
        return ToolOutcome::Failed(reason);
    }

    let unused = unused_chips(vgm);
    let names: Vec<&str> = unused
        .iter()
        .filter_map(|kind| {
            STRIP_NAMES
                .iter()
                .find(|(candidate, _)| candidate == kind)
                .map(|(_, name)| *name)
        })
        .collect();
    if names.is_empty() {
        return ToolOutcome::Unchanged;
    }

    let workspace = match Workspace::new(Tool::Patch) {
        Ok(workspace) => workspace,
        Err(error) => return ToolOutcome::Failed(format!("no working directory: {error}")),
    };
    // `vgm_ptch` patches in place, so it gets a copy of its own and the copy is
    // what comes back.
    let work = workspace.dir.join("work.vgm");
    let log = workspace.dir.join("tool.log");
    if let Err(error) = std::fs::write(&work, vgm) {
        return ToolOutcome::Failed(format!("could not stage the file: {error}"));
    }

    // `-MinHeader` and `-MinVer` are upstream's own advice for after a strip
    // ("useful after stripping chips"): with a chip gone, the header can often
    // be shorter and the version lower.
    let strip = format!("-Strip:{}", names.join(";"));
    let args = [
        std::ffi::OsStr::new(strip.as_str()),
        std::ffi::OsStr::new("-MinVer"),
        std::ffi::OsStr::new("-MinHeader"),
        work.as_os_str(),
    ];

    match run::run_args(Tool::Patch, &args, &log) {
        Err(reason) => ToolOutcome::Failed(reason),
        Ok(run::Ended::TimedOut) => ToolOutcome::Failed(format!(
            "vgm_ptch did not finish within {}s and was stopped",
            run::TIMEOUT.as_secs()
        )),
        Ok(run::Ended::Exited(Some(0))) => match std::fs::read(&work) {
            Err(error) => ToolOutcome::Failed(format!("could not read vgm_ptch's output: {error}")),
            Ok(bytes) => {
                if bytes.len() == vgm.len() && bytes == vgm {
                    ToolOutcome::Unchanged
                } else if let Err(reason) = crate::check_output(&bytes) {
                    ToolOutcome::Failed(format!("vgm_ptch wrote {reason}"))
                } else {
                    ToolOutcome::Smaller(bytes)
                }
            }
        },
        Ok(run::Ended::Exited(Some(code))) => {
            ToolOutcome::Failed(format!("vgm_ptch exited with {code} ({})", run::tail(&log)))
        }
        Ok(run::Ended::Exited(None)) => ToolOutcome::Failed("vgm_ptch was terminated".to_owned()),
    }
}
