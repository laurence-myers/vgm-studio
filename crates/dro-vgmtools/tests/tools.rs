//! End-to-end tests: real files through the real tools.
//!
//! These are the crate's golden tests. Because the binding *is* the upstream
//! program, they are not checking that we reproduce `vgm_cmp` -- they pin what
//! the pinned submodule does, so a pin bump that changes behaviour shows up
//! here rather than quietly in someone's pack.

use dro_vgmtools::{ToolOutcome, clean_dac_runs, optimize_writes, trim_sample_roms};

mod offset {
    pub(crate) const EOF: usize = 0x04;
    pub(crate) const VERSION: usize = 0x08;
    pub(crate) const TOTAL_SAMPLES: usize = 0x18;
    pub(crate) const YM2612_CLOCK: usize = 0x2C;
    pub(crate) const DATA_OFFSET: usize = 0x34;
    /// The OKIM6295 -- a chip with a sample ROM, which is what makes `vgm_sro`
    /// look at a file at all.
    pub(crate) const OKIM6295_CLOCK: usize = 0x98;
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
    let file = dro_core::vgm::file::read("test.vgm", bytes).expect("a readable VGM");
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
