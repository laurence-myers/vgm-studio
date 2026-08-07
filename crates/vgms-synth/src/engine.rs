//! Playback timing and the OPL mix vocabulary the engine shares.
//!
//! What remains here after the DRO engine's retirement: the [`FrameClock`] that
//! turns delays into output frames, the loop config and count, the [`Position`]
//! readout, and the OPL [`Muting`]/[`Panning`] vocabulary the DRO chip panel and
//! the audio backends still speak (translated to the generic per-chip masks
//! before the one [`VgmEngine`](crate::vgm_engine::VgmEngine) plays them).
//!
//! - **Frames, not bytes.** A position here is a `u64` frame count.
//! - **One honest delay clock.** DRO delays are milliseconds, VGM delays are
//!   44100 Hz samples; both become output frames through an exact integer carry.
//! - **No dropped samples.** The integer carry keeps every frame.

use vgms_core::regdata::PERCUSSION_REGISTER;
use vgms_core::util::VGM_SAMPLE_RATE;
use vgms_core::{Bank, DroSong};

/// The nine per-channel key-on/frequency registers, `0xB0..=0xB8`, whose writes
/// channel muting gates. The bank bit is tracked separately.
const CHANNEL_REGISTERS: core::ops::RangeInclusive<u8> = 0xB0..=0xB8;

/// Milliseconds (or VGM samples) to output frames, carrying the fractional
/// remainder exactly so it cannot drift over a long song.
///
/// The conversion, done in integers: `frames =
/// (delay * rate + carry) / unit`, where `unit` is `1000` for DRO milliseconds or
/// `44100` for VGM samples.
#[derive(Debug, Clone)]
pub struct FrameClock {
    output_rate: u64,
    /// `1000` when delays are milliseconds, `44100` when they are VGM samples.
    delay_unit: u64,
    carry: u64,
}

impl FrameClock {
    /// A clock rendering at `output_rate` Hz over delays expressed in units of
    /// `delay_unit` per second: `1000` for DRO milliseconds, `44100` for VGM
    /// samples.
    #[must_use]
    pub fn new(output_rate: u32, delay_unit: u32) -> Self {
        Self {
            output_rate: u64::from(output_rate),
            delay_unit: u64::from(delay_unit),
            carry: 0,
        }
    }

    /// The number of output frames a delay of `delay` units lasts, folding in the
    /// remainder carried from previous delays.
    pub fn frames_for(&mut self, delay: u32) -> u64 {
        let numerator = u64::from(delay) * self.output_rate + self.carry;
        self.carry = numerator % self.delay_unit;
        numerator / self.delay_unit
    }

    /// Discards the carried remainder, as after a seek.
    pub fn reset(&mut self) {
        self.carry = 0;
    }
}

/// Which channels and percussion voices are audible: the OPL playback engine's
/// muting, the GUI channel panel's muting/soloing, and the source the
/// OPL->generic translation ([`opl_chip_muting`](crate::opl_chip_muting)) reads.
///
/// A muted melodic channel's key-on writes (`0xB0..=0xB8`) are dropped; the
/// percussion register (`0xBD`) is AND-masked per bank. Every other register --
/// operator settings, frequencies, feedback -- always passes, which is why
/// isolating a voice only needs to gate its key-on register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Muting {
    /// Bit `bank * 9 + (reg - 0xB0)` set means channel `reg` on that bank is
    /// audible. 18 bits: nine channels, two banks.
    channels: u32,
    /// The AND-mask applied to `0xBD` writes, indexed by bank. `0xFF` passes
    /// everything; `0xE0` keeps the tremolo/vibrato-depth/rhythm-enable control
    /// bits but silences all five drums.
    percussion: [u8; 2],
}

impl Muting {
    const ALL_CHANNELS: u32 = (1 << 18) - 1;

    /// Everything audible -- the default for ordinary playback.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            channels: Self::ALL_CHANNELS,
            percussion: [0xFF, 0xFF],
        }
    }

    /// Nothing melodic audible, and drums silenced but their control bits kept
    /// (`0xE0`) -- a fully-muted OPL device, the base a single voice can be
    /// allowed back on top of.
    #[must_use]
    pub const fn silent() -> Self {
        Self {
            channels: 0,
            percussion: [0xE0, 0xE0],
        }
    }

    fn channel_bit(bank: Bank, channel: u8) -> u32 {
        debug_assert!(CHANNEL_REGISTERS.contains(&channel));
        let index = u32::from(bank.index()) * 9 + u32::from(channel - 0xB0);
        1 << index
    }

    /// Makes channel register `channel` (`0xB0..=0xB8`) on `bank` audible.
    pub fn allow_channel(&mut self, bank: Bank, channel: u8) {
        self.channels |= Self::channel_bit(bank, channel);
    }

    /// Mutes channel register `channel` (`0xB0..=0xB8`) on `bank`.
    pub fn mute_channel(&mut self, bank: Bank, channel: u8) {
        self.channels &= !Self::channel_bit(bank, channel);
    }

    /// Sets the AND-mask applied to `0xBD` (percussion) writes on `bank`.
    pub fn set_percussion(&mut self, bank: Bank, mask: u8) {
        self.percussion[usize::from(bank.index())] = mask;
    }

    /// Rebuilds a muting from its raw parts. The inverse of
    /// [`channels_raw`](Self::channels_raw) + [`percussion_raw`](Self::percussion_raw),
    /// for carrying a `Muting` across a boundary that can only ship primitives --
    /// the AudioWorklet ABI, which posts the two numbers to the worklet module and
    /// rebuilds the `Muting` there.
    #[must_use]
    pub const fn from_raw(channels: u32, percussion: [u8; 2]) -> Self {
        Self {
            channels: channels & Self::ALL_CHANNELS,
            percussion,
        }
    }

    /// The 18-bit channel-audible mask (bit `bank * 9 + (reg - 0xB0)`), for the
    /// worklet ABI to ship as a single primitive.
    #[must_use]
    pub const fn channels_raw(&self) -> u32 {
        self.channels
    }

    /// The two per-bank `0xBD` AND-masks, for the worklet ABI.
    #[must_use]
    pub const fn percussion_raw(&self) -> [u8; 2] {
        self.percussion
    }

    /// The value to write for register `reg` on `bank`, or `None` if muting drops
    /// it: a muted melodic channel (`0xB0..=0xB8`) is dropped, `0xBD` is AND-masked
    /// per bank, and every other register passes unchanged.
    ///
    /// Shared by the playback engine and the DRO capture so their muting decisions
    /// cannot diverge.
    #[must_use]
    pub fn gate(&self, bank: Bank, reg: u8, value: u8) -> Option<u8> {
        if reg == PERCUSSION_REGISTER {
            Some(value & self.percussion[usize::from(bank.index())])
        } else if !CHANNEL_REGISTERS.contains(&reg) || self.channel_allowed(bank, reg) {
            // A non-channel register always passes; a channel register passes only
            // if audible. The `||` short-circuits before `channel_allowed` (whose
            // debug assert requires a channel register) for non-channel writes.
            Some(value)
        } else {
            None
        }
    }

    /// The value a *seek replay* should write for `reg` on `bank`.
    ///
    /// A replay must leave the chip's state complete -- instruments, frequencies,
    /// feedback -- or unmuting a channel later would sound it half-configured. So
    /// unlike [`Self::gate`], nothing is dropped. But it must never *arm* a key
    /// that playback would gate: a muted channel's key-on rings the moment
    /// samples are generated (or, on real hardware, the moment the write lands),
    /// and every later write that would silence it is gated away -- it rings on,
    /// drifting in pitch as the channel's `0xA0` writes keep passing. So a muted
    /// channel's `0xB0..=0xB8` replays with the key bit cleared, and `0xBD`
    /// replays masked, exactly as `gate` would mask it.
    #[must_use]
    pub fn mask_replay(&self, bank: Bank, reg: u8, value: u8) -> u8 {
        const KEY_ON: u8 = 0x20;
        if reg == PERCUSSION_REGISTER {
            value & self.percussion[usize::from(bank.index())]
        } else if CHANNEL_REGISTERS.contains(&reg) && !self.channel_allowed(bank, reg) {
            value & !KEY_ON
        } else {
            value
        }
    }

    #[must_use]
    fn channel_allowed(&self, bank: Bank, channel: u8) -> bool {
        self.channels & Self::channel_bit(bank, channel) != 0
    }

    /// Whether channel register `channel` (`0xB0..=0xB8`) on `bank` is audible.
    ///
    /// What [`opl_chip_muting`](crate::opl_chip_muting) reads to translate this
    /// OPL muting into the generic per-channel mute mask.
    #[must_use]
    pub fn is_channel_audible(&self, bank: Bank, channel: u8) -> bool {
        self.channel_allowed(bank, channel)
    }
}

impl Default for Muting {
    fn default() -> Self {
        Self::all()
    }
}

/// How the 18 melodic channels are panned: the song's own stereo image, or an
/// explicit constant-power pan per channel.
///
/// `Custom` holds one pan byte per channel, indexed `bank * 9 + ch`, on the scale
/// `0x00` (hard left) .. `0x80` (centre) .. `0xFF` (hard right) -- the OPL core's
/// `stereo-ext` panpots (register `0xD0+ch`). The engine is policy-free: what a
/// given song type's `Custom` image should be is the app layer's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panning {
    /// The song's own `0xC0` speaker-enable bits rule; stereo-ext stays disengaged
    /// and output is bit-identical to a build without the feature.
    #[default]
    Original,
    /// A static per-channel pan override, engaging the stereo-ext panpots.
    Custom([u8; 18]),
}

/// How many times a loop region is played before playback carries on past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopCount {
    /// Repeat until playback is stopped.
    #[default]
    Infinite,
    /// Play the region this many times in total, then continue into whatever
    /// follows it. `0` and `1` both mean "no repeat": forward playback already
    /// plays the region once.
    Times(u32),
}

impl LoopCount {
    /// How many times playback jumps back, or `None` for "without end".
    #[must_use]
    pub(crate) fn wraps(self) -> Option<u32> {
        match self {
            Self::Infinite => None,
            Self::Times(times) => Some(times.saturating_sub(1)),
        }
    }
}

/// A region to loop over, and how often.
///
/// `start_frames` is carried rather than derived because [`DroEngine::set_loop`]
/// runs inside the audio callback, where walking the song to sum its delays would
/// be real-time work. Build one with [`LoopConfig::for_song`] off the audio thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopConfig {
    /// The instruction playback jumps back to.
    pub start: usize,
    /// One past the last instruction of the loop; `song.len()` loops the tail in.
    pub end: usize,
    pub count: LoopCount,
    /// The frame position of `start`, i.e. what `frames_rendered` rewinds to.
    pub start_frames: u64,
}

impl LoopConfig {
    /// A config for looping `song` over `[start, end)`, computing `start_frames`
    /// up front so the audio thread never has to.
    ///
    /// The frame count matches what [`FrameClock`] accumulates by `start` exactly:
    /// both floor the same `delays * rate / unit`, so the seam lands on the frame
    /// forward playback would have been on.
    #[must_use]
    pub fn for_song(song: &DroSong, start: usize, end: usize, count: LoopCount, rate: u32) -> Self {
        let rate = u64::from(rate);
        // A `DroSong` is always a DRO, whose delays are milliseconds.
        let start_frames = u64::from(song.ms_offset_at(start).unwrap_or(0)) * rate / 1000;
        Self {
            start,
            end,
            count,
            start_frames,
        }
    }

    /// The same, for a VGM played through
    /// [`VgmEngine`](crate::vgm_engine::VgmEngine).
    ///
    /// Its delays are always samples, and its rows are commands rather than
    /// instructions, but the arithmetic is the one the other engine's
    /// [`FrameClock`] does -- both floor `samples * rate / 44100`, so the seam
    /// lands on the frame forward playback would have been on.
    #[must_use]
    pub fn for_vgm(
        file: &vgms_core::VgmFile,
        start: usize,
        end: usize,
        count: LoopCount,
        rate: u32,
    ) -> Self {
        let before = file.stream().map_or(0, |stream| {
            stream.total_samples() - stream.samples_from(start)
        });
        Self {
            start,
            end,
            count,
            start_frames: before * u64::from(rate) / u64::from(VGM_SAMPLE_RATE),
        }
    }
}

/// Where playback currently is. Position is tracked as a `u64` frame count; the
/// milliseconds and instruction index are derived from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Total frames rendered since the last seek or rewind. Authoritative.
    ///
    /// This is a position *in the song*, not a count of frames sent to the
    /// device: a seek restarts it, and so does a loop, which rewinds it to the
    /// loop start so the readout and cursor wrap with the audio.
    pub frames_rendered: u64,
    /// Elapsed playback time in milliseconds, derived from `frames_rendered`.
    pub elapsed_ms: u32,
    /// The next instruction the engine will execute. While a delay is playing
    /// this already points *past* that delay; the sounding row is
    /// [`DroSong::seek_index_for_ms`] of `elapsed_ms`.
    pub next_instruction: usize,
    /// How many times playback has jumped back to the loop start since the last
    /// seek. `0` unless a loop is set.
    pub loop_iteration: u32,
}

impl Position {
    /// Elapsed playback milliseconds for `frames` output frames at `sample_rate`
    /// Hz, saturating rather than wrapping on the (~27-hour) `u32` overflow. This
    /// is the single frames-to-ms formula the engine, the native audio poll, and
    /// the CLI render progress all share, so a rounding change cannot desync them.
    #[must_use]
    pub fn ms_from_frames(frames: u64, sample_rate: u32) -> u32 {
        u32::try_from(frames * 1000 / u64::from(sample_rate)).unwrap_or(u32::MAX)
    }

    /// A position from the authoritative rendered-frame count, deriving
    /// `elapsed_ms` through [`Position::ms_from_frames`].
    #[must_use]
    pub fn from_frames(frames: u64, sample_rate: u32, next_instruction: usize) -> Self {
        Self::looping(frames, sample_rate, next_instruction, 0)
    }

    /// As [`Position::from_frames`], for a stream that has looped
    /// `loop_iteration` times since the last seek.
    #[must_use]
    pub fn looping(
        frames: u64,
        sample_rate: u32,
        next_instruction: usize,
        loop_iteration: u32,
    ) -> Self {
        Self {
            frames_rendered: frames,
            elapsed_ms: Self::ms_from_frames(frames, sample_rate),
            next_instruction,
            loop_iteration,
        }
    }
}
