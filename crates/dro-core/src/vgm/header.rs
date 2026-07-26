//! The VGM header: every chip the spec can declare, and the fields around them.
//!
//! # The header ends where the data starts
//!
//! Every pointer in a VGM header is relative to its own position, and the
//! absolute data start (`0x34 + data offset`) is also the end of the header: a
//! field at or past it does not exist, and reads as zero. Real files lean on
//! this -- a minimal PC-AT rip can put its data at 0x60, so everything from
//! 0x60 on is absent rather than zero-filled, and reading it would be reading
//! the command stream.
//!
//! Versions before 1.50 have no data-offset field at all; their data always
//! starts at 0x40.
//!
//! # Tolerant, but never wrong
//!
//! The data-start rule is enforced strictly, because breaking it means
//! misreading command bytes as clocks. The version a file declares is treated
//! as advisory: a chip whose field is inside the header but whose version
//! postdates the declared one is still reported, with a warning. Such files
//! exist, the bytes are unambiguous, and refusing to see the chip helps nobody.

use crate::error::{Error, Result};
use crate::io::ByteReader;

/// Header field offsets, as the VGM spec numbers them.
///
/// This is the one table; [`super::io`] reads the OPL subset from it.
pub(crate) mod offset {
    pub(crate) const MAGIC: usize = 0x00;
    pub(crate) const EOF: usize = 0x04;
    pub(crate) const VERSION: usize = 0x08;
    pub(crate) const GD3: usize = 0x14;
    pub(crate) const TOTAL_SAMPLES: usize = 0x18;
    pub(crate) const LOOP_OFFSET: usize = 0x1C;
    pub(crate) const LOOP_NUM_SAMPLES: usize = 0x20;
    pub(crate) const RATE: usize = 0x24;
    pub(crate) const SN76489_FEEDBACK: usize = 0x28;
    pub(crate) const SN76489_SHIFT_WIDTH: usize = 0x2A;
    pub(crate) const SN76489_FLAGS: usize = 0x2B;
    pub(crate) const DATA_OFFSET: usize = 0x34;
    pub(crate) const SEGA_PCM_INTERFACE: usize = 0x3C;
    pub(crate) const YM3812_CLOCK: usize = 0x50;
    pub(crate) const YMF262_CLOCK: usize = 0x5C;
    pub(crate) const AY8910_TYPE: usize = 0x78;
    pub(crate) const AY8910_FLAGS: usize = 0x79;
    pub(crate) const YM2203_AY_FLAGS: usize = 0x7A;
    pub(crate) const YM2608_AY_FLAGS: usize = 0x7B;
    pub(crate) const VOLUME_MODIFIER: usize = 0x7C;
    pub(crate) const LOOP_BASE: usize = 0x7E;
    pub(crate) const LOOP_MODIFIER: usize = 0x7F;
    pub(crate) const OKIM6258_FLAGS: usize = 0x94;
    pub(crate) const K054539_FLAGS: usize = 0x95;
    pub(crate) const C140_TYPE: usize = 0x96;
    pub(crate) const EXTRA_HEADER: usize = 0xBC;
    pub(crate) const ES5503_CHANNELS: usize = 0xD4;
    pub(crate) const ES5505_CHANNELS: usize = 0xD5;
    pub(crate) const C352_CLOCK_DIVIDER: usize = 0xD6;
}

/// Where the data starts when the file is too old to say (or declines to).
pub(crate) const LEGACY_DATA_START: usize = 0x40;
/// The first version with a data-offset field.
const DATA_OFFSET_VERSION: u32 = 0x0000_0150;
/// The oldest version this reader will open.
pub const MINIMUM_VERSION: u32 = 0x0000_0100;

/// Bit 30 of a clock: a second instance of this chip is present.
pub(crate) const DUAL_CHIP_FLAG: u32 = 0x4000_0000;
/// Bit 31 of a clock: the chip's variant, where the spec defines one.
pub(crate) const VARIANT_FLAG: u32 = 0x8000_0000;
/// What is left of a clock once the two flag bits are masked off.
const CLOCK_MASK: u32 = 0x3FFF_FFFF;

/// One of the sound chips a VGM file can declare.
///
/// Ordered by header offset, which is also roughly the order the spec added
/// them, and the order [`VgmHeader::chips`] reports them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChipKind {
    Sn76489,
    Ym2413,
    Ym2612,
    Ym2151,
    SegaPcm,
    Rf5c68,
    Ym2203,
    Ym2608,
    Ym2610,
    Ym3812,
    Ym3526,
    Y8950,
    Ymf262,
    Ymf278b,
    Ymf271,
    Ymz280b,
    Rf5c164,
    Pwm,
    Ay8910,
    GameBoyDmg,
    NesApu,
    MultiPcm,
    Upd7759,
    Okim6258,
    Okim6295,
    K051649,
    K054539,
    HuC6280,
    C140,
    K053260,
    Pokey,
    QSound,
    Scsp,
    WonderSwan,
    Vsu,
    Saa1099,
    Es5503,
    Es5505,
    X1010,
    C352,
    Ga20,
    Mikey,
}

/// How many chips the spec defines a clock field for.
pub const CHIP_COUNT: usize = 42;

/// What the spec says about one chip's clock field.
struct ChipSpec {
    kind: ChipKind,
    clock: usize,
    since: u32,
    name: &'static str,
    /// What bit 31 of the clock means, where the spec gives it a meaning.
    variant: Option<&'static str>,
}

/// The chip table, in [`ChipKind`] order -- which is also ascending by offset.
///
/// `ChipKind::spec` indexes this by the variant's discriminant, so the two must
/// stay in step; `the_chip_table_is_in_enum_order` is the guard.
const CHIPS: [ChipSpec; CHIP_COUNT] = {
    const fn chip(
        kind: ChipKind,
        clock: usize,
        since: u32,
        name: &'static str,
        variant: Option<&'static str>,
    ) -> ChipSpec {
        ChipSpec {
            kind,
            clock,
            since,
            name,
            variant,
        }
    }
    use ChipKind as K;
    [
        chip(K::Sn76489, 0x0C, 0x100, "SN76489", Some("T6W28")),
        chip(K::Ym2413, 0x10, 0x100, "YM2413", None),
        chip(K::Ym2612, 0x2C, 0x110, "YM2612", Some("YM3438")),
        chip(K::Ym2151, 0x30, 0x110, "YM2151", Some("YM2164")),
        chip(K::SegaPcm, 0x38, 0x151, "Sega PCM", None),
        chip(K::Rf5c68, 0x40, 0x151, "RF5C68", None),
        chip(K::Ym2203, 0x44, 0x151, "YM2203", None),
        chip(K::Ym2608, 0x48, 0x151, "YM2608", None),
        chip(K::Ym2610, 0x4C, 0x151, "YM2610", Some("YM2610B")),
        chip(K::Ym3812, 0x50, 0x151, "YM3812", None),
        chip(K::Ym3526, 0x54, 0x151, "YM3526", None),
        chip(K::Y8950, 0x58, 0x151, "Y8950", None),
        chip(K::Ymf262, 0x5C, 0x151, "YMF262", None),
        chip(K::Ymf278b, 0x60, 0x151, "YMF278B", None),
        chip(K::Ymf271, 0x64, 0x151, "YMF271", None),
        chip(K::Ymz280b, 0x68, 0x151, "YMZ280B", None),
        chip(K::Rf5c164, 0x6C, 0x151, "RF5C164", None),
        chip(K::Pwm, 0x70, 0x151, "PWM", None),
        chip(K::Ay8910, 0x74, 0x151, "AY8910", None),
        chip(K::GameBoyDmg, 0x80, 0x161, "Game Boy DMG", None),
        chip(K::NesApu, 0x84, 0x161, "NES APU", Some("NES APU + FDS")),
        chip(K::MultiPcm, 0x88, 0x161, "MultiPCM", None),
        chip(K::Upd7759, 0x8C, 0x161, "uPD7759", None),
        chip(K::Okim6258, 0x90, 0x161, "OKIM6258", None),
        chip(K::Okim6295, 0x98, 0x161, "OKIM6295", None),
        chip(K::K051649, 0x9C, 0x161, "K051649", Some("K052539")),
        chip(K::K054539, 0xA0, 0x161, "K054539", None),
        chip(K::HuC6280, 0xA4, 0x161, "HuC6280", None),
        chip(K::C140, 0xA8, 0x161, "C140", None),
        chip(K::K053260, 0xAC, 0x161, "K053260", None),
        chip(K::Pokey, 0xB0, 0x161, "POKEY", None),
        chip(K::QSound, 0xB4, 0x161, "QSound", None),
        chip(K::Scsp, 0xB8, 0x171, "SCSP", None),
        chip(K::WonderSwan, 0xC0, 0x171, "WonderSwan", None),
        chip(K::Vsu, 0xC4, 0x171, "VSU", None),
        chip(K::Saa1099, 0xC8, 0x171, "SAA1099", None),
        chip(K::Es5503, 0xCC, 0x171, "ES5503", None),
        chip(K::Es5505, 0xD0, 0x171, "ES5505", Some("ES5506")),
        chip(K::X1010, 0xD8, 0x171, "X1-010", None),
        chip(K::C352, 0xDC, 0x171, "C352", None),
        chip(K::Ga20, 0xE0, 0x171, "GA20", None),
        chip(K::Mikey, 0xE4, 0x172, "Mikey", None),
    ]
};

impl ChipKind {
    /// Every chip, in header order.
    pub fn all() -> impl Iterator<Item = Self> {
        CHIPS.iter().map(|spec| spec.kind)
    }

    const fn spec(self) -> &'static ChipSpec {
        &CHIPS[self as usize]
    }

    /// The chip's name as the spec writes it, e.g. `"YM2612"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.spec().name
    }

    /// The offset of this chip's clock field in the header.
    #[must_use]
    pub const fn clock_offset(self) -> usize {
        self.spec().clock
    }

    /// The first VGM version that defines this chip's clock field, as BCD.
    #[must_use]
    pub const fn since_version(self) -> u32 {
        self.spec().since
    }

    /// What bit 31 of this chip's clock means, where the spec defines it.
    #[must_use]
    pub const fn variant_name(self) -> Option<&'static str> {
        self.spec().variant
    }
}

/// A chip a file actually declares: the clock, and what the flag bits say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipUse {
    pub kind: ChipKind,
    /// The clock in Hz, with the dual and variant bits masked off.
    pub clock: u32,
    /// Bit 30: a second instance of this chip is present.
    pub dual: bool,
    /// Bit 31: the chip's variant, named by [`ChipKind::variant_name`].
    pub variant: bool,
}

impl ChipUse {
    /// Whether this is a T6W28 -- an SN76489 with split stereo, which the spec
    /// signals by setting *both* flag bits rather than by a variant bit alone.
    #[must_use]
    pub const fn is_t6w28(&self) -> bool {
        matches!(self.kind, ChipKind::Sn76489) && self.variant && self.dual
    }

    /// How to name this chip in a description or a UI, e.g. `"YM3438"`,
    /// `"YMF262 x2"`.
    ///
    /// The variant bit only renames the chip where the spec says what it means;
    /// `dro2vgm` sets it on dual OPL2 clocks where it means nothing, so an
    /// unnamed variant is passed over rather than guessed at.
    #[must_use]
    pub fn label(&self) -> String {
        if self.is_t6w28() {
            return "T6W28".to_owned();
        }
        let base = match (self.variant, self.kind.variant_name()) {
            (true, Some(variant)) => variant,
            _ => self.kind.name(),
        };
        if self.dual {
            format!("{base} x2")
        } else {
            base.to_owned()
        }
    }
}

/// A second-instance clock from the v1.70 extra header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraClock {
    /// The chip's id in the extra header's own numbering, not [`ChipKind`].
    pub chip_id: u8,
    pub clock: u32,
}

/// A per-chip volume from the v1.70 extra header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraVolume {
    pub chip_id: u8,
    /// Bit 7 of the chip id: the volume applies to the paired chip.
    pub paired: bool,
    /// Bit 0 of the flags byte: the volume applies to the second instance.
    pub second_instance: bool,
    /// Bit 15 clear: an absolute volume. Bit 15 set: a factor of `volume/0x100`.
    pub relative: bool,
    pub volume: u16,
}

/// The v1.70 extra header: per-chip clocks and volumes that do not fit the
/// fixed header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtraHeader {
    pub clocks: Vec<ExtraClock>,
    pub volumes: Vec<ExtraVolume>,
}

/// Chip settings that live in their own bytes rather than in a clock field.
///
/// Every one of these is zero when its field is outside the header. They exist
/// for the playback engine to come; nothing in the metadata tier reads them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChipSettings {
    pub sn76489_feedback: u16,
    pub sn76489_shift_width: u8,
    pub sn76489_flags: u8,
    pub sega_pcm_interface: u32,
    pub ay8910_type: u8,
    pub ay8910_flags: u8,
    pub ym2203_ay_flags: u8,
    pub ym2608_ay_flags: u8,
    pub okim6258_flags: u8,
    pub k054539_flags: u8,
    pub c140_type: u8,
    pub es5503_channels: u8,
    pub es5505_channels: u8,
    pub c352_clock_divider: u8,
}

/// A parsed VGM header, with its bytes kept verbatim.
///
/// The raw bytes are what a write emits, patched only where a value can have
/// changed -- the same discipline as [`VgmMeta`](super::VgmMeta), and what makes
/// an unedited round trip byte-exact even for chips this app knows nothing
/// about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmHeader {
    raw: Vec<u8>,
    version: u32,
    data_start: usize,
    gd3_offset: Option<usize>,
    total_samples: u32,
    loop_offset: u32,
    loop_num_samples: u32,
    rate: u32,
    volume_modifier: u8,
    loop_base: u8,
    loop_modifier: u8,
    chips: Vec<ChipUse>,
    settings: ChipSettings,
    extra: Option<ExtraHeader>,
}

impl VgmHeader {
    /// Parses the header of a (decompressed) VGM file.
    ///
    /// # Errors
    /// If the magic is wrong, the version predates 1.00, or the data offset
    /// points outside the file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let magic = reader.take(4)?;
        if magic != super::io::MAGIC {
            return Err(Error::file(format!(
                "Does not appear to be a VGM file (invalid header. Expected {}, found {}).",
                String::from_utf8_lossy(super::io::MAGIC),
                String::from_utf8_lossy(magic),
            )));
        }

        reader.seek(offset::VERSION)?;
        let version = reader.u32_le()?;
        if version < MINIMUM_VERSION {
            return Err(Error::file(format!(
                "Unsupported VGM version {}; v1.00 is the minimum.",
                format_version(version)
            )));
        }

        let data_start = data_start(bytes, version)?;
        if data_start > bytes.len() {
            return Err(Error::file(format!(
                "VGM data starts at {data_start:#X}, past the end of the {} byte file",
                bytes.len()
            )));
        }
        // Reads stop at the data, never past it: beyond this point the bytes are
        // the command stream, not header fields.
        let header = &bytes[..data_start];

        let gd3_offset = match u32_at(header, offset::GD3) {
            0 => None,
            relative => Some(offset::GD3 + relative as usize),
        };

        let chips = read_chips(header, version);
        let extra = read_extra_header(bytes, header, version);

        Ok(Self {
            raw: header.to_vec(),
            version,
            data_start,
            gd3_offset,
            total_samples: u32_at(header, offset::TOTAL_SAMPLES),
            loop_offset: u32_at(header, offset::LOOP_OFFSET),
            loop_num_samples: u32_at(header, offset::LOOP_NUM_SAMPLES),
            rate: u32_at(header, offset::RATE),
            volume_modifier: u8_at(header, offset::VOLUME_MODIFIER),
            loop_base: u8_at(header, offset::LOOP_BASE),
            loop_modifier: u8_at(header, offset::LOOP_MODIFIER),
            chips,
            settings: read_settings(header),
            extra,
        })
    }

    /// The header bytes, from the magic up to the start of the command stream.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The declared version, as BCD: `0x151` is v1.51.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// The version as it is written, e.g. `"1.51"`.
    #[must_use]
    pub fn version_string(&self) -> String {
        format_version(self.version)
    }

    /// The absolute offset the command stream starts at, which is also the end
    /// of the header.
    #[must_use]
    pub const fn data_start(&self) -> usize {
        self.data_start
    }

    /// The absolute offset of the GD3 tag, if the file declares one.
    #[must_use]
    pub const fn gd3_offset(&self) -> Option<usize> {
        self.gd3_offset
    }

    /// The song's length in samples, as the header declares it.
    ///
    /// The metadata tier trusts this field, as `vgm_stat` does; once the command
    /// stream is parsed (mc-4) it can be cross-checked against the waits.
    #[must_use]
    pub const fn total_samples(&self) -> u32 {
        self.total_samples
    }

    /// The loop's length in samples, or `None` if the file does not loop.
    #[must_use]
    pub const fn loop_samples(&self) -> Option<u32> {
        if self.loop_offset == 0 {
            None
        } else {
            Some(self.loop_num_samples)
        }
    }

    /// The absolute offset the loop restarts at, or `None` if it does not loop.
    #[must_use]
    pub const fn loop_offset(&self) -> Option<usize> {
        if self.loop_offset == 0 {
            None
        } else {
            Some(offset::LOOP_OFFSET + self.loop_offset as usize)
        }
    }

    /// The `rate` field: the playback rate of the recorded system, or 0.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    #[must_use]
    pub const fn volume_modifier(&self) -> u8 {
        self.volume_modifier
    }

    #[must_use]
    pub const fn loop_base(&self) -> u8 {
        self.loop_base
    }

    #[must_use]
    pub const fn loop_modifier(&self) -> u8 {
        self.loop_modifier
    }

    /// Every chip the file declares a non-zero clock for, in header order.
    #[must_use]
    pub fn chips(&self) -> &[ChipUse] {
        &self.chips
    }

    /// Whether `kind` is one of the chips this file declares.
    #[must_use]
    pub fn has_chip(&self, kind: ChipKind) -> bool {
        self.chips.iter().any(|chip| chip.kind == kind)
    }

    /// Whether every chip this file declares is an OPL the editor can open.
    ///
    /// A file with no chips at all is not OPL: there is nothing to play.
    #[must_use]
    pub fn is_opl_only(&self) -> bool {
        !self.chips.is_empty()
            && self.chips.iter().all(|chip| {
                matches!(
                    chip.kind,
                    ChipKind::Ym3812 | ChipKind::Ymf262 | ChipKind::Ym3526 | ChipKind::Y8950
                )
            })
    }

    /// The chips as a comma-separated list of labels, e.g. `"SN76489, YM2413"`.
    #[must_use]
    pub fn chip_list(&self) -> String {
        self.chips
            .iter()
            .map(ChipUse::label)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The per-chip settings bytes.
    #[must_use]
    pub const fn settings(&self) -> &ChipSettings {
        &self.settings
    }

    /// The v1.70 extra header, if the file has one.
    #[must_use]
    pub const fn extra(&self) -> Option<&ExtraHeader> {
        self.extra.as_ref()
    }
}

// ---------------------------------------------------------------------------

/// Where the command stream starts.
///
/// Before v1.50 there is no data-offset field and the data always starts at
/// 0x40; a zero field means the same thing, which some writers rely on.
fn data_start(bytes: &[u8], version: u32) -> Result<usize> {
    if version < DATA_OFFSET_VERSION {
        return Ok(LEGACY_DATA_START);
    }
    let mut reader = ByteReader::new(bytes);
    reader.seek(offset::DATA_OFFSET)?;
    Ok(match reader.u32_le()? {
        0 => LEGACY_DATA_START,
        relative => offset::DATA_OFFSET + relative as usize,
    })
}

/// A `u32` field, or zero if it falls outside the header.
fn u32_at(header: &[u8], at: usize) -> u32 {
    match header.get(at..at + 4) {
        Some(slice) => u32::from_le_bytes(slice.try_into().expect("a four byte slice")),
        None => 0,
    }
}

fn u16_at(header: &[u8], at: usize) -> u16 {
    match header.get(at..at + 2) {
        Some(slice) => u16::from_le_bytes(slice.try_into().expect("a two byte slice")),
        None => 0,
    }
}

fn u8_at(header: &[u8], at: usize) -> u8 {
    header.get(at).copied().unwrap_or(0)
}

fn read_chips(header: &[u8], version: u32) -> Vec<ChipUse> {
    let mut chips = Vec::new();
    for spec in &CHIPS {
        let raw = u32_at(header, spec.clock);
        let clock = raw & CLOCK_MASK;
        if clock == 0 {
            continue;
        }
        if version < spec.since {
            log::warn!(
                "VGM declares a {} clock, which arrived in v{}, but calls itself v{}; reading it \
                 anyway",
                spec.name,
                format_version(spec.since),
                format_version(version)
            );
        }
        chips.push(ChipUse {
            kind: spec.kind,
            clock,
            dual: raw & DUAL_CHIP_FLAG != 0,
            variant: raw & VARIANT_FLAG != 0,
        });
    }
    chips
}

fn read_settings(header: &[u8]) -> ChipSettings {
    ChipSettings {
        sn76489_feedback: u16_at(header, offset::SN76489_FEEDBACK),
        sn76489_shift_width: u8_at(header, offset::SN76489_SHIFT_WIDTH),
        sn76489_flags: u8_at(header, offset::SN76489_FLAGS),
        sega_pcm_interface: u32_at(header, offset::SEGA_PCM_INTERFACE),
        ay8910_type: u8_at(header, offset::AY8910_TYPE),
        ay8910_flags: u8_at(header, offset::AY8910_FLAGS),
        ym2203_ay_flags: u8_at(header, offset::YM2203_AY_FLAGS),
        ym2608_ay_flags: u8_at(header, offset::YM2608_AY_FLAGS),
        okim6258_flags: u8_at(header, offset::OKIM6258_FLAGS),
        k054539_flags: u8_at(header, offset::K054539_FLAGS),
        c140_type: u8_at(header, offset::C140_TYPE),
        es5503_channels: u8_at(header, offset::ES5503_CHANNELS),
        es5505_channels: u8_at(header, offset::ES5505_CHANNELS),
        c352_clock_divider: u8_at(header, offset::C352_CLOCK_DIVIDER),
    }
}

/// Reads the v1.70 extra header, if the file points at one.
///
/// A malformed extra header is dropped with a warning rather than failing the
/// read: it only carries second-instance clocks and per-chip volumes, and the
/// bytes survive verbatim in the header either way, so losing the parse costs
/// nothing that matters before playback.
fn read_extra_header(bytes: &[u8], header: &[u8], version: u32) -> Option<ExtraHeader> {
    let relative = u32_at(header, offset::EXTRA_HEADER);
    if relative == 0 {
        return None;
    }
    let at = offset::EXTRA_HEADER + relative as usize;
    match parse_extra_header(bytes, at) {
        Ok(extra) => {
            if version < 0x0000_0170 {
                log::warn!(
                    "VGM has a v1.70 extra header but calls itself v{}; reading it anyway",
                    format_version(version)
                );
            }
            Some(extra)
        }
        Err(error) => {
            log::warn!("Ignoring a malformed VGM extra header at {at:#X}: {error}");
            None
        }
    }
}

fn parse_extra_header(bytes: &[u8], at: usize) -> Result<ExtraHeader> {
    let mut reader = ByteReader::new(bytes);
    reader.seek(at)?;
    let size = reader.u32_le()? as usize;

    // The size covers the extra header's own fields, so it says which of them
    // exist -- a 4-byte one has neither list.
    let clocks_at = (size >= 8).then(|| reader.u32_le()).transpose()?;
    let volumes_at = (size >= 12).then(|| reader.u32_le()).transpose()?;

    let mut extra = ExtraHeader::default();
    // Each offset is relative to its own position: the clock field sits at
    // `at + 4`, the volume field at `at + 8`.
    if let Some(relative) = clocks_at.filter(|&relative| relative != 0) {
        reader.seek(at + 4 + relative as usize)?;
        let count = reader.u8()?;
        for _ in 0..count {
            extra.clocks.push(ExtraClock {
                chip_id: reader.u8()?,
                clock: reader.u32_le()?,
            });
        }
    }
    if let Some(relative) = volumes_at.filter(|&relative| relative != 0) {
        reader.seek(at + 8 + relative as usize)?;
        let count = reader.u8()?;
        for _ in 0..count {
            let chip_id = reader.u8()?;
            let flags = reader.u8()?;
            let volume = reader.u16_le()?;
            extra.volumes.push(ExtraVolume {
                chip_id: chip_id & 0x7F,
                paired: chip_id & 0x80 != 0,
                second_instance: flags & 0x01 != 0,
                relative: volume & 0x8000 != 0,
                volume: volume & 0x7FFF,
            });
        }
    }
    Ok(extra)
}

/// Formats a BCD version field the way the spec writes it: `0x151` is `1.51`.
#[must_use]
pub fn format_version(version: u32) -> String {
    format!("{:X}.{:02X}", (version >> 8) & 0xFF, version & 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VGM_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up.vgm");

    /// Builds a header of `size` bytes declaring `version`, with the data
    /// starting right after it.
    fn header(version: u32, size: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; size];
        bytes[..4].copy_from_slice(super::super::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, version);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (size - offset::DATA_OFFSET) as u32,
        );
        bytes
    }

    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A header plus a one-command body, so the data start is real.
    fn file(header: Vec<u8>) -> Vec<u8> {
        let mut bytes = header;
        bytes.push(0x66);
        bytes
    }

    #[test]
    fn the_chip_table_is_in_enum_order() {
        for (index, spec) in CHIPS.iter().enumerate() {
            assert_eq!(spec.kind as usize, index, "{} is out of order", spec.name);
        }
        assert_eq!(ChipKind::all().count(), CHIP_COUNT);
    }

    #[test]
    fn the_chip_table_is_sorted_by_offset() {
        for pair in CHIPS.windows(2) {
            assert!(
                pair[0].clock < pair[1].clock,
                "{} at {:#X} should precede {} at {:#X}",
                pair[0].name,
                pair[0].clock,
                pair[1].name,
                pair[1].clock
            );
        }
    }

    /// Every clock field must be far enough into the header to be reachable,
    /// and none may collide with a field the spec puts elsewhere.
    #[test]
    fn no_chip_clock_overlaps_a_settings_byte() {
        let settings = [
            offset::SN76489_FEEDBACK,
            offset::SN76489_SHIFT_WIDTH,
            offset::SN76489_FLAGS,
            offset::SEGA_PCM_INTERFACE,
            offset::AY8910_TYPE,
            offset::VOLUME_MODIFIER,
            offset::LOOP_BASE,
            offset::LOOP_MODIFIER,
            offset::EXTRA_HEADER,
            offset::ES5503_CHANNELS,
            offset::C352_CLOCK_DIVIDER,
        ];
        for spec in &CHIPS {
            for byte in spec.clock..spec.clock + 4 {
                assert!(
                    !settings.contains(&byte),
                    "{}'s clock covers {byte:#X}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn the_opl2_fixture_parses() {
        let parsed = VgmHeader::parse(VGM_FIXTURE).unwrap();
        assert_eq!(parsed.version(), 0x151);
        assert_eq!(parsed.version_string(), "1.51");
        assert_eq!(parsed.data_start(), 0x80);
        assert_eq!(parsed.raw(), &VGM_FIXTURE[..0x80]);
        assert_eq!(parsed.total_samples(), 118_320);
        assert_eq!(parsed.loop_offset(), None);
        assert_eq!(parsed.loop_samples(), None);
        assert_eq!(parsed.gd3_offset(), None);
        assert_eq!(parsed.rate(), 1000);

        let chips = parsed.chips();
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].kind, ChipKind::Ym3812);
        assert_eq!(chips[0].clock, 3_579_545);
        assert!(!chips[0].dual);
        assert_eq!(parsed.chip_list(), "YM3812");
        assert!(parsed.is_opl_only());
    }

    #[test]
    fn a_field_past_the_data_start_does_not_exist() {
        // The rule the current reader gets wrong: a minimal header stopping at
        // 0x60 has no YM3812 field at 0x50... and no AY8910 field at 0x74 to
        // misread as a clock either.
        let mut bytes = header(0x151, 0x60);
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        let mut bytes = file(bytes);
        // Command bytes that would read as a fat AY8910 clock if the header
        // were allowed to run past its end.
        bytes.extend_from_slice(&[0xFF; 32]);

        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.data_start(), 0x60);
        assert_eq!(parsed.raw().len(), 0x60);
        let kinds: Vec<ChipKind> = parsed.chips().iter().map(|chip| chip.kind).collect();
        assert_eq!(kinds, [ChipKind::Ym2612], "only the in-header clock counts");
    }

    #[test]
    fn a_pre_1_50_file_starts_its_data_at_0x40() {
        let mut bytes = vec![0u8; 0x40];
        bytes[..4].copy_from_slice(super::super::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x101);
        put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        put_u32(&mut bytes, ChipKind::Ym2413.clock_offset(), 3_579_545);
        // Deliberately garbage: a pre-1.50 file has no data-offset field, so
        // these bytes are not one.
        put_u32(&mut bytes, offset::DATA_OFFSET, 0xDEAD_BEEF);
        let bytes = file(bytes);

        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.data_start(), 0x40);
        assert_eq!(parsed.chip_list(), "SN76489, YM2413");
    }

    #[test]
    fn a_zero_data_offset_also_means_0x40() {
        let mut bytes = header(0x150, 0x40);
        put_u32(&mut bytes, offset::DATA_OFFSET, 0);
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        let bytes = file(bytes);
        assert_eq!(VgmHeader::parse(&bytes).unwrap().data_start(), 0x40);
    }

    #[test]
    fn the_dual_bit_and_the_variant_bit_are_read_apart() {
        let mut bytes = header(0x151, 0x80);
        put_u32(
            &mut bytes,
            ChipKind::Ym2612.clock_offset(),
            7_670_454 | VARIANT_FLAG,
        );
        put_u32(
            &mut bytes,
            ChipKind::Ym2151.clock_offset(),
            3_579_545 | DUAL_CHIP_FLAG,
        );
        let bytes = file(bytes);

        let parsed = VgmHeader::parse(&bytes).unwrap();
        let [ym2612, ym2151] = parsed.chips() else {
            panic!("expected two chips, got {:?}", parsed.chips());
        };
        assert_eq!(ym2612.clock, 7_670_454, "the flag bits are masked off");
        assert!(ym2612.variant && !ym2612.dual);
        assert_eq!(ym2612.label(), "YM3438");
        assert!(ym2151.dual && !ym2151.variant);
        assert_eq!(ym2151.label(), "YM2151 x2");
        assert_eq!(parsed.chip_list(), "YM3438, YM2151 x2");
    }

    /// The T6W28 sets both bits, and is one chip rather than a pair.
    #[test]
    fn a_t6w28_is_named_rather_than_doubled() {
        let mut bytes = header(0x151, 0x80);
        put_u32(
            &mut bytes,
            ChipKind::Sn76489.clock_offset(),
            3_579_545 | VARIANT_FLAG | DUAL_CHIP_FLAG,
        );
        let bytes = file(bytes);
        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert!(parsed.chips()[0].is_t6w28());
        assert_eq!(parsed.chip_list(), "T6W28");
    }

    /// `dro2vgm` sets bit 31 on dual OPL2 clocks, where the spec gives it no
    /// meaning. It must not invent a variant name.
    #[test]
    fn an_unnamed_variant_bit_does_not_rename_the_chip() {
        let mut bytes = header(0x151, 0x80);
        put_u32(
            &mut bytes,
            ChipKind::Ym3812.clock_offset(),
            3_579_545 | VARIANT_FLAG | DUAL_CHIP_FLAG,
        );
        let bytes = file(bytes);
        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.chip_list(), "YM3812 x2");
        assert!(parsed.is_opl_only());
    }

    #[test]
    fn every_chip_the_spec_defines_can_be_declared_and_read_back() {
        // One file with all 42 clocks set, so nothing is missing from the table
        // and no two fields collide.
        let mut bytes = header(0x172, 0x100);
        for (index, kind) in ChipKind::all().enumerate() {
            put_u32(&mut bytes, kind.clock_offset(), 1_000 + index as u32);
        }
        let bytes = file(bytes);

        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.chips().len(), CHIP_COUNT);
        for (index, chip) in parsed.chips().iter().enumerate() {
            assert_eq!(chip.clock, 1_000 + index as u32, "{}", chip.kind.name());
            assert!(!chip.dual && !chip.variant);
        }
        assert!(!parsed.is_opl_only(), "a file this crowded is not OPL-only");
    }

    #[test]
    fn a_chip_from_a_later_version_is_still_read() {
        // Tolerant reader: the byte is inside the header and unambiguous, so a
        // stale version field does not hide the chip.
        let mut bytes = header(0x151, 0x100);
        put_u32(&mut bytes, ChipKind::Mikey.clock_offset(), 16_000_000);
        let bytes = file(bytes);
        assert_eq!(VgmHeader::parse(&bytes).unwrap().chip_list(), "Mikey");
    }

    #[test]
    fn loop_fields_resolve_to_absolute_offsets() {
        let mut bytes = header(0x151, 0x80);
        put_u32(&mut bytes, ChipKind::Ym3812.clock_offset(), 3_579_545);
        put_u32(&mut bytes, offset::LOOP_OFFSET, (0x90 - 0x1C) as u32);
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 44_100);
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 88_200);
        let mut bytes = file(bytes);
        bytes.resize(0x100, 0);

        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.loop_offset(), Some(0x90));
        assert_eq!(parsed.loop_samples(), Some(44_100));
        assert_eq!(parsed.total_samples(), 88_200);
    }

    #[test]
    fn a_zero_loop_offset_is_no_loop_whatever_the_length_says() {
        let mut bytes = header(0x151, 0x80);
        put_u32(&mut bytes, ChipKind::Ym3812.clock_offset(), 3_579_545);
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 44_100);
        let bytes = file(bytes);
        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert_eq!(parsed.loop_offset(), None);
        assert_eq!(parsed.loop_samples(), None);
    }

    #[test]
    fn settings_bytes_are_read() {
        let mut bytes = header(0x171, 0x100);
        put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        bytes[offset::SN76489_FEEDBACK..offset::SN76489_FEEDBACK + 2]
            .copy_from_slice(&0x0009u16.to_le_bytes());
        bytes[offset::SN76489_SHIFT_WIDTH] = 16;
        bytes[offset::AY8910_TYPE] = 0x03;
        bytes[offset::C140_TYPE] = 0x02;
        bytes[offset::ES5503_CHANNELS] = 8;
        bytes[offset::C352_CLOCK_DIVIDER] = 72; // the usual 288, divided by 4
        let bytes = file(bytes);

        let settings = *VgmHeader::parse(&bytes).unwrap().settings();
        assert_eq!(settings.sn76489_feedback, 0x0009);
        assert_eq!(settings.sn76489_shift_width, 16);
        assert_eq!(settings.ay8910_type, 0x03);
        assert_eq!(settings.c140_type, 0x02);
        assert_eq!(settings.es5503_channels, 8);
        assert_eq!(settings.c352_clock_divider, 72);
    }

    /// A settings byte outside a short header reads as zero rather than as a
    /// command byte.
    #[test]
    fn settings_outside_the_header_read_as_zero() {
        let bytes = file(header(0x151, 0x40));
        let settings = *VgmHeader::parse(&bytes).unwrap().settings();
        assert_eq!(settings, ChipSettings::default());
    }

    // -- the v1.70 extra header ---------------------------------------------

    /// Builds a v1.71 file whose extra header sits right after the 0x100
    /// header, listing one second-instance clock and one chip volume.
    fn file_with_extra_header() -> Vec<u8> {
        let mut bytes = header(0x171, 0x100);
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);

        // The extra header itself: {size, clock offset, volume offset}.
        let extra_at = 0xE8;
        put_u32(&mut bytes, offset::EXTRA_HEADER, (extra_at - 0xBC) as u32);
        put_u32(&mut bytes, extra_at, 12);
        // The clock list follows the three fields; the volume list follows it.
        put_u32(&mut bytes, extra_at + 4, 8);
        put_u32(&mut bytes, extra_at + 8, 11);
        let clocks_at = extra_at + 4 + 8;
        bytes[clocks_at] = 1; // one entry
        bytes[clocks_at + 1] = 0x02; // chip id
        put_u32(&mut bytes, clocks_at + 2, 8_000_000);
        let volumes_at = extra_at + 8 + 11;
        bytes[volumes_at] = 1;
        bytes[volumes_at + 1] = 0x02 | 0x80; // paired
        bytes[volumes_at + 2] = 0x01; // second instance
        bytes[volumes_at + 3..volumes_at + 5].copy_from_slice(&(0x8000u16 | 0x180).to_le_bytes());
        file(bytes)
    }

    #[test]
    fn the_extra_header_is_parsed() {
        let parsed = VgmHeader::parse(&file_with_extra_header()).unwrap();
        let extra = parsed.extra().expect("the file declares one");
        assert_eq!(
            extra.clocks,
            [ExtraClock {
                chip_id: 0x02,
                clock: 8_000_000
            }]
        );
        assert_eq!(
            extra.volumes,
            [ExtraVolume {
                chip_id: 0x02,
                paired: true,
                second_instance: true,
                relative: true,
                volume: 0x180,
            }]
        );
    }

    #[test]
    fn no_extra_header_offset_means_no_extra_header() {
        let bytes = file(header(0x171, 0x100));
        assert!(VgmHeader::parse(&bytes).unwrap().extra().is_none());
    }

    /// The extra header is a playback nicety; a broken one must not cost the
    /// file its tags.
    #[test]
    fn a_malformed_extra_header_is_dropped_rather_than_fatal() {
        let mut bytes = file(header(0x171, 0x100));
        put_u32(&mut bytes, offset::EXTRA_HEADER, 0x7FFF_FFFF);
        let parsed = VgmHeader::parse(&bytes).unwrap();
        assert!(parsed.extra().is_none());
        assert_eq!(parsed.version(), 0x171);
    }

    #[test]
    fn a_four_byte_extra_header_has_neither_list() {
        let mut bytes = header(0x171, 0x100);
        put_u32(&mut bytes, offset::EXTRA_HEADER, (0xE8 - 0xBC) as u32);
        put_u32(&mut bytes, 0xE8, 4);
        let bytes = file(bytes);
        let extra = VgmHeader::parse(&bytes).unwrap().extra().cloned().unwrap();
        assert_eq!(extra, ExtraHeader::default());
    }

    // -- rejections ---------------------------------------------------------

    #[test]
    fn rejects_a_bad_magic() {
        let mut bytes = file(header(0x151, 0x80));
        bytes[0] = b'X';
        assert!(VgmHeader::parse(&bytes).is_err());
    }

    #[test]
    fn rejects_a_version_below_1_00() {
        let bytes = file(header(0x99, 0x80));
        let error = VgmHeader::parse(&bytes).unwrap_err().to_string();
        assert!(error.contains("v1.00 is the minimum"), "{error}");
    }

    #[test]
    fn rejects_a_data_offset_past_the_end_of_the_file() {
        let mut bytes = file(header(0x151, 0x80));
        put_u32(&mut bytes, offset::DATA_OFFSET, 0x1000);
        let error = VgmHeader::parse(&bytes).unwrap_err().to_string();
        assert!(error.contains("past the end"), "{error}");
    }

    #[test]
    fn version_formatting_matches_the_spec() {
        assert_eq!(format_version(0x100), "1.00");
        assert_eq!(format_version(0x110), "1.10");
        assert_eq!(format_version(0x151), "1.51");
        assert_eq!(format_version(0x172), "1.72");
    }
}
