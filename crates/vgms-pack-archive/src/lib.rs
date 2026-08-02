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

/// The most a single pack entry may decompress to: 128 MiB. A pack holds VGM,
/// VGZ, PNG and text files, none of which reach this in practice; the cap is a
/// zip-bomb guard, so a tiny entry declaring a huge uncompressed length cannot
/// exhaust memory.
const MAX_ENTRY_SIZE: u64 = 128 * 1024 * 1024;

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
        Self::open_capped(zip_bytes, MAX_ENTRY_SIZE)
    }

    /// [`open`](Self::open) with an explicit per-entry ceiling, so a test can
    /// prove the bomb guard without a 128 MiB entry.
    fn open_capped(zip_bytes: &[u8], cap: u64) -> Result<Self, String> {
        // `Cursor<&[u8]>` is `Read + Seek`, so the zip reader needs no owned copy.
        let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
            .map_err(|error| format!("not a readable zip: {error}"))?;

        let mut entries = BTreeMap::new();
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
            // Skip an entry that declares more than the ceiling, and still read
            // through `take` in case the header lied -- so neither the declared
            // size nor the real inflated size can beat the cap.
            if file.size() > cap {
                continue;
            }
            let mut bytes = Vec::new();
            if file.take(cap + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > cap {
                // Skip an unreadable or oversize entry, as the native scan skips
                // an unreadable file, rather than failing the whole open.
                continue;
            }
            entries.insert(name, bytes);
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
        // is case-sensitive and can hold both "Song.vgm" and "song.vgm" at once,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
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
        let archive = PackArchive::open_capped(&zip, 1024).unwrap();
        assert_eq!(names(&archive), ["01 A.vgm"], "the oversize entry was dropped");
    }

    #[test]
    fn rename_case_only_fails_rather_than_clobbering_a_distinct_entry() {
        // A case-sensitive map can hold both; a case-only rename must not
        // silently overwrite the distinct lowercase entry.
        let mut archive =
            PackArchive::open(&build_zip(&[("Song.vgm", b"upper"), ("song.vgm", b"lower")]))
                .unwrap();
        let error = archive.rename("Song.vgm", "song.vgm").unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        // Both entries survive, untouched.
        assert_eq!(archive.len(), 2);
        assert_eq!(archive.files(), [
            ("Song.vgm".to_owned(), b"upper".to_vec()),
            ("song.vgm".to_owned(), b"lower".to_vec()),
        ]);
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
