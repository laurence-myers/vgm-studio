//! Prints the [`vgms_synth::speed::BASELINE`] table, freshly measured.
//!
//! Not a test of anything -- a harness for re-measuring the reference
//! machine when cores are added or the machine changes:
//!
//! ```text
//! cargo test --release -p vgms-app -- --ignored core_speed_baseline --nocapture
//! ```
//!
//! Paste the printed rows into `crates/vgms-synth/src/speed.rs`. **Release
//! only**: a debug build measures the compiler, not the cores.

use vgms_core::vgm::ChipKind;

/// The rows worth a number, at their chips' usual clocks: every die sim, and
/// the emulators that share a picker with one.
const MEASURED: &[(&str, ChipKind, u32)] = &[
    ("opl3.nuked", ChipKind::Ymf262, 14_318_180),
    ("opl3.cqm", ChipKind::Ymf262, 14_318_180),
    ("opl3.opl2-lite", ChipKind::Ym3812, 3_579_545),
    ("opl3.ym3812-lle", ChipKind::Ym3812, 3_579_545),
    ("opl3.ymf262-lle", ChipKind::Ymf262, 14_318_180),
    ("ym2151.nuked", ChipKind::Ym2151, 3_579_545),
    ("ym2151.lle", ChipKind::Ym2151, 3_579_545),
    ("ym2612.nuked", ChipKind::Ym2612, 7_670_453),
    ("ym2612.lle", ChipKind::Ym2612, 7_670_453),
    ("ym2612.ymf276-lle", ChipKind::Ym2612, 7_670_453),
    ("ym2608.lle", ChipKind::Ym2608, 7_987_200),
    ("ym2203.lle", ChipKind::Ym2203, 3_993_600),
    ("ym2413.nuked", ChipKind::Ym2413, 3_579_545),
    ("sn76489.nuked-psg", ChipKind::Sn76489, 3_579_545),
];

#[test]
#[ignore = "a measurement harness, not a test; run --release --nocapture"]
fn core_speed_baseline() {
    vgms_app::install_cores();
    let registry = vgms_synth::registry();
    println!("pub const BASELINE: &[(&str, f32)] = &[");
    for &(id, chip, clock) in MEASURED {
        let Some(info) = registry.find(chip, id) else {
            println!("    // {id}: not registered in this build");
            continue;
        };
        let Some(speed) = vgms_synth::speed::measure_speed(info, clock) else {
            println!("    // {id}: not buildable");
            continue;
        };
        // Two significant-ish digits: these are estimates, not benchmarks.
        let rounded = if speed >= 10.0 {
            (speed / 10.0).round() * 10.0
        } else {
            (speed * 100.0).round() / 100.0
        };
        println!("    (\"{id}\", {rounded:.2}),");
    }
    println!("];");
}
