//! Why the YM2612 fails the built-in optimiser's render gate, and the YM2608,
//! YM2610 and every OPL part do not.
//!
//! Nuked-OPN2 is cycle-accurate about the *write bus*: this app's wrapper
//! (`vgms-cores-nuked/src/opn2.rs`) queues register writes and lets one through
//! per 24-cycle rotation, because that is what the real chip's busy flag does.
//! Removing a write from a zero-delay burst therefore lets every write behind it
//! reach the chip a rotation earlier. libvgm's `fmopn.c`, which renders the
//! YM2608/YM2610 and is available for the YM2612 as `ym2612.libvgm`, applies
//! writes the instant they arrive.
//!
//! So the render gate's oracle -- "the engine renders byte-exact under write
//! removal, so a difference is a dropped write that mattered" -- holds for an
//! immediate-write core and *not* for a write-paced one. This test measures the
//! split: the same optimised files, rendered through both cores.
//!
//!   $env:VGMSTUDIO_VGMRIPS_CORPUS = 'F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17'
//!   cargo test -p vgms-app --release --test opn_write_pacing -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use vgms_core::vgm::VgmFile;
use vgms_synth::registry::{CoreChoices, with_render_choices};
use vgms_synth::vgm_engine::VgmEngine;

mod common;

const OUTPUT_RATE: u32 = 44_100;
const FRAMES: usize = 44_100 * 4;

fn render(file: &VgmFile, choices: Option<CoreChoices>) -> Vec<i16> {
    with_render_choices(choices, || {
        let mut engine = VgmEngine::new(Arc::new(file.clone()), OUTPUT_RATE);
        let mut out = vec![0i16; FRAMES * 2];
        let mut done = 0usize;
        while done < FRAMES {
            let rendered = engine.render(&mut out[done * 2..]);
            if rendered == 0 {
                break;
            }
            done += rendered;
        }
        out.truncate(done * 2);
        out
    })
}

fn differs(a: &[i16], b: &[i16]) -> bool {
    a.len() != b.len() || a.iter().zip(b).any(|(x, y)| x != y)
}

#[test]
#[ignore = "diagnostic, needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn the_ym2612_gate_failures_are_the_nuked_write_queue() {
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory"),
    );
    vgms_app::install_cores();
    let all = common::collect_songs(&root);
    let libvgm = CoreChoices::from([("ym2612".to_owned(), "libvgm".to_owned())]);

    let mut checked = 0usize;
    let mut nuked_differs = 0usize;
    let mut libvgm_differs = 0usize;

    for path in &all {
        if checked >= 24 {
            break;
        }
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(file) = vgms_core::vgm::file::read(&name, &raw) else {
            continue;
        };
        if !file.chip_list().contains("YM2612") && !file.chip_list().contains("YM3438") {
            continue;
        }
        let mut optimized = file.clone();
        if optimized.optimize().is_none() {
            continue;
        }
        checked += 1;

        let nuked = differs(&render(&file, None), &render(&optimized, None));
        let libvgm = differs(
            &render(&file, Some(libvgm.clone())),
            &render(&optimized, Some(libvgm.clone())),
        );
        nuked_differs += usize::from(nuked);
        libvgm_differs += usize::from(libvgm);
        println!("{name}: nuked differs {nuked}, libvgm differs {libvgm}");
    }

    println!(
        "\n{checked} files: Nuked-OPN2 differs on {nuked_differs}, libvgm (immediate writes) on {libvgm_differs}"
    );
    assert!(checked > 0, "no YM2612 files were reached");
}
