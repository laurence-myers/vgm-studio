//! Building the release zip: optimise the PNGs, optionally gzip the songs, and
//! pack it all flat. Pure bytes-in/bytes-out, like [`dro_synth::split`] -- the
//! service layer owns the thread and the disk.
//!
//! The native-only crates (`zip`, `oxipng`) live here rather than in `dro-ui`'s
//! wasm-clean `run_task`, which is why the pack export goes through its own
//! service port instead of `TaskService`.

use std::io::{Cursor, Write as _};

use anyhow::Context as _;
use dro_ui::{PackEntry, PackEntryKind};
use flate2::Compression;
use flate2::write::GzEncoder;
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
/// A song is optimised first (stripping redundant OPL writes, `vgm_cmp`-style),
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
                if dro_core::vgm::io::is_gzipped(&bytes) {
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

/// Optimises one song's bytes when it is a parseable VGM that shrinks, logging the
/// saving. A DRO, an already-optimal VGM, or any read/write failure passes through
/// unchanged and never fails the export -- the same never-fatal posture as the PNG
/// path. The bytes stay in the song's own container (a `.vgm` stays plain, so the
/// gzip step can still compress it; a `.vgz` stays gzipped).
fn optimize_song(name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
    let mut song = match dro_core::io::read_song(name, bytes) {
        Ok(song) => song,
        Err(error) => {
            // The optimiser folds an OPL register file, so it can only reason
            // about OPL writes. A VGM for other chips is a perfectly good file
            // that simply has nothing here to strip -- say that, rather than
            // implying it is broken.
            match dro_core::vgm::file::read(name, bytes) {
                Ok(file) => log.push(format!(
                    "{name}: kept as-is ({} is not optimised yet)",
                    file.chip_list()
                )),
                Err(_) => log.push(format!("{name}: kept as-is (could not read: {error})")),
            }
            return bytes.to_vec();
        }
    };
    let Some(outcome) = dro_core::optimize::optimize(&song) else {
        return bytes.to_vec(); // a DRO, or already optimal
    };
    outcome.install(&mut song);
    match dro_core::io::write_song(&song) {
        Ok(optimised) => {
            log.push(format!(
                "{name}: {} -> {} bytes (optimized)",
                bytes.len(),
                optimised.len()
            ));
            optimised
        }
        Err(error) => {
            log.push(format!("{name}: kept as-is (could not write: {error})"));
            bytes.to_vec()
        }
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
        use dro_core::vgm::io::synthesise_header;
        use dro_core::{OplType, Song, VgmData, VgmMeta};
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
        dro_core::io::write_song(&song).unwrap()
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
            dro_core::io::read_song("01 Song.vgm", &files[0].1).is_ok(),
            "the optimised bytes are still a valid VGM"
        );
        assert!(
            output.log.iter().any(|line| line.contains("(optimized)")),
            "log: {:?}",
            output.log
        );
    }

    /// A VGM for chips the optimiser cannot reason about ships exactly as it
    /// arrived. The optimiser folds an OPL register file, so widening it to
    /// other chips would corrupt them -- the export must leave them alone, and
    /// say so honestly rather than reporting a read failure.
    #[test]
    fn a_foreign_vgm_ships_verbatim_with_the_optimiser_on() {
        let mut original = vec![0u8; 0x100];
        original[..4].copy_from_slice(b"Vgm ");
        original[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
        original[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
        // A YM2612 at 0x2C, and a body the OPL command table cannot size.
        original[0x2C..0x30].copy_from_slice(&7_670_454u32.to_le_bytes());
        original.extend_from_slice(&[0x52, 0x28, 0xF0, 0x80, 0x66]);
        let eof = original.len();
        original[0x04..0x08].copy_from_slice(&((eof - 4) as u32).to_le_bytes());

        let entries = [song("01 Mega Drive.vgm", &original)];
        let output = build_pack_zip(&entries, false, true, &never())
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, original, "byte for byte");
        assert!(
            output
                .log
                .iter()
                .any(|line| line.contains("YM2612 is not optimised yet")),
            "log: {:?}",
            output.log
        );
        assert!(
            !output
                .log
                .iter()
                .any(|line| line.contains("could not read")),
            "a foreign VGM is not unreadable: {:?}",
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
        assert!(output.log.iter().any(|line| line.contains("(optimized)")));
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
