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

/// What the editor's Edit > Optimize concluded.
///
/// The web arm only ever produces [`Self::Accepted`]/[`Self::Nothing`] -- its
/// built-in pass is ungated (D-orw-7) -- while the native arm can also keep the
/// original when the render gate rejects a shrink.
#[derive(Debug, Clone)]
pub enum EditorOptimize {
    /// The (verified) optimised bytes, ready to install.
    Accepted(Vec<u8>),
    /// Nothing to gain -- the file is already optimal.
    Nothing,
    /// The pass shrank the file but it was kept because the render differed or
    /// could not be verified; carries a one-line reason.
    Kept(String),
}

/// The editor's Edit > Optimize, render-gated on native.
///
/// Native runs the full pipeline and verifies the result by rendering, keeping
/// the original if the samples differ (`optimize_verified`). `optimizer` is the
/// Settings choice -- built-in, the external tools, or the routing between.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn optimized_editor(
    bytes: &[u8],
    optimizer: vgms_core::config::OptimizerChoice,
) -> EditorOptimize {
    let options = vgms_vgmtools::Options {
        optimizer,
        ..Default::default()
    };
    let verified = optimize_verified(
        bytes,
        options,
        &vgms_vgmtools::NativeTools,
        vgms_synth::VerifyOptions::default(),
    );
    match verified.outcome {
        VerifiedOutcome::Optimized(bytes) => EditorOptimize::Accepted(bytes),
        VerifiedOutcome::Unchanged => EditorOptimize::Nothing,
        VerifiedOutcome::KeptOriginal(verdict) => EditorOptimize::Kept(describe_verdict(verdict)),
        VerifiedOutcome::Unverifiable(reason) => EditorOptimize::Kept(reason),
    }
}

/// The editor's Edit > Optimize on the web: the built-in pass, ungated (the wasm
/// tool modules are the pack worker's, not the editor's, so the Settings choice
/// does not bite here, and there is no render gate yet -- D-orw-7).
#[cfg(target_arch = "wasm32")]
pub(crate) fn optimized_editor(
    bytes: &[u8],
    _optimizer: vgms_core::config::OptimizerChoice,
) -> EditorOptimize {
    let optimized = vgms_core::vgm::file::read("optimizing.vgm", bytes)
        .ok()
        .and_then(|mut file| {
            file.optimize()?;
            vgms_core::vgm::file::write(&file).ok()
        });
    match optimized {
        Some(bytes) => EditorOptimize::Accepted(bytes),
        None => EditorOptimize::Nothing,
    }
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

/// Optimises one pack song and verifies it, preserving its on-disk format and
/// narrating the pass for the pack log -- the whole per-track job the native
/// pack service runs off the UI thread.
///
/// `request.bytes` are the file as it sits on disk (a `.vgz` is unpacked,
/// optimised, and re-packed so the format is kept). A result is accepted only
/// when it both verifies identical *and* is smaller on disk, so a `.vgz` whose
/// gzip already subsumed the redundancy is reported as already optimal rather
/// than rewritten larger. Never fatal: any failure keeps the original bytes.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn optimize_song_verified(
    request: &crate::platform::SongOptimizeRequest,
) -> crate::platform::SongOptimizeResult {
    use crate::platform::{SongOptimizeOutcome, SongOptimizeResult};

    let name = request.name.clone();
    let original_len = request.bytes.len();
    let gzipped = vgms_core::vgm::io::is_gzipped(&request.bytes);
    let kept = |outcome, log| SongOptimizeResult {
        name: name.clone(),
        original_len,
        outcome,
        log,
    };

    // Plain bytes for the tools -- a `.vgz` is unpacked here.
    let plain = match vgms_core::vgm::file::read(&name, &request.bytes)
        .and_then(|file| vgms_core::vgm::file::write(&file))
    {
        Ok(plain) => plain,
        Err(error) => {
            let reason = format!("could not read {name}: {error}");
            return kept(SongOptimizeOutcome::Failed(reason.clone()), vec![reason]);
        }
    };

    let options = vgms_vgmtools::Options {
        sample_roms: request.sample_roms,
        dac_runs: request.dac_runs,
        optimizer: request.optimizer,
    };
    let verify = vgms_synth::VerifyOptions::new(request.output_rate);
    let verified = optimize_verified(&plain, options, &vgms_vgmtools::NativeTools, verify);

    let mut log = narrate_stages(&name, &verified);
    let outcome = match &verified.outcome {
        VerifiedOutcome::Unchanged => SongOptimizeOutcome::Unchanged,
        VerifiedOutcome::KeptOriginal(verdict) => {
            let reason = describe_verdict(*verdict);
            log.push(format!("{name}: kept original -- {reason}"));
            SongOptimizeOutcome::KeptDiffered(reason)
        }
        VerifiedOutcome::Unverifiable(reason) => {
            log.push(format!("{name}: kept original -- {reason}"));
            SongOptimizeOutcome::Unverifiable(reason.clone())
        }
        VerifiedOutcome::Optimized(optimized_plain) => {
            // Re-encode to the on-disk format so a .vgz stays a .vgz.
            let reencoded = if gzipped {
                vgms_core::vgm::file::read(&name, optimized_plain)
                    .and_then(|file| vgms_core::vgm::file::write_gzipped(&file))
            } else {
                Ok(optimized_plain.clone())
            };
            match reencoded {
                Ok(final_bytes) if final_bytes.len() < original_len => {
                    log.push(format!(
                        "{name}: {original_len} -> {} bytes (verified, {} saved)",
                        final_bytes.len(),
                        original_len - final_bytes.len()
                    ));
                    SongOptimizeOutcome::Optimized(final_bytes)
                }
                // Verified, but no smaller on disk (a .vgz whose gzip already
                // subsumed the redundancy): keep the original bytes.
                Ok(_) => {
                    log.push(format!("{name}: verified, but no smaller on disk"));
                    SongOptimizeOutcome::Unchanged
                }
                Err(error) => {
                    let reason = format!("could not re-encode: {error}");
                    log.push(format!("{name}: kept original -- {reason}"));
                    SongOptimizeOutcome::Unverifiable(reason)
                }
            }
        }
    };

    kept(outcome, log)
}

/// The per-stage narration a pack log wants, in the order the stages ran --
/// the same lines `optimize_song_logged` produces, minus the untouched-chip
/// note (a per-track action reports on one file at a time).
#[cfg(not(target_arch = "wasm32"))]
fn narrate_stages(name: &str, verified: &VerifiedOptimized) -> Vec<String> {
    let mut log = Vec::new();
    for stage in &verified.stages {
        match &stage.outcome {
            vgms_vgmtools::StageOutcome::Shrank { from, to } => {
                log.push(format!("{name}:   {} {from} -> {to} bytes", stage.name));
            }
            vgms_vgmtools::StageOutcome::Failed(reason) => {
                log.push(format!("{name}:   {} failed: {reason}", stage.name));
            }
            vgms_vgmtools::StageOutcome::Skipped(reason) => {
                log.push(format!("{name}:   {} skipped: {reason}", stage.name));
            }
            vgms_vgmtools::StageOutcome::Unchanged => {}
        }
    }
    log
}

/// A one-line description of a render-gate verdict, for a log or status line.
#[cfg(not(target_arch = "wasm32"))]
fn describe_verdict(verdict: vgms_synth::Verdict) -> String {
    match verdict {
        vgms_synth::Verdict::Identical => "renders identically".to_owned(),
        vgms_synth::Verdict::DiffersAt { sample, of } => {
            format!("render differed at sample {sample} of {of}")
        }
    }
}
