//! Hostile-file corpus: a downloaded `.vgm` or `.vgz` must never crash the app.
//!
//! Each payload is a shape that could once panic, wrap a pointer on wasm32, or
//! reserve unbounded memory (Stage A of the 2026-08 review remediation). The
//! property proved here is *"returns an error, or opens harmlessly -- but never
//! panics"*, proved for the any-chip reader (`vgm::file::read`), the one file-open
//! path there is now.
//!
//! The narrow, single-function crash sites (a >32-bit compression width, an
//! absurd DAC stream rate, the gunzip ceiling) are proved next to their code;
//! this file covers the whole file-open path.

use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

use vgms_core::vgm::file;

const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

// Standard VGM header field offsets (spec-stable), so the test needs no access
// to the crate's private `offset` module.
const EOF: usize = 0x04;
const GD3: usize = 0x14;
const LOOP_OFFSET: usize = 0x1C;
const DATA_OFFSET: usize = 0x34;
const EXTRA_HEADER: usize = 0xBC;

/// What a reader is allowed to do with a payload.
#[derive(Clone, Copy)]
enum Expect {
    /// Must be refused outright.
    Rejected,
    /// May open or may reject -- only "does not panic" is required.
    OpensOrRejects,
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// The fixture with a 32-bit field overwritten -- used to plant absurd pointers.
fn with_field(field: usize, value: u32) -> Vec<u8> {
    let mut bytes = VGM_FIXTURE.to_vec();
    bytes[field..field + 4].copy_from_slice(&value.to_le_bytes());
    bytes
}

#[test]
fn no_hostile_file_crashes_either_reader() {
    let corpus: Vec<(&str, Vec<u8>, Expect)> = vec![
        ("empty", Vec::new(), Expect::Rejected),
        (
            "junk magic",
            b"XXXX not a vgm at all".to_vec(),
            Expect::Rejected,
        ),
        (
            "gzip magic then garbage",
            vec![0x1F, 0x8B, 0x08, 0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0x00],
            Expect::Rejected,
        ),
        (
            "truncated before the data offset",
            VGM_FIXTURE[..0x30].to_vec(),
            Expect::Rejected,
        ),
        (
            "data offset past the end",
            with_field(DATA_OFFSET, u32::MAX),
            Expect::Rejected,
        ),
        (
            "gd3 pointer near 4 GiB",
            with_field(GD3, u32::MAX),
            Expect::OpensOrRejects,
        ),
        (
            "eof pointer near 4 GiB",
            with_field(EOF, u32::MAX),
            Expect::OpensOrRejects,
        ),
        (
            "loop offset near 4 GiB",
            with_field(LOOP_OFFSET, u32::MAX),
            Expect::OpensOrRejects,
        ),
        (
            "extra-header pointer near 4 GiB",
            with_field(EXTRA_HEADER, u32::MAX),
            Expect::OpensOrRejects,
        ),
        (
            "valid vgz, so the happy path still opens",
            gzip(VGM_FIXTURE),
            Expect::OpensOrRejects,
        ),
    ];

    for (name, bytes, expect) in corpus {
        // The call completing at all is the no-panic proof.
        let any = file::read(name, &bytes);
        if let Expect::Rejected = expect {
            assert!(any.is_err(), "{name}: any-chip reader should reject it");
        }
    }
}

#[test]
fn the_clean_fixture_still_opens() {
    // A guard against a corpus entry that accidentally rejects everything.
    assert!(file::read("clean.vgm", VGM_FIXTURE).is_ok());
    assert!(file::read("clean.vgz", &gzip(VGM_FIXTURE)).is_ok());
}
