//! Shared helpers for the corpus-driven integration tests.
//!
//! Several suites walk the same `VGMSTUDIO_VGMRIPS_CORPUS` tree for `.vgm`/`.vgz`
//! files. The recursive collector lived four times over, each subtly different --
//! one sorted, one capped, one lower-cased the extension, one did not. This is
//! the single copy. (The heavier `ChipIndex` in `vgms_app::corpus`, with its
//! per-chip cache, is a different job and stays where it is.)

#![allow(dead_code)] // Each integration test binary uses a different subset.

use std::path::{Path, PathBuf};

/// True for a path ending `.vgm` or `.vgz`, case-insensitively.
fn is_song(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("vgm") || extension.eq_ignore_ascii_case("vgz")
        })
}

/// Every `.vgm`/`.vgz` under `root`, recursively and **sorted**, so a run visits
/// files in a stable order regardless of the filesystem's directory order.
/// Unreadable directories are skipped, not fatal -- a corpus is a loose tree.
pub(crate) fn collect_songs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, usize::MAX, &mut out);
    out.sort();
    out
}

/// Like [`collect_songs`], but stops once `limit` files are gathered. The order
/// is the filesystem's own (not sorted): the callers that cap only want *some*
/// files, cheaply, and pay for exactly that many `read`s downstream.
pub(crate) fn collect_songs_capped(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, limit, &mut out);
    out
}

fn walk(dir: &Path, limit: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            walk(&path, limit, out);
        } else if is_song(&path) {
            out.push(path);
        }
    }
}
