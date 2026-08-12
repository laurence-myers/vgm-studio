//! The `0x90`–`0x95` DAC stream engine: playing a data bank at a chip.
//!
//! A VGM can hand a chip a stream of bytes to be written to one register at a
//! fixed rate, instead of spelling out a write and a wait per byte. It is how a
//! Mega Drive rip carries its samples without a command every 1/16000th of a
//! second. The setup command names the chip, the port and the register; the
//! rate names *commands* per second.
//!
//! The stream may be chip-agnostic in spirit, but the wire format is not: each
//! chip has its own idea of what one stream command is. This module mirrors
//! libvgm's `dac_control.c` (`daccontrol_SendCommand` and friends), which the
//! reference player routes every stream through:
//!
//! * most chips: one data byte, one register write;
//! * the 32X PWM: **two** data bytes forming one 12-bit write;
//! * the SN76496: two bytes for a frequency (two bus writes), one for a volume;
//! * the OKIM6295: a sample start is the sample id **then** a channel-mask
//!   write, a stop is a channel-mask write alone;
//! * the RF5C68/RF5C164 and HuC6280: a channel-select write, the data write,
//!   and (HuC6280 only) a restore of the previously selected channel;
//! * the QSound: two bytes forming one 16-bit register write.
//!
//! One command consumes `command size x step size` bytes, so a 12-bit PWM
//! stream advances two bytes per tick where a YM2612 stream advances one.
//!
//! The six commands:
//!
//! | Opcode | Meaning |
//! |--------|---------|
//! | `0x90` | set up stream *n*: which chip, which port, which register |
//! | `0x91` | bind it to a data-bank type, with a step size and offset |
//! | `0x92` | set its rate, in commands per second |
//! | `0x93` | start at an offset, with a length mode (see [`Stream::start`]) |
//! | `0x94` | stop |
//! | `0x95` | start the *n*th block of the bound bank type -- the fast form |

use vgms_core::vgm::ChipKind;

/// Where a stream's bytes are written, once `0x90` has said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTarget {
    pub kind: ChipKind,
    /// Which instance of that chip: bit 7 of the chip-id byte.
    pub instance: u8,
    /// The `pp` byte: the write port for two-port chips, the channel for the
    /// chips whose commands name one (OKIM6295, RF5C68, HuC6280).
    pub port: u8,
    /// The `cc` byte: the register (or per-chip command encoding) each stream
    /// command is written against.
    pub register: u8,
}

/// One of the up to 256 streams a file can define.
#[derive(Debug, Clone, Default)]
struct Stream {
    target: Option<StreamTarget>,
    /// The data-bank type `0x91` bound, already normalised to its uncompressed
    /// number so a compressed bank is found by the same key.
    bank_type: u8,
    /// How far apart consecutive commands' bytes are, in units of the command
    /// size. `0` means "the spec's default", a step of one.
    step_size: u8,
    /// Which step within each group is the one played.
    step_base: u8,
    /// Data bytes one command consumes: 2 for the PWM, the QSound and an
    /// SN76496 frequency stream, 1 for everything else. Upstream's `CmdSize`.
    cmd_size: u8,
    /// Commands per second.
    hz: u32,
    /// The bank being played, copied at start: a later block must not change
    /// what a running stream is playing.
    data: Vec<u8>,
    /// First byte of command 0, `DataPos + cmd_size * step_base` clamped to
    /// the bank. Upstream's `DataStart`.
    data_start: usize,
    /// How many commands one pass plays. Upstream's `CmdsToSend`.
    commands_total: u32,
    /// Commands left in this pass. Upstream's `RemainCmds`.
    remaining: u32,
    /// The command about to play, an index scaled by the data step.
    cmd_index: u32,
    /// Play the pass backwards (`0x93`/`0x95` flag bit 4).
    reverse: bool,
    looping: bool,
    playing: bool,
    /// Fractional time carried between output frames, in output-rate units.
    accumulator: u64,
}

/// A register write a stream wants performed, right now, in the engine's
/// normalised `(port, addr, data)` form -- the same shape the command decoder
/// produces, so it takes the same per-chip write rules on the way to a core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingWrite {
    pub target: StreamTarget,
    /// The port of *this* write, which is not always the setup port: a PWM
    /// pair or an SN76496 latch write goes to port 0 whatever `pp` said.
    pub port: u8,
    pub addr: u16,
    pub value: u16,
}

/// How many times the output rate a stream may exceed before its rate is
/// clamped. A real DAC is at most a small multiple of 44.1 kHz; this leaves wide
/// headroom while bounding a corrupt `0x92` to at most this many writes a frame.
const STREAM_RATE_CEILING_FACTOR: u32 = 64;

/// Every stream a file has defined, and the clock they run against.
#[derive(Debug, Clone)]
pub struct DacStreams {
    streams: Vec<Stream>,
    /// The ids currently playing, ascending. Kept because
    /// [`advance_frame`](Self::advance_frame) runs once per *output frame* and
    /// almost every file has none playing at all: walking 256 slots per sample
    /// to find that out is the difference between an engine that keeps up and
    /// one that does not.
    active: Vec<u8>,
    output_rate: u32,
}

impl DacStreams {
    /// Streams rendered against an output of `output_rate` Hz.
    #[must_use]
    pub fn new(output_rate: u32) -> Self {
        Self {
            // 256 ids, and a file may use any of them; allocating them up front
            // is 256 small structs once, against a lookup on every byte.
            streams: vec![Stream::default(); 256],
            active: Vec::new(),
            output_rate: output_rate.max(1),
        }
    }

    /// Forgets every stream, as a rewind does.
    pub fn clear(&mut self) {
        for stream in &mut self.streams {
            *stream = Stream::default();
        }
        self.active.clear();
    }

    /// Whether any stream is playing -- the engine's cheap "is there anything
    /// to service this frame" test.
    #[must_use]
    pub fn any_playing(&self) -> bool {
        !self.active.is_empty()
    }

    /// Adds or removes `id` from the playing list, keeping it sorted.
    fn set_active(&mut self, id: u8, playing: bool) {
        match (self.active.binary_search(&id), playing) {
            (Err(at), true) => self.active.insert(at, id),
            (Ok(at), false) => {
                self.active.remove(at);
            }
            _ => {}
        }
    }

    /// `0x90 ss tt pp cc` — stream `id` writes to chip `chip_id`'s register
    /// `register` on `port`.
    ///
    /// Bit 7 of `chip_id` selects the chip's second instance, the same rule the
    /// rest of the format uses. An unknown chip id leaves the stream unset,
    /// which makes every later command on it a no-op rather than a misroute.
    pub fn setup(&mut self, id: u8, chip_id: u8, port: u8, register: u8) {
        let Some(kind) = ChipKind::from_id(chip_id & 0x7F) else {
            return;
        };
        let stream = &mut self.streams[id as usize];
        stream.target = Some(StreamTarget {
            kind,
            instance: u8::from(chip_id & 0x80 != 0),
            port,
            register,
        });
        // Upstream's `daccontrol_setup_chip` command sizing.
        stream.cmd_size = match kind {
            // Volume writes carry one nibble; frequency writes carry ten bits.
            ChipKind::Sn76489 => {
                if register & 0x10 != 0 {
                    1
                } else {
                    2
                }
            }
            ChipKind::Pwm | ChipKind::QSound => 2,
            _ => 1,
        };
    }

    /// `0x91 ss tt ll bb` — bind stream `id` to bank type `bank_type`, stepping
    /// `step_size` commands and taking the one at `step_base` within each group.
    pub fn bind(&mut self, id: u8, bank_type: u8, step_size: u8, step_base: u8) {
        let stream = &mut self.streams[id as usize];
        stream.bank_type = crate::banks::BlockKind::uncompressed_type(bank_type);
        stream.step_size = step_size;
        stream.step_base = step_base;
    }

    /// `0x92 ss dddddddd` — stream `id` plays `hz` commands per second.
    ///
    /// A stream faster than the output legitimately emits several commands a
    /// frame, which the accumulator handles. A corrupt `0x92` claiming billions
    /// of hertz would emit tens of thousands a frame and wedge the render, so
    /// the rate is clamped to a generous multiple of the output rate -- far
    /// above any real DAC, still O(1) work per frame.
    pub fn set_rate(&mut self, id: u8, hz: u32) {
        let ceiling = self.output_rate.saturating_mul(STREAM_RATE_CEILING_FACTOR);
        self.streams[id as usize].hz = hz.min(ceiling);
    }

    /// The bank type stream `id` is bound to, so the caller can fetch its data.
    #[must_use]
    pub fn bank_type(&self, id: u8) -> u8 {
        self.streams[id as usize].bank_type
    }

    /// One command's stride through the data, `cmd_size x step_size` --
    /// upstream's `DataStep`.
    fn data_step(stream: &Stream) -> usize {
        usize::from(stream.cmd_size.max(1)) * usize::from(stream.step_size.max(1))
    }

    /// `0x93 ss aaaaaaaa mm llllllll` — play `data` from `offset`, and the
    /// `0x95` fast form through the same path (mode `0x04`, a byte count).
    ///
    /// `mode`'s low nibble is the length mode, exactly upstream's `DCTRL_LMODE`:
    /// `0` keeps the previously set length, `1` counts commands, `2` is
    /// milliseconds at the stream's rate, `3` plays to the end of the bank, `4`
    /// counts raw bytes. Bit 4 plays the pass backwards; bit 7 loops it. An
    /// `offset` of `0xFFFFFFFF` keeps the previous start.
    ///
    /// `data` is the whole bound bank; the stream copies it so a data block
    /// arriving mid-playback cannot change what is being played. Starting with
    /// no rate set is legal: the stream arms and begins when `0x92` arrives
    /// (upstream's `Running` flag works the same way).
    pub fn start(&mut self, id: u8, data: &[u8], offset: u32, mode: u8, length: u32) {
        let stream = &mut self.streams[id as usize];
        let Some(_) = stream.target else {
            return;
        };
        stream.data = data.to_vec();
        let data_step = Self::data_step(stream);
        let cmd_step_base = usize::from(stream.cmd_size.max(1)) * usize::from(stream.step_base);

        if offset != u32::MAX {
            // Bad values force silence rather than a wild read, as upstream.
            stream.data_start = (offset as usize)
                .saturating_add(cmd_step_base)
                .min(stream.data.len());
        }
        stream.commands_total = match mode & 0x0F {
            // `DCTRL_LMODE_IGNORE`: the length is already set.
            0x00 => stream.commands_total,
            // `DCTRL_LMODE_CMDS`.
            0x01 => length,
            // `DCTRL_LMODE_MSEC`: upstream divides by the frequency verbatim;
            // the zero guard is ours (C would fault).
            0x02 => {
                if stream.hz == 0 {
                    0
                } else {
                    (u64::from(length) * 1000 / u64::from(stream.hz)) as u32
                }
            }
            // `DCTRL_LMODE_TOEND`: from the *un-based* start, as upstream
            // subtracts `CmdStepBase` back off.
            0x03 => {
                let from = stream.data_start.saturating_sub(cmd_step_base);
                (stream.data.len().saturating_sub(from) / data_step) as u32
            }
            // `DCTRL_LMODE_BYTES`, the `0x95` block form.
            0x04 => length / data_step as u32,
            _ => 0,
        };
        stream.reverse = mode & 0x10 != 0;
        stream.looping = mode & 0x80 != 0;
        stream.remaining = stream.commands_total;
        stream.cmd_index = if stream.reverse {
            stream.commands_total.saturating_sub(1)
        } else {
            0
        };
        // Upstream's `RC_RESET_PRESTEP`: the first command fires on the very
        // next output frame, not one full stream period later.
        stream.accumulator = u64::from(self.output_rate.saturating_sub(stream.hz));
        stream.playing = stream.remaining > 0;
        let playing = stream.playing;
        self.set_active(id, playing);
    }

    /// `0x94 ss` — stop stream `id`.
    pub fn stop(&mut self, id: u8) {
        self.streams[id as usize].playing = false;
        self.set_active(id, false);
    }

    /// Whether stream `id` is playing.
    #[must_use]
    pub fn is_playing(&self, id: u8) -> bool {
        self.streams[id as usize].playing
    }

    /// Advances every stream by one output frame, collecting the register
    /// writes that fell due.
    ///
    /// `huc6280_channel` is the engine's shadow of each HuC6280 instance's
    /// channel-select register, so a stream's channel switch can restore what
    /// the song had selected (upstream reads the register back; the shadow is
    /// the same value without an FFI read path).
    pub fn advance_frame(&mut self, due: &mut Vec<PendingWrite>, huc6280_channel: [u8; 2]) {
        if self.active.is_empty() {
            return;
        }
        let rate = u64::from(self.output_rate);
        let mut stopped = false;
        for &id in &self.active {
            let stream = &mut self.streams[id as usize];
            let Some(target) = stream.target else {
                continue;
            };
            stream.accumulator += u64::from(stream.hz);
            // Whole ticks fire, the fraction carries -- and any tick beyond
            // the remaining commands is discarded, exactly as upstream masks
            // its ratio counter after clamping to `RemainCmds`.
            let ticks = (stream.accumulator / rate) as u32;
            stream.accumulator %= rate;
            let fire = ticks.min(stream.remaining);
            for _ in 0..fire {
                emit_command(stream, target, huc6280_channel, due);
                stream.cmd_index = if stream.reverse {
                    stream.cmd_index.wrapping_sub(1)
                } else {
                    stream.cmd_index + 1
                };
            }
            stream.remaining -= fire;
            if stream.remaining == 0 {
                if stream.looping && stream.commands_total > 0 {
                    stream.remaining = stream.commands_total;
                    stream.cmd_index = if stream.reverse {
                        stream.commands_total - 1
                    } else {
                        0
                    };
                } else {
                    stream.playing = false;
                    stopped = true;
                }
            }
        }
        if stopped {
            let streams = &self.streams;
            self.active.retain(|&id| streams[id as usize].playing);
        }
    }
}

/// Translates one due command into the write(s) it means for the target chip.
///
/// Transcribed from `daccontrol_SendCommand`, emitting the engine's normalised
/// `(port, addr, data)` forms so each write then takes exactly the path the
/// equivalent ordinary VGM command would. A command whose bytes have run out
/// emits nothing but still advances, as upstream's early return does.
fn emit_command(
    stream: &Stream,
    target: StreamTarget,
    huc6280_channel: [u8; 2],
    due: &mut Vec<PendingWrite>,
) {
    let base = stream.data_start + stream.cmd_index as usize * DacStreams::data_step(stream);
    let Some(&b0) = stream.data.get(base) else {
        return;
    };
    let b1 = stream.data.get(base + 1).copied().unwrap_or(0);
    let mut put = |port: u8, addr: u16, value: u16| {
        due.push(PendingWrite {
            target,
            port,
            addr,
            value,
        });
    };
    let reg = target.register;
    match target.kind {
        // 4-bit register, 12-bit little-endian data, one write.
        ChipKind::Pwm => put(
            0,
            u16::from(reg & 0x0F),
            (u16::from(b1 & 0x0F) << 8) | u16::from(b0),
        ),
        // A volume stream reformats the nibble under the latch command; a
        // frequency stream is the classic latch/data pair.
        ChipKind::Sn76489 => {
            let command = reg & 0xF0;
            put(0, 0, u16::from(command | (b0 & 0x0F)));
            if stream.cmd_size == 2 {
                put(0, 0, u16::from(((b1 & 0x03) << 4) | ((b0 & 0xF0) >> 4)));
            }
        }
        // Register 0 with bit 7 set starts a sample: the id write, then the
        // channel mask. Bit 7 clear stops the channel. Any other register is a
        // plain write (the pin-7 strip happens in the chip's write rule).
        ChipKind::Okim6295 => {
            let channel = target.port & 0x0F;
            if reg == 0 {
                if b0 & 0x80 != 0 {
                    put(0, 0, u16::from(b0));
                    put(0, 0, u16::from(channel) << 4);
                } else {
                    put(0, 0, u16::from(channel) << 3);
                }
            } else {
                put(0, u16::from(reg), u16::from(b0));
            }
        }
        // One 16-bit register write; the chip's write rule splits it into the
        // MSB/LSB/register triple on the bus.
        ChipKind::QSound => put(0, u16::from(reg), (u16::from(b0) << 8) | u16::from(b1)),
        // Channel select, then data. `pp == 0xFF` skips the select. Upstream
        // leaves the RF5C pair's previous channel unrestored (its own TODO);
        // the HuC6280 restores from the engine's register shadow.
        ChipKind::Rf5c68 | ChipKind::Rf5c164 => {
            if target.port == 0xFF {
                put(0, u16::from(reg & 0x0F), u16::from(b0));
            } else {
                put(0, u16::from(reg >> 4), u16::from(target.port));
                put(0, u16::from(reg & 0x0F), u16::from(b0));
            }
        }
        ChipKind::HuC6280 => {
            if target.port == 0xFF {
                put(0, u16::from(reg & 0x0F), u16::from(b0));
            } else {
                let previous = huc6280_channel[usize::from(target.instance != 0)];
                put(0, u16::from(reg >> 4), u16::from(target.port));
                put(0, u16::from(reg & 0x0F), u16::from(b0));
                if previous != target.port {
                    put(0, u16::from(reg >> 4), u16::from(previous));
                }
            }
        }
        // The chips whose write rule takes a whole 16-bit offset: recombine
        // `pp cc` into it, as upstream passes the full command word.
        ChipKind::Scsp | ChipKind::Vsu | ChipKind::X1010 => put(
            0,
            (u16::from(target.port) << 8) | u16::from(reg),
            u16::from(b0),
        ),
        // Everything else: the ordinary one-byte register write on the setup
        // port, which each chip's write rule turns into its own bus shape
        // (latch pairs, reversed pairs, offset bases, FDS remaps...).
        _ => put(target.port, u16::from(reg), u16::from(b0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `frames` output frames and returns every write that fell due.
    fn run_writes(streams: &mut DacStreams, frames: usize) -> Vec<PendingWrite> {
        let mut due = Vec::new();
        for _ in 0..frames {
            streams.advance_frame(&mut due, [0, 0]);
        }
        due
    }

    /// As [`run_writes`], but only the written values -- for the byte-stream
    /// shaped tests.
    fn run(streams: &mut DacStreams, frames: usize) -> Vec<u16> {
        run_writes(streams, frames)
            .into_iter()
            .map(|write| write.value)
            .collect()
    }

    #[test]
    fn a_stream_plays_its_bank_at_the_rate_it_was_given() {
        let mut streams = DacStreams::new(44_100);
        // Chip id 0x02 is the YM2612; register 0x2A is its DAC.
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 4_410); // one command every ten output frames
        streams.start(0, &[10, 20, 30], 0, 0x03, 0);

        // The pre-stepped counter sends the first command on the very next
        // frame (upstream's RC_RESET_PRESTEP), then one per period.
        assert_eq!(run(&mut streams, 1), vec![10]);
        assert_eq!(run(&mut streams, 9), Vec::<u16>::new());
        assert_eq!(run(&mut streams, 1), vec![20]);
        assert_eq!(run(&mut streams, 10), vec![30]);
        assert_eq!(run(&mut streams, 100), Vec::<u16>::new(), "and it stops");
        assert!(!streams.is_playing(0));
    }

    #[test]
    fn an_absurd_stream_rate_does_not_run_the_frame_forever() {
        let mut streams = DacStreams::new(44_100);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, u32::MAX);
        // A tiny looping bank: unclamped, one frame would emit ~97k bytes and
        // keep looping forever.
        streams.start(0, &[0x11; 16], 0, 0x83, 0);

        let mut due = Vec::new();
        streams.advance_frame(&mut due, [0, 0]);
        assert!(
            due.len() <= 2 * STREAM_RATE_CEILING_FACTOR as usize,
            "one frame emitted {} writes",
            due.len()
        );
    }

    #[test]
    fn the_setup_command_says_where_every_byte_goes() {
        let mut streams = DacStreams::new(1);
        streams.setup(3, 0x82, 1, 0x2A); // bit 7: the second YM2612
        streams.bind(3, 0x00, 1, 0);
        streams.set_rate(3, 1);
        streams.start(3, &[99], 0, 0x03, 0);

        let mut due = Vec::new();
        streams.advance_frame(&mut due, [0, 0]);
        assert_eq!(
            due,
            [PendingWrite {
                target: StreamTarget {
                    kind: ChipKind::Ym2612,
                    instance: 1,
                    port: 1,
                    register: 0x2A,
                },
                port: 1,
                addr: 0x2A,
                value: 99,
            }]
        );
    }

    #[test]
    fn a_stream_for_a_chip_the_spec_does_not_number_stays_silent() {
        let mut streams = DacStreams::new(1);
        streams.setup(0, 0x7E, 0, 0x2A); // past the last chip id
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 1);
        streams.start(0, &[1, 2, 3], 0, 0x03, 0);
        assert!(!streams.is_playing(0), "an unrouted stream never starts");
        assert!(run(&mut streams, 10).is_empty());
    }

    #[test]
    fn a_step_takes_one_byte_of_each_group() {
        // Stereo data, playing only the right channel: step 2, base 1.
        let mut streams = DacStreams::new(4);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 2, 1);
        streams.set_rate(0, 4); // one command per frame
        streams.start(0, &[1, 2, 3, 4, 5, 6], 0, 0x03, 0);
        assert_eq!(run(&mut streams, 3), vec![2, 4, 6]);
    }

    #[test]
    fn a_looping_stream_goes_back_to_where_it_started() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2); // one command per frame
        // Length mode 1 (a command count) with the loop bit set.
        streams.start(0, &[7, 8, 9], 0, 0x81, 2);
        assert_eq!(run(&mut streams, 6), vec![7, 8, 7, 8, 7, 8]);
        assert!(streams.is_playing(0), "and it never stops on its own");
    }

    #[test]
    fn a_length_in_milliseconds_uses_the_references_formula() {
        let mut streams = DacStreams::new(1000);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 500);
        // Mode 2: upstream computes `1000 * Length / Frequency` commands --
        // dimensionally inverted (3 ms at 500 Hz is 1.5 commands, not 6), but
        // it is what the reference renders, so it is what this engine does.
        streams.start(0, &[1, 2, 3, 4, 5, 6, 7, 8], 0, 0x02, 3);
        assert_eq!(run(&mut streams, 12), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn mode_zero_keeps_the_previously_set_length() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        streams.start(0, &[1, 2, 3, 4], 0, 0x01, 2); // two commands...
        assert_eq!(run(&mut streams, 2), vec![1, 2]);
        // ...and mode 0 reuses that length rather than playing to the end.
        streams.start(0, &[1, 2, 3, 4], 0, 0x00, 0);
        assert_eq!(run(&mut streams, 10), vec![1, 2]);
    }

    #[test]
    fn an_offset_of_all_ones_keeps_the_previous_start() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        streams.start(0, &[1, 2, 3, 4], 2, 0x01, 1);
        assert_eq!(run(&mut streams, 1), vec![3]);
        // `0xFFFFFFFF`: play again from the same place.
        streams.start(0, &[1, 2, 3, 4], u32::MAX, 0x01, 2);
        assert_eq!(run(&mut streams, 2), vec![3, 4]);
    }

    #[test]
    fn a_reversed_stream_plays_its_pass_backwards() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        // Mode 3 (to end) with bit 4: the whole bank, last byte first.
        streams.start(0, &[1, 2, 3], 0, 0x13, 0);
        assert_eq!(run(&mut streams, 5), vec![3, 2, 1]);
        assert!(!streams.is_playing(0));
    }

    #[test]
    fn starting_before_the_rate_arms_and_plays_once_the_rate_arrives() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        // `0x93` before any `0x92`: upstream starts the stream and it begins
        // playing when the frequency arrives.
        streams.start(0, &[5, 6], 0, 0x03, 0);
        assert!(streams.is_playing(0));
        assert_eq!(run(&mut streams, 3), vec![5], "the pre-step fires one");
        streams.set_rate(0, 2);
        assert_eq!(run(&mut streams, 2), vec![6]);
    }

    #[test]
    fn stopping_a_stream_stops_it() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        streams.start(0, &[1, 2, 3, 4], 0, 0x03, 0);
        assert_eq!(run(&mut streams, 1), vec![1]);
        streams.stop(0);
        assert_eq!(run(&mut streams, 10), Vec::<u16>::new());
    }

    #[test]
    fn a_faster_stream_than_the_output_emits_more_than_one_command_a_frame() {
        let mut streams = DacStreams::new(100);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 250); // two and a half commands per frame
        streams.start(0, &(1..=10).collect::<Vec<u8>>(), 0, 0x03, 0);

        let mut due = Vec::new();
        streams.advance_frame(&mut due, [0, 0]);
        assert_eq!(due.len(), 2);
        due.clear();
        streams.advance_frame(&mut due, [0, 0]);
        assert_eq!(due.len(), 3, "the carried remainder brings a third");
    }

    #[test]
    fn clearing_forgets_every_stream() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        streams.start(0, &[1, 2], 0, 0x03, 0);
        streams.clear();
        assert!(!streams.is_playing(0));
        assert_eq!(streams.bank_type(0), 0);
    }

    // -- per-chip command translation (daccontrol_SendCommand) --------------

    /// One frame's writes for a single-command stream on `chip_id`, with the
    /// `pp cc` setup bytes and two data bytes -- the translation test rig.
    fn one_command(chip_id: u8, pp: u8, cc: u8, data: &[u8]) -> Vec<PendingWrite> {
        let mut streams = DacStreams::new(1);
        streams.setup(0, chip_id, pp, cc);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 1);
        streams.start(0, data, 0, 0x01, 1);
        let mut due = Vec::new();
        streams.advance_frame(&mut due, [0, 0]);
        due
    }

    fn flat(writes: &[PendingWrite]) -> Vec<(u8, u16, u16)> {
        writes.iter().map(|w| (w.port, w.addr, w.value)).collect()
    }

    /// The PWM (chip id 0x11) assembles a 12-bit value from two little-endian
    /// bytes into one write -- not two byte writes.
    #[test]
    fn a_pwm_stream_command_is_a_twelve_bit_pair() {
        let due = one_command(0x11, 0, 0x02, &[0x55, 0x01, 0xAA, 0x02]);
        assert_eq!(flat(&due), [(0, 0x02, 0x0155)]);
        // And the stream consumes both bytes per command: the next command
        // plays the next pair, not the high byte of the first.
        let mut streams = DacStreams::new(1);
        streams.setup(0, 0x11, 0, 0x02);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 1);
        streams.start(0, &[0x55, 0x01, 0xAA, 0x02], 0, 0x03, 0);
        let writes = run_writes(&mut streams, 2);
        assert_eq!(flat(&writes), [(0, 0x02, 0x0155), (0, 0x02, 0x02AA)]);
    }

    /// An SN76489 (chip id 0x00) frequency stream (latch command without the
    /// volume bit) consumes two bytes and emits the latch/data pair.
    #[test]
    fn an_sn76489_frequency_stream_emits_the_latch_and_data_pair() {
        let due = one_command(0x00, 0, 0x80, &[0x4E, 0x02]);
        assert_eq!(flat(&due), [(0, 0, 0x8E), (0, 0, 0x24)]);
    }

    /// A volume stream (latch bit 4 set) is one nibble under the command.
    #[test]
    fn an_sn76489_volume_stream_reformats_the_nibble() {
        let due = one_command(0x00, 0, 0x90, &[0x07]);
        assert_eq!(flat(&due), [(0, 0, 0x97)]);
    }

    /// An OKIM6295 (chip id 0x18) sample start writes the id then the channel
    /// mask; a stop writes the stop mask alone.
    #[test]
    fn an_okim6295_stream_keys_its_channel_on_and_off() {
        let start = one_command(0x18, 0x01, 0x00, &[0x83]);
        assert_eq!(flat(&start), [(0, 0, 0x83), (0, 0, 0x10)]);
        let stop = one_command(0x18, 0x01, 0x00, &[0x00]);
        assert_eq!(flat(&stop), [(0, 0, 0x08)]);
    }

    /// A QSound (chip id 0x1F) command is one 16-bit big-endian write.
    #[test]
    fn a_qsound_stream_command_is_sixteen_bits() {
        let due = one_command(0x1F, 0, 0x09, &[0x12, 0x34]);
        assert_eq!(flat(&due), [(0, 0x09, 0x1234)]);
    }

    /// An RF5C68 (chip id 0x05) stream selects its channel before the data
    /// write; `pp == 0xFF` skips the select.
    #[test]
    fn an_rf5c68_stream_selects_its_channel_first() {
        let due = one_command(0x05, 0xC2, 0x76, &[0x40]);
        assert_eq!(flat(&due), [(0, 0x07, 0xC2), (0, 0x06, 0x40)]);
        let direct = one_command(0x05, 0xFF, 0x76, &[0x40]);
        assert_eq!(flat(&direct), [(0, 0x06, 0x40)]);
    }

    /// A HuC6280 (chip id 0x1B) stream selects, writes, and restores the song's
    /// channel from the engine's shadow.
    #[test]
    fn a_huc6280_stream_restores_the_previous_channel() {
        let mut streams = DacStreams::new(1);
        streams.setup(0, 0x1B, 0x04, 0x06);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 1);
        streams.start(0, &[0x7F], 0, 0x01, 1);
        let mut due = Vec::new();
        streams.advance_frame(&mut due, [0x02, 0x00]); // the song had channel 2
        assert_eq!(
            flat(&due),
            [(0, 0x00, 0x04), (0, 0x06, 0x7F), (0, 0x00, 0x02)]
        );
    }
}
