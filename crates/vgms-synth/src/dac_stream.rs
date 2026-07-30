//! The `0x90`–`0x95` DAC stream engine: playing a data bank at a chip.
//!
//! A VGM can hand a chip a stream of bytes to be written to one register at a
//! fixed rate, instead of spelling out a write and a wait per byte. It is how a
//! Mega Drive rip carries its samples without a command every 1/16000th of a
//! second, and it is deliberately chip-agnostic in the spec: the setup command
//! names the chip, the port and the register, and everything after that is
//! timing.
//!
//! So it is chip-agnostic here too. This module owns no chip and emits no
//! samples; it says *when* a byte is due and *where* it goes, and the engine
//! does the writing.
//!
//! The six commands:
//!
//! | Opcode | Meaning |
//! |--------|---------|
//! | `0x90` | set up stream *n*: which chip, which port, which register |
//! | `0x91` | bind it to a data-bank type, with a step size and offset |
//! | `0x92` | set its rate in Hz |
//! | `0x93` | start at an offset, for a length (or to the end, or looping) |
//! | `0x94` | stop |
//! | `0x95` | start the *n*th block of the bound bank type -- the fast form |

use vgms_core::vgm::ChipKind;

/// Where a stream's bytes are written, once `0x90` has said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTarget {
    pub kind: ChipKind,
    /// Which instance of that chip: bit 7 of the chip-id byte.
    pub instance: u8,
    pub port: u8,
    /// The register every byte of the stream is written to.
    pub register: u8,
}

/// What `0x93`'s length mode asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LengthMode {
    /// Play exactly this many commands, then stop.
    Commands,
    /// Play for this many milliseconds.
    Milliseconds,
    /// Play to the end of the data bank.
    ToEnd,
}

impl LengthMode {
    const fn of(flags: u8) -> Self {
        match flags & 0x03 {
            0x01 => Self::Commands,
            0x02 => Self::Milliseconds,
            _ => Self::ToEnd,
        }
    }

    /// Bit 7: restart from the beginning rather than stopping.
    const fn loops(flags: u8) -> bool {
        flags & 0x80 != 0
    }
}

/// One of the up to 256 streams a file can define.
#[derive(Debug, Clone, Default)]
struct Stream {
    target: Option<StreamTarget>,
    /// The data-bank type `0x91` bound, already normalised to its uncompressed
    /// number so a compressed bank is found by the same key.
    bank_type: u8,
    /// How far to advance per step. `0` means "the spec's default", which is a
    /// step of one byte.
    step_size: u8,
    /// Which byte within each step is the one played.
    step_base: u8,
    /// Bytes per second.
    hz: u32,
    /// The bank being played, copied at start: a later block must not change
    /// what a running stream is playing.
    data: Vec<u8>,
    /// The next byte to play, as an index into `data`.
    position: usize,
    /// One past the last byte to play.
    end: usize,
    /// Where to go back to when looping.
    start: usize,
    looping: bool,
    playing: bool,
    /// Fractional time carried between output frames, in output-rate units.
    accumulator: u64,
}

/// A byte a stream wants written, right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingWrite {
    pub target: StreamTarget,
    pub value: u8,
}

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
        self.streams[id as usize].target = Some(StreamTarget {
            kind,
            instance: u8::from(chip_id & 0x80 != 0),
            port,
            register,
        });
    }

    /// `0x91 ss tt ll bb` — bind stream `id` to bank type `bank_type`, stepping
    /// `step_size` bytes and taking the byte at `step_base` within each step.
    pub fn bind(&mut self, id: u8, bank_type: u8, step_size: u8, step_base: u8) {
        let stream = &mut self.streams[id as usize];
        stream.bank_type = crate::banks::BlockKind::uncompressed_type(bank_type);
        stream.step_size = step_size;
        stream.step_base = step_base;
    }

    /// `0x92 ss dddddddd` — stream `id` plays `hz` bytes per second.
    pub fn set_rate(&mut self, id: u8, hz: u32) {
        self.streams[id as usize].hz = hz;
    }

    /// The bank type stream `id` is bound to, so the caller can fetch its data.
    #[must_use]
    pub fn bank_type(&self, id: u8) -> u8 {
        self.streams[id as usize].bank_type
    }

    /// `0x93 ss aaaaaaaa mm llllllll` — play `data` from `offset`.
    ///
    /// `data` is the whole bound bank; the stream copies the part it will play
    /// so a data block arriving mid-playback cannot change it underneath.
    pub fn start(&mut self, id: u8, data: &[u8], offset: u32, flags: u8, length: u32) {
        let stream = &mut self.streams[id as usize];
        let Some(_) = stream.target else {
            return;
        };
        let start = (offset as usize).min(data.len());
        let end = match LengthMode::of(flags) {
            LengthMode::Commands => start.saturating_add(length as usize).min(data.len()),
            LengthMode::Milliseconds => {
                // Milliseconds at the stream's own rate, which is what makes a
                // length in time mean a length in bytes.
                let bytes = (u64::from(length) * u64::from(stream.hz) / 1000) as usize;
                start.saturating_add(bytes).min(data.len())
            }
            LengthMode::ToEnd => data.len(),
        };
        stream.data = data.to_vec();
        stream.start = start;
        stream.position = start;
        stream.end = end;
        stream.looping = LengthMode::loops(flags);
        stream.playing = start < end && stream.hz > 0;
        stream.accumulator = 0;
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

    /// Advances every stream by one output frame, collecting what fell due.
    ///
    /// A stream faster than the output rate emits more than one byte per frame,
    /// which is normal: a 16 kHz DAC against a 44.1 kHz output emits roughly one
    /// byte every third frame, but a 96 kHz one emits two some frames. The
    /// accumulator carries the remainder so neither drifts.
    pub fn advance_frame(&mut self, due: &mut Vec<PendingWrite>) {
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
            while stream.accumulator >= rate {
                stream.accumulator -= rate;
                let step = usize::from(stream.step_size.max(1));
                let at = stream.position + usize::from(stream.step_base);
                match stream.data.get(at) {
                    Some(&value) if stream.position < stream.end => {
                        due.push(PendingWrite { target, value });
                        stream.position += step;
                    }
                    _ => {
                        stream.position = stream.end;
                    }
                }
                if stream.position >= stream.end {
                    if stream.looping {
                        stream.position = stream.start;
                    } else {
                        stream.playing = false;
                        stopped = true;
                        break;
                    }
                }
            }
        }
        if stopped {
            let streams = &self.streams;
            self.active.retain(|&id| streams[id as usize].playing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `frames` output frames and returns every byte that fell due.
    fn run(streams: &mut DacStreams, frames: usize) -> Vec<u8> {
        let mut due = Vec::new();
        for _ in 0..frames {
            streams.advance_frame(&mut due);
        }
        due.into_iter().map(|write| write.value).collect()
    }

    #[test]
    fn a_stream_plays_its_bank_at_the_rate_it_was_given() {
        let mut streams = DacStreams::new(44_100);
        // Chip id 0x02 is the YM2612; register 0x2A is its DAC.
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 4_410); // one byte every ten output frames
        streams.start(0, &[10, 20, 30], 0, 0x00, 0);

        assert_eq!(run(&mut streams, 9), Vec::<u8>::new(), "not due yet");
        assert_eq!(run(&mut streams, 1), vec![10]);
        assert_eq!(run(&mut streams, 10), vec![20]);
        assert_eq!(run(&mut streams, 10), vec![30]);
        assert_eq!(run(&mut streams, 100), Vec::<u8>::new(), "and it stops");
        assert!(!streams.is_playing(0));
    }

    #[test]
    fn the_setup_command_says_where_every_byte_goes() {
        let mut streams = DacStreams::new(1);
        streams.setup(3, 0x82, 1, 0x2A); // bit 7: the second YM2612
        streams.bind(3, 0x00, 1, 0);
        streams.set_rate(3, 1);
        streams.start(3, &[99], 0, 0x00, 0);

        let mut due = Vec::new();
        streams.advance_frame(&mut due);
        assert_eq!(
            due,
            [PendingWrite {
                target: StreamTarget {
                    kind: ChipKind::Ym2612,
                    instance: 1,
                    port: 1,
                    register: 0x2A,
                },
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
        streams.start(0, &[1, 2, 3], 0, 0x00, 0);
        assert!(!streams.is_playing(0), "an unrouted stream never starts");
        assert!(run(&mut streams, 10).is_empty());
    }

    #[test]
    fn a_step_takes_one_byte_of_each_group() {
        // Stereo data, playing only the right channel: step 2, base 1.
        let mut streams = DacStreams::new(4);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 2, 1);
        streams.set_rate(0, 4); // one byte per frame
        streams.start(0, &[1, 2, 3, 4, 5, 6], 0, 0x00, 0);
        assert_eq!(run(&mut streams, 3), vec![2, 4, 6]);
    }

    #[test]
    fn a_looping_stream_goes_back_to_where_it_started() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2); // one byte per frame
        // Length mode 1 (a byte count) with the loop bit set.
        streams.start(0, &[7, 8, 9], 0, 0x81, 2);
        assert_eq!(run(&mut streams, 6), vec![7, 8, 7, 8, 7, 8]);
        assert!(streams.is_playing(0), "and it never stops on its own");
    }

    #[test]
    fn a_length_in_milliseconds_is_a_length_in_bytes_at_the_streams_rate() {
        let mut streams = DacStreams::new(1000);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 1000); // one byte per millisecond
        // Mode 2: play for 3 ms, so three bytes.
        streams.start(0, &[1, 2, 3, 4, 5], 0, 0x02, 3);
        assert_eq!(run(&mut streams, 10), vec![1, 2, 3]);
    }

    #[test]
    fn stopping_a_stream_stops_it() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        streams.start(0, &[1, 2, 3, 4], 0, 0x00, 0);
        assert_eq!(run(&mut streams, 1), vec![1]);
        streams.stop(0);
        assert_eq!(run(&mut streams, 10), Vec::<u8>::new());
    }

    #[test]
    fn a_faster_stream_than_the_output_emits_more_than_one_byte_a_frame() {
        let mut streams = DacStreams::new(100);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 250); // two and a half bytes per frame
        streams.start(0, &(1..=10).collect::<Vec<u8>>(), 0, 0x00, 0);

        let mut due = Vec::new();
        streams.advance_frame(&mut due);
        assert_eq!(due.len(), 2);
        due.clear();
        streams.advance_frame(&mut due);
        assert_eq!(due.len(), 3, "the carried remainder brings a third");
    }

    #[test]
    fn clearing_forgets_every_stream() {
        let mut streams = DacStreams::new(2);
        streams.setup(0, 0x02, 0, 0x2A);
        streams.bind(0, 0x00, 1, 0);
        streams.set_rate(0, 2);
        streams.start(0, &[1, 2], 0, 0x00, 0);
        streams.clear();
        assert!(!streams.is_playing(0));
        assert_eq!(streams.bank_type(0), 0);
    }
}
