//! `vgmstudio optimize`: make a VGM smaller without changing what it plays.
//!
//! Runs the vgmtools optimisers -- `optdac`, `vgm_cmp`, `vgm_sro` -- and then
//! this app's own pass, which finishes with a byte-minimal delay re-encoding.
//! A wholly-OPL file skips the tools and takes the built-in path alone, which
//! has covered that family from the start.
//!
//! In-place by default (like `vgm_cmp`); pass an explicit output to keep the
//! original. A DRO is rejected, and a VGM that is already optimal is written
//! back byte for byte with a note rather than an error.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use vgms_vgmtools::{Options, StageOutcome};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The VGM or VGZ file to optimize.
    pub input: PathBuf,
    /// Where to write the result. Defaults to overwriting the input, as
    /// `vgm_cmp` does. The extension chooses the container: `.vgz` gzips.
    pub output: Option<PathBuf>,
    /// Skip the sample-ROM trim (`vgm_sro`), which drops ROM regions no
    /// register write ever reaches.
    #[arg(long)]
    pub no_rom_trim: bool,
    /// Skip the DAC-run clean-up (`optdac`), which collapses long runs of
    /// identical YM2612 DAC writes.
    #[arg(long)]
    pub no_dac_clean: bool,
    /// Also remove chips the header declares that the stream never writes to.
    ///
    /// Off by default: this changes what the file *declares*, not just how it
    /// is spelled, so it is asked for rather than assumed. Skipped entirely on
    /// a file carrying data blocks, where a chip can be fed without a register
    /// write ever naming it.
    #[arg(long)]
    pub strip_unused_chips: bool,
}

/// Optimises `args.input`, writing to `args.output` (or back over the input).
///
/// # Errors
/// If the song cannot be read, it is a DRO rather than a VGM, or the result
/// cannot be written.
pub fn run(args: &Args) -> Result<()> {
    let raw =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let name = args
        .input
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.vgm");
    // Read it once up front, so a DRO or an unreadable file is refused here
    // rather than by a tool three frames down.
    let file = vgms_core::vgm::file::read(name, &raw).map_err(|error| {
        anyhow::anyhow!(
            "optimize only applies to VGM files; {} could not be read as one ({error})",
            args.input.display()
        )
    })?;

    // The optimisers take plain bytes; `.vgz` is this app's business, on the
    // way in as much as on the way out.
    let plain = vgms_core::vgm::file::write(&file).context("preparing the song")?;

    let options = Options {
        sample_roms: !args.no_rom_trim,
        dac_runs: !args.no_dac_clean,
        // The CLI takes the routing default (built-in where it covers the file,
        // the tools as fallback). The GUI's Settings choice is a GUI concern.
        optimizer: vgms_core::config::OptimizerChoice::Auto,
    };
    let mut result = vgms_vgmtools::optimize_vgm(&plain, options);

    // Asked for separately, and applied to what the pass produced: a chip that
    // is unused is unused whether or not its writes were tidied first.
    let mut stripped: Vec<&'static str> = Vec::new();
    if args.strip_unused_chips {
        stripped = vgms_vgmtools::unused_chips(&result.bytes)
            .into_iter()
            .map(vgms_core::ChipKind::name)
            .collect();
        match vgms_vgmtools::strip_unused_chips(&result.bytes) {
            vgms_vgmtools::ToolOutcome::Smaller(bytes) => result.bytes = bytes,
            vgms_vgmtools::ToolOutcome::Unchanged => stripped.clear(),
            vgms_vgmtools::ToolOutcome::Failed(reason) => {
                stripped.clear();
                println!("  vgm_ptch  FAILED: {reason}");
            }
        }
    }

    let output = args.output.clone().unwrap_or_else(|| args.input.clone());
    let bytes = if output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vgz"))
    {
        let mut optimized = vgms_core::vgm::file::read(name, &result.bytes)
            .context("re-reading the optimized song")?;
        optimized.name = name.to_owned();
        vgms_core::vgm::file::write_gzipped(&optimized).context("gzipping the optimized song")?
    } else {
        result.bytes.clone()
    };
    std::fs::write(&output, &bytes).with_context(|| format!("writing {}", output.display()))?;

    report(args, &output, &result, bytes.len());
    if !stripped.is_empty() {
        println!(
            "  {:<9} removed {} (declared but never written to)",
            "vgm_ptch",
            stripped.join(", ")
        );
    }
    Ok(())
}

/// Prints what each stage did, then the total.
///
/// Per stage rather than one number, because "already optimal" and "that chip
/// has no rules" and "the tool refused this file" are different answers and a
/// single byte count cannot tell them apart.
fn report(args: &Args, output: &Path, result: &vgms_vgmtools::Optimized, written: usize) {
    if result.changed() {
        println!(
            "Optimized {} -> {}: {} -> {} bytes, {} saved ({written} bytes written)",
            args.input.display(),
            output.display(),
            result.original_len,
            result.bytes.len(),
            result.saved(),
        );
    } else {
        println!(
            "{} is already optimal; wrote {} ({written} bytes)",
            args.input.display(),
            output.display(),
        );
    }

    for stage in &result.stages {
        match &stage.outcome {
            StageOutcome::Shrank { from, to } => {
                println!("  {:<9} {from} -> {to} bytes", stage.name);
            }
            StageOutcome::Unchanged => println!("  {:<9} nothing to gain", stage.name),
            StageOutcome::Skipped(reason) => println!("  {:<9} skipped: {reason}", stage.name),
            // Never fatal: the pass carried on from the bytes this stage was
            // handed, so the file is sound and the user should still know.
            StageOutcome::Failed(reason) => println!("  {:<9} FAILED: {reason}", stage.name),
        }
    }

    // Chips no tool has rules for keep every write; say so rather than let a
    // file that could not shrink look like one that had nothing to gain.
    let untouched = passthrough_chips_in(&result.bytes);
    if !untouched.is_empty() {
        println!(
            "  (no redundancy rules for {} -- their writes were all kept)",
            untouched.join(", ")
        );
    }
}

/// The chips in this file that `vgm_cmp` copies through untouched.
fn passthrough_chips_in(bytes: &[u8]) -> Vec<&'static str> {
    let Ok(file) = vgms_core::vgm::file::read("optimized.vgm", bytes) else {
        return Vec::new();
    };
    file.header
        .chips()
        .iter()
        .filter(|chip| vgms_vgmtools::passthrough_chips().contains(&chip.kind))
        .map(|chip| chip.kind.name())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgms_core::io::write_song;
    use vgms_core::vgm::io::synthesise_header;
    use vgms_core::{DroDataV1, DroSong, OplType};

    /// A distinct temp path per test, namespaced by the process so parallel runs
    /// of the binary cannot collide.
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vgmstudio-opt-{}-{name}", std::process::id()))
    }

    fn args(input: &Path, output: Option<PathBuf>) -> Args {
        Args {
            input: input.to_path_buf(),
            output,
            no_rom_trim: false,
            no_dac_clean: false,
            strip_unused_chips: false,
        }
    }

    /// A wholly-OPL2 VGM with a redundant write between two delays, assembled as a
    /// `VgmFile` (a synthesised v1.51 header with the YM3812 clock, the stream, an
    /// end marker) and canonicalised through the reader/writer.
    fn redundant_vgm_bytes() -> Vec<u8> {
        let stream = [
            0x5A, 0x20, 0x01, // write
            0x61, 0x64, 0x00, // wait 100
            0x5A, 0x20, 0x01, // redundant write
            0x61, 0xC8, 0x00, // wait 200
            0x5A, 0x21, 0x02, // write
        ];
        let mut bytes = synthesise_header();
        // The YM3812 clock (offset 0x50, spec-stable) makes it a wholly-OPL2 file,
        // so the optimiser takes the built-in path.
        bytes[0x50..0x54].copy_from_slice(&3_579_545u32.to_le_bytes());
        bytes.extend_from_slice(&stream);
        bytes.push(0x66); // end marker
        let eof = (bytes.len() - 0x04) as u32;
        bytes[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
        let file = vgms_core::vgm::file::read("x.vgm", &bytes).unwrap();
        vgms_core::vgm::file::write(&file).unwrap()
    }

    #[test]
    fn optimizes_a_vgm_to_a_smaller_still_valid_file() {
        let input = temp_path("in.vgm");
        let output = temp_path("out.vgm");
        std::fs::write(&input, redundant_vgm_bytes()).unwrap();
        let original_len = std::fs::metadata(&input).unwrap().len();

        run(&args(&input, Some(output.clone()))).unwrap();

        let optimized = std::fs::read(&output).unwrap();
        assert!(
            (optimized.len() as u64) < original_len,
            "the optimized file should be smaller"
        );
        // Still a valid VGM.
        vgms_core::vgm::file::read("out.vgm", &optimized).unwrap();

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[test]
    fn gzips_when_the_output_is_a_vgz() {
        let input = temp_path("gz-in.vgm");
        let output = temp_path("gz-out.vgz");
        std::fs::write(&input, redundant_vgm_bytes()).unwrap();

        run(&args(&input, Some(output.clone()))).unwrap();

        let written = std::fs::read(&output).unwrap();
        assert!(
            vgms_core::vgm::io::is_gzipped(&written),
            "a .vgz output should be gzipped"
        );
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[test]
    fn reads_a_gzipped_input() {
        // The tools refuse gzip, so unpacking on the way in is this app's job.
        let input = temp_path("gz-round.vgz");
        let file = vgms_core::vgm::file::read("x.vgm", &redundant_vgm_bytes()).unwrap();
        std::fs::write(&input, vgms_core::vgm::file::write_gzipped(&file).unwrap()).unwrap();
        let output = temp_path("gz-round-out.vgm");

        run(&args(&input, Some(output.clone()))).unwrap();

        let written = std::fs::read(&output).unwrap();
        assert!(!vgms_core::vgm::io::is_gzipped(&written));
        vgms_core::vgm::file::read("out.vgm", &written).expect("a valid VGM");
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[test]
    fn refuses_a_dro() {
        let input = temp_path("in.dro");
        let dro = DroSong::dro_v1(
            "x.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01]).unwrap(),
            0,
            OplType::Opl2,
        );
        std::fs::write(&input, write_song(&dro).unwrap()).unwrap();

        let error = run(&args(&input, None)).unwrap_err();
        assert!(
            error.to_string().contains("only applies to VGM"),
            "got {error}"
        );
        std::fs::remove_file(&input).ok();
    }
}
