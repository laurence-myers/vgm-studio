//! Per-chip write gating: keeping one chip channel silent by filtering the
//! register writes bound for it, for the cores that have no native mute of their
//! own.
//!
//! The OPL `Muting` vocabulary already expresses this
//! ([`Muting::gate`](crate::clock::Muting::gate)): it drops a muted channel's
//! key-on writes and masks its rhythm register. A
//! [`ChannelGate`] is the same idea generalised to any chip and hosted behind a
//! [`ChipCore`](crate::ChipCore) wrapper, so per-channel muting works even on a
//! core (Nuked-OPM, Nuked-PSG, the LLE tier) whose emulator offers no mute of its
//! own. Where a core *does* mute natively (every libvgm device, Nuked-OPN2), that
//! path stays -- output-masking isolates better than write-gating, because it
//! preserves the shared state (envelopes, noise) a channel leaves behind.
//!
//! ## Three strategies, not one
//!
//! "Drop the note-on writes" is right for only some chips. The register that
//! makes a channel audible takes one of three shapes, and the gate answers each
//! differently:
//!
//! - **DROP** -- a clean key-on-only register (the YM2151's `0x08`, the OPN
//!   family's `0x28`): a muted channel's write is dropped whole.
//! - **TRANSFORM** -- the key bit shares a register with pitch or mode, or one
//!   register enables several channels at once (OPL's `0xBn`/`0xBD`): the write
//!   passes with just the muted channel's bit cleared, so the others survive.
//! - **VOLUME** -- no key-on exists; audibility is an attenuation level (the
//!   SN76489, the AY8910 / OPN SSG): a muted channel's volume is forced silent,
//!   and its real level is shadowed so unmuting restores it.
//!
//! Some chips also carry **stateful protocols** -- a latch a byte at a time -- so
//! the gate cannot be a pure function of the write; it shadows what it needs.
//!
//! ## Licence
//!
//! This lives in the permissive `vgms-synth` crate, so like
//! [`chip_docs`](crate::credits) its register knowledge is written from
//! datasheets and the public VGM register documentation, **not** from any GPL
//! emulator's source. The libvgm-verified knowledge in the plan's appendix drives
//! the app-side A/B tests, not this table code.
//!
//! Coverage grows chip by chip. [`ChannelGate::exists`] answers `false` for a
//! chip with no table yet -- which keeps today's honest behaviour: an ungated
//! channel plays, rather than pretending to mute. A chip appears here only once
//! *every* one of its channels can be gated, so `exists` never over-promises;
//! the rhythm/ADPCM-bearing OPN parts (YM2608/YM2610) and the OPLL wait for the
//! A/B harness that validates their tables.

use vgms_core::vgm::{ChipKind, ChipSettings, channels_of};

/// What the gate does with one write.
///
/// [`ChannelGate::filter`] returns one of these; the [`GatedCore`] that hosts the
/// gate applies it against the wrapped core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateAction {
    /// Write it through unchanged.
    Pass,
    /// Drop it: a muted channel's key-on on a clean key-only register.
    Drop,
    /// Write this value instead: the muted channel's key bit cleared, or its
    /// volume forced silent.
    Replace(u16),
}

/// A per-chip write filter plus the shadow state a stateful chip needs.
///
/// One gate serves one chip instance. It is built for a [`ChipKind`]
/// ([`new`](Self::new)), re-keyed on [`reset`](Self::reset) once the header's
/// variant flag is known, told which channels are muted through
/// [`set_mask`](Self::set_mask), and consulted on every write through
/// [`filter`](Self::filter).
#[derive(Debug)]
pub struct ChannelGate {
    kind: ChipKind,
    /// The header's bit-31 variant flag, which for some chips changes the channel
    /// roster. Known only from [`reset`](Self::reset); `false` until then.
    variant: bool,
    /// The roster length for `(kind, variant)`, so the mute mask can be clamped:
    /// the channel splitter deliberately sets `u32::MAX` on the instances it is
    /// not soloing, and only this chip's own channels may be read from it.
    channels: u8,
    /// The channels muted right now, clamped to the roster. Edge-triggered:
    /// [`set_mask`](Self::set_mask) compares against it to emit only what changed.
    mask: u32,
    inner: GateInner,
}

/// The per-family logic and shadow state.
#[derive(Debug)]
enum GateInner {
    /// YM2151 (OPM). Reg `0x08` is a pure key register whose low three bits name
    /// the channel, so a muted channel's `0x08` is dropped. No shadow.
    Opm,
    /// SN76489. No key exists -- audibility is the 4-bit attenuation in the
    /// volume latch -- so a muted channel's level is forced silent and the real
    /// one shadowed. The chip latches a register a byte at a time.
    Sn76489 {
        /// Each channel's last real attenuation, `0x0` loud .. `0xF` silent, as
        /// the chip powers up (`0xF`).
        volumes: [u8; 4],
        /// The last latch byte the song wrote (`1cctdddd`), so a mute-edge volume
        /// force can re-point the latch to the song's last *tone* register.
        last_latch: u8,
    },
    /// The OPN FM family (YM2612, YM2203). FM channels drop reg `0x28`; the
    /// YM2612's DAC forces its sample data to silence; the YM2203's SSG channels
    /// use the volume strategy.
    Opn(Opn),
    /// The OPL family (YM3812/YM3526 = OPL2, YMF262 = OPL3). Melodic channels
    /// clear the key bit of `0xBn` (transform, not drop, so a replay is the same
    /// as live play); the five rhythm voices AND-mask their bits out of `0xBD`.
    Opl(Opl),
    /// A bare AY8910 (three SSG channels) -- the same volume strategy as the OPN
    /// SSG, based at channel 0.
    Ay(Ssg),
}

/// The OPN FM family's layout and SSG shadow.
#[derive(Debug)]
struct Opn {
    /// FM channels (`channels_of` indices `0..fm`): 6 on the YM2612, 3 on the
    /// YM2203.
    fm: u8,
    /// The DAC's channel index, when the chip has one (the YM2612's channel 6).
    dac: Option<u8>,
    /// The SSG voices: their base `channels_of` index and volume shadow, when the
    /// chip has an SSG (the YM2203's channels 3..6). `None` on the YM2612.
    ssg: Option<Ssg>,
}

/// The volume-strategy shadow shared by the AY8910 and the OPN SSG: three
/// channels whose attenuation register is forced silent while muted.
///
/// `base` is the `channels_of` index of SSG channel A, so one struct serves both
/// a bare AY8910 (base 0) and an OPN's SSG block (base 3).
#[derive(Debug)]
struct Ssg {
    base: u8,
    /// Each channel's last real amplitude byte (bits 0-3 level, bit 4 envelope
    /// mode); `0x00` (silent, fixed level) at power-up.
    volumes: [u8; 3],
}

/// The OPL family's melodic width and rhythm shadow.
#[derive(Debug)]
struct Opl {
    /// Melodic channels (`channels_of` indices `0..melodic`): 9 on OPL2, 18 on
    /// OPL3 (two banks of nine, selected by the write's port). The five rhythm
    /// voices follow, at `melodic..melodic+5`.
    melodic: u8,
    /// The last value written to the rhythm register `0xBD`, so a rhythm mute
    /// edge can re-apply it with the muted voices masked out.
    bd_shadow: u8,
}

/// The AY8910 / OPN SSG amplitude registers: reg `0x08` is channel A, `0x09` B,
/// `0x0A` C.
const SSG_VOLUME_BASE: u16 = 0x08;
/// The OPN FM key-on/off register: `SSSS_xCCC`, slots in the high nibble, the
/// channel in the low three bits.
const OPN_KEY: u16 = 0x28;
/// The YM2612 DAC sample-data register; `0x80` is the mid-scale (silent) sample.
const YM2612_DAC_DATA: u16 = 0x2A;
const DAC_SILENCE: u16 = 0x80;
/// The OPL per-channel key/frequency-high registers `0xB0..=0xB8`, whose bit 5 is
/// the key.
const OPL_KEY_LOW: u16 = 0xB0;
const OPL_KEY_HIGH: u16 = 0xB8;
const OPL_KEY_ON: u16 = 0x20;
/// The OPL rhythm register: bits 0-4 key the five drums, bits 5-7 are global
/// controls that must survive.
const OPL_RHYTHM: u16 = 0xBD;

impl ChannelGate {
    /// Whether this build gates `kind` at all -- a build-time predicate over the
    /// kind alone (the variant is not known until [`reset`](Self::reset)).
    ///
    /// Drives which registry rows get a [`GatedCore`] and what `mute_capable`
    /// reports. `false` for a chip with no complete table yet, so an ungated
    /// channel stays honestly un-muteable rather than silently ignored.
    #[must_use]
    pub fn exists(kind: ChipKind) -> bool {
        matches!(
            kind,
            ChipKind::Ym2151
                | ChipKind::Sn76489
                | ChipKind::Ym2612
                | ChipKind::Ym2203
                | ChipKind::Ay8910
                | ChipKind::Ym3812
                | ChipKind::Ym3526
                | ChipKind::Ymf262
        )
    }

    /// A gate for `kind`, or `None` when [`exists`](Self::exists) is `false`.
    #[must_use]
    pub fn new(kind: ChipKind) -> Option<Self> {
        let inner = match kind {
            ChipKind::Ym2151 => GateInner::Opm,
            ChipKind::Sn76489 => GateInner::Sn76489 {
                volumes: [0x0F; 4],
                last_latch: 0,
            },
            ChipKind::Ym2612 => GateInner::Opn(Opn {
                fm: 6,
                dac: Some(6),
                ssg: None,
            }),
            ChipKind::Ym2203 => GateInner::Opn(Opn {
                fm: 3,
                dac: None,
                ssg: Some(Ssg {
                    base: 3,
                    volumes: [0; 3],
                }),
            }),
            ChipKind::Ay8910 => GateInner::Ay(Ssg {
                base: 0,
                volumes: [0; 3],
            }),
            ChipKind::Ym3812 | ChipKind::Ym3526 => GateInner::Opl(Opl {
                melodic: 9,
                bd_shadow: 0,
            }),
            ChipKind::Ymf262 => GateInner::Opl(Opl {
                melodic: 18,
                bd_shadow: 0,
            }),
            _ => return None,
        };
        let mut gate = Self {
            kind,
            variant: false,
            channels: channel_count(kind, false),
            mask: 0,
            inner,
        };
        gate.channels = channel_count(kind, false);
        Some(gate)
    }

    /// Re-keys the tables for a chip clocked at `clock`, whose header carries
    /// `variant`, and clears the shadows.
    ///
    /// Forwarded from [`ChipCore::reset`](crate::ChipCore::reset). The mute mask
    /// does not survive a reset -- the engine restates it through
    /// [`set_mask`](Self::set_mask) right after -- so this drops it to "nothing
    /// muted", against which that restatement re-emits every still-muted channel.
    pub fn reset(&mut self, _clock: u32, variant: bool) {
        self.variant = variant;
        self.channels = channel_count(self.kind, variant);
        self.mask = 0;
        match &mut self.inner {
            GateInner::Opm => {}
            GateInner::Sn76489 {
                volumes,
                last_latch,
            } => {
                *volumes = [0x0F; 4];
                *last_latch = 0;
            }
            GateInner::Opn(opn) => {
                if let Some(ssg) = &mut opn.ssg {
                    ssg.volumes = [0; 3];
                }
            }
            GateInner::Opl(opl) => opl.bd_shadow = 0,
            GateInner::Ay(ssg) => ssg.volumes = [0; 3],
        }
    }

    /// Hands the gate the header's per-chip settings, for the chips whose channel
    /// map they change (the C140 vs C219 distinction). A no-op for the families
    /// covered so far.
    pub fn configure(&mut self, _settings: &ChipSettings) {}

    /// The mask of every channel this chip has, so an out-of-roster bit (the
    /// split's `u32::MAX` on other instances) is ignored.
    fn roster_mask(&self) -> u32 {
        if self.channels >= 32 {
            u32::MAX
        } else {
            (1u32 << self.channels) - 1
        }
    }

    /// Sets which channels are muted, appending the writes that carry the change
    /// into `out` for the host to apply to the inner core.
    ///
    /// Edge-triggered and idempotent: only the channels whose state changed emit
    /// anything, so the engine restating the same mask after every seek or pan
    /// change (its `apply_mix`) writes nothing. A channel going audible->muted
    /// emits a key-off or a volume force; a channel going muted->audible restores
    /// a level-type channel but never re-keys an edge-triggered one -- a muted
    /// channel rejoins at its next natural key-on, exactly as OPL muting does.
    ///
    /// After a whole-chip stand-down (§1.1), the host calls
    /// [`reassert_mask`](Self::reassert_mask) instead, since the stored mask can
    /// no longer be trusted as the baseline.
    pub fn set_mask(&mut self, mask: u32, out: &mut Vec<(u8, u16, u16)>) {
        self.apply_mask(mask, self.mask, false, out);
    }

    /// As [`set_mask`](Self::set_mask), but re-asserting every channel outright
    /// rather than diffing against the stored mask -- how a
    /// [`GatedCore`](crate::channel_gate) leaves the whole-chip stand-down (§1.1).
    ///
    /// While standing down the gate passed every write untouched, so nothing about
    /// the inner core's per-channel state can be assumed: a channel muted here has
    /// its real (audible) level or key on the chip and must be forced/keyed off,
    /// and a channel audible here that was forced silent *before* the stand-down
    /// and never rewritten during it is still silent and must be restored. So a
    /// volume channel is set to exactly what the mask asks for, unconditionally;
    /// key channels (which rejoin at their next natural key-on) only need the
    /// muted ones keyed off.
    pub fn reassert_mask(&mut self, mask: u32, out: &mut Vec<(u8, u16, u16)>) {
        self.apply_mask(mask, 0, true, out);
    }

    /// Whether `mask` mutes every channel this chip has -- a whole-chip mute.
    ///
    /// The host stands the gate down for one of these (see
    /// [`stand_down`](Self::stand_down)): the engine's own whole-chip silence
    /// (`Voice::silenced`) already zeroes the output, and letting the chip state
    /// evolve untouched is what makes un-muting resume held notes, exactly as a
    /// native-mute chip does.
    #[must_use]
    pub fn is_full(&self, mask: u32) -> bool {
        let roster = self.roster_mask();
        mask & roster == roster
    }

    /// Enters the whole-chip stand-down: forgets the mute mask, so
    /// [`filter`](Self::filter) passes every write and keeps the shadows current,
    /// while emitting nothing. [`reassert_mask`](Self::reassert_mask) then
    /// re-establishes per-channel state when the chip is partly un-muted again.
    pub fn stand_down(&mut self) {
        self.mask = 0;
    }

    fn apply_mask(&mut self, mask: u32, old: u32, reassert: bool, out: &mut Vec<(u8, u16, u16)>) {
        let mask = mask & self.roster_mask();
        let channels = self.channels;
        match &mut self.inner {
            GateInner::Opm => {
                // A key chip: only the muted channels need keying off; an audible
                // one rejoins at its next natural key-on, so `reassert` (which
                // passes `old == 0`) needs no extra work here.
                for channel in 0..channels {
                    if newly_muted(old, mask, channel) {
                        out.push((0, 0x08, u16::from(channel)));
                    }
                }
            }
            GateInner::Sn76489 {
                volumes,
                last_latch,
            } => sn76489_set_mask(volumes, *last_latch, old, mask, reassert, channels, out),
            GateInner::Opn(opn) => opn.set_mask(old, mask, reassert, out),
            GateInner::Opl(opl) => opl.set_mask(old, mask, out),
            GateInner::Ay(ssg) => ssg.set_mask(old, mask, reassert, out),
        }
        self.mask = mask;
    }

    /// The channel a DAC stream (`0x90`-`0x95`) writing `register` on `port`
    /// drives, if this chip has a stream-fed channel there.
    ///
    /// The song-format splitter uses it to tell whether a stream is bound to a
    /// muted channel: it must drop the *start* of such a stream, because the
    /// stream's samples are synthesised at render time and would otherwise sound
    /// on a channel this stem means to silence. Among the gated chips only the
    /// YM2612's DAC (register `0x2A` → channel 6) is stream-fed; everything else
    /// answers `None`, and the splitter keeps the stream (silencing a real voice
    /// is worse than leaving one it does not model).
    #[must_use]
    pub fn stream_channel(&self, port: u8, register: u8) -> Option<u8> {
        match &self.inner {
            GateInner::Opn(opn) => opn
                .dac
                .filter(|_| port == 0 && u16::from(register) == YM2612_DAC_DATA),
            _ => None,
        }
    }

    /// The action for one write, updating any shadow state the chip keeps
    /// (latches, levels) whether or not the channel is muted.
    pub fn filter(&mut self, port: u8, addr: u16, data: u16) -> GateAction {
        let mask = self.mask;
        match &mut self.inner {
            GateInner::Opm => {
                if addr == 0x08 {
                    // Reg 0x08: the low three bits name the channel to key.
                    let channel = u32::from(data & 0x07);
                    if mask & (1 << channel) != 0 {
                        return GateAction::Drop;
                    }
                }
                GateAction::Pass
            }
            GateInner::Sn76489 {
                volumes,
                last_latch,
            } => sn76489_filter(volumes, last_latch, mask, addr, data),
            GateInner::Opn(opn) => opn.filter(mask, port, addr, data),
            GateInner::Opl(opl) => opl.filter(mask, port, addr, data),
            GateInner::Ay(ssg) => ssg.filter(mask, addr, data),
        }
    }
}

// -- SN76489 -------------------------------------------------------------------

fn sn76489_filter(
    volumes: &mut [u8; 4],
    last_latch: &mut u8,
    mask: u32,
    addr: u16,
    data: u16,
) -> GateAction {
    // Address 1 is the Game Gear stereo mask; only address 0 carries the command
    // byte the latches ride.
    if addr != 0 {
        return GateAction::Pass;
    }
    let byte = data as u8;
    if byte & 0x80 != 0 {
        // Latch byte `1cctdddd`: cc = channel, t = tone(0)/volume(1).
        *last_latch = byte;
        if byte & 0x10 != 0 {
            let channel = usize::from((byte >> 5) & 0x03);
            // Shadow the real level even while muted, so unmute can restore it.
            volumes[channel] = byte & 0x0F;
            if mask & (1u32 << channel) != 0 {
                // Muted: force silent (`vvvv = 0xF`).
                return GateAction::Replace(u16::from(byte | 0x0F));
            }
        }
    }
    // A data byte `0-dddddd` extends the last-latched *tone* register; a volume is
    // a single 4-bit latch, so a data byte never carries a level and always passes.
    GateAction::Pass
}

fn sn76489_set_mask(
    volumes: &[u8; 4],
    last_latch: u8,
    old: u32,
    mask: u32,
    reassert: bool,
    channels: u8,
    out: &mut Vec<(u8, u16, u16)>,
) {
    let mut synthesised = false;
    for channel in 0..channels.min(4) {
        let cc = u16::from(channel);
        let silent = 0x90 | (cc << 5) | 0x0F;
        let real = 0x90 | (cc << 5) | u16::from(volumes[channel as usize]);
        let muted = mask & (1u32 << channel) != 0;
        // `reassert` sets every channel to its exact state; otherwise only the
        // channels whose state changed emit anything.
        let write = if reassert {
            Some(if muted { silent } else { real })
        } else if newly_muted(old, mask, channel) {
            Some(silent)
        } else if newly_audible(old, mask, channel) {
            Some(real)
        } else {
            None
        };
        if let Some(write) = write {
            out.push((0, 0, write));
            synthesised = true;
        }
    }
    // A synthesised volume latch re-pointed the chip's latch. Re-point it to the
    // song's last latch *only* if that was a tone register (channels 0-2, type
    // bit clear): a data byte may still be coming for its high frequency bits.
    // A volume latch is self-contained (no data byte follows), and the noise
    // register (channel 3) likewise takes no data byte -- and re-emitting it
    // would reset the noise LFSR to its seed -- so neither is re-pointed.
    let cc = (last_latch >> 5) & 0x03;
    if synthesised && last_latch & 0x80 != 0 && last_latch & 0x10 == 0 && cc != 3 {
        out.push((0, 0, u16::from(last_latch)));
    }
}

// -- OPN FM (YM2612, YM2203) ---------------------------------------------------

impl Opn {
    fn filter(&mut self, mask: u32, port: u8, addr: u16, data: u16) -> GateAction {
        // FM key-on/off: reg 0x28 on port 0, the channel in the low three bits.
        if addr == OPN_KEY
            && let Some(channel) = opn_key_channel(data)
            && channel < self.fm
            && mask & (1u32 << channel) != 0
        {
            return GateAction::Drop;
        }
        // The YM2612's DAC: force its sample data to mid-scale while muted.
        if let Some(dac) = self.dac
            && addr == YM2612_DAC_DATA
            && mask & (1u32 << dac) != 0
        {
            return GateAction::Replace(DAC_SILENCE);
        }
        // The SSG block shares port 0's low registers.
        if let Some(ssg) = &mut self.ssg {
            return ssg.filter(mask, addr, data);
        }
        let _ = port;
        GateAction::Pass
    }

    fn set_mask(&mut self, old: u32, mask: u32, reassert: bool, out: &mut Vec<(u8, u16, u16)>) {
        // FM and the DAC are key/edge channels: keying off the muted ones is
        // enough, and `reassert` reaches here with `old == 0` so every muted one
        // is covered.
        for channel in 0..self.fm {
            if newly_muted(old, mask, channel) {
                out.push((0, OPN_KEY, u16::from(opn_key_field(channel))));
            }
        }
        if let Some(dac) = self.dac
            && newly_muted(old, mask, dac)
        {
            // Silence a DAC caught mid-sample: force one mid-scale write now.
            out.push((0, YM2612_DAC_DATA, DAC_SILENCE));
        }
        // The SSG is a volume block, so it honours `reassert` (restore audible,
        // force muted).
        if let Some(ssg) = &mut self.ssg {
            ssg.set_mask(old, mask, reassert, out);
        }
    }
}

/// The `channels_of` FM channel a reg-`0x28` write targets, if any. The data's
/// low three bits are `0,1,2` for FM 1-3 and `4,5,6` for FM 4-6 (bit 2 is the
/// bank); `3` and `7` are unused.
fn opn_key_channel(data: u16) -> Option<u8> {
    match (data & 0x07) as u8 {
        field @ 0..=2 => Some(field),
        field @ 4..=6 => Some(field - 1),
        _ => None,
    }
}

/// The reg-`0x28` channel field for a `channels_of` FM channel: the inverse of
/// [`opn_key_channel`] (`0,1,2 -> 0,1,2`, `3,4,5 -> 4,5,6`).
fn opn_key_field(channel: u8) -> u8 {
    if channel < 3 { channel } else { channel + 1 }
}

// -- AY8910 / OPN SSG (VOLUME) -------------------------------------------------

impl Ssg {
    fn filter(&mut self, mask: u32, addr: u16, data: u16) -> GateAction {
        // Reg 0x08/0x09/0x0A: channel A/B/C amplitude.
        if (SSG_VOLUME_BASE..SSG_VOLUME_BASE + 3).contains(&addr) {
            let local = (addr - SSG_VOLUME_BASE) as usize;
            self.volumes[local] = data as u8;
            let channel = self.base + local as u8;
            if mask & (1u32 << channel) != 0 {
                // Force fixed level 0 (and clear the envelope-mode bit, so an
                // envelope does not sound in place of the forced level).
                return GateAction::Replace(0);
            }
        }
        GateAction::Pass
    }

    fn set_mask(&mut self, old: u32, mask: u32, reassert: bool, out: &mut Vec<(u8, u16, u16)>) {
        for local in 0..3u8 {
            let channel = self.base + local;
            let addr = SSG_VOLUME_BASE + u16::from(local);
            let real = u16::from(self.volumes[local as usize]);
            let muted = mask & (1u32 << channel) != 0;
            // `reassert` sets every channel outright; otherwise only edges emit.
            if reassert {
                out.push((0, addr, if muted { 0 } else { real }));
            } else if newly_muted(old, mask, channel) {
                out.push((0, addr, 0));
            } else if newly_audible(old, mask, channel) {
                out.push((0, addr, real));
            }
        }
    }
}

// -- OPL (TRANSFORM) -----------------------------------------------------------

impl Opl {
    fn filter(&mut self, mask: u32, port: u8, addr: u16, data: u16) -> GateAction {
        // A melodic key/frequency register: 0xB0..=0xB8, the bank selected by the
        // write's port (OPL3's second register array).
        if (OPL_KEY_LOW..=OPL_KEY_HIGH).contains(&addr) {
            let channel = port * 9 + (addr - OPL_KEY_LOW) as u8;
            if channel < self.melodic && mask & (1u32 << channel) != 0 {
                // Clear the key bit but keep the frequency, so a seek replay lands
                // the same as live play (the frequency stays complete).
                return GateAction::Replace(data & !OPL_KEY_ON);
            }
            return GateAction::Pass;
        }
        // The rhythm register: mask out the muted drums' bits, keep the global
        // control bits (5-7). Only the low bank (port 0) carries it.
        if addr == OPL_RHYTHM && port == 0 {
            self.bd_shadow = data as u8;
            let drum_mask = self.drum_mask(mask);
            if drum_mask != 0xFF {
                return GateAction::Replace(u16::from((data as u8) & drum_mask));
            }
        }
        GateAction::Pass
    }

    fn set_mask(&mut self, old: u32, mask: u32, out: &mut Vec<(u8, u16, u16)>) {
        for channel in 0..self.melodic {
            if newly_muted(old, mask, channel) {
                // Key-off: clear 0xBn entirely. The frequency is re-established at
                // the channel's next natural key-on after unmute.
                let bank = u16::from(channel / 9);
                let reg = OPL_KEY_LOW + u16::from(channel % 9);
                out.push((bank as u8, reg, 0));
            }
        }
        // If which drums are masked changed, re-apply the shadowed 0xBD masked.
        if self.drum_mask(old) != self.drum_mask(mask) {
            out.push((
                0,
                OPL_RHYTHM,
                u16::from(self.bd_shadow & self.drum_mask(mask)),
            ));
        }
    }

    /// The AND-mask for `0xBD`: the five drum bits (0-4) of every muted drum
    /// cleared, the control bits (5-7) kept. `channels_of` lists the drums after
    /// the melodic channels as Bass Drum, Snare, Tom, Cymbal, Hi-Hat, which map to
    /// `0xBD` bits 4, 3, 2, 1, 0.
    fn drum_mask(&self, mask: u32) -> u8 {
        let mut and = 0xFFu8;
        for drum in 0..5u8 {
            let channel = self.melodic + drum;
            if mask & (1u32 << channel) != 0 {
                and &= !(1u8 << (4 - drum));
            }
        }
        and
    }
}

// -- shared helpers ------------------------------------------------------------

/// The channel count for `(kind, variant)` as a `u8` -- the roster is never
/// longer than 32, so this cannot truncate.
fn channel_count(kind: ChipKind, variant: bool) -> u8 {
    channels_of(kind, variant).len() as u8
}

/// Whether `channel` is muted in `new` but was not in `old`.
fn newly_muted(old: u32, new: u32, channel: u8) -> bool {
    let bit = 1u32 << channel;
    new & bit != 0 && old & bit == 0
}

/// Whether `channel` is audible in `new` but was muted in `old`.
fn newly_audible(old: u32, new: u32, channel: u8) -> bool {
    let bit = 1u32 << channel;
    new & bit == 0 && old & bit != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(kind: ChipKind) -> ChannelGate {
        ChannelGate::new(kind).unwrap_or_else(|| panic!("{kind:?} is gated"))
    }

    #[test]
    fn only_the_covered_chips_are_gated() {
        for kind in [
            ChipKind::Ym2151,
            ChipKind::Sn76489,
            ChipKind::Ym2612,
            ChipKind::Ym2203,
            ChipKind::Ay8910,
            ChipKind::Ym3812,
            ChipKind::Ymf262,
        ] {
            assert!(ChannelGate::exists(kind), "{kind:?} should be gated");
            assert!(ChannelGate::new(kind).is_some());
        }
        // Not covered yet: honestly report no gate rather than pretend to mute.
        for kind in [ChipKind::Ym2608, ChipKind::Ym2610, ChipKind::Y8950] {
            assert!(!ChannelGate::exists(kind), "{kind:?} not covered yet");
            assert!(ChannelGate::new(kind).is_none());
        }
    }

    // -- OPM (DROP) ----------------------------------------------------------

    #[test]
    fn opm_drops_a_muted_channels_key_and_passes_the_rest() {
        let mut gate = make(ChipKind::Ym2151);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out); // mute channel 2
        assert_eq!(out, [(0, 0x08, 0x02)], "the mute edge keys channel 2 off");

        assert_eq!(gate.filter(0, 0x08, 0x78 | 0x02), GateAction::Drop);
        assert_eq!(gate.filter(0, 0x08, 0x78 | 0x03), GateAction::Pass);
        assert_eq!(gate.filter(0, 0x28, 0x0A), GateAction::Pass);
    }

    #[test]
    fn opm_restating_the_same_mask_emits_nothing() {
        let mut gate = make(ChipKind::Ym2151);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out);
        out.clear();
        gate.set_mask(0b0000_0100, &mut out);
        assert!(out.is_empty(), "a no-edge restatement is silent");
    }

    #[test]
    fn opm_unmuting_re_keys_nothing() {
        let mut gate = make(ChipKind::Ym2151);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out);
        out.clear();
        gate.set_mask(0, &mut out);
        assert!(
            out.is_empty(),
            "a channel rejoins at its next natural key-on"
        );
        assert_eq!(gate.filter(0, 0x08, 0x78 | 0x02), GateAction::Pass);
    }

    // -- SN76489 (VOLUME, latch-aware) ---------------------------------------

    #[test]
    fn psg_forces_a_muted_channels_volume_silent_and_shadows_the_real_one() {
        let mut gate = make(ChipKind::Sn76489);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out); // mute channel 0
        assert_eq!(out, [(0, 0, 0x9F)], "force channel 0's attenuation to 0xF");

        assert_eq!(gate.filter(0, 0, 0x90), GateAction::Replace(0x9F));
        assert_eq!(gate.filter(0, 0, 0xB0), GateAction::Pass);
        assert_eq!(gate.filter(0, 0, 0x80), GateAction::Pass);
    }

    #[test]
    fn psg_unmuting_restores_the_shadowed_volume() {
        let mut gate = make(ChipKind::Sn76489);
        assert_eq!(gate.filter(0, 0, 0x93), GateAction::Pass);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out);
        out.clear();
        gate.set_mask(0, &mut out);
        assert_eq!(out, [(0, 0, 0x93)], "the real level, not a guess");
    }

    #[test]
    fn psg_re_points_the_latch_after_a_synthesised_write() {
        let mut gate = make(ChipKind::Sn76489);
        assert_eq!(gate.filter(0, 0, 0xA0), GateAction::Pass); // latch channel 1's tone
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out);
        assert_eq!(out, [(0, 0, 0x9F), (0, 0, 0xA0)], "force then re-point");
    }

    #[test]
    fn psg_does_not_re_point_after_a_volume_latch() {
        let mut gate = make(ChipKind::Sn76489);
        assert_eq!(gate.filter(0, 0, 0xB0), GateAction::Pass); // last latch was a volume
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out);
        assert_eq!(
            out,
            [(0, 0, 0x9F)],
            "no spurious re-point after a volume latch"
        );
    }

    #[test]
    fn psg_does_not_re_point_after_a_noise_control_latch() {
        // The noise register (channel 3, 0xE0) is self-contained -- no data byte
        // follows it -- and re-emitting it would reset the noise LFSR. So a mute
        // edge must not re-point to it, even though its type bit is clear.
        let mut gate = make(ChipKind::Sn76489);
        assert_eq!(gate.filter(0, 0, 0xE0), GateAction::Pass); // latch the noise control
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out); // mute tone 0
        assert_eq!(
            out,
            [(0, 0, 0x9F)],
            "no re-point to the noise register: it would reset the LFSR"
        );
    }

    #[test]
    fn psg_clamps_a_mask_to_the_roster() {
        let mut gate = make(ChipKind::Sn76489); // 4 channels
        let mut out = Vec::new();
        gate.set_mask(u32::MAX, &mut out);
        assert_eq!(
            out,
            [(0, 0, 0x9F), (0, 0, 0xBF), (0, 0, 0xDF), (0, 0, 0xFF)]
        );
    }

    // -- OPN FM + DAC + SSG --------------------------------------------------

    #[test]
    fn ym2612_drops_fm_keys_and_silences_the_dac() {
        let mut gate = make(ChipKind::Ym2612); // FM 1-6 (0-5) + DAC (6)
        let mut out = Vec::new();
        gate.set_mask(0b0000_1000 | 0b0100_0000, &mut out); // mute FM4 (idx 3) + DAC (idx 6)
        // FM4 keys off via 0x28 field 4; the DAC gets one mid-scale write.
        assert_eq!(out, [(0, 0x28, 0x04), (0, YM2612_DAC_DATA, DAC_SILENCE)]);

        // A key-on for FM4 (field 4) is dropped; FM5 (field 5) passes.
        assert_eq!(gate.filter(0, 0x28, 0xF0 | 0x04), GateAction::Drop);
        assert_eq!(gate.filter(0, 0x28, 0xF0 | 0x05), GateAction::Pass);
        // DAC data is forced to silence; an unmuted DAC would pass.
        assert_eq!(
            gate.filter(0, YM2612_DAC_DATA, 0xFF),
            GateAction::Replace(DAC_SILENCE)
        );
    }

    #[test]
    fn ym2203_drops_fm_keys_and_forces_ssg_volumes() {
        let mut gate = make(ChipKind::Ym2203); // FM 1-3 (0-2) + SSG A/B/C (3-5)
        // SSG B is channels_of index 4 -> reg 0x09.
        assert_eq!(gate.filter(0, 0x09, 0x0C), GateAction::Pass); // shadow B's level
        let mut out = Vec::new();
        gate.set_mask(0b0000_0010 | 0b0001_0000, &mut out); // mute FM2 (idx1) + SSG B (idx4)
        // FM2 keys off (field 1); SSG B's volume is forced to 0.
        assert_eq!(out, [(0, 0x28, 0x01), (0, 0x09, 0x00)]);

        // FM3 (index 2) is not muted, so its key passes.
        assert_eq!(gate.filter(0, 0x28, 0xF0 | 0x02), GateAction::Pass);

        // While muted, SSG B's volume writes are forced silent -- but the shadow
        // still tracks them, so unmuting restores B's *last* real level (the 0x0F
        // the song wrote while muted), not the earlier 0x0C.
        assert_eq!(gate.filter(0, 0x09, 0x0F), GateAction::Replace(0));
        out.clear();
        gate.set_mask(0, &mut out);
        assert_eq!(out, [(0, 0x09, 0x0F)]);
    }

    // -- AY8910 (VOLUME) -----------------------------------------------------

    #[test]
    fn ay8910_forces_a_muted_channels_volume() {
        let mut gate = make(ChipKind::Ay8910); // A/B/C at 0-2, regs 0x08-0x0A
        assert_eq!(gate.filter(0, 0x0A, 0x0D), GateAction::Pass); // shadow C
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out); // mute channel C (idx 2)
        assert_eq!(out, [(0, 0x0A, 0x00)]);
        assert_eq!(gate.filter(0, 0x0A, 0x0F), GateAction::Replace(0));
        assert_eq!(
            gate.filter(0, 0x08, 0x0F),
            GateAction::Pass,
            "channel A untouched"
        );
    }

    // -- OPL (TRANSFORM) -----------------------------------------------------

    #[test]
    fn opl2_clears_a_muted_melodic_channels_key_bit() {
        let mut gate = make(ChipKind::Ym3812); // 9 melodic (0-8) + 5 drums (9-13)
        let mut out = Vec::new();
        gate.set_mask(0b0000_0010, &mut out); // mute FM2 (idx 1)
        assert_eq!(out, [(0, 0xB1, 0x00)], "FM2 keyed off");

        // A 0xB1 write for FM2 keeps its frequency but loses the key bit (0x20).
        assert_eq!(gate.filter(0, 0xB1, 0x3F), GateAction::Replace(0x1F));
        // FM1's key passes untouched.
        assert_eq!(gate.filter(0, 0xB0, 0x3F), GateAction::Pass);
    }

    #[test]
    fn opl3_uses_the_port_as_the_bank() {
        let mut gate = make(ChipKind::Ymf262); // 18 melodic (0-17) + 5 drums (18-22)
        // Channel 10 (index 9) is bank 1 (port 1), reg 0xB0.
        let mut out = Vec::new();
        gate.set_mask(1 << 9, &mut out);
        assert_eq!(out, [(1, 0xB0, 0x00)], "the second bank keys off on port 1");
        assert_eq!(gate.filter(1, 0xB0, 0x3F), GateAction::Replace(0x1F));
        // The same reg on port 0 is a *different* channel (index 0), unmuted.
        assert_eq!(gate.filter(0, 0xB0, 0x3F), GateAction::Pass);
    }

    #[test]
    fn opl_masks_muted_drums_out_of_the_rhythm_register() {
        let mut gate = make(ChipKind::Ym3812); // drums at 9..14: BD SD TT CY HH
        // Prime the 0xBD shadow with rhythm mode on and all drums keyed.
        assert_eq!(gate.filter(0, 0xBD, 0x3F), GateAction::Pass);
        let mut out = Vec::new();
        gate.set_mask(1 << 9, &mut out); // mute Bass Drum (idx 9 -> 0xBD bit 4)
        // The shadowed 0xBD is re-applied with bit 4 cleared (0x3F & 0xEF = 0x2F).
        assert_eq!(out, [(0, 0xBD, 0x2F)]);
        // A later 0xBD write is masked the same way; the control bits (5) survive.
        assert_eq!(gate.filter(0, 0xBD, 0x3F), GateAction::Replace(0x2F));
        // With no drum muted the register passes untouched.
        let mut gate = make(ChipKind::Ym3812);
        assert_eq!(gate.filter(0, 0xBD, 0x1F), GateAction::Pass);
    }

    // -- stand-down re-assertion (§1.1) --------------------------------------

    #[test]
    fn reassert_sets_every_channel_from_a_clean_baseline() {
        // Leaving the whole-chip stand-down: writes flowed untouched, so the inner
        // core's state is unknown. reassert_mask re-forces every muted channel and
        // restores every audible one, regardless of the gate's stored mask.
        let mut gate = make(ChipKind::Ym2203);
        gate.filter(0, 0x08, 0x0A); // SSG A shadow = 0x0A
        let mut out = Vec::new();
        gate.set_mask(0b0000_1000, &mut out); // SSG A (idx 3) muted -- stored mask
        out.clear();

        // Now re-assert the same mask against a clean baseline: SSG A is still
        // muted (re-forced), and the audible SSG B/C are restored to their
        // shadows -- even though set_mask against the stored mask would have been
        // silent.
        gate.reassert_mask(0b0000_1000, &mut out);
        assert!(
            out.contains(&(0, 0x08, 0x00)),
            "still-muted SSG A is re-forced silent: {out:?}"
        );
        assert!(
            out.contains(&(0, 0x09, 0x00)) && out.contains(&(0, 0x0A, 0x00)),
            "audible SSG B/C restored to their (power-up 0) shadows: {out:?}"
        );
    }

    // -- reset ---------------------------------------------------------------

    #[test]
    fn a_reset_clears_the_shadows_and_the_mask() {
        let mut gate = make(ChipKind::Sn76489);
        gate.filter(0, 0, 0x93); // channel 0 -> attenuation 0x3
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out);

        gate.reset(3_579_545, false);
        out.clear();
        gate.set_mask(0b0000_0001, &mut out);
        assert_eq!(
            out,
            [(0, 0, 0x9F)],
            "muting again forces silence, not a stale shadow"
        );
        out.clear();
        gate.set_mask(0, &mut out);
        assert_eq!(
            out,
            [(0, 0, 0x9F)],
            "reset restored the powered-up 0xF level"
        );
    }
}
