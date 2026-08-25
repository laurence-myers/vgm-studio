//! `vgmstudio volume`: batch-apply a volume boost to VGM files.
//!
//! Writes the header's Volume Modifier byte (offset 0x7C, a v1.60 field) across
//! one or more files. The boost is given either as a linear multiplier
//! (`--boost 2`, snapped to the modifier ladder) or as the raw byte
//! (`--modifier 0x20`). In place by default, like `optimize`; `--suffix` writes
//! copies beside the originals instead.
//!
//! A DRO is rejected (it has no VGM header), and a header too short to hold the
//! field -- one predating v1.60 -- is reported and skipped rather than silently
//! doing nothing. The batch continues past a failing file; it only errors out if
//! every file failed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The player-reachable multiplier range the modifier ladder spans (`0.25x` at
/// `0xC1` up to `64x` at `0xC0`); a `--boost` outside it cannot be encoded.
const MIN_BOOST: f32 = 0.25;
const MAX_BOOST: f32 = 64.0;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The VGM or VGZ files to adjust. Overwritten in place unless `--suffix` is
    /// given. (Windows shells do not expand `*.vgm`; pass a list, or expand it in
    /// the shell.)
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// The boost as a linear multiplier (e.g. `2` for +6 dB), snapped to the
    /// nearest value the modifier byte can express. Mutually exclusive with
    /// `--modifier`.
    #[arg(short = 'b', long, conflicts_with = "modifier")]
    pub boost: Option<f32>,
    /// The raw Volume Modifier byte to write, decimal or `0xNN` (e.g. `0x20`
    /// = 2x, `0x00` = unity). Mutually exclusive with `--boost`.
    #[arg(long, value_parser = parse_byte)]
    pub modifier: Option<u8>,
    /// Insert this suffix before each file's extension and write a copy there
    /// (e.g. `--suffix _vol` writes `song_vol.vgm`), leaving the originals
    /// untouched. Without it, files are overwritten in place.
    #[arg(long)]
    pub suffix: Option<String>,
}

/// Parses a `--modifier` byte: decimal, or hex with a `0x`/`0X` prefix.
fn parse_byte(text: &str) -> std::result::Result<u8, String> {
    let trimmed = text.trim();
    let parsed = match trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Some(hex) => u8::from_str_radix(hex, 16),
        None => trimmed.parse::<u8>(),
    };
    parsed.map_err(|_| format!("expected a byte 0-255 (decimal or 0xNN), got `{text}`"))
}

/// Applies the boost to every input.
///
/// # Errors
/// If neither (or both) of `--boost`/`--modifier` resolve to a value, or if
/// *every* input failed. A single file's failure is reported and skipped.
pub fn run(args: &Args) -> Result<()> {
    let modifier = resolve_modifier(args)?;
    let mut failures = 0usize;
    for input in &args.inputs {
        match apply_one(input, modifier, args.suffix.as_deref()) {
            Ok(output) => {
                let factor = vgms_core::volume_modifier_factor(modifier);
                println!(
                    "{} -> {}: volume modifier {modifier:#04X} ({factor:.2}x)",
                    input.display(),
                    output.display()
                );
            }
            Err(error) => {
                eprintln!("{}: {error:#}", input.display());
                failures += 1;
            }
        }
    }
    if failures > 0 && failures == args.inputs.len() {
        bail!("no files could be adjusted");
    }
    Ok(())
}

/// The modifier byte to write: `--modifier` verbatim, else `--boost` snapped onto
/// the ladder. Exactly one of the two must be given.
fn resolve_modifier(args: &Args) -> Result<u8> {
    match (args.modifier, args.boost) {
        (Some(byte), _) => Ok(byte),
        (None, Some(boost)) => {
            if !(MIN_BOOST..=MAX_BOOST).contains(&boost) {
                bail!("boost {boost} is outside the reachable {MIN_BOOST}..={MAX_BOOST} range");
            }
            Ok(vgms_core::nearest_volume_modifier(boost))
        }
        (None, None) => bail!("give a --boost multiplier or a --modifier byte"),
    }
}

/// Reads one VGM, writes the modifier, and serialises it to the chosen output
/// (gzipping when that path is a `.vgz`). Returns the path written.
fn apply_one(input: &Path, modifier: u8, suffix: Option<&str>) -> Result<PathBuf> {
    let raw = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let name = input
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input.vgm");
    let mut file = vgms_core::vgm::file::read(name, &raw).map_err(|error| {
        anyhow::anyhow!(
            "volume only applies to VGM files; {} could not be read as one ({error})",
            input.display()
        )
    })?;
    if !file.header.set_volume_modifier(modifier) {
        bail!("header predates VGM v1.60 (no volume-modifier slot); left unchanged");
    }
    let output = output_path(input, suffix);
    let bytes = if is_vgz(&output) {
        file.name = name.to_owned();
        vgms_core::vgm::file::write_gzipped(&file).context("gzipping the song")?
    } else {
        vgms_core::vgm::file::write(&file).context("serialising the song")?
    };
    std::fs::write(&output, &bytes).with_context(|| format!("writing {}", output.display()))?;
    Ok(output)
}

fn is_vgz(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vgz"))
}

/// In place (no suffix), or a sibling with `suffix` inserted before the
/// extension: `song.vgm` + `_vol` -> `song_vol.vgm`.
fn output_path(input: &Path, suffix: Option<&str>) -> PathBuf {
    let Some(suffix) = suffix else {
        return input.to_path_buf();
    };
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let file_name = match input.extension().and_then(|s| s.to_str()) {
        Some(extension) => format!("{stem}{suffix}.{extension}"),
        None => format!("{stem}{suffix}"),
    };
    input.with_file_name(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgms_core::io::write_song;
    use vgms_core::vgm::io::synthesise_header;
    use vgms_core::{DroDataV1, DroSong, OplType};

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vgmstudio-vol-{}-{name}", std::process::id()))
    }

    fn args(inputs: Vec<PathBuf>, boost: Option<f32>, modifier: Option<u8>) -> Args {
        Args {
            inputs,
            boost,
            modifier,
            suffix: None,
        }
    }

    /// A minimal but valid wholly-OPL2 VGM (a synthesised 0x80-byte header that
    /// covers the 0x7C volume-modifier slot, one write and an end marker).
    fn vgm_bytes() -> Vec<u8> {
        let mut bytes = synthesise_header();
        bytes[0x50..0x54].copy_from_slice(&3_579_545u32.to_le_bytes()); // YM3812 clock
        bytes.extend_from_slice(&[0x5A, 0x20, 0x01, 0x66]); // write, end marker
        let eof = (bytes.len() - 0x04) as u32;
        bytes[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
        let file = vgms_core::vgm::file::read("x.vgm", &bytes).unwrap();
        vgms_core::vgm::file::write(&file).unwrap()
    }

    #[test]
    fn a_boost_writes_the_snapped_modifier_in_place() {
        let input = temp_path("boost.vgm");
        std::fs::write(&input, vgm_bytes()).unwrap();

        run(&args(vec![input.clone()], Some(2.0), None)).unwrap();

        let written = std::fs::read(&input).unwrap();
        let file = vgms_core::vgm::file::read("x.vgm", &written).unwrap();
        // 2.0x snaps to the +6 dB rung, byte 0x20.
        assert_eq!(file.header.volume_modifier(), 0x20);
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn a_raw_modifier_is_written_verbatim() {
        let input = temp_path("mod.vgm");
        std::fs::write(&input, vgm_bytes()).unwrap();

        run(&args(vec![input.clone()], None, Some(0x40))).unwrap();

        let file = vgms_core::vgm::file::read("x.vgm", &std::fs::read(&input).unwrap()).unwrap();
        assert_eq!(file.header.volume_modifier(), 0x40);
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn a_suffix_writes_a_copy_and_leaves_the_original() {
        let input = temp_path("suffix.vgm");
        std::fs::write(&input, vgm_bytes()).unwrap();

        let with_suffix = Args {
            suffix: Some("_vol".to_owned()),
            ..args(vec![input.clone()], None, Some(0x20))
        };
        run(&with_suffix).unwrap();

        // The original is untouched (unity), the copy carries the modifier.
        let original =
            vgms_core::vgm::file::read("x.vgm", &std::fs::read(&input).unwrap()).unwrap();
        assert_eq!(original.header.volume_modifier(), 0x00);
        let copy = output_path(&input, Some("_vol"));
        let bumped = vgms_core::vgm::file::read("x.vgm", &std::fs::read(&copy).unwrap()).unwrap();
        assert_eq!(bumped.header.volume_modifier(), 0x20);
        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&copy).ok();
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

        // The only input fails, so the batch errors out.
        let error = run(&args(vec![input.clone()], None, Some(0x20))).unwrap_err();
        assert!(error.to_string().contains("no files"), "got {error}");
        std::fs::remove_file(&input).ok();
    }

    #[test]
    fn a_boost_out_of_range_is_refused() {
        let error = resolve_modifier(&args(Vec::new(), Some(1000.0), None)).unwrap_err();
        assert!(error.to_string().contains("outside"), "got {error}");
    }

    #[test]
    fn neither_boost_nor_modifier_is_refused() {
        let error = resolve_modifier(&args(Vec::new(), None, None)).unwrap_err();
        assert!(error.to_string().contains("--boost"), "got {error}");
    }

    #[test]
    fn output_path_inserts_the_suffix_before_the_extension() {
        assert_eq!(
            output_path(Path::new("dir/song.vgm"), Some("_vol")),
            PathBuf::from("dir/song_vol.vgm")
        );
        assert_eq!(
            output_path(Path::new("song.vgz"), Some("-x")),
            PathBuf::from("song-x.vgz")
        );
        assert_eq!(
            output_path(Path::new("song.vgm"), None),
            PathBuf::from("song.vgm"),
            "no suffix overwrites in place"
        );
    }

    #[test]
    fn parse_byte_accepts_hex_and_decimal() {
        assert_eq!(parse_byte("32"), Ok(32));
        assert_eq!(parse_byte("0x20"), Ok(0x20));
        assert_eq!(parse_byte("0XFF"), Ok(0xFF));
        assert!(parse_byte("256").is_err());
        assert!(parse_byte("zz").is_err());
    }
}
