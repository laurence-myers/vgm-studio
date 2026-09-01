//! The native side of the release-zip export: the two passes that only exist on
//! the desktop, wrapped around the shared builder.
//!
//! The build logic -- gzip, rename, pack flat, keep one bad file rather than
//! fail -- lives once in [`vgms_pack_archive::build_pack_zip`], shared with the
//! web. Here we supply what is native-only: the song pass over the vgmtools child
//! processes, and the PNG pass over oxipng (C + rayon, no browser). Those crates
//! never reach `vgms-pack-archive`, which is why they live here.

use vgms_pack_archive::{ImageOptimizer, PackEntry, SongOptimizer};

pub use vgms_pack_archive::PackZipOutput;

/// The desktop song pass: the full vgmtools pipeline over child processes,
/// routed by the Settings optimiser choice and tool-stage switches.
struct NativeSongOptimizer {
    optimizer: vgms_core::config::OptimizerChoice,
    sample_roms: bool,
    dac_runs: bool,
}

impl SongOptimizer for NativeSongOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
        // The pass and its narration live in `vgms_vgmtools::optimize_song_logged`
        // -- one copy shared with the web pack worker -- driven here by the native
        // tool runner. Never fatal: a DRO, an already-optimal VGM or any failure
        // passes through unchanged, the same posture as the PNG path.
        vgms_vgmtools::optimize_song_logged(
            name,
            bytes,
            vgms_vgmtools::Options {
                optimizer: self.optimizer,
                sample_roms: self.sample_roms,
                dac_runs: self.dac_runs,
                // The export is unverified, so the hold-backs stand.
                ..Default::default()
            },
            &vgms_vgmtools::NativeTools,
            log,
        )
    }
}

/// The desktop image pass: oxipng, keeping the PNG verbatim (with a note) when it
/// cannot process the file.
struct OxipngOptimizer;

impl ImageOptimizer for OxipngOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
        match oxipng::optimize_from_memory(bytes, &png_options()) {
            Ok(optimized) => {
                log.push(format!(
                    "{name}: {} -> {} bytes (oxipng)",
                    bytes.len(),
                    optimized.len()
                ));
                optimized
            }
            Err(error) => {
                log.push(format!("{name}: kept as-is (oxipng failed: {error})"));
                bytes.to_vec()
            }
        }
    }
}

/// Builds the release zip from `entries` (already in final order), optimising the
/// songs through the vgmtools pipeline when `optimize_vgms`, and the PNGs through
/// oxipng always. Returns `Ok(None)` if `is_cancelled` fired partway through.
///
/// A PNG oxipng cannot process, or a song the tools cannot read, is kept verbatim
/// and logged, never fatal: one bad file must not sink the whole export.
pub fn build_pack_zip(
    entries: &[PackEntry],
    gzip_vgms: bool,
    optimize_vgms: bool,
    optimizer: vgms_core::config::OptimizerChoice,
    sample_roms: bool,
    dac_runs: bool,
    is_cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<Option<PackZipOutput>> {
    let native_song = NativeSongOptimizer {
        optimizer,
        sample_roms,
        dac_runs,
    };
    let song: Option<&dyn SongOptimizer> = optimize_vgms.then_some(&native_song);
    vgms_pack_archive::build_pack_zip(
        entries,
        gzip_vgms,
        song,
        Some(&OxipngOptimizer),
        is_cancelled,
        &|| {},
    )
    .map_err(anyhow::Error::msg)
}

/// The oxipng settings shared by the export job and the explicit
/// optimise-in-place action.
pub(crate) fn png_options() -> oxipng::Options {
    let mut options = oxipng::Options::from_preset(2);
    // Drop non-critical chunks that do not affect rendering (comments, etc.).
    options.strip = oxipng::StripChunks::Safe;
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read as _};

    use vgms_pack_archive::PackEntryKind;
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

    /// A Mega Drive rip with a split wait comes out smaller -- proving
    /// the native song optimizer is actually wired to `build_pack_zip` (the
    /// shared builder cannot run it).
    #[test]
    fn a_ym2612_vgm_is_optimized_through_the_native_tools() {
        // The YM2612's cores pace writes, so no register write is ever
        // dropped from it -- what its files can still lose is delay spelling,
        // the split wait below re-encoding as one.
        let original = non_opl_vgm(
            0x2C,
            &[
                0x52, 0x30, 0x71, // an operator register
                0x61, 0x64, 0x00, // wait 100 --
                0x61, 0xC8, 0x00, // wait 200 -- merged into one wait 300
                0x52, 0x34, 0x71, // another register, so the file has body
                0x62, //
                0x66,
            ],
        );
        let output = build_pack_zip(
            &[song("01 MD.vgm", &original)],
            false,
            true,
            vgms_core::config::OptimizerChoice::Auto,
            true,
            true,
            &never(),
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        assert!(files[0].1.len() < original.len(), "it shrank");
        assert!(
            output.log.iter().any(|line| line.contains("(optimized")),
            "log: {:?}",
            output.log
        );
        let reread = vgms_core::vgm::file::read("01 MD.vgm", &files[0].1).unwrap();
        assert_eq!(reread.chip_list(), "YM2612");
    }

    /// `vgm_cmp` has a table for the YMZ280B: a chip the built-in pass cannot
    /// touch is still optimised through the bound tools.
    #[test]
    fn a_chip_the_built_in_pass_cannot_touch_is_optimized_by_the_tools() {
        let original = non_opl_vgm(0x68, &[0x5D, 0x01, 0x40, 0x5D, 0x01, 0x40, 0x66]);
        let output = build_pack_zip(
            &[song("01 Arcade.vgm", &original)],
            false,
            true,
            vgms_core::config::OptimizerChoice::Auto,
            true,
            true,
            &never(),
        )
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

    /// A chip `vgm_cmp` copies through is optimised in house instead, and the
    /// export log says so when the tool is the one that ran.
    #[test]
    fn a_chip_vgm_cmp_passes_through_is_optimized_by_the_built_in() {
        // A K053260: `vgm_cmp` has a handler for it, but it is commented out
        // (`chip_cmp.c:10` still lists it as a TODO), so the tool keeps every
        // write. The built-in has its own rule, so `Auto` shrinks the file.
        let original = non_opl_vgm(0xAC, &[0xBA, 0x01, 0x40, 0xBA, 0x01, 0x40, 0x66]);
        let optimized = build_pack_zip(
            &[song("01 Arcade.vgm", &original)],
            false,
            true,
            vgms_core::config::OptimizerChoice::Auto,
            true,
            true,
            &never(),
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&optimized.bytes);
        assert!(
            files[0].1.len() < original.len(),
            "the K053260's repeated write should be dropped in house"
        );
        assert!(
            !optimized
                .log
                .iter()
                .any(|line| line.contains("could not read")),
            "and it is not unreadable: {:?}",
            optimized.log
        );

        // Forced onto the tools, the same file names the gap in their table
        // rather than looking unreadable.
        let tools = build_pack_zip(
            &[song("01 Arcade.vgm", &original)],
            false,
            true,
            vgms_core::config::OptimizerChoice::Tools,
            true,
            true,
            &never(),
        )
        .unwrap()
        .unwrap();
        assert!(
            tools
                .log
                .iter()
                .any(|line| line.contains("K053260 not optimized by vgm_cmp")),
            "log: {:?}",
            tools.log
        );
    }

    #[test]
    fn an_already_optimal_vgm_passes_through_the_tools_unchanged() {
        // The fixture predates the zero-wait-override rule, so one pass may
        // still find dead init writes in it; what "already optimal" promises is
        // that a file the pass cannot improve keeps its exact bytes -- so
        // optimise once, then require the second pass to change nothing.
        const CLEAN: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
        let pack = |bytes: &[u8]| {
            build_pack_zip(
                &[song("01 Clean.vgm", bytes)],
                false,
                true,
                vgms_core::config::OptimizerChoice::Auto,
                true,
                true,
                &never(),
            )
            .unwrap()
            .unwrap()
        };
        let once = read_zip(&pack(CLEAN).bytes);
        let twice = read_zip(&pack(&once[0].1).bytes);
        assert_eq!(
            twice[0].1, once[0].1,
            "an optimal VGM is untouched, byte for byte"
        );
    }

    /// The PNG path is the native-only half: oxipng shrinks a real screenshot,
    /// and the songs still pack flat around it.
    #[test]
    fn oxipng_shrinks_the_png_and_packs_everything_flat() {
        let entries = [
            song("01 First.vgm", b"raw vgm one"),
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
        let output = build_pack_zip(
            &entries,
            true,
            false,
            vgms_core::config::OptimizerChoice::Auto,
            true,
            true,
            &never(),
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["01 First.vgz", "Game.txt", "Game.png"]);

        assert_eq!(files[1].1, b"description", "the doc is verbatim");
        assert_eq!(&files[2].1[..8], b"\x89PNG\r\n\x1a\n", "still a PNG");
        assert!(files[2].1.len() <= PNG.len(), "oxipng did not grow it");
        assert!(
            output.log.iter().any(|line| line.contains("(oxipng)")),
            "log: {:?}",
            output.log
        );
    }

    #[test]
    fn a_corrupt_png_is_kept_verbatim_and_logged() {
        let entries = [PackEntry {
            name: "Bad.png".to_owned(),
            bytes: b"not really a png".to_vec(),
            kind: PackEntryKind::Image,
        }];
        let output = build_pack_zip(
            &entries,
            true,
            false,
            vgms_core::config::OptimizerChoice::Auto,
            true,
            true,
            &never(),
        )
        .unwrap()
        .unwrap();
        let files = read_zip(&output.bytes);
        assert_eq!(files[0].1, b"not really a png");
        assert!(output.log.iter().any(|line| line.contains("kept as-is")));
    }

    /// The native wrapper adds nothing to a song/doc pack beyond its image and
    /// song passes: with optimize off and no images, it must match the shared
    /// builder called with no optimizers at all -- the proof that both targets
    /// pack the same bytes, names and log for the same entries.
    #[test]
    fn optimize_off_matches_the_shared_builder_byte_for_byte() {
        let entries = [
            song("01 First.vgm", b"raw vgm one"),
            song("02 Second.vgm", b"raw vgm two"),
            PackEntry {
                name: "Game.txt".to_owned(),
                bytes: b"description".to_vec(),
                kind: PackEntryKind::Doc,
            },
        ];
        let via_wrapper = build_pack_zip(
            &entries,
            true,
            false,
            vgms_core::config::OptimizerChoice::Auto,
            true,
            true,
            &never(),
        )
        .unwrap()
        .unwrap();
        let via_shared =
            vgms_pack_archive::build_pack_zip(&entries, true, None, None, &never(), &|| {})
                .unwrap()
                .unwrap();
        assert_eq!(via_wrapper.bytes, via_shared.bytes, "identical zip bytes");
        assert_eq!(via_wrapper.log, via_shared.log, "identical log");
    }
}
