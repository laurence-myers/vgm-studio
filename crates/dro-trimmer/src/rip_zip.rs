//! Building the release zip: optimise the PNGs, optionally gzip the songs, and
//! pack it all flat. Pure bytes-in/bytes-out, like [`crate::split`] -- the
//! service layer owns the thread and the disk.
//!
//! The native-only crates (`zip`, `oxipng`) live here rather than in `dro-ui`'s
//! wasm-clean `run_task`, which is why the rip export goes through its own
//! service port instead of `TaskService`.

use std::io::{Cursor, Write as _};

use anyhow::Context as _;
use dro_ui::{RipEntry, RipEntryKind};
use flate2::Compression;
use flate2::write::GzEncoder;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

/// The finished archive, plus human-readable notes about what the job did.
#[derive(Debug)]
pub struct RipZipOutput {
    pub bytes: Vec<u8>,
    pub log: Vec<String>,
}

/// Builds the release zip from `entries` (already in final order). Returns
/// `Ok(None)` if `is_cancelled` fired partway through.
///
/// A PNG that oxipng cannot process is kept verbatim and logged, never fatal: a
/// bad screenshot must not sink the whole export.
pub fn build_rip_zip(
    entries: &[RipEntry],
    gzip_vgms: bool,
    is_cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Option<RipZipOutput>> {
    let mut log: Vec<String> = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in entries {
        if is_cancelled() {
            return Ok(None);
        }
        let (name, bytes) = process_entry(entry, gzip_vgms, &mut log)?;
        zip.start_file(name.as_str(), options)
            .with_context(|| format!("adding {name} to the zip"))?;
        zip.write_all(&bytes)
            .with_context(|| format!("writing {name} into the zip"))?;
    }

    if is_cancelled() {
        return Ok(None);
    }
    let cursor = zip.finish().context("finalising the zip")?;
    Ok(Some(RipZipOutput {
        bytes: cursor.into_inner(),
        log,
    }))
}

/// The final `(name, bytes)` for one entry, applying gzip/oxipng as its kind and
/// the job settings dictate.
fn process_entry(
    entry: &RipEntry,
    gzip_vgms: bool,
    log: &mut Vec<String>,
) -> anyhow::Result<(String, Vec<u8>)> {
    match entry.kind {
        RipEntryKind::Song if gzip_vgms && has_extension(&entry.name, "vgm") => {
            let name = to_vgz_name(&entry.name);
            if dro_core::vgm::io::is_gzipped(&entry.bytes) {
                // Already compressed despite the .vgm name: just rename it.
                Ok((name, entry.bytes.clone()))
            } else {
                let compressed = gzip(&entry.bytes).context("gzipping a song")?;
                log.push(format!(
                    "{} -> {name} ({} -> {} bytes)",
                    entry.name,
                    entry.bytes.len(),
                    compressed.len()
                ));
                Ok((name, compressed))
            }
        }
        RipEntryKind::Image => match oxipng::optimize_from_memory(&entry.bytes, &png_options()) {
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
        RipEntryKind::Song | RipEntryKind::Doc => Ok((entry.name.clone(), entry.bytes.clone())),
    }
}

fn png_options() -> oxipng::Options {
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

    fn song(name: &str, bytes: &[u8]) -> RipEntry {
        RipEntry {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
            kind: RipEntryKind::Song,
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

    #[test]
    fn gzips_songs_and_packs_everything_flat() {
        let entries = [
            song("01 First.vgm", b"raw vgm one"),
            song("02 Second.vgm", b"raw vgm two"),
            RipEntry {
                name: "Game.txt".to_owned(),
                bytes: b"description".to_vec(),
                kind: RipEntryKind::Doc,
            },
            RipEntry {
                name: "Game.png".to_owned(),
                bytes: PNG.to_vec(),
                kind: RipEntryKind::Image,
            },
        ];
        let output = build_rip_zip(&entries, true, &never()).unwrap().unwrap();
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
        let output = build_rip_zip(&entries, false, &never()).unwrap().unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 First.vgm");
        assert_eq!(files[0].1, b"raw");
    }

    #[test]
    fn an_already_gzipped_vgm_is_renamed_but_not_recompressed() {
        let gzipped = gzip(b"already compressed").unwrap();
        let entries = [song("01 First.vgm", &gzipped)];
        let output = build_rip_zip(&entries, true, &never()).unwrap().unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 First.vgz");
        assert_eq!(files[0].1, gzipped, "the bytes are untouched");
    }

    #[test]
    fn a_corrupt_png_is_kept_verbatim_and_logged() {
        let entries = [RipEntry {
            name: "Bad.png".to_owned(),
            bytes: b"not really a png".to_vec(),
            kind: RipEntryKind::Image,
        }];
        let output = build_rip_zip(&entries, true, &never()).unwrap().unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, b"not really a png");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    #[test]
    fn cancellation_yields_none() {
        let entries = [song("01 First.vgm", b"raw")];
        let output = build_rip_zip(&entries, true, &|| true).unwrap();
        assert!(output.is_none());
    }
}
