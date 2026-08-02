// SPDX-License-Identifier: GPL-2.0-or-later
//! The web's thin adapter over the shared pack-zip builder.
//!
//! The build logic -- optimise, gzip, pack flat -- lives once in
//! [`vgms_pack_archive::build_pack_zip`], shared with the native app. The only
//! web-specific part is the PNG step: oxipng is C + rayon and does not come to
//! the browser, so a screenshot is kept with a note. The song optimizer is
//! passed in by the caller (the Worker's wasm pipeline, or the built-in pass).

pub use vgms_pack_archive::{BuiltInOptimizer, PackZipOutput, SongOptimizer};

use vgms_pack_archive::{ImageOptimizer, PackEntry};

/// The web's image pass: keep the PNG's bytes and say why. This is the one
/// browser-specific sentence, and it lives here rather than in the shared,
/// target-independent builder.
struct BrowserImageOptimizer;

impl ImageOptimizer for BrowserImageOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
        log.push(format!(
            "{name}: kept as-is (PNG optimization is not available in this browser)"
        ));
        bytes.to_vec()
    }
}

/// The shared builder with the browser's PNG behaviour supplied. Songs run
/// through `optimize` when it is `Some`, kept verbatim when `None`; `on_progress`
/// fires before each entry (the pack Worker posts a heartbeat from it). Returns
/// `Ok(None)` if `is_cancelled` fired partway through.
pub fn build_pack_zip(
    entries: &[PackEntry],
    gzip_vgms: bool,
    optimize: Option<&dyn SongOptimizer>,
    is_cancelled: &dyn Fn() -> bool,
    on_progress: &dyn Fn(),
) -> Result<Option<PackZipOutput>, String> {
    vgms_pack_archive::build_pack_zip(
        entries,
        gzip_vgms,
        optimize,
        Some(&BrowserImageOptimizer),
        is_cancelled,
        on_progress,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read as _};

    use vgms_pack_archive::PackEntryKind;
    use zip::ZipArchive;

    /// The whole point of the web adapter: a PNG is kept verbatim, with the
    /// browser-specific note that lives only here. The song/gzip/pack behaviour
    /// is proven once in `vgms-pack-archive`.
    #[test]
    fn a_png_is_kept_verbatim_with_the_browser_note() {
        let entries = [PackEntry {
            name: "Game.png".to_owned(),
            bytes: b"pretend png".to_vec(),
            kind: PackEntryKind::Image,
        }];
        let output = build_pack_zip(&entries, false, None, &|| false, &|| {})
            .unwrap()
            .unwrap();

        let mut archive = ZipArchive::new(Cursor::new(output.bytes)).unwrap();
        let mut data = Vec::new();
        archive.by_index(0).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"pretend png", "the PNG is untouched");
        assert!(
            output
                .log
                .iter()
                .any(|line| line.contains("not available in this browser")),
            "log: {:?}",
            output.log
        );
    }
}
