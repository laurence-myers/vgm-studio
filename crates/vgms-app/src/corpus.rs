// SPDX-License-Identifier: GPL-2.0-or-later
//! An index of the VGMRips corpus by chip, so a core can be tested against the
//! files that actually exercise it.
//!
//! The corpus is organised by system (Arcade, MegaDrive, NeoGeo), the wrong axis
//! here: a YM2612 core wants YM2612 files, which are spread across systems and
//! live in folders whose names never mention the chip. So this walks the tree
//! once, reads each header, and inverts it to chip-to-files. Reading tens of
//! thousands of headers is slow, so the result is cached and the walk only
//! happens when the cache is missing. Point it at a corpus with
//! `VGMSTUDIO_VGMRIPS_CORPUS`.
//!
//! ```text
//! VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
//!     cargo test -p vgms-app --release --test chip_index -- --ignored --nocapture
//! ```
//!
//! Only the header is read, never the stream: the question is "which files name
//! this chip", so a file whose stream will not walk still earns an index entry.
//! The cache is tab-separated (the workspace has no JSON dependency).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use vgms_core::vgm::ChipKind;

/// The environment variable naming the corpus root.
pub const CORPUS_ENV: &str = "VGMSTUDIO_VGMRIPS_CORPUS";

/// The cache file's format marker. Bump it when the columns change, so a stale
/// cache is rebuilt rather than misread.
const CACHE_HEADER: &str = "# vgmstudio chip index v1";

/// Which corpus files name which chips.
#[derive(Debug, Default, Clone)]
pub struct ChipIndex {
    root: PathBuf,
    /// Chip to paths, relative to [`root`](Self::root), each list sorted.
    by_chip: BTreeMap<ChipKind, Vec<PathBuf>>,
    /// How many files were walked, including ones with no readable header.
    scanned: usize,
    /// How many could not be read at all.
    unreadable: usize,
}

impl ChipIndex {
    /// Walks `root` and reads every `.vgm`/`.vgz` header.
    ///
    /// Slow -- tens of thousands of files -- so prefer
    /// [`open_or_build`](Self::open_or_build), which caches.
    #[must_use]
    pub fn build(root: &Path) -> Self {
        let mut index = Self {
            root: root.to_path_buf(),
            ..Self::default()
        };
        let mut files = Vec::new();
        collect(root, &mut files);
        files.sort();
        for path in files {
            index.scanned += 1;
            let Ok(bytes) = std::fs::read(&path) else {
                index.unreadable += 1;
                continue;
            };
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let Ok(file) = vgms_core::vgm::file::read(&name, &bytes) else {
                index.unreadable += 1;
                continue;
            };
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            for chip in file.header.chips() {
                index
                    .by_chip
                    .entry(chip.kind)
                    .or_default()
                    .push(relative.clone());
            }
        }
        for paths in index.by_chip.values_mut() {
            paths.sort();
            paths.dedup();
        }
        index
    }

    /// Reads a cache written by [`save`](Self::save).
    ///
    /// `None` for a missing, unreadable or differently-versioned cache, which
    /// all mean the same thing to the caller: walk again.
    #[must_use]
    pub fn load(cache: &Path, root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(cache).ok()?;
        let mut lines = text.lines();
        if lines.next()? != CACHE_HEADER {
            return None;
        }
        let mut index = Self {
            root: root.to_path_buf(),
            ..Self::default()
        };
        for line in lines {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let (slug, path) = line.split_once('\t')?;
            // A slug this build does not know means the chip table changed
            // under the cache; rebuilding is cheaper than reasoning about it.
            let chip = ChipKind::from_slug(slug)?;
            index.by_chip.entry(chip).or_default().push(path.into());
        }
        index.scanned = index.by_chip.values().map(Vec::len).sum();
        Some(index)
    }

    /// Writes the cache, creating parent directories as needed.
    ///
    /// # Errors
    /// If the file or its parent directories cannot be written.
    pub fn save(&self, cache: &Path) -> std::io::Result<()> {
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(cache)?);
        writeln!(out, "{CACHE_HEADER}")?;
        writeln!(out, "# root: {}", self.root.display())?;
        writeln!(out, "# {} files scanned", self.scanned)?;
        for (chip, paths) in &self.by_chip {
            for path in paths {
                // Forward slashes so a cache is readable on either platform.
                writeln!(out, "{}\t{}", chip.slug(), path.display())?;
            }
        }
        out.flush()
    }

    /// The cached index, walking the corpus only if there is no usable cache.
    #[must_use]
    pub fn open_or_build(root: &Path, cache: &Path) -> Self {
        if let Some(index) = Self::load(cache, root) {
            return index;
        }
        let index = Self::build(root);
        if let Err(error) = index.save(cache) {
            // A cache that cannot be written costs time, not correctness.
            log::warn!(
                "could not write the chip index to {}: {error}",
                cache.display()
            );
        }
        index
    }

    /// The corpus root these paths are relative to.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Files naming `chip`, relative to [`root`](Self::root).
    #[must_use]
    pub fn files(&self, chip: ChipKind) -> &[PathBuf] {
        self.by_chip.get(&chip).map_or(&[], Vec::as_slice)
    }

    /// Up to `want` files naming `chip`, as absolute paths, spread across the
    /// whole list rather than taken from its head.
    ///
    /// The corpus is sorted by path, so a head sample would be one system's
    /// first pack -- one game, one ripper. A stride samples across systems for
    /// the variety a core test wants. Deterministic, so a failure names a file
    /// that can be re-run.
    #[must_use]
    pub fn sample(&self, chip: ChipKind, want: usize) -> Vec<PathBuf> {
        let files = self.files(chip);
        if want == 0 || files.is_empty() {
            return Vec::new();
        }
        if files.len() <= want {
            return files.iter().map(|path| self.root.join(path)).collect();
        }
        let stride = files.len() / want;
        (0..want)
            .map(|index| self.root.join(&files[index * stride]))
            .collect()
    }

    /// Every chip present in the corpus, with its file count, commonest first.
    #[must_use]
    pub fn by_frequency(&self) -> Vec<(ChipKind, usize)> {
        let mut counts: Vec<(ChipKind, usize)> = self
            .by_chip
            .iter()
            .map(|(&chip, paths)| (chip, paths.len()))
            .collect();
        // Descending by count, then by chip so ties are stable.
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        counts
    }

    /// How many files were walked, and how many could not be read.
    #[must_use]
    pub fn scanned(&self) -> (usize, usize) {
        (self.scanned, self.unreadable)
    }
}

/// The corpus root from the environment, if it is set and exists.
#[must_use]
pub fn corpus_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os(CORPUS_ENV)?);
    root.is_dir().then_some(root)
}

/// Where the cache lives: beside the corpus if that is writable, else under
/// `target/`.
#[must_use]
pub fn cache_path(root: &Path) -> PathBuf {
    root.join("vgmstudio-chip-index.tsv")
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("vgm") || extension.eq_ignore_ascii_case("vgz")
            })
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(pairs: &[(ChipKind, &str)]) -> ChipIndex {
        let mut by_chip: BTreeMap<ChipKind, Vec<PathBuf>> = BTreeMap::new();
        for (chip, path) in pairs {
            by_chip.entry(*chip).or_default().push(PathBuf::from(path));
        }
        ChipIndex {
            root: PathBuf::from("/corpus"),
            scanned: pairs.len(),
            by_chip,
            unreadable: 0,
        }
    }

    #[test]
    fn a_sample_spreads_across_the_list_rather_than_taking_its_head() {
        let paths: Vec<String> = (0..100).map(|n| format!("pack{n:03}/song.vgz")).collect();
        let pairs: Vec<(ChipKind, &str)> = paths
            .iter()
            .map(|path| (ChipKind::Ym2612, path.as_str()))
            .collect();
        let index = index_of(&pairs);

        let sample = index.sample(ChipKind::Ym2612, 4);
        assert_eq!(sample.len(), 4);
        assert!(
            sample
                .iter()
                .any(|path| path.to_string_lossy().contains("pack075")),
            "a sample from the head only would be four songs by one ripper: {sample:?}"
        );
        // Deterministic, so a failure names a file that can be re-run.
        assert_eq!(index.sample(ChipKind::Ym2612, 4), sample);
    }

    #[test]
    fn asking_for_more_than_there_are_gives_all_of_them() {
        let index = index_of(&[(ChipKind::Sn76489, "a.vgz"), (ChipKind::Sn76489, "b.vgz")]);
        assert_eq!(index.sample(ChipKind::Sn76489, 10).len(), 2);
        assert!(index.sample(ChipKind::Sn76489, 0).is_empty());
        assert!(index.sample(ChipKind::Ym2612, 5).is_empty(), "no such chip");
    }

    #[test]
    fn samples_are_absolute_but_the_index_stores_relative_paths() {
        // Relative in the cache so it survives the corpus being moved or read
        // from another machine; absolute on the way out because that is what
        // opens a file.
        let index = index_of(&[(ChipKind::Sn76489, "Arcade/x.vgz")]);
        assert_eq!(
            index.files(ChipKind::Sn76489),
            [PathBuf::from("Arcade/x.vgz")]
        );
        assert_eq!(
            index.sample(ChipKind::Sn76489, 1),
            [PathBuf::from("/corpus").join("Arcade/x.vgz")]
        );
    }

    #[test]
    fn frequency_order_is_commonest_first_and_stable_on_ties() {
        let index = index_of(&[
            (ChipKind::Ym2612, "a.vgz"),
            (ChipKind::Sn76489, "a.vgz"),
            (ChipKind::Sn76489, "b.vgz"),
            (ChipKind::Ym2151, "c.vgz"),
        ]);
        assert_eq!(
            index.by_frequency(),
            [
                (ChipKind::Sn76489, 2),
                // Tied at one: enum order breaks it, so the report never
                // reshuffles between runs.
                (ChipKind::Ym2612, 1),
                (ChipKind::Ym2151, 1),
            ]
        );
    }

    #[test]
    fn a_cache_round_trips_and_a_foreign_one_is_rejected() {
        let dir = std::env::temp_dir().join("vgmstudio-chip-index-test");
        let cache = dir.join("index.tsv");
        let index = index_of(&[
            (ChipKind::Sn76489, "Arcade/x.vgz"),
            (ChipKind::Ym2612, "MegaDrive/y.vgz"),
        ]);
        index.save(&cache).expect("writing the cache");

        let reread = ChipIndex::load(&cache, Path::new("/corpus")).expect("reading it back");
        assert_eq!(
            reread.files(ChipKind::Sn76489),
            index.files(ChipKind::Sn76489)
        );
        assert_eq!(
            reread.files(ChipKind::Ym2612),
            index.files(ChipKind::Ym2612)
        );

        // A cache from another tool, or an older format, must be rebuilt rather
        // than half-read.
        std::fs::write(&cache, "something else\nsn76489\tx.vgz\n").expect("writing junk");
        assert!(ChipIndex::load(&cache, Path::new("/corpus")).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
