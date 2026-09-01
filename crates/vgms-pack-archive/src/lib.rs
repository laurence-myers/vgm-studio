// SPDX-License-Identifier: GPL-2.0-or-later
//! An in-memory pack archive backing a zip-opened pack (wt-8).
//!
//! Opening a `.zip` unpacks it -- through the same `vgm/vgz/png/txt` filter and
//! flat, lowercase-sorted listing the native folder scan uses -- into a name to
//! bytes map. Pack edits (reorder, retag, delete, screenshot) mutate the map,
//! and an explicit **Save Pack** re-exports it. The mutation semantics mirror the
//! native file service exactly, so a pack behaves the same however it was opened:
//!
//! - **write** inserts or overwrites (an in-place save always overwrites);
//! - **delete** removes, erroring if the entry is absent (as `remove_file` does);
//! - **rename** is a same-name no-op, allows a case-only change, and otherwise
//!   **fails rather than overwrites** an existing target.
//!
//! The map is case-sensitive, so the NTFS case-only temp bounce the native
//! service needs is unnecessary here; the reorder's temp-name dance still works
//! because each step is an ordinary rename.
//!
//! Portable and native-tested: `zip` builds for wasm32 too, and the round-trip
//! proofs run in the ordinary `cargo test`.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};

/// The extensions a pack folder keeps, matching the native scan filter.
pub const PACK_EXTENSIONS: [&str; 4] = ["vgm", "vgz", "png", "txt"];

/// What a [`PackEntry`] is, which decides how the export job treats its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackEntryKind {
    /// A `.vgm`/`.vgz` song. Gzipped to `.vgz` when the job asks for it.
    Song,
    /// A `.png` screenshot. Optimised when an image optimizer is supplied.
    Image,
    /// A generated `.txt`/`.m3u` document. Stored verbatim.
    Doc,
}

/// One file bound for the release zip.
#[derive(Debug, Clone)]
pub struct PackEntry {
    /// The name inside the zip (flat -- no directories).
    pub name: String,
    pub bytes: Vec<u8>,
    pub kind: PackEntryKind,
}

/// The most a single pack entry may decompress to: 128 MiB. A pack holds VGM,
/// VGZ, PNG and text files, none of which reach this in practice; the cap is a
/// zip-bomb guard, so a tiny entry declaring a huge uncompressed length cannot
/// exhaust memory.
const MAX_ENTRY_SIZE: u64 = 128 * 1024 * 1024;

/// The most a whole pack may decompress to: 512 MiB. The per-entry cap alone
/// does not bound the archive -- deflate runs to ~1000:1, so a few-MiB zip
/// holding many at-cap entries would still inflate to gigabytes held live.
/// A real pack is a handful of songs, a screenshot and a text file, far below
/// this; entries past the budget are skipped like any other bad entry.
const MAX_TOTAL_SIZE: u64 = 512 * 1024 * 1024;

/// A pack's files, held in memory and mutated in place until saved.
#[derive(Debug, Clone, Default)]
pub struct PackArchive {
    /// Exact-case file name -> bytes. `BTreeMap` for deterministic iteration; the
    /// listing re-sorts case-insensitively to match the native scan.
    entries: BTreeMap<String, Vec<u8>>,
}

impl PackArchive {
    /// Unpacks a `.zip`'s bytes into the pack file set: flat (basenames only),
    /// filtered to the pack extensions, one bad entry skipped rather than fatal.
    ///
    /// # Errors
    /// Only when the bytes are not a readable zip at all.
    pub fn open(zip_bytes: &[u8]) -> Result<Self, String> {
        Self::open_capped(zip_bytes, MAX_ENTRY_SIZE, MAX_TOTAL_SIZE)
    }

    /// [`open`](Self::open) with explicit per-entry and whole-archive ceilings,
    /// so a test can prove both bomb guards without a 128 MiB entry.
    fn open_capped(zip_bytes: &[u8], cap: u64, total_cap: u64) -> Result<Self, String> {
        // `Cursor<&[u8]>` is `Read + Seek`, so the zip reader needs no owned copy.
        let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
            .map_err(|error| format!("not a readable zip: {error}"))?;

        let mut entries = BTreeMap::new();
        // The running total of bytes kept live. The per-entry cap alone is not a
        // bound on the archive: many at-cap entries would still add up, so each
        // entry also has to fit in what is left of the whole-archive budget.
        let mut total: u64 = 0;
        for index in 0..archive.len() {
            let Ok(file) = archive.by_index(index) else {
                continue;
            };
            if !file.is_file() {
                continue;
            }
            // Flatten any directory prefix to a bare name -- a pack folder is flat.
            let name = basename(file.name());
            if name.is_empty() || !has_pack_extension(&name) {
                continue;
            }
            // Skip an entry that declares more than either ceiling, and still
            // read through `take` in case the header lied -- so neither the
            // declared size nor the real inflated size can beat the caps.
            let budget = cap.min(total_cap - total);
            if file.size() > budget {
                continue;
            }
            let mut bytes = Vec::new();
            if file.take(budget + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > budget
            {
                // Skip an unreadable or oversize entry, as the native scan skips
                // an unreadable file, rather than failing the whole open.
                continue;
            }
            // An overwrite (duplicate name in the zip) releases the old bytes, so
            // the total tracks what is actually held.
            total += bytes.len() as u64;
            if let Some(replaced) = entries.insert(name, bytes) {
                total -= replaced.len() as u64;
            }
        }
        Ok(Self { entries })
    }

    /// The current files, `(name, bytes)`, case-insensitively sorted by name --
    /// the order the native scan produces, so the pack table matches.
    #[must_use]
    pub fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<(String, Vec<u8>)> = self
            .entries
            .iter()
            .map(|(name, bytes)| (name.clone(), bytes.clone()))
            .collect();
        files.sort_by_key(|(name, _)| name.to_lowercase());
        files
    }

    /// Whether the archive holds no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many files the archive holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Inserts or overwrites a file. An in-place save always overwrites.
    pub fn write(&mut self, name: &str, bytes: Vec<u8>) {
        self.entries.insert(name.to_owned(), bytes);
    }

    /// Removes a file.
    ///
    /// # Errors
    /// If the file is not present, as `fs::remove_file` errors on a missing path.
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        self.entries
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| format!("{name}: no such file in the pack"))
    }

    /// Renames a file, mirroring the native decision tree: a same-name no-op, a
    /// permitted case-only change to a free name, and otherwise a move that
    /// **fails rather than overwrites** an existing target.
    ///
    /// # Errors
    /// If the source is missing, or the target already exists as a distinct
    /// entry.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if from == to {
            return Ok(());
        }
        // Check for a collision unconditionally. The old `eq_ignore_ascii_case`
        // short-circuit meant a case-only rename skipped the check -- but this map
        // is case-sensitive and can hold both "DroSong.vgm" and "song.vgm" at once,
        // so renaming one onto the other would silently clobber it. It also only
        // recognised ASCII case, which an unconditional check sidesteps.
        if self.entries.contains_key(to) {
            return Err(format!("{to} already exists"));
        }
        let bytes = self
            .entries
            .remove(from)
            .ok_or_else(|| format!("{from}: no such file in the pack"))?;
        self.entries.insert(to.to_owned(), bytes);
        Ok(())
    }
}

/// The last path component of a zip entry name, treating `/` and `\` as
/// separators (a zip may carry either).
fn basename(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_owned()
}

/// Whether `name`'s extension is one a pack keeps (case-insensitive).
fn has_pack_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        PACK_EXTENSIONS
            .iter()
            .any(|want| ext.eq_ignore_ascii_case(want))
    })
}

// -- The release-zip builder ------------------------------------------------
//
// One builder, driven by both the native app and the web worker. Everything
// target-specific is a trait the caller supplies: the song pass (native tools,
// the web's wasm pipeline, or the built-in pass) and the image pass (oxipng
// natively, a note on the web). So no `oxipng`, no `vgms-vgmtools`, and no
// browser sentence lands here, and the round-trip proofs run in ordinary
// `cargo test`.

use std::io::Write as _;

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

/// Optimises one song's bytes for the pack, appending notes to `log` and
/// returning the result. Never fatal: anything it cannot improve or read comes
/// back as the original bytes.
///
/// The native app supplies the vgmtools pipeline over child processes; the web
/// Worker supplies the same pipeline over the tool `.wasm` modules; both fall
/// back to (and the tests use) [`BuiltInOptimizer`].
pub trait SongOptimizer {
    /// Optimise `bytes` (named `name` for log lines), returning the new bytes.
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8>;
}

/// Optimises one screenshot's bytes for the pack, appending notes to `log`.
/// Never fatal. The native app supplies an oxipng pass; the web supplies one
/// that keeps the bytes and notes that the browser has no PNG optimizer.
pub trait ImageOptimizer {
    /// Optimise `bytes` (named `name` for log lines), returning the new bytes.
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8>;
}

/// `vgms_core`'s own built-in pass -- the optimiser with no external tools, the
/// honest fallback and what the round-trip tests drive.
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltInOptimizer;

impl SongOptimizer for BuiltInOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
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
}

/// Builds the release zip from `entries` (already in final order). Songs are run
/// through `song` when it is `Some` and kept verbatim when `None`; screenshots
/// through `image` likewise. `on_progress` fires before each entry -- the pack
/// Worker posts a heartbeat from it so the page's inactivity watchdog can tell a
/// slow job from a hung one. Returns `Ok(None)` if `is_cancelled` fired partway
/// through, `Err` only on a genuine zip write failure. One bad song or PNG is
/// kept verbatim and logged, never fatal.
pub fn build_pack_zip(
    entries: &[PackEntry],
    gzip_vgms: bool,
    song: Option<&dyn SongOptimizer>,
    image: Option<&dyn ImageOptimizer>,
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
        let (name, bytes) = process_entry(entry, gzip_vgms, song, image, &mut log);
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
    song: Option<&dyn SongOptimizer>,
    image: Option<&dyn ImageOptimizer>,
    log: &mut Vec<String>,
) -> (String, Vec<u8>) {
    match entry.kind {
        PackEntryKind::Song => {
            let bytes = match song {
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
        PackEntryKind::Image => {
            let bytes = match image {
                Some(optimizer) => optimizer.optimize(&entry.name, &entry.bytes, log),
                None => entry.bytes.clone(),
            };
            (entry.name.clone(), bytes)
        }
        PackEntryKind::Doc => (entry.name.clone(), entry.bytes.clone()),
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
    use zip::write::{SimpleFileOptions, ZipWriter};

    /// Builds a zip from `(name, bytes)` entries, in memory.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn names(archive: &PackArchive) -> Vec<String> {
        archive.files().into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn open_filters_flattens_and_sorts() {
        let zip = build_zip(&[
            ("02 Second.vgz", b"b"),
            ("01 First.vgm", b"a"),
            ("Game.txt", b"desc"),
            ("Cover.png", b"png"),
            ("notes.md", b"ignored"),    // wrong extension
            ("sub/03 Nested.vgm", b"c"), // flattened to basename
        ]);
        let archive = PackArchive::open(&zip).unwrap();
        assert_eq!(
            names(&archive),
            [
                "01 First.vgm",
                "02 Second.vgz",
                "03 Nested.vgm",
                "Cover.png",
                "Game.txt"
            ],
        );
        // The wrong-extension file was dropped.
        assert_eq!(archive.len(), 5);
    }

    #[test]
    fn open_rejects_non_zip() {
        assert!(PackArchive::open(b"not a zip at all").is_err());
    }

    #[test]
    fn an_entry_bigger_than_the_cap_is_skipped_not_read() {
        // One small entry and one that inflates past a deliberately small cap.
        let big = vec![0u8; 8192];
        let zip = build_zip(&[("01 A.vgm", b"ok"), ("02 Big.vgm", &big)]);
        let archive = PackArchive::open_capped(&zip, 1024, u64::MAX).unwrap();
        assert_eq!(
            names(&archive),
            ["01 A.vgm"],
            "the oversize entry was dropped"
        );
    }

    #[test]
    fn many_under_cap_entries_cannot_beat_the_whole_archive_budget() {
        // The multi-entry bomb: every entry is under the per-entry cap, but
        // together they pass the whole-archive budget. Entries are admitted in
        // zip order until the budget is spent; the rest are skipped, and a
        // later entry small enough for the remainder still fits.
        let chunk = vec![0u8; 1000];
        let zip = build_zip(&[
            ("01 A.vgm", chunk.as_slice()), // 1000 -> total 1000
            ("02 B.vgm", chunk.as_slice()), // 1000 -> total 2000
            ("03 C.vgm", chunk.as_slice()), // over the 2500 budget: skipped
            ("04 D.vgm", b"tail"),          // 4 fits the remaining 500
        ]);
        let archive = PackArchive::open_capped(&zip, 1024, 2500).unwrap();
        assert_eq!(
            names(&archive),
            ["01 A.vgm", "02 B.vgm", "04 D.vgm"],
            "the entry that would pass the budget is dropped, not the pack"
        );
    }

    #[test]
    fn rename_case_only_fails_rather_than_clobbering_a_distinct_entry() {
        // A case-sensitive map can hold both; a case-only rename must not
        // silently overwrite the distinct lowercase entry.
        let mut archive = PackArchive::open(&build_zip(&[
            ("Song.vgm", b"upper"),
            ("song.vgm", b"lower"),
        ]))
        .unwrap();
        let error = archive.rename("Song.vgm", "song.vgm").unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        // Both entries survive, untouched.
        assert_eq!(archive.len(), 2);
        assert_eq!(
            archive.files(),
            [
                ("Song.vgm".to_owned(), b"upper".to_vec()),
                ("song.vgm".to_owned(), b"lower".to_vec()),
            ]
        );
    }

    #[test]
    fn write_inserts_and_overwrites() {
        let mut archive = PackArchive::open(&build_zip(&[("01 A.vgm", b"one")])).unwrap();
        archive.write("02 B.vgm", b"two".to_vec());
        assert_eq!(names(&archive), ["01 A.vgm", "02 B.vgm"]);
        archive.write("01 A.vgm", b"one-edited".to_vec());
        let files = archive.files();
        assert_eq!(files[0], ("01 A.vgm".to_owned(), b"one-edited".to_vec()));
    }

    #[test]
    fn delete_removes_and_errors_when_absent() {
        let mut archive = PackArchive::open(&build_zip(&[("01 A.vgm", b"one")])).unwrap();
        archive.delete("01 A.vgm").unwrap();
        assert!(archive.is_empty());
        assert!(archive.delete("01 A.vgm").is_err());
    }

    #[test]
    fn rename_same_name_is_a_noop() {
        let mut archive = PackArchive::open(&build_zip(&[("01 A.vgm", b"one")])).unwrap();
        archive.rename("01 A.vgm", "01 A.vgm").unwrap();
        assert_eq!(names(&archive), ["01 A.vgm"]);
    }

    #[test]
    fn rename_case_only_is_allowed() {
        let mut archive = PackArchive::open(&build_zip(&[("Song.VGM", b"one")])).unwrap();
        archive.rename("Song.VGM", "song.vgm").unwrap();
        assert_eq!(names(&archive), ["song.vgm"]);
    }

    #[test]
    fn rename_fails_rather_than_overwrites() {
        let mut archive =
            PackArchive::open(&build_zip(&[("01 A.vgm", b"a"), ("02 B.vgm", b"b")])).unwrap();
        let error = archive.rename("01 A.vgm", "02 B.vgm").unwrap_err();
        assert!(error.contains("already exists"));
        // Both files are untouched.
        assert_eq!(names(&archive), ["01 A.vgm", "02 B.vgm"]);
    }

    #[test]
    fn rename_moves_bytes_to_the_new_name() {
        let mut archive = PackArchive::open(&build_zip(&[("01 A.vgm", b"one")])).unwrap();
        archive.rename("01 A.vgm", "05 Renamed.vgm").unwrap();
        assert_eq!(
            archive.files(),
            [("05 Renamed.vgm".to_owned(), b"one".to_vec())]
        );
    }

    #[test]
    fn reorder_temp_dance_swaps_two_tracks() {
        // What the pack reorder executor does: rename through temp names so a swap
        // never transiently collides.
        let mut archive =
            PackArchive::open(&build_zip(&[("01 A.vgm", b"a"), ("02 B.vgm", b"b")])).unwrap();
        archive.rename("01 A.vgm", "01 A.vgm.tmp").unwrap();
        archive.rename("02 B.vgm", "01 B.vgm").unwrap();
        archive.rename("01 A.vgm.tmp", "02 A.vgm").unwrap();
        assert_eq!(names(&archive), ["01 B.vgm", "02 A.vgm"]);
        // The bytes travelled with the names.
        let files = archive.files();
        assert_eq!(files[0].1, b"b");
        assert_eq!(files[1].1, b"a");
    }
}

#[cfg(test)]
mod pack_zip_tests {
    use super::*;

    use flate2::read::GzDecoder;
    use zip::ZipArchive;

    /// A stand-in image pass for the tests: keeps the bytes, notes it did. Stands
    /// for the web's browser-note optimizer; the native oxipng pass is proven in
    /// `vgms-app`, where the native-only crate lives.
    struct NotingImageOptimizer;

    impl ImageOptimizer for NotingImageOptimizer {
        fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
            log.push(format!("{name}: kept as-is (no optimizer here)"));
            bytes.to_vec()
        }
    }

    fn song(name: &str, bytes: &[u8]) -> PackEntry {
        PackEntry {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
            kind: PackEntryKind::Song,
        }
    }

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
    /// optimiser has something to strip and merge. Assembled as a `VgmFile`: a
    /// synthesised v1.51 header with the YMZ280B clock (offset 0x68, spec-stable
    /// -- a chip whose one core applies writes immediately, so dedup applies),
    /// the stream, an end marker, then canonicalised through the reader/writer.
    fn optimizable_vgm_bytes() -> Vec<u8> {
        use vgms_core::vgm::io::synthesise_header;
        let stream = [
            0x5D, 0x20, 0x01, // write
            0x61, 0x64, 0x00, // wait 100
            0x5D, 0x20, 0x01, // redundant write
            0x61, 0xC8, 0x00, // wait 200
            0x5D, 0x21, 0x02, // write
        ];
        let mut bytes = synthesise_header();
        bytes[0x68..0x6C].copy_from_slice(&16_934_400u32.to_le_bytes());
        bytes.extend_from_slice(&stream);
        bytes.push(0x66); // end marker
        let eof = (bytes.len() - 0x04) as u32;
        bytes[0x04..0x08].copy_from_slice(&eof.to_le_bytes());
        let file = vgms_core::vgm::file::read("x.vgm", &bytes).unwrap();
        vgms_core::vgm::file::write(&file).unwrap()
    }

    #[test]
    fn an_optimizable_vgm_is_shrunk_and_logged() {
        let original = optimizable_vgm_bytes();
        let output = build_pack_zip(
            &[song("01 Song.vgm", &original)],
            false,
            Some(&BuiltInOptimizer),
            None,
            &never(),
            &|| {},
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 Song.vgm");
        assert!(files[0].1.len() < original.len(), "optimizing shrinks it");
        assert!(
            vgms_core::vgm::file::read("01 Song.vgm", &files[0].1).is_ok(),
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
        let output = build_pack_zip(
            &[song("01 Song.vgm", &original)],
            false,
            None,
            None,
            &never(),
            &|| {},
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, original, "optimize off means verbatim bytes");
    }

    #[test]
    fn an_unreadable_song_is_kept_verbatim_and_logged() {
        let output = build_pack_zip(
            &[song("01 Bad.vgm", b"not a vgm")],
            false,
            Some(&BuiltInOptimizer),
            None,
            &never(),
            &|| {},
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, b"not a vgm", "passes through, never fatal");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    #[test]
    fn optimizing_then_gzipping_shrinks_and_renames() {
        let original = optimizable_vgm_bytes();
        let output = build_pack_zip(
            &[song("01 Song.vgm", &original)],
            true,
            Some(&BuiltInOptimizer),
            None,
            &never(),
            &|| {},
        )
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
        let output = build_pack_zip(
            &entries,
            true,
            None,
            Some(&NotingImageOptimizer),
            &never(),
            &|| {},
        )
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

        // The doc is verbatim; the image went through the supplied optimizer.
        assert_eq!(files[2].1, b"description");
        assert_eq!(files[3].1, b"pretend png");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    #[test]
    fn no_image_optimizer_keeps_the_png_verbatim_and_silent() {
        let entries = [PackEntry {
            name: "Game.png".to_owned(),
            bytes: b"pretend png".to_vec(),
            kind: PackEntryKind::Image,
        }];
        let output = build_pack_zip(&entries, false, None, None, &never(), &|| {})
            .unwrap()
            .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, b"pretend png");
        assert!(
            output.log.is_empty(),
            "no optimizer, no note: {:?}",
            output.log
        );
    }

    #[test]
    fn an_already_gzipped_vgm_is_renamed_but_not_recompressed() {
        let gzipped = gzip(b"already compressed");
        let output = build_pack_zip(
            &[song("01 First.vgm", &gzipped)],
            true,
            None,
            None,
            &never(),
            &|| {},
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].0, "01 First.vgz");
        assert_eq!(files[0].1, gzipped, "the bytes are untouched");
    }

    #[test]
    fn a_heartbeat_fires_once_per_entry() {
        use std::cell::Cell;
        let beats = Cell::new(0);
        let entries = [song("01 First.vgm", b"raw"), song("02 Second.vgm", b"raw")];
        build_pack_zip(&entries, false, None, None, &never(), &|| {
            beats.set(beats.get() + 1);
        })
        .unwrap();
        assert_eq!(beats.get(), 2, "one heartbeat per entry");
    }

    #[test]
    fn cancellation_yields_none() {
        let output = build_pack_zip(
            &[song("01 First.vgm", b"raw")],
            true,
            None,
            None,
            &|| true,
            &|| {},
        )
        .unwrap();
        assert!(output.is_none());
    }
}
