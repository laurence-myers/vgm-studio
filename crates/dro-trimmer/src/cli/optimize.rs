//! `drotrim optimize`: strip audibly-redundant OPL writes from a VGM and merge
//! the delays left behind -- the `vgm_cmp` step of the VGMRips pipeline.
//!
//! In-place by default (like `vgm_cmp`); pass an explicit output to keep the
//! original. A DRO is rejected, and a VGM that is already optimal is written back
//! byte for byte with a note rather than an error.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The VGM or VGZ file to optimise.
    pub input: PathBuf,
    /// Where to write the result. Defaults to overwriting the input, as
    /// `vgm_cmp` does. The extension chooses the container: `.vgz` gzips.
    pub output: Option<PathBuf>,
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
    let mut file = dro_core::vgm::file::read(name, &raw).map_err(|error| {
        anyhow::anyhow!(
            "optimize only applies to VGM files; {} could not be read as one ({error})",
            args.input.display()
        )
    })?;

    let output = args.output.clone().unwrap_or_else(|| args.input.clone());
    // Writing follows the output's extension (`.vgz` gzips), so name the file
    // after where it is going.
    if let Some(name) = output.file_name().and_then(|s| s.to_str()) {
        file.name = name.to_owned();
    }

    let before = file.body.raw().len();
    let removed = file.optimize();
    let saved = removed.map(|removed| (removed, before - file.body.raw().len()));

    let bytes = if file.name.to_ascii_lowercase().ends_with(".vgz") {
        dro_core::vgm::file::write_gzipped(&file)
    } else {
        dro_core::vgm::file::write(&file)
    }
    .context("serialising the optimised song")?;
    std::fs::write(&output, &bytes).with_context(|| format!("writing {}", output.display()))?;

    match saved {
        Some((commands, saved)) => println!(
            "Optimised {} -> {}: removed {commands} command(s), {saved} stream byte(s) smaller \
             ({} bytes written)",
            args.input.display(),
            output.display(),
            bytes.len(),
        ),
        None => println!(
            "{} is already optimal; wrote {} ({} bytes)",
            args.input.display(),
            output.display(),
            bytes.len(),
        ),
    }
    // Chips the rules do not cover keep every write; say so rather than let a
    // file that could not shrink look like one that had nothing to gain.
    let skipped = file.unoptimised_chips();
    if !skipped.is_empty() {
        println!(
            "  (no redundancy rules for {} -- its writes were all kept)",
            skipped.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::io::write_song;
    use dro_core::vgm::io::synthesise_header;
    use dro_core::{DroDataV1, OplType, Song, VgmData, VgmMeta};

    /// A distinct temp path per test, namespaced by the process so parallel runs
    /// of the binary cannot collide.
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("drotrim-opt-{}-{name}", std::process::id()))
    }

    /// A VGM with a redundant write between two delays.
    fn redundant_vgm_bytes() -> Vec<u8> {
        let stream = vec![
            0x5A, 0x20, 0x01, // write
            0x61, 0x64, 0x00, // wait 100
            0x5A, 0x20, 0x01, // redundant write
            0x61, 0xC8, 0x00, // wait 200
            0x5A, 0x21, 0x02, // write
        ];
        let song = Song::vgm(
            "x.vgm".to_owned(),
            0x151,
            VgmData::new(stream).unwrap(),
            OplType::Opl2,
            VgmMeta::new(synthesise_header()),
        );
        write_song(&song).unwrap()
    }

    #[test]
    fn optimises_a_vgm_to_a_smaller_still_valid_file() {
        let input = temp_path("in.vgm");
        let output = temp_path("out.vgm");
        std::fs::write(&input, redundant_vgm_bytes()).unwrap();
        let original_len = std::fs::metadata(&input).unwrap().len();

        run(&Args {
            input: input.clone(),
            output: Some(output.clone()),
        })
        .unwrap();

        let optimised = std::fs::read(&output).unwrap();
        assert!(
            (optimised.len() as u64) < original_len,
            "the optimised file should be smaller"
        );
        // Still a valid VGM, and now optimal (a second pass finds nothing).
        let song = dro_core::io::read_song("out.vgm", &optimised).unwrap();
        assert!(dro_core::optimize::optimize(&song).is_none());

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[test]
    fn gzips_when_the_output_is_a_vgz() {
        let input = temp_path("gz-in.vgm");
        let output = temp_path("gz-out.vgz");
        std::fs::write(&input, redundant_vgm_bytes()).unwrap();

        run(&Args {
            input: input.clone(),
            output: Some(output.clone()),
        })
        .unwrap();

        let written = std::fs::read(&output).unwrap();
        assert!(
            dro_core::vgm::io::is_gzipped(&written),
            "a .vgz output should be gzipped"
        );
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[test]
    fn refuses_a_dro() {
        let input = temp_path("in.dro");
        let dro = Song::dro_v1(
            "x.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01]).unwrap(),
            0,
            OplType::Opl2,
        );
        std::fs::write(&input, write_song(&dro).unwrap()).unwrap();

        let error = run(&Args {
            input: input.clone(),
            output: None,
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("only applies to VGM"),
            "got {error}"
        );
        std::fs::remove_file(&input).ok();
    }
}
