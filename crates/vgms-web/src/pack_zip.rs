// SPDX-License-Identifier: GPL-2.0-or-later
//! The web pack-export zip builder: the wasm-portable half of vgms-app's
//! `pack_zip`. Optionally optimise the songs, optionally gzip them, and pack it
//! all flat -- pure bytes-in/bytes-out, so the Worker (and a native test) can
//! drive it.
//!
//! Songs are optimised through a caller-supplied [`SongOptimizer`], so this
//! builder stays target-independent and its round-trip proofs run in an ordinary
//! `cargo test`. The web pack Worker passes the **full vgmtools pipeline** over
//! the tool `.wasm` modules (`crate::optimize_tools::WebPipelineOptimizer`,
//! ow-7); the native tests pass [`BuiltInOptimizer`], `vgms_core`'s own pass.
//!
//! One difference from the native builder stays by design: **PNGs** keep their
//! original bytes with a log line -- oxipng is C + rayon and does not come to
//! the browser.
//!
//! `zip` + `flate2` build for both targets, so this module is portable and its
//! round-trip proofs run in the ordinary `cargo test`.

use std::io::{Cursor, Write};

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

/// Optimises one song's bytes in place of the pack, appending human-readable
/// notes to `log` and returning the result (the original bytes, never fatal, on
/// anything it cannot improve or read).
///
/// The web pack Worker supplies the full vgmtools pipeline over the tool `.wasm`
/// modules; the native round-trip tests supply [`BuiltInOptimizer`].
pub trait SongOptimizer {
    /// Optimise `bytes` (named `name` for log lines), returning the new bytes.
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8>;
}

/// `vgms_core`'s own built-in pass -- the optimiser with no external tools, used
/// by the native round-trip tests and as the honest fallback.
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltInOptimizer;

impl SongOptimizer for BuiltInOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
        optimize_song(name, bytes, log)
    }
}

/// Builds the release zip from `entries` (already in final order). Songs are run
/// through `optimize` when it is `Some`, kept verbatim when it is `None`.
/// `on_progress` fires before each entry -- the pack Worker posts a heartbeat
/// from it so the page's inactivity watchdog can tell a slow job from a hung
/// one. Returns `Ok(None)` if `is_cancelled` fired partway through, `Err` only
/// on a genuine zip write failure. One bad song or PNG is kept verbatim and
/// logged, never fatal.
pub fn build_pack_zip(
    entries: &[PackEntry],
    gzip_vgms: bool,
    optimize: Option<&dyn SongOptimizer>,
    is_cancelled: &dyn Fn() -> bool,
    on_progress: &dyn Fn(),
) -> Result<Option<PackZipOutput>, String> {
    let mut log: Vec<String> = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for entry in entries {
        if is_cancelled() {
            return Ok(None);
        }
        on_progress();
        let (name, bytes) = process_entry(entry, gzip_vgms, optimize, &mut log);
        zip.start_file(name.as_str(), options)
            .map_err(|error| format!("adding {name} to the zip: {error}"))?;
        zip.write_all(&bytes)
            .map_err(|error| format!("writing {name} into the zip: {error}"))?;
    }

    if is_cancelled() {
        return Ok(None);
    }
    let cursor = zip
        .finish()
        .map_err(|error| format!("finalising the zip: {error}"))?;
    Ok(Some(PackZipOutput {
        bytes: cursor.into_inner(),
        log,
    }))
}

/// The final `(name, bytes)` for one entry, applying optimise/gzip as its kind
/// and the job settings dictate. A song is optimised first, then gzipped, so the
/// log shows the two savings on their own lines.
fn process_entry(
    entry: &PackEntry,
    gzip_vgms: bool,
    optimize: Option<&dyn SongOptimizer>,
    log: &mut Vec<String>,
) -> (String, Vec<u8>) {
    match entry.kind {
        PackEntryKind::Song => {
            let bytes = match optimize {
                Some(optimizer) => optimizer.optimize(&entry.name, &entry.bytes, log),
                None => entry.bytes.clone(),
            };
            if gzip_vgms && has_extension(&entry.name, "vgm") {
                let name = to_vgz_name(&entry.name);
                if vgms_core::vgm::io::is_gzipped(&bytes) {
                    // Already compressed despite the .vgm name: just rename it.
                    (name, bytes)
                } else {
                    let compressed = gzip(&bytes);
                    log.push(format!(
                        "{} -> {name} ({} -> {} bytes)",
                        entry.name,
                        bytes.len(),
                        compressed.len()
                    ));
                    (name, compressed)
                }
            } else {
                (entry.name.clone(), bytes)
            }
        }
        // oxipng is native-only; the web keeps the PNG's bytes with a note.
        PackEntryKind::Image => {
            log.push(format!(
                "{}: kept as-is (PNG optimization is not available in this browser)",
                entry.name
            ));
            (entry.name.clone(), entry.bytes.clone())
        }
        PackEntryKind::Doc => (entry.name.clone(), entry.bytes.clone()),
    }
}

/// Optimises one song's bytes through `vgms_core`'s built-in pass when it is a
/// VGM that shrinks, logging the saving. A DRO, an already-optimal VGM, or any
/// failure passes through unchanged and is never fatal. The result is plain
/// bytes so the gzip step can still compress it.
fn optimize_song(name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
    let Ok(mut file) = vgms_core::vgm::file::read(name, bytes) else {
        // A DRO, or something unreadable. Either way it passes through.
        log.push(format!("{name}: kept as-is (not a readable VGM)"));
        return bytes.to_vec();
    };
    // `optimize` returns the bytes saved, or `None` when there was nothing to
    // gain -- in which case the original bytes (possibly a `.vgz`) pass through.
    match file.optimize() {
        Some(saved) => match vgms_core::vgm::file::write(&file) {
            Ok(optimized) => {
                log.push(format!(
                    "{name}: {} -> {} bytes (optimized, {saved} saved)",
                    bytes.len(),
                    optimized.len()
                ));
                optimized
            }
            Err(_) => {
                log.push(format!("{name}: kept as-is (could not be written)"));
                bytes.to_vec()
            }
        },
        None => bytes.to_vec(),
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // Writing to an in-memory Vec never fails; unwrap keeps the signature clean.
    encoder.write_all(bytes).expect("gzip write to Vec");
    encoder.finish().expect("gzip finish to Vec")
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

    /// A real VGM carrying a redundant write between two delays, so the built-in
    /// optimiser has something to strip and merge.
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
        let output = build_pack_zip(&[song("01 Song.vgm", &original)], false, Some(&BuiltInOptimizer), &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 Song.vgm");
        assert!(
            files[0].1.len() < original.len(),
            "optimizing should shrink the file"
        );
        assert!(
            vgms_core::io::read_song("01 Song.vgm", &files[0].1).is_ok(),
            "the optimized bytes are still a valid VGM"
        );
        assert!(
            output.log.iter().any(|line| line.contains("(optimized,")),
            "log: {:?}",
            output.log
        );
    }

    #[test]
    fn optimize_off_leaves_the_song_verbatim() {
        let original = optimizable_vgm_bytes();
        let output = build_pack_zip(&[song("01 Song.vgm", &original)], false, None, &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, original, "optimize off means verbatim bytes");
    }

    #[test]
    fn an_unreadable_song_is_kept_verbatim_and_logged() {
        let output = build_pack_zip(&[song("01 Bad.vgm", b"not a vgm")], false, Some(&BuiltInOptimizer), &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, b"not a vgm", "passes through, never fatal");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    #[test]
    fn optimizing_then_gzipping_shrinks_and_renames() {
        let original = optimizable_vgm_bytes();
        let output = build_pack_zip(&[song("01 Song.vgm", &original)], true, Some(&BuiltInOptimizer), &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 Song.vgz", "gzip still renames the entry");
        let mut decoded = Vec::new();
        GzDecoder::new(files[0].1.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert!(decoded.len() < original.len(), "the .vgz gunzips smaller");
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
                bytes: b"pretend png".to_vec(),
                kind: PackEntryKind::Image,
            },
        ];
        let output = build_pack_zip(&entries, true, None, &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["01 First.vgz", "02 Second.vgz", "Game.txt", "Game.png"]
        );

        // The .vgz entry gunzips back to the original (unreadable-as-vgm) bytes.
        let mut decoded = Vec::new();
        GzDecoder::new(files[0].1.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, b"raw vgm one");

        // The doc is verbatim; the PNG is kept as-is (no oxipng on the web).
        assert_eq!(files[2].1, b"description");
        assert_eq!(files[3].1, b"pretend png");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    #[test]
    fn an_already_gzipped_vgm_is_renamed_but_not_recompressed() {
        let gzipped = gzip(b"already compressed");
        let output = build_pack_zip(&[song("01 First.vgm", &gzipped)], true, None, &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 First.vgz");
        assert_eq!(files[0].1, gzipped, "the bytes are untouched");
    }

    #[test]
    fn cancellation_yields_none() {
        let output =
            build_pack_zip(&[song("01 First.vgm", b"raw")], true, None, &|| true, &|| {}).unwrap();
        assert!(output.is_none());
    }
}
