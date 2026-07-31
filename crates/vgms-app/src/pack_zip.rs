//! Building the release zip: optimise the PNGs, optionally gzip the songs, and
//! pack it all flat. Pure bytes-in/bytes-out, like [`vgms_synth::split`] -- the
//! service layer owns the thread and the disk.
//!
//! The native-only crates (`zip`, `oxipng`) live here rather than in `vgms-ui`'s
//! wasm-clean `run_task`, which is why the pack export goes through its own
//! service port instead of `TaskService`.

use std::io::{Cursor, Write as _};

use anyhow::Context as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use vgms_ui::{PackEntry, PackEntryKind};
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

/// The finished archive, plus human-readable notes about what the job did.
#[derive(Debug)]
pub struct PackZipOutput {
    pub bytes: Vec<u8>,
    pub log: Vec<String>,
}

/// Builds the release zip from `entries` (already in final order). Returns
/// `Ok(None)` if `is_cancelled` fired partway through.
///
/// A PNG that oxipng cannot process, or a song the optimiser cannot read, is kept
/// verbatim and logged, never fatal: one bad file must not sink the whole export.
pub fn build_pack_zip(
    entries: &[PackEntry],
    gzip_vgms: bool,
    optimize_vgms: bool,
    is_cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Option<PackZipOutput>> {
    let mut log: Vec<String> = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in entries {
        if is_cancelled() {
            return Ok(None);
        }
        let (name, bytes) = process_entry(entry, gzip_vgms, optimize_vgms, &mut log)?;
        zip.start_file(name.as_str(), options)
            .with_context(|| format!("adding {name} to the zip"))?;
        zip.write_all(&bytes)
            .with_context(|| format!("writing {name} into the zip"))?;
    }

    if is_cancelled() {
        return Ok(None);
    }
    let cursor = zip.finish().context("finalising the zip")?;
    Ok(Some(PackZipOutput {
        bytes: cursor.into_inner(),
        log,
    }))
}

/// The final `(name, bytes)` for one entry, applying optimise/gzip/oxipng as its
/// kind and the job settings dictate.
///
/// A song is optimised first (stripping redundant writes, `vgm_cmp`-style),
/// then gzipped -- so the log shows the two savings on their own lines.
fn process_entry(
    entry: &PackEntry,
    gzip_vgms: bool,
    optimize_vgms: bool,
    log: &mut Vec<String>,
) -> anyhow::Result<(String, Vec<u8>)> {
    match entry.kind {
        PackEntryKind::Song => {
            let bytes = if optimize_vgms {
                optimize_song(&entry.name, &entry.bytes, log)
            } else {
                entry.bytes.clone()
            };
            if gzip_vgms && has_extension(&entry.name, "vgm") {
                let name = to_vgz_name(&entry.name);
                if vgms_core::vgm::io::is_gzipped(&bytes) {
                    // Already compressed despite the .vgm name: just rename it.
                    Ok((name, bytes))
                } else {
                    let compressed = gzip(&bytes).context("gzipping a song")?;
                    log.push(format!(
                        "{} -> {name} ({} -> {} bytes)",
                        entry.name,
                        bytes.len(),
                        compressed.len()
                    ));
                    Ok((name, compressed))
                }
            } else {
                Ok((entry.name.clone(), bytes))
            }
        }
        PackEntryKind::Image => match oxipng::optimize_from_memory(&entry.bytes, &png_options()) {
            Ok(optimised) => {
                log.push(format!(
                    "{}: {} -> {} bytes (oxipng)",
                    entry.name,
                    entry.bytes.len(),
                    optimised.len()
                ));
                Ok((entry.name.clone(), optimised))
            }
            Err(error) => {
                log.push(format!(
                    "{}: kept as-is (oxipng failed: {error})",
                    entry.name
                ));
                Ok((entry.name.clone(), entry.bytes.clone()))
            }
        },
        PackEntryKind::Doc => Ok((entry.name.clone(), entry.bytes.clone())),
    }
}

/// Optimises one song's bytes when it is a VGM that shrinks, logging the saving.
///
/// A DRO, an already-optimal VGM, or any failure passes through unchanged and
/// never fails the export -- the same never-fatal posture as the PNG path. The
/// result is plain bytes, so the gzip step can still compress it.
///
/// Runs every chip through the vgmtools optimisers plus this app's own pass. Each
/// stage's outcome (shrank, held back, or failed) goes in the log, since one byte
/// count cannot tell them apart, and chips left untouched are named so a rip that
/// comes back byte for byte does not look unreadable.
fn optimize_song(name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
    let Ok(file) = vgms_core::vgm::file::read(name, bytes) else {
        // A DRO, or something unreadable. Either way it passes through.
        log.push(format!("{name}: kept as-is (not a readable VGM)"));
        return bytes.to_vec();
    };
    // The optimisers take plain bytes, and a pack entry may already be a `.vgz`.
    let Ok(plain) = vgms_core::vgm::file::write(&file) else {
        log.push(format!("{name}: kept as-is (could not be prepared)"));
        return bytes.to_vec();
    };

    let result = vgms_vgmtools::optimize_vgm(&plain, vgms_vgmtools::Options::default());

    if result.changed() {
        log.push(format!(
            "{name}: {} -> {} bytes (optimized, {} saved)",
            bytes.len(),
            result.bytes.len(),
            result.saved()
        ));
    }
    // Only the stages worth a line: "nothing to gain" is the common case and
    // would bury the rest.
    for stage in &result.stages {
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

    let untouched: Vec<&str> = file
        .header
        .chips()
        .iter()
        .filter(|chip| vgms_vgmtools::passthrough_chips().contains(&chip.kind))
        .map(|chip| chip.kind.name())
        .collect();
    if !untouched.is_empty() {
        log.push(format!(
            "{name}: {} not optimised yet -- their writes were all kept",
            untouched.join(", ")
        ));
    }

    if result.changed() {
        result.bytes
    } else {
        bytes.to_vec()
    }
}

/// The oxipng settings shared by the export job and the explicit
/// optimise-in-place action.
pub(crate) fn png_options() -> oxipng::Options {
    let mut options = oxipng::Options::from_preset(2);
    // Drop non-critical chunks that do not affect rendering (comments, etc.).
    options.strip = oxipng::StripChunks::Safe;
    options
}

fn gzip(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    Ok(encoder.finish()?)
}

fn has_extension(name: &str, extension: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case(extension))
}

fn to_vgz_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("vgm") => format!("{stem}.vgz"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    use flate2::read::GzDecoder;
    use zip::ZipArchive;

    const PNG: &[u8] = include_bytes!("../../../tests/screenshot.png");

    fn song(name: &str, bytes: &[u8]) -> PackEntry {
        PackEntry {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
            kind: PackEntryKind::Song,
        }
    }

    /// Reads a built archive back into `(name, bytes)` pairs, in order.
    fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        (0..archive.len())
            .map(|i| {
                let mut file = archive.by_index(i).unwrap();
                let name = file.name().to_owned();
                let mut data = Vec::new();
                file.read_to_end(&mut data).unwrap();
                (name, data)
            })
            .collect()
    }

    fn never() -> impl Fn() -> bool {
        || false
    }

    /// A real VGM file (header + stream) carrying a redundant write between two
    /// delays, so the optimiser has something to strip and merge.
    fn optimizable_vgm_bytes() -> Vec<u8> {
        use vgms_core::vgm::io::synthesise_header;
        use vgms_core::{OplType, Song, VgmData, VgmMeta};
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
        vgms_core::io::write_song(&song).unwrap()
    }

    #[test]
    fn an_optimizable_vgm_is_shrunk_and_logged() {
        let original = optimizable_vgm_bytes();
        let entries = [song("01 Song.vgm", &original)];
        // Optimise on, gzip off: the file shrinks but keeps its .vgm name.
        let output = build_pack_zip(&entries, false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 Song.vgm");
        assert!(
            files[0].1.len() < original.len(),
            "optimising should shrink the file"
        );
        assert!(
            vgms_core::io::read_song("01 Song.vgm", &files[0].1).is_ok(),
            "the optimised bytes are still a valid VGM"
        );
        assert!(
            output.log.iter().any(|line| line.contains("(optimized,")),
            "log: {:?}",
            output.log
        );
    }

    /// A non-OPL VGM, with `chip` clocked at `at` and `stream` for a body.
    fn non_opl_vgm(at: usize, stream: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        bytes[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
        bytes[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
        bytes[at..at + 4].copy_from_slice(&7_670_454u32.to_le_bytes());
        bytes.extend_from_slice(stream);
        let eof = bytes.len();
        bytes[0x04..0x08].copy_from_slice(&((eof - 4) as u32).to_le_bytes());
        bytes
    }

    /// A Mega Drive rip with a repeated register write comes out smaller,
    /// through the chip-agnostic pass.
    #[test]
    fn a_ym2612_vgm_is_optimised_like_any_other() {
        let original = non_opl_vgm(
            0x2C,
            &[
                0x52, 0x22, 0x08, // LFO
                0x62, //
                0x52, 0x22, 0x08, // the same value again -- droppable
                0x62, //
                0x66,
            ],
        );
        let output = build_pack_zip(&[song("01 MD.vgm", &original)], false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert!(files[0].1.len() < original.len(), "it shrank");
        assert!(
            output.log.iter().any(|line| line.contains("(optimized")),
            "log: {:?}",
            output.log
        );
        // Still a valid VGM, and still the same chip.
        let reread = vgms_core::vgm::file::read("01 MD.vgm", &files[0].1).unwrap();
        assert_eq!(reread.chip_list(), "YM2612");
    }

    /// `vgm_cmp` has a table for the YMZ280B: a chip the app's own built-in pass
    /// cannot touch is still optimised through the bound tools.
    #[test]
    fn a_chip_the_built_in_pass_cannot_touch_is_optimised_by_the_tools() {
        let original = non_opl_vgm(0x68, &[0x5D, 0x01, 0x40, 0x5D, 0x01, 0x40, 0x66]);
        let output = build_pack_zip(&[song("01 Arcade.vgm", &original)], false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert!(
            files[0].1.len() < original.len(),
            "the YMZ280B's repeated write should now be dropped"
        );
        let reread = vgms_core::vgm::file::read("01 Arcade.vgm", &files[0].1).unwrap();
        assert_eq!(reread.chip_list(), "YMZ280B");
    }

    /// A chip with no redundancy rules keeps every write. The export says which
    /// chip it left alone rather than implying the file was unreadable -- being
    /// smaller is not worth being silently wrong.
    #[test]
    fn a_chip_without_rules_ships_verbatim_and_says_so() {
        // A K053260: `vgm_cmp` has a handler for it, but it is commented out
        // (`chip_cmp.c:10` still lists it as a TODO), so every write is kept.
        let original = non_opl_vgm(0xAC, &[0xBA, 0x01, 0x40, 0xBA, 0x01, 0x40, 0x66]);
        let output = build_pack_zip(&[song("01 Arcade.vgm", &original)], false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, original, "byte for byte");
        assert!(
            output
                .log
                .iter()
                .any(|line| line.contains("K053260 not optimised yet")),
            "log: {:?}",
            output.log
        );
        assert!(
            !output
                .log
                .iter()
                .any(|line| line.contains("could not read")),
            "and it is not unreadable: {:?}",
            output.log
        );
    }

    #[test]
    fn an_already_optimal_vgm_passes_through_unchanged() {
        const CLEAN: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
        let entries = [song("01 Clean.vgm", CLEAN)];
        let output = build_pack_zip(&entries, false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(
            files[0].1, CLEAN,
            "an optimal VGM is untouched, byte for byte"
        );
        assert!(
            !output.log.iter().any(|line| line.contains("(optimized)")),
            "nothing to report: {:?}",
            output.log
        );
    }

    #[test]
    fn optimize_off_leaves_the_song_verbatim() {
        let original = optimizable_vgm_bytes();
        let entries = [song("01 Song.vgm", &original)];
        let output = build_pack_zip(&entries, false, false, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, original, "optimise off means verbatim bytes");
    }

    #[test]
    fn an_unreadable_song_is_kept_verbatim_and_logged() {
        let entries = [song("01 Bad.vgm", b"not a vgm at all")];
        let output = build_pack_zip(&entries, false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(
            files[0].1, b"not a vgm at all",
            "an unreadable song passes through, never fatal"
        );
        assert!(
            output.log.iter().any(|line| line.contains("kept as-is")),
            "log: {:?}",
            output.log
        );
    }

    #[test]
    fn optimizing_then_gzipping_shrinks_and_renames() {
        let original = optimizable_vgm_bytes();
        let entries = [song("01 Song.vgm", &original)];
        let output = build_pack_zip(&entries, true, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 Song.vgz", "gzip still renames the entry");
        let mut decoded = Vec::new();
        GzDecoder::new(files[0].1.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert!(
            decoded.len() < original.len(),
            "the .vgz gunzips to the optimised VGM"
        );
        // Both steps are reported, on their own lines.
        assert!(output.log.iter().any(|line| line.contains("(optimized,")));
        assert!(
            output
                .log
                .iter()
                .any(|line| line.contains("-> 01 Song.vgz"))
        );
    }

    #[test]
    fn gzips_songs_and_packs_everything_flat() {
        let entries = [
            song("01 First.vgm", b"raw vgm one"),
            song("02 Second.vgm", b"raw vgm two"),
            PackEntry {
                name: "Game.txt".to_owned(),
                bytes: b"description".to_vec(),
                kind: PackEntryKind::Doc,
            },
            PackEntry {
                name: "Game.png".to_owned(),
                bytes: PNG.to_vec(),
                kind: PackEntryKind::Image,
            },
        ];
        let output = build_pack_zip(&entries, true, false, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);

        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["01 First.vgz", "02 Second.vgz", "Game.txt", "Game.png"]
        );

        // The .vgz entry gunzips back to the original song bytes.
        let mut decoded = Vec::new();
        GzDecoder::new(files[0].1.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, b"raw vgm one");

        // The doc is verbatim; the PNG is still a valid PNG and no larger.
        assert_eq!(files[2].1, b"description");
        assert_eq!(&files[3].1[..8], b"\x89PNG\r\n\x1a\n");
        assert!(files[3].1.len() <= PNG.len());
    }

    #[test]
    fn leaves_songs_alone_when_not_gzipping() {
        let entries = [song("01 First.vgm", b"raw")];
        let output = build_pack_zip(&entries, false, false, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 First.vgm");
        assert_eq!(files[0].1, b"raw");
    }

    #[test]
    fn an_already_gzipped_vgm_is_renamed_but_not_recompressed() {
        let gzipped = gzip(b"already compressed").unwrap();
        let entries = [song("01 First.vgm", &gzipped)];
        let output = build_pack_zip(&entries, true, false, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 First.vgz");
        assert_eq!(files[0].1, gzipped, "the bytes are untouched");
    }

    #[test]
    fn a_corrupt_png_is_kept_verbatim_and_logged() {
        let entries = [PackEntry {
            name: "Bad.png".to_owned(),
            bytes: b"not really a png".to_vec(),
            kind: PackEntryKind::Image,
        }];
        let output = build_pack_zip(&entries, true, false, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, b"not really a png");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    #[test]
    fn cancellation_yields_none() {
        let entries = [song("01 First.vgm", b"raw")];
        let output = build_pack_zip(&entries, true, false, &|| true).unwrap();
        assert!(output.is_none());
    }
}
