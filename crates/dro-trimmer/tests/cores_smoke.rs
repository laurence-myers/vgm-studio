//! The real-core end-to-end: a synthetic file, the app's own registry, and
//! audio out the other end.
//!
//! `dro-synth`'s engine tests run against a stub (that crate ships no core of
//! its own), so this is the other half: providers linked and registered exactly
//! as the app does, so a silent default -- a device missing from the libvgm
//! build table, a write rule fetching no writer, a registration order slip --
//! fails a plain `cargo test` rather than waiting for a corpus run.

use std::sync::Arc;

/// A VGM declaring `chips` with `stream` as its body.
fn vgm_file(chips: &[(dro_core::ChipKind, u32)], stream: &[u8]) -> Arc<dro_core::vgm::VgmFile> {
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x171);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    for (kind, clock) in chips {
        put_u32(&mut bytes, kind.clock_offset(), *clock);
    }
    bytes.extend_from_slice(stream);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);
    Arc::new(dro_core::vgm::file::read("smoke.vgm", &bytes).expect("a walkable VGM"))
}

/// A Master System tone through the registry's default SN76489 -- which is
/// libvgm's -- must come out audible.
#[test]
fn the_default_sn76489_is_libvgms_and_makes_sound() {
    dro_trimmer::install_cores();

    let registry = dro_synth::registry();
    let default = registry
        .default_for(dro_core::ChipKind::Sn76489)
        .expect("a default exists");
    assert_eq!(
        default.id, "sn76489.libvgm",
        "libvgm is the default provider (the 2026-07-29 decision)"
    );

    let file = vgm_file(
        &[(dro_core::ChipKind::Sn76489, 3_579_545)],
        &[
            0x50, 0x8E, 0x50, 0x0F, // tone 0, period 254
            0x50, 0x90, // full volume
            0x61, 0x44, 0xAC, // a second
            0x66,
        ],
    );
    let mut engine = dro_synth::VgmEngine::new(file, 44_100);
    let mut out = vec![0i16; 44_100 * 2];
    assert_eq!(engine.render(&mut out), 44_100);

    let peak = out.iter().copied().map(i16::abs).max().unwrap_or(0);
    assert!(peak > 1000, "audible, not silence: peak {peak}");
}

/// Every libvgm-served chip is that chip's default -- except the three the
/// owner named back to Nuked -- and every default `VgmEngine` can build
/// actually builds. OPL stays routed to `PlayerEngine`.
#[test]
fn libvgm_leads_every_chip_it_serves_and_opl_is_untouched() {
    dro_trimmer::install_cores();
    let registry = dro_synth::registry();

    // The owner's exceptions: Nuked keeps these three defaults, libvgm demoted
    // to the picker.
    let nuked_led = [
        (dro_core::ChipKind::Ym2612, "ym2612.nuked"),
        (dro_core::ChipKind::Ym2151, "ym2151.nuked"),
        (dro_core::ChipKind::Ym2413, "ym2413.nuked"),
    ];

    for chip in dro_core::ChipKind::all() {
        let Some(default) = registry.default_for(chip) else {
            continue;
        };
        if dro_synth::registry::is_opl(chip) {
            assert_eq!(
                default.id,
                "opl3.nuked",
                "{}: OPL keeps Nuked-OPL3",
                chip.name()
            );
            continue;
        }
        let libvgm_id = format!("{}.libvgm", chip.slug());
        if let Some(&(_, nuked_id)) = nuked_led.iter().find(|&&(named, _)| named == chip) {
            assert_eq!(
                default.id,
                nuked_id,
                "{}: the owner named Nuked back to this default",
                chip.name()
            );
            assert!(
                registry.find(chip, &libvgm_id).is_some(),
                "{}: libvgm stays on the picker behind Nuked",
                chip.name()
            );
        } else if registry.find(chip, &libvgm_id).is_some() {
            assert_eq!(
                default.id,
                libvgm_id,
                "{}: libvgm serves this chip and must lead it",
                chip.name()
            );
        }
        assert!(
            default.build().is_some() || !matches!(default.make, dro_synth::CoreMaker::Generic(_)),
            "{}: a generic default must actually build",
            chip.name()
        );
    }
}
