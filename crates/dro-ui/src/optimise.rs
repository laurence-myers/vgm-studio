//! Making a VGM smaller, by whichever route this build has.
//!
//! On the desktop that is the vgmtools optimisers -- `optdac`, `vgm_cmp`,
//! `vgm_sro` -- followed by `dro_core`'s own pass, which is where the
//! byte-minimal delay re-encoding comes from. They run as child processes, so
//! they cannot come to the web.
//!
//! On wasm the same action still works; it just reaches the three chips
//! `dro_core` has rules for rather than the thirty `vgm_cmp` does. That is a
//! smaller answer, never a wrong one -- the built-in pass drops nothing from a
//! chip it has no rules for. `docs/vgm-multichip-2026-07/OPTIMIZER-WASM-PLAN.md`
//! is how the web catches up.
//!
//! Both arms take and return whole-file bytes, so the caller does not have to
//! know which one it got.

/// The About box's stanza for the optimisers.
///
/// They are GPL-2.0 programs shipped inside this binary, so crediting them and
/// pointing at their source is a licence obligation, not a courtesy -- the same
/// reason the emulator cores have a stanza. Empty on the web, where they are
/// not shipped at all.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const fn credit() -> &'static str {
    "\nVGM optimisers (vgm_cmp, vgm_sro, optdac) from the vgmtools\n\
     project, used under the GPL-2.0 and built into this binary.\n\
     Source: https://github.com/vgmrips/vgmtools\n"
}

/// The About box's stanza for the optimisers -- nothing, on the web.
#[cfg(target_arch = "wasm32")]
pub(crate) const fn credit() -> &'static str {
    ""
}

/// The optimised file, or `None` when there was nothing to gain.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn optimised(bytes: &[u8]) -> Option<Vec<u8>> {
    let result = dro_vgmtools::optimize_vgm(bytes, dro_vgmtools::Options::default());
    for stage in &result.stages {
        match &stage.outcome {
            // Never fatal: the pass carried on from the bytes this stage was
            // handed, so the document is sound and the log is where this goes.
            dro_vgmtools::StageOutcome::Failed(reason) => {
                log::warn!("optimising: {} failed: {reason}", stage.name);
            }
            dro_vgmtools::StageOutcome::Skipped(reason) => {
                log::debug!("optimising: {} skipped: {reason}", stage.name);
            }
            _ => {}
        }
    }
    result.changed().then_some(result.bytes)
}

/// The optimised file, or `None` when there was nothing to gain.
#[cfg(target_arch = "wasm32")]
pub(crate) fn optimised(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut file = dro_core::vgm::file::read("optimising.vgm", bytes).ok()?;
    file.optimize()?;
    dro_core::vgm::file::write(&file).ok()
}
