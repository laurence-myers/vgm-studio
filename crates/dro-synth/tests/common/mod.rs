//! Shared helpers: the DRO fixture's register script, and how to render it.

#![allow(dead_code)] // Each integration test uses a different subset.

use dro_core::{DroDataV2, DroInstruction, SongData};
use dro_synth::OplChip;

/// `tests/lsl3_score_up_dro2.dro`, the fixture the Python suite uses.
const FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up_dro2.dro");

/// The fixture's own length, from its header.
pub(crate) const FIXTURE_MS: u32 = 2683;

/// An OPL3 coda appended to the (OPL2-only) fixture, so the scripts also cover
/// high-bank writes and the OPL3-mode-enable register.
pub(crate) const OPL3_TAIL: &[Op] = &[
    Op::Write(0x105, 0x01), // OPL3 mode enable: high bank, register 0x05
    Op::Write(0x120, 0x21),
    Op::Write(0x140, 0x00),
    Op::Write(0x160, 0xF0),
    Op::Write(0x180, 0x77),
    Op::Write(0x1A0, 0x98),
    Op::Write(0x1B0, 0x31), // key on
    Op::Delay(200),
    Op::Write(0x1B0, 0x11), // key off
    Op::Delay(100),
];

pub(crate) const TAIL_MS: u32 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Write(u16, u8),
    Delay(u32),
}

/// Parses just enough of the DRO v2 container to reach the instruction stream.
///
/// Step 2 replaces the header parsing with `dro_core`'s real reader; decoding the
/// stream itself already goes through [`DroDataV2`], so a regression in codemap
/// handling, bank extraction or delay arithmetic shows up here.
pub(crate) fn decode_fixture() -> Vec<Op> {
    assert_eq!(&FIXTURE[..8], b"DBRAWOPL");
    assert_eq!(
        u16::from_le_bytes([FIXTURE[8], FIXTURE[9]]),
        2,
        "major version"
    );

    let length_pairs = u32::from_le_bytes(FIXTURE[12..16].try_into().unwrap()) as usize;
    let length_ms = u32::from_le_bytes(FIXTURE[16..20].try_into().unwrap());
    let short_delay_code = FIXTURE[23];
    let long_delay_code = FIXTURE[24];
    let codemap_length = usize::from(FIXTURE[25]);
    let codemap = FIXTURE[26..26 + codemap_length].to_vec();
    let data = FIXTURE[26 + codemap_length..26 + codemap_length + length_pairs * 2].to_vec();

    assert_eq!(length_ms, FIXTURE_MS);
    assert_eq!(length_pairs, 299);

    let data = SongData::V2(
        DroDataV2::new(data, codemap, short_delay_code, long_delay_code)
            .expect("the fixture is a valid DRO v2 file"),
    );

    let ops: Vec<Op> = data
        .iter()
        .map(|instruction| match instruction {
            DroInstruction::DelayMs { ms, .. } => Op::Delay(ms),
            DroInstruction::Register { reg, value, bank } => Op::Write(
                bank.expect("v2 register writes carry a bank")
                    .register_offset()
                    | u16::from(reg),
                value,
            ),
            DroInstruction::BankSwitch(_) | DroInstruction::DelaySamples { .. } => {
                unreachable!("DRO v2 has neither bank switches nor sample delays")
            }
        })
        .collect();

    assert_eq!(ops.len(), 299);
    let summed: u32 = ops.iter().map(Op::delay_ms).sum();
    assert_eq!(summed, FIXTURE_MS, "summed delays must match the header");
    ops
}

impl Op {
    pub(crate) fn delay_ms(&self) -> u32 {
        match self {
            Self::Delay(ms) => *ms,
            Self::Write(..) => 0,
        }
    }
}

/// The fixture's script, plus the OPL3 coda.
pub(crate) fn script() -> Vec<Op> {
    let mut ops = decode_fixture();
    ops.extend_from_slice(OPL3_TAIL);
    ops
}

/// Milliseconds to frames, carrying the fractional remainder exactly.
///
/// This is `OPLStream.sample_overflow`, done in integers so that native and wasm
/// cannot diverge.
pub(crate) struct FrameClock {
    rate: u64,
    carry: u64,
}

impl FrameClock {
    pub(crate) fn new(rate: u32) -> Self {
        Self {
            rate: u64::from(rate),
            carry: 0,
        }
    }

    pub(crate) fn frames_for_ms(&mut self, ms: u32) -> usize {
        let numerator = u64::from(ms) * self.rate + self.carry;
        self.carry = numerator % 1000;
        usize::try_from(numerator / 1000).expect("frame counts fit in usize")
    }
}

/// Renders `ops` through `chip`, pulling `chunk_frames` at a time.
pub(crate) fn render(
    chip: &mut impl OplChip,
    sample_rate: u32,
    ops: &[Op],
    chunk_frames: usize,
) -> Vec<i16> {
    assert!(chunk_frames > 0);
    let mut clock = FrameClock::new(sample_rate);
    let mut pcm = Vec::new();
    let mut scratch = vec![0i16; chunk_frames * 2];

    for op in ops {
        match *op {
            Op::Write(reg, value) => chip.write_reg(reg, value),
            Op::Delay(ms) => {
                let mut remaining = clock.frames_for_ms(ms);
                while remaining > 0 {
                    let frames = remaining.min(chunk_frames);
                    let buffer = &mut scratch[..frames * 2];
                    chip.generate_samples(buffer);
                    pcm.extend_from_slice(buffer);
                    remaining -= frames;
                }
            }
        }
    }
    pcm
}
