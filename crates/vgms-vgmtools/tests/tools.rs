//! End-to-end tests: real files through the real tools.
//!
//! These are the crate's golden tests. Because the binding *is* the upstream
//! program, they are not checking that we reproduce `vgm_cmp` -- they pin what
//! the pinned submodule does, so a pin bump that changes behaviour shows up
//! here rather than quietly in someone's pack.

use vgms_vgmtools::{
    Options, StageOutcome, ToolOutcome, clean_dac_runs, optimize_vgm, optimize_writes,
    passthrough_chips, trim_sample_roms,
};

mod offset {
    pub(crate) const EOF: usize = 0x04;
    pub(crate) const VERSION: usize = 0x08;
    pub(crate) const TOTAL_SAMPLES: usize = 0x18;
    pub(crate) const YM2612_CLOCK: usize = 0x2C;
    pub(crate) const DATA_OFFSET: usize = 0x34;
    /// The OKIM6295 -- a chip with a sample ROM, which is what makes `vgm_sro`
    /// look at a file at all.
    pub(crate) const OKIM6295_CLOCK: usize = 0x98;
    /// The YM3812: an OPL2, so a file declaring one takes the bypass.
    pub(crate) const YM3812_CLOCK: usize = 0x50;
    pub(crate) const SAA1099_CLOCK: usize = 0xC8;
    /// QSound: a sample-ROM chip the trim is held back from.
    pub(crate) const QSOUND_CLOCK: usize = 0xB4;
    pub(crate) const YM2151_CLOCK: usize = 0x30;
}

const HEADER_LEN: usize = 0x100;

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// A VGM 1.61 file for one YM2612, carrying `stream` as its command data.
fn vgm_with(stream: &[u8], total_samples: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; HEADER_LEN];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, offset::VERSION, 0x161);
    put_u32(
        &mut bytes,
        offset::DATA_OFFSET,
        (HEADER_LEN - offset::DATA_OFFSET) as u32,
    );
    put_u32(&mut bytes, offset::YM2612_CLOCK, 7_670_454);
    put_u32(&mut bytes, offset::TOTAL_SAMPLES, total_samples);
    bytes.extend_from_slice(stream);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);
    bytes
}

/// The total delay a file's command stream spells out.
///
/// The one property every optimiser here must preserve: dropping a write that
/// changes nothing must not change when anything happens.
fn total_samples(bytes: &[u8]) -> u64 {
    let file = vgms_core::vgm::file::read("test.vgm", bytes).expect("a readable VGM");
    file.stream().expect("a walkable stream").total_samples()
}

fn smaller(outcome: ToolOutcome) -> Vec<u8> {
    match outcome {
        ToolOutcome::Smaller(bytes) => bytes,
        other => panic!("expected the file to shrink, got {other:?}"),
    }
}

#[test]
fn a_repeated_write_is_dropped_and_the_timing_survives() {
    // Register 0x22 is the YM2612's LFO control: a plain latch, so writing the
    // value it already holds is exactly the redundancy `vgm_cmp` exists to
    // remove. The delays between them must survive as one.
    let stream = [
        0x52, 0x22, 0x08, // write
        0x61, 0x10, 0x27, // wait
        0x52, 0x22, 0x08, // same value again -- droppable
        0x61, 0x20, 0x4E, // wait
        0x52, 0x22, 0x08, // and again
        0x62, // wait 735
        0x66, // end
    ];
    let original = vgm_with(&stream, 30_735);

    let optimised = smaller(optimize_writes(&original));

    assert!(
        optimised.len() < original.len(),
        "{} bytes did not beat {}",
        optimised.len(),
        original.len()
    );
    assert_eq!(
        total_samples(&optimised),
        total_samples(&original),
        "the delay total moved"
    );
}

#[test]
fn a_write_that_changes_something_is_kept() {
    // The negative control for the test above: same shape, different values,
    // so there is nothing to drop and the tool should decline.
    let stream = [
        0x52, 0x22, 0x08, //
        0x61, 0x10, 0x27, //
        0x52, 0x22, 0x09, // different value
        0x61, 0x20, 0x4E, //
        0x52, 0x22, 0x0A, // different again
        0x62, //
        0x66,
    ];
    let original = vgm_with(&stream, 30_735);

    assert_eq!(
        optimize_writes(&original),
        ToolOutcome::Unchanged,
        "nothing here is redundant"
    );
}

#[test]
fn optimising_twice_finds_nothing_the_second_time() {
    let stream = [
        0x52, 0x22, 0x08, //
        0x61, 0x10, 0x27, //
        0x52, 0x22, 0x08, //
        0x61, 0x20, 0x4E, //
        0x52, 0x22, 0x08, //
        0x62, //
        0x66,
    ];
    let once = smaller(optimize_writes(&vgm_with(&stream, 30_735)));
    assert_eq!(
        optimize_writes(&once),
        ToolOutcome::Unchanged,
        "the first pass should have left nothing to find"
    );
}

#[test]
fn a_long_run_of_identical_dac_writes_is_collapsed() {
    // optdac only fires at 128 consecutive identical writes to port 0 register
    // 0x2A, so 200 of them is the shape it is looking for; each carries a
    // one-sample wait via the `0x7n` form.
    let mut stream = Vec::new();
    for _ in 0..200 {
        stream.extend_from_slice(&[0x52, 0x2A, 0x80]);
        stream.push(0x70); // wait 1
    }
    stream.push(0x66);
    let original = vgm_with(&stream, 200);

    let cleaned = smaller(clean_dac_runs(&original));

    assert!(cleaned.len() < original.len());
    assert_eq!(
        total_samples(&cleaned),
        total_samples(&original),
        "collapsing the run must not lose its time"
    );
}

#[test]
fn a_file_with_no_sample_rom_is_left_alone() {
    // vgm_sro exits 2 -- "No chips with Sample-ROM used!" -- for any file
    // whose header declares no ROM-bearing chip, which is most of them. That
    // is a refusal, not a fault, and must not reach a caller as one.
    let stream = [0x52, 0x22, 0x08, 0x62, 0x66];
    let outcome = trim_sample_roms(&vgm_with(&stream, 735));
    assert_eq!(outcome, ToolOutcome::Unchanged);
}

#[test]
fn a_stream_vgm_sro_refuses_still_reads_as_untouched() {
    // `0x68` is a PCM RAM write, which vgm_sro says outright it cannot
    // optimise; it cancels and exits 9. The file is untouched and still
    // valid, so the answer is Unchanged -- the other half of the exit-code
    // mapping, and the one a pack export would otherwise fill its log with.
    let mut bytes = vgm_with(&[0x66], 0);
    // Declare an OKIM6295 so the tool gets past its "no sample ROM" check and
    // actually walks the stream.
    put_u32(&mut bytes, offset::OKIM6295_CLOCK, 1_000_000);
    put_u32(&mut bytes, offset::VERSION, 0x161);
    let mut bytes = bytes[..HEADER_LEN].to_vec();
    bytes.extend_from_slice(&[
        0x68, 0x66, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, // PCM RAM write
        0x66,
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

    assert_eq!(trim_sample_roms(&bytes), ToolOutcome::Unchanged);
}

/// The stage names a pass actually ran, paired with what became of each.
fn stage_names(bytes: &[u8], options: Options) -> Vec<(&'static str, StageOutcome)> {
    optimize_vgm(bytes, options)
        .stages
        .into_iter()
        .map(|stage| (stage.name, stage.outcome))
        .collect()
}

#[test]
fn the_pass_shrinks_a_redundant_file_and_keeps_its_timing() {
    let stream = [
        0x52, 0x22, 0x08, //
        0x61, 0x10, 0x27, //
        0x52, 0x22, 0x08, //
        0x61, 0x20, 0x4E, //
        0x52, 0x22, 0x08, //
        0x62, //
        0x66,
    ];
    let original = vgm_with(&stream, 30_735);

    let result = optimize_vgm(&original, Options::default());

    assert!(result.changed(), "stages: {:?}", result.stages);
    assert_eq!(result.saved(), original.len() - result.bytes.len());
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(
        total_samples(&result.bytes),
        total_samples(&original),
        "the delay total moved"
    );
}

#[test]
fn an_opl_file_reaches_only_the_built_in() {
    // vgms_core has covered the OPL family from the start and its output is
    // pinned byte-for-byte over the corpus. Running the C tools over an OPL
    // file would re-spell it through a second implementation for no gain, so
    // the pass must not. Every tool stage is still *reported*, each with the
    // reason it had nothing to do -- no YM2612, no sample ROM, and a write
    // dedup the built-in already covers.
    let mut bytes = vec![0u8; HEADER_LEN];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, offset::VERSION, 0x161);
    put_u32(
        &mut bytes,
        offset::DATA_OFFSET,
        (HEADER_LEN - offset::DATA_OFFSET) as u32,
    );
    put_u32(&mut bytes, offset::YM3812_CLOCK, 3_579_545);
    bytes.extend_from_slice(&[
        0x5A, 0x20, 0x01, // OPL2 write
        0x5A, 0x20, 0x01, // the same again
        0x62, //
        0x66,
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

    let stages = stage_names(&bytes, Options::default());

    assert_eq!(
        stages.iter().map(|(name, _)| *name).collect::<Vec<&str>>(),
        vec!["optdac", "vgm_sro", "vgm_cmp", "built-in"],
    );
    for (name, outcome) in &stages[..3] {
        assert!(
            matches!(outcome, StageOutcome::Skipped(_)),
            "{name} should not have run on an OPL file: {outcome:?}"
        );
    }
    assert!(matches!(stages[2].1, StageOutcome::Skipped(reason) if reason.contains("built-in")));
}

#[test]
fn a_file_naming_an_saa1099_is_held_back_from_vgm_cmp() {
    // vgm_cmp.c:537 is missing a `break`, so SAA1099 writes are judged by the
    // YM2413's rules -- which dedupe every register, including the SAA1099's
    // envelope registers, where a repeated write is a retrigger rather than a
    // latch. So the file goes through untouched.
    //
    // Under `Auto` the built-in's own SAA1099 rule takes the file and vgm_cmp
    // never sees it; the hold-back is what stands behind the `Tools` A/B
    // control, so that is what this asks for.
    let mut bytes = vgm_with(&[0x66], 0)[..HEADER_LEN].to_vec();
    put_u32(&mut bytes, offset::SAA1099_CLOCK, 8_000_000);
    put_u32(&mut bytes, offset::VERSION, 0x171);
    bytes.extend_from_slice(&[0xBD, 0x18, 0x0F, 0xBD, 0x18, 0x0F, 0x62, 0x66]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

    let options = Options {
        optimizer: vgms_core::config::OptimizerChoice::Tools,
        ..Options::default()
    };
    let stages = stage_names(&bytes, options);
    let vgm_cmp = stages
        .iter()
        .find(|(name, _)| *name == "vgm_cmp")
        .expect("vgm_cmp should still be reported");

    match &vgm_cmp.1 {
        StageOutcome::Skipped(reason) => assert!(
            reason.contains("SAA1099"),
            "the reason should name the chip: {reason}"
        ),
        other => panic!("vgm_cmp should have been held back, got {other:?}"),
    }
}

#[test]
fn the_sample_rom_trim_runs_before_the_write_dedup() {
    // The VGMRips wiki's order. vgm_sro reads the sample ROM out of the write
    // history, using chip models that are not vgm_cmp's, so it must see the
    // file's own writes rather than what another tool left of them.
    let bytes = vgm_with(&[0x52, 0x22, 0x08, 0x62, 0x66], 735);
    let names: Vec<&str> = stage_names(&bytes, Options::default())
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    let sro = names.iter().position(|name| *name == "vgm_sro");
    let cmp = names.iter().position(|name| *name == "vgm_cmp");
    assert!(sro < cmp, "vgm_sro should come first, got {names:?}");
    assert_eq!(names.last(), Some(&"built-in"), "{names:?}");
}

#[test]
fn a_chip_the_rom_trim_gets_wrong_is_held_back_from_it() {
    // QSound: measured to change what 12 of 23 corpus files play. The K053260
    // and SegaPCM are held back on upstream's own warnings.
    let mut bytes = vgm_with(&[0x66], 0)[..HEADER_LEN].to_vec();
    put_u32(&mut bytes, offset::QSOUND_CLOCK, 4_000_000);
    put_u32(&mut bytes, offset::VERSION, 0x161);
    // A sample ROM for the trim to have designs on, so the hold-back is what
    // stops it rather than there being nothing to do.
    let mut payload = 0x0006_0000u32.to_le_bytes().to_vec(); // total ROM size
    payload.extend_from_slice(&0u32.to_le_bytes()); // start address
    payload.extend_from_slice(&[0xAB; 4]);
    bytes.extend_from_slice(&[0x67, 0x66, 0x8F]); // QSound ROM image
    bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&[0x66]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

    let stages = stage_names(&bytes, Options::default());
    let sro = stages
        .iter()
        .find(|(name, _)| *name == "vgm_sro")
        .expect("the stage should still be reported");

    match &sro.1 {
        StageOutcome::Skipped(reason) => assert!(
            reason.contains("QSound"),
            "the reason should name the chip: {reason}"
        ),
        other => panic!("vgm_sro should have been held back, got {other:?}"),
    }
}

#[test]
fn turning_a_stage_off_keeps_it_out_of_the_pass() {
    let stream = [0x52, 0x22, 0x08, 0x62, 0x66];
    let bytes = vgm_with(&stream, 735);

    let options = Options {
        sample_roms: false,
        dac_runs: false,
        // Force the tools so the stage list is the tools', not the built-in path.
        optimizer: vgms_core::config::OptimizerChoice::Tools,
        ..Default::default()
    };
    let names: Vec<&str> = stage_names(&bytes, options)
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    assert!(!names.contains(&"vgm_sro"), "{names:?}");
    assert!(!names.contains(&"optdac"), "{names:?}");
    assert!(names.contains(&"vgm_cmp"), "write dedup is not optional");
}

#[test]
fn the_passthrough_list_names_only_chips_vgm_cmp_has_no_rules_for() {
    // A guard against the list drifting into a claim that is not true: the
    // SAA1099 must never appear here, because vgm_cmp does touch it -- just
    // with the wrong chip's rules.
    let chips = passthrough_chips();
    assert!(!chips.is_empty());
    assert!(
        !chips.contains(&vgms_core::vgm::ChipKind::Saa1099),
        "the SAA1099 is processed, not passed through"
    );
    assert!(chips.contains(&vgms_core::vgm::ChipKind::K053260));
}

/// A rip that declares two chips and only ever writes to one.
fn vgm_declaring_an_unused_chip() -> Vec<u8> {
    let mut bytes = vec![0u8; HEADER_LEN];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, offset::VERSION, 0x161);
    put_u32(
        &mut bytes,
        offset::DATA_OFFSET,
        (HEADER_LEN - offset::DATA_OFFSET) as u32,
    );
    put_u32(&mut bytes, offset::YM2612_CLOCK, 7_670_454);
    // Declared, never written to -- the whole point.
    put_u32(&mut bytes, offset::YM2151_CLOCK, 3_579_545);
    bytes.extend_from_slice(&[0x52, 0x22, 0x08, 0x62, 0x66]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);
    bytes
}

#[test]
fn a_declared_chip_that_is_never_written_to_is_spotted() {
    let bytes = vgm_declaring_an_unused_chip();
    assert_eq!(
        vgms_vgmtools::unused_chips(&bytes),
        vec![vgms_core::ChipKind::Ym2151]
    );
}

#[test]
fn a_chip_that_is_written_to_is_not_spotted() {
    // The negative control. Both chips get a write, so neither is unused.
    let mut bytes = vgm_declaring_an_unused_chip()[..HEADER_LEN].to_vec();
    bytes.extend_from_slice(&[
        0x52, 0x22, 0x08, // YM2612
        0x54, 0x20, 0x01, // YM2151
        0x62, 0x66,
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

    assert!(vgms_vgmtools::unused_chips(&bytes).is_empty());
}

#[test]
fn a_file_carrying_data_blocks_is_not_analysed_at_all() {
    // A sample ROM handed to a chip is a use of it that no register write need
    // record, so the conservative answer is to say nothing rather than to
    // guess a chip is idle and strip it.
    let mut bytes = vgm_declaring_an_unused_chip()[..HEADER_LEN].to_vec();
    bytes.extend_from_slice(&[
        0x67, 0x66, 0x00, 0x02, 0x00, 0x00, 0x00, 0xAA, 0xBB, // data block
        0x52, 0x22, 0x08, //
        0x62, 0x66,
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

    assert!(
        vgms_vgmtools::unused_chips(&bytes).is_empty(),
        "a file with data blocks should be left alone"
    );
}

#[test]
fn stripping_removes_the_chip_the_stream_never_writes_to() {
    let original = vgm_declaring_an_unused_chip();
    assert_eq!(
        vgms_core::vgm::file::read("x.vgm", &original)
            .unwrap()
            .chip_list(),
        "YM2612, YM2151"
    );

    let stripped = match vgms_vgmtools::strip_unused_chips(&original) {
        ToolOutcome::Smaller(bytes) => bytes,
        other => panic!("expected the unused chip to go, got {other:?}"),
    };

    let reread = vgms_core::vgm::file::read("x.vgm", &stripped).unwrap();
    assert_eq!(reread.chip_list(), "YM2612", "the YM2151 should be gone");
    assert_eq!(
        total_samples(&stripped),
        total_samples(&original),
        "stripping a chip must not touch the music's timing"
    );
}

#[test]
fn stripping_a_file_with_nothing_unused_leaves_it_alone() {
    let stream = [0x52, 0x22, 0x08, 0x62, 0x66];
    assert_eq!(
        vgms_vgmtools::strip_unused_chips(&vgm_with(&stream, 735)),
        ToolOutcome::Unchanged
    );
}

#[test]
fn the_tools_run_one_after_another_without_interfering() {
    // The tools are children, so state cannot carry between runs -- but this
    // is the property the whole design rests on, so it gets a test rather than
    // an argument.
    let stream = [
        0x52, 0x22, 0x08, //
        0x61, 0x10, 0x27, //
        0x52, 0x22, 0x08, //
        0x62, //
        0x66,
    ];
    let original = vgm_with(&stream, 10_735);

    let first = optimize_writes(&original);
    let _ = trim_sample_roms(&original);
    let _ = clean_dac_runs(&original);
    let again = optimize_writes(&original);

    assert_eq!(first, again, "the same input gave two different answers");
}
