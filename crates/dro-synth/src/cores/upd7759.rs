// SPDX-License-Identifier: MIT OR Apache-2.0
//! The NEC uPD7759: a speech chip with its own idea of structure -- samples
//! are *phrases* of rate-tagged blocks, not flat streams. 240 corpus files
//! (Konami and Sega arcade boards, mostly percussion and voices).
//!
//! **Route B, from the documented behaviour.** The block format, step
//! table and state walk are as MAME's `upd7759.cpp` (BSD-3-Clause,
//! copyright Buchmueller, Balfour, Cohen, Galibert, Giles) documents them;
//! the tables below are transcriptions from that documentation, and the
//! module keeps the upstream notice per the sourcing policy.
//!
//! Two drive modes, both real in the corpus. **Slave mode** dominates (a
//! million FIFO writes across the rips): the CPU streams the block data
//! through the data port and the chip decodes as it arrives. **Master
//! mode** hands the chip a phrase number and it reads its own ROM through
//! the phrase table at the top.
//!
//! Stated approximations: the DRQ handshake timing is not modelled (the
//! FIFO is unbounded here, and a stream that outruns the real chip's
//! request pacing still decodes in order), and the repeat-block command
//! replays its span once per count without the silicon's exact fetch
//! timing.

use crate::chip::ChipCore;

/// The state machine runs at chip clock; one output frame consumes 32
/// clocks, putting the hold-DAC output at 20 kHz for the usual 640 kHz.
const CLOCKS_PER_FRAME: u32 = 32;

/// The ADPCM step matrix: 16 states by 16 nibbles, as documented.
const STEP: [[i32; 16]; 16] = [
    [0, 0, 1, 2, 3, 5, 7, 10, 0, 0, -1, -2, -3, -5, -7, -10],
    [0, 1, 2, 3, 4, 6, 8, 13, 0, -1, -2, -3, -4, -6, -8, -13],
    [0, 1, 2, 4, 5, 7, 10, 15, 0, -1, -2, -4, -5, -7, -10, -15],
    [0, 1, 3, 4, 6, 9, 13, 19, 0, -1, -3, -4, -6, -9, -13, -19],
    [0, 2, 3, 5, 8, 11, 15, 23, 0, -2, -3, -5, -8, -11, -15, -23],
    [
        0, 2, 4, 7, 10, 14, 19, 29, 0, -2, -4, -7, -10, -14, -19, -29,
    ],
    [
        0, 3, 5, 8, 12, 16, 22, 33, 0, -3, -5, -8, -12, -16, -22, -33,
    ],
    [
        1, 4, 7, 10, 15, 20, 29, 43, -1, -4, -7, -10, -15, -20, -29, -43,
    ],
    [
        1, 4, 8, 13, 18, 25, 35, 53, -1, -4, -8, -13, -18, -25, -35, -53,
    ],
    [
        1, 6, 10, 16, 22, 31, 43, 64, -1, -6, -10, -16, -22, -31, -43, -64,
    ],
    [
        2, 7, 12, 19, 27, 37, 51, 76, -2, -7, -12, -19, -27, -37, -51, -76,
    ],
    [
        2, 9, 16, 24, 34, 46, 64, 96, -2, -9, -16, -24, -34, -46, -64, -96,
    ],
    [
        3, 11, 19, 29, 41, 57, 79, 117, -3, -11, -19, -29, -41, -57, -79, -117,
    ],
    [
        4, 13, 24, 36, 50, 69, 96, 143, -4, -13, -24, -36, -50, -69, -96, -143,
    ],
    [
        4, 16, 29, 44, 62, 85, 118, 175, -4, -16, -29, -44, -62, -85, -118, -175,
    ],
    [
        6, 20, 36, 54, 76, 104, 144, 214, -6, -20, -36, -54, -76, -104, -144, -214,
    ],
];

/// How a nibble moves the state, as documented.
const STATE_ADJUST: [i32; 16] = [-1, -1, 0, 0, 1, 2, 2, 3, -1, -1, 0, 0, 1, 2, 2, 3];

/// Where the decoder is in a phrase.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// Reading the phrase's block count.
    BlockCount,
    /// Reading the next block's header.
    BlockHeader,
    /// Silence, counted in chip clocks.
    Silence,
    /// Nibbles at a rate: `sample_rate * 4` clocks apiece.
    Nibbles,
}

/// The uPD7759.
#[derive(Debug)]
pub struct Upd7759 {
    rate: u32,
    /// Master mode reads this; slave mode reads the FIFO.
    rom: Vec<u8>,
    slave: bool,
    fifo: std::collections::VecDeque<u8>,
    /// The port latch: the phrase number in master mode.
    port: u8,
    /// Master-mode read position and bank base.
    address: u32,
    bank: u32,
    reset_line: bool,
    start_line: bool,

    phase: Phase,
    blocks_left: u32,
    clocks_left: u32,
    nibbles_left: u32,
    sample_rate: u32,
    high_nibble: bool,
    current_byte: u8,
    /// Repeat-block state: where the span began, and plays remaining.
    repeat_from: u32,
    repeat_count: u32,
    /// The decoder.
    sample: i32,
    state: i32,
}

impl Upd7759 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 20_000,
            rom: Vec::new(),
            slave: false,
            fifo: std::collections::VecDeque::new(),
            port: 0,
            address: 0,
            bank: 0,
            reset_line: true,
            start_line: true,
            phase: Phase::Idle,
            blocks_left: 0,
            clocks_left: 0,
            nibbles_left: 0,
            sample_rate: 1,
            high_nibble: false,
            current_byte: 0,
            repeat_from: 0,
            repeat_count: 0,
            sample: 0,
            state: 0,
        }
    }

    /// The next data byte, from wherever this mode sources it.
    fn next_byte(&mut self) -> Option<u8> {
        if self.slave {
            self.fifo.pop_front()
        } else {
            let byte = self.rom.get((self.bank + self.address) as usize).copied();
            self.address = self.address.wrapping_add(1);
            byte
        }
    }

    /// Begins a phrase: master mode resolves the port through the phrase
    /// table; slave mode starts on the FIFO.
    fn start(&mut self) {
        if !self.slave {
            // The ROM opens with [count][5A A5 69 55]; the phrase table's
            // big-endian pointers (in 2-byte units) start at offset 5 --
            // the corpus's own ROMs said so, their signature bytes in
            // plain sight.
            let entry = (self.bank as usize) + usize::from(self.port) * 2 + 5;
            let Some(pointer) = self.rom.get(entry..entry + 2) else {
                return;
            };
            self.address = (u32::from(pointer[0]) << 9) | (u32::from(pointer[1]) << 1);
        }
        self.phase = Phase::BlockCount;
        self.repeat_count = 0;
        self.sample = 0;
        self.state = 0;
    }

    /// One chip clock of the state machine.
    fn clock(&mut self) {
        match self.phase {
            Phase::Idle => {}
            Phase::BlockCount => {
                let Some(count) = self.next_byte() else {
                    // A slave FIFO underrun: wait for more data.
                    if !self.slave {
                        self.phase = Phase::Idle;
                    }
                    return;
                };
                self.blocks_left = u32::from(count) + 1;
                self.phase = Phase::BlockHeader;
            }
            Phase::BlockHeader => {
                if self.blocks_left == 0 {
                    self.phase = Phase::Idle;
                    self.sample = 0;
                    return;
                }
                let Some(header) = self.next_byte() else {
                    if !self.slave {
                        self.phase = Phase::Idle;
                    }
                    return;
                };
                self.blocks_left -= 1;
                match header >> 6 {
                    // Silence: 1024 x (n + 1) clocks, decoder reset.
                    0 => {
                        self.clocks_left = 1024 * (u32::from(header & 0x3F) + 1);
                        self.sample = 0;
                        self.state = 0;
                        self.phase = Phase::Silence;
                    }
                    // 256 nibbles at the header's rate.
                    1 => {
                        self.sample_rate = u32::from(header & 0x3F) + 1;
                        self.nibbles_left = 256;
                        self.high_nibble = false;
                        self.clocks_left = self.sample_rate * 4;
                        self.phase = Phase::Nibbles;
                    }
                    // A counted run of nibbles.
                    2 => {
                        self.sample_rate = u32::from(header & 0x3F) + 1;
                        let count = self.next_byte().unwrap_or(0);
                        self.nibbles_left = u32::from(count) + 1;
                        self.high_nibble = false;
                        self.clocks_left = self.sample_rate * 4;
                        self.phase = Phase::Nibbles;
                    }
                    // Repeat: replay the span from here, count times.
                    _ => {
                        self.repeat_count = u32::from(header & 0x07) + 1;
                        self.repeat_from = self.address;
                    }
                }
            }
            Phase::Silence => {
                self.clocks_left = self.clocks_left.saturating_sub(1);
                if self.clocks_left == 0 {
                    self.phase = Phase::BlockHeader;
                }
            }
            Phase::Nibbles => {
                self.clocks_left = self.clocks_left.saturating_sub(1);
                if self.clocks_left != 0 {
                    return;
                }
                self.clocks_left = self.sample_rate * 4;
                let nibble = if self.high_nibble {
                    self.current_byte & 0x0F
                } else {
                    let Some(byte) = self.next_byte() else {
                        if !self.slave {
                            self.phase = Phase::Idle;
                        }
                        return;
                    };
                    self.current_byte = byte;
                    byte >> 4
                };
                self.high_nibble = !self.high_nibble;
                self.sample += STEP[self.state as usize][usize::from(nibble)];
                self.state = (self.state + STATE_ADJUST[usize::from(nibble)]).clamp(0, 15);
                self.nibbles_left -= 1;
                if self.nibbles_left == 0 {
                    // Repeat spans rewind master-mode reads; a slave stream
                    // simply carries the data again.
                    if self.repeat_count > 0 && !self.slave {
                        self.repeat_count -= 1;
                        self.address = self.repeat_from;
                    }
                    self.phase = Phase::BlockHeader;
                }
            }
        }
    }
}

impl Default for Upd7759 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Upd7759 {
    /// `variant` is the header's bit 31: slave (streamed) mode.
    fn reset(&mut self, clock: u32, variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        *self = Self {
            rate: (clock / CLOCKS_PER_FRAME).max(1),
            slave: variant,
            ..Self::new()
        };
        self.rom = rom;
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The four lines: reset, start, the data port, the bank.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let data = (data & 0xFF) as u8;
        match addr & 0x03 {
            // /RESET: low clears everything.
            0 => {
                let level = data != 0;
                if !level && self.reset_line {
                    self.phase = Phase::Idle;
                    self.fifo.clear();
                    self.sample = 0;
                    self.state = 0;
                }
                self.reset_line = level;
            }
            // /START: a falling edge begins the phrase.
            1 => {
                let level = data != 0;
                if !level && self.start_line && self.phase == Phase::Idle {
                    self.start();
                }
                self.start_line = level;
            }
            // The data port: the FIFO in slave mode, the port latch in
            // master mode.
            2 => {
                if self.slave {
                    self.fifo.push_back(data);
                    // A stream arriving into idle is the phrase starting:
                    // rips lean on the data itself rather than the start
                    // line.
                    if self.phase == Phase::Idle {
                        self.start();
                    }
                } else {
                    self.port = data;
                }
            }
            // The bank, in 128 KiB units.
            _ => self.bank = u32::from(data) * 0x20000,
        }
    }

    /// The sample ROM: block type `0x8A`.
    fn load_rom(&mut self, _block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        let total = total_size as usize;
        if self.rom.len() < total {
            self.rom.resize(total, 0);
        }
        let at = start as usize;
        let end = (at + data.len()).min(self.rom.len());
        if at < end {
            self.rom[at..end].copy_from_slice(&data[..end - at]);
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            for _ in 0..CLOCKS_PER_FRAME {
                self.clock();
            }
            // The decoder's ~9-bit range scaled to the usual headroom.
            let value = self.sample * 32;
            frame[0] = value;
            frame[1] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual 640 kHz.
    const CLOCK: u32 = 640_000;

    fn render(chip: &mut Upd7759, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// One counted block of loud rising nibbles, as a slave stream.
    fn slave_stream(chip: &mut Upd7759) {
        chip.write(0, 0, 0x01); // release reset
        let mut bytes = vec![
            0x00, // one block
            0x80, // counted nibbles, rate 1
            0x3F, // 64 nibbles
        ];
        bytes.extend(std::iter::repeat_n(0x77u8, 32));
        for byte in bytes {
            chip.write(0, 2, byte.into());
        }
    }

    #[test]
    fn a_slave_stream_decodes_and_ends() {
        let mut chip = Upd7759::new();
        chip.reset(CLOCK, true);
        assert_eq!(energy(&render(&mut chip, 100)), 0);
        slave_stream(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0, "the stream must sound");
        render(&mut chip, 2000);
        assert_eq!(
            energy(&render(&mut chip, 100)),
            0,
            "an exhausted phrase is silence"
        );
    }

    /// Master mode: the phrase table at the top of the ROM points at the
    /// block data.
    #[test]
    fn master_mode_reads_the_phrase_table() {
        let mut chip = Upd7759::new();
        chip.reset(CLOCK, false);
        let mut rom = vec![0u8; 0x400];
        rom[0] = 0x01; // phrase count
        rom[1..5].copy_from_slice(&[0x5A, 0xA5, 0x69, 0x55]); // the signature
        // Phrase 0's pointer at offset 5: address 0x100 in 2-byte units.
        rom[5] = 0x00;
        rom[6] = 0x80;
        rom[0x100] = 0x00; // one block
        rom[0x101] = 0x80; // counted nibbles, rate 1
        rom[0x102] = 0x3F; // 64 nibbles
        for byte in &mut rom[0x103..0x123] {
            *byte = 0x77;
        }
        chip.load_rom(0x8A, rom.len() as u32, 0, &rom);
        chip.write(0, 0, 0x01); // release reset
        chip.write(0, 2, 0x00); // phrase 0
        chip.write(0, 1, 0x00); // start: falling edge
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// The silence block holds the line at zero for its counted clocks.
    #[test]
    fn the_silence_block_counts_clocks() {
        let mut chip = Upd7759::new();
        chip.reset(CLOCK, true);
        chip.write(0, 0, 0x01);
        for byte in [0x01u8, 0x3F, 0x80, 0x3F] {
            chip.write(0, 2, byte.into()); // two blocks: long silence, then data
        }
        for _ in 0..32 {
            chip.write(0, 2, 0x77);
        }
        // 1024 * 64 clocks of silence = 2048 frames at 32 clocks a frame.
        assert_eq!(
            energy(&render(&mut chip, 1500)),
            0,
            "still inside the silence"
        );
        assert!(energy(&render(&mut chip, 1500)) > 0, "then the data plays");
    }
}
