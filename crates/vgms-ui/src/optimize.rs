//! Making a VGM smaller, by whichever route this build has.
//!
//! On the desktop that is the vgmtools optimisers -- `optdac`, `vgm_cmp`,
//! `vgm_sro` -- followed by `vgms_core`'s own pass, which is where the
//! byte-minimal delay re-encoding comes from. They run as child processes, so
//! they cannot come to the web.
//!
//! On wasm the same action still works; it just reaches the three chips
//! `vgms_core` has rules for rather than the thirty `vgm_cmp` does. That is a
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
    "\nVGM optimizers (vgm_cmp, vgm_sro, optdac) from the vgmtools\n\
     project, used under the GPL-2.0 and built into this binary.\n\
     Source: https://github.com/vgmrips/vgmtools\n"
}

/// The About box's stanza for the optimisers -- nothing, on the web.
#[cfg(target_arch = "wasm32")]
pub(crate) const fn credit() -> &'static str {
    ""
}

/// The optimised file, or `None` when there was nothing to gain. `optimizer` is
/// the Settings choice -- built-in, the external tools, or the routing between.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn optimized(
    bytes: &[u8],
    optimizer: vgms_core::config::OptimizerChoice,
) -> Option<Vec<u8>> {
    let options = vgms_vgmtools::Options {
        optimizer,
        ..Default::default()
    };
    let result = vgms_vgmtools::optimize_vgm(bytes, options);
    for stage in &result.stages {
        match &stage.outcome {
            // Never fatal: the pass carried on from the bytes this stage was
            // handed, so the document is sound and the log is where this goes.
            vgms_vgmtools::StageOutcome::Failed(reason) => {
                log::warn!("optimizing: {} failed: {reason}", stage.name);
            }
            vgms_vgmtools::StageOutcome::Skipped(reason) => {
                log::debug!("optimizing: {} skipped: {reason}", stage.name);
            }
            _ => {}
        }
    }
    result.changed().then_some(result.bytes)
}

/// The optimised file, or `None` when there was nothing to gain. The web editor
/// optimise is always the built-in (the wasm tool modules are the pack worker's,
/// not the editor's), so the Settings choice does not bite here.
#[cfg(target_arch = "wasm32")]
pub(crate) fn optimized(
    bytes: &[u8],
    _optimizer: vgms_core::config::OptimizerChoice,
) -> Option<Vec<u8>> {
    let mut file = vgms_core::vgm::file::read("optimizing.vgm", bytes).ok()?;
    file.optimize()?;
    vgms_core::vgm::file::write(&file).ok()
}

/// What a verified optimise did, once the render gate has had its say.
///
/// The gate ([`vgms_synth::renders_identically`]) renders the original and the
/// optimised file and requires identical samples before the smaller file is
/// accepted; a difference keeps the original, never fatally (D-orw-4).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub enum VerifiedOutcome {
    /// The pass shrank the file and both renders matched: the bytes are safe to
    /// write in place.
    Optimized(Vec<u8>),
    /// The pass found nothing to gain -- the file is already optimal. The
    /// original is kept.
    Unchanged,
    /// The pass shrank the file but the renders diverged: the original is kept,
    /// and the verdict says where they parted.
    KeptOriginal(vgms_synth::Verdict),
    /// The pass shrank the file but its output (or the original) could not be
    /// read back to render, so the change could not be verified. The original is
    /// kept.
    Unverifiable(String),
}

/// The result of [`optimize_verified`]: the pass's stages (for a log) and what
/// the render gate concluded.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct VerifiedOptimized {
    /// The size before the pass, for a savings figure.
    pub original_len: usize,
    /// Every pipeline stage in the order it ran.
    pub stages: Vec<vgms_vgmtools::Stage>,
    /// What became of the file.
    pub outcome: VerifiedOutcome,
}

#[cfg(not(target_arch = "wasm32"))]
impl VerifiedOptimized {
    /// The bytes safe to write in place, or `None` when the original must be
    /// kept (unchanged, a failed verification, or an unverifiable result).
    #[must_use]
    pub fn accepted_bytes(&self) -> Option<&[u8]> {
        match &self.outcome {
            VerifiedOutcome::Optimized(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// How many bytes the accepted result saved, or `0` when nothing was
    /// accepted.
    #[must_use]
    pub fn saved(&self) -> usize {
        self.accepted_bytes()
            .map_or(0, |bytes| self.original_len.saturating_sub(bytes.len()))
    }
}

/// Optimises `bytes` and verifies the result by rendering: the original and the
/// optimised file are rendered through the real engine and must produce
/// identical samples before the smaller file is accepted (D-orw-1). A
/// difference -- or an unreadable result -- keeps the original bytes and says
/// so, never fatally (D-orw-4).
///
/// `bytes` are plain (uncompressed) VGM bytes, as the tools take them.
/// Rendering uses the ambient core registry, so the caller must have installed
/// cores; a shell that has not would render silence on both sides and accept
/// everything, which is why this only runs where playback does. Native-only
/// (D-orw-7): the web pack path stays ungated for now.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn optimize_verified(
    bytes: &[u8],
    options: vgms_vgmtools::Options,
    tools: &dyn vgms_vgmtools::Tools,
    verify: vgms_synth::VerifyOptions,
) -> VerifiedOptimized {
    use std::sync::Arc;

    let result = vgms_vgmtools::optimize_vgm_with(bytes, options, tools);
    let original_len = result.original_len;

    if !result.changed() {
        // Nothing was dropped, so there is nothing to be wrong about -- and
        // nothing to render.
        return VerifiedOptimized {
            original_len,
            stages: result.stages,
            outcome: VerifiedOutcome::Unchanged,
        };
    }

    // The two sides: the original bytes as handed in, and what the pass made.
    let read = |label: &str, raw: &[u8]| -> Result<Arc<vgms_core::vgm::VgmFile>, String> {
        vgms_core::vgm::file::read("optimizing.vgm", raw)
            .map(Arc::new)
            .map_err(|error| format!("the {label} file no longer reads: {error}"))
    };
    let (original, candidate) = match (read("original", bytes), read("optimized", &result.bytes)) {
        (Ok(original), Ok(candidate)) => (original, candidate),
        (Err(reason), _) | (_, Err(reason)) => {
            return VerifiedOptimized {
                original_len,
                stages: result.stages,
                outcome: VerifiedOutcome::Unverifiable(reason),
            };
        }
    };

    let outcome = match vgms_synth::renders_identically(&original, &candidate, verify) {
        vgms_synth::Verdict::Identical => VerifiedOutcome::Optimized(result.bytes),
        differs => VerifiedOutcome::KeptOriginal(differs),
    };
    VerifiedOptimized {
        original_len,
        stages: result.stages,
        outcome,
    }
}
