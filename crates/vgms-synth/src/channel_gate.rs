//! Per-chip write gating: keeping one chip channel silent by filtering the
//! register writes bound for it, for the cores that have no native mute of their
//! own.
//!
//! The OPL player already does this ([`Muting::gate`](crate::engine::Muting::gate)):
//! it drops a muted channel's key-on writes and masks its rhythm register. A
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
//! - **DROP** -- a clean key-on-only register (the YM2151's `0x08`): a muted
//!   channel's write is dropped whole.
//! - **TRANSFORM** -- the key bit shares a register with pitch or mode, or one
//!   register enables several channels at once: the write passes with the muted
//!   channel's bit cleared, so the others survive.
//! - **VOLUME** -- no key-on exists; audibility is an attenuation level (the
//!   SN76489): a muted channel's volume is forced silent, and its real level is
//!   shadowed so unmuting restores it.
//!
//! Some chips also carry **stateful protocols** -- a latch a byte at a time, a
//! channel-select register -- so the gate cannot be a pure function of the write;
//! it shadows what it needs.
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
//! chip with no table yet, which keeps today's honest behaviour: an ungated
//! channel plays, rather than pretending to mute.

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
    /// Write this value instead: the muted channel's bit cleared, or its volume
    /// forced silent.
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
    /// YM2151 (OPM). Reg `0x08` is a pure key-on/off register whose low three
    /// bits name the channel, so a muted channel's `0x08` is simply dropped.
    Opm,
    /// SN76489. A muted channel has no key to drop -- audibility is the 4-bit
    /// attenuation in its volume latch -- so the level is forced silent and the
    /// real one shadowed. The chip latches a register a byte at a time, so the
    /// last latch byte is kept to re-point after a synthesised write.
    Sn76489 {
        /// Each channel's last real attenuation, `0x0` loud .. `0xF` silent, as
        /// the chip powers up (`0xF`). Shadowed on every volume write so unmuting
        /// restores the level the song set, not a guess.
        volumes: [u8; 4],
        /// The last latch byte the song wrote (`1cctdddd`), so a mute-edge volume
        /// force can re-point the latch to where the song left it.
        last_latch: u8,
    },
}

impl ChannelGate {
    /// Whether this build gates `kind` at all -- a build-time predicate over the
    /// kind alone (the variant is not known until [`reset`](Self::reset)).
    ///
    /// Drives which registry rows get a [`GatedCore`] and what
    /// `mute_capable` reports. `false` for a chip with no table yet, so an
    /// ungated channel stays honestly un-muteable rather than silently ignored.
    #[must_use]
    pub fn exists(kind: ChipKind) -> bool {
        matches!(kind, ChipKind::Ym2151 | ChipKind::Sn76489)
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
            _ => return None,
        };
        let mut gate = Self {
            kind,
            variant: false,
            channels: 0,
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
    pub fn set_mask(&mut self, mask: u32, out: &mut Vec<(u8, u16, u16)>) {
        let mask = mask & self.roster_mask();
        let old = self.mask;
        let channels = self.channels;
        match &mut self.inner {
            GateInner::Opm => {
                for channel in 0..channels {
                    if newly_muted(old, mask, channel) {
                        // Key-off: reg 0x08 with every slot bit clear and the
                        // channel in the low three bits.
                        out.push((0, 0x08, u16::from(channel)));
                    }
                    // Unmute re-keys nothing: the channel sounds again at its
                    // next natural key-on.
                }
            }
            GateInner::Sn76489 {
                volumes,
                last_latch,
            } => {
                let mut synthesised = false;
                for channel in 0..channels.min(4) {
                    let cc = u16::from(channel);
                    if newly_muted(old, mask, channel) {
                        // Force the attenuation to silent (`vvvv = 0xF`).
                        out.push((0, 0, 0x90 | (cc << 5) | 0x0F));
                        synthesised = true;
                    } else if newly_audible(old, mask, channel) {
                        // Restore the level the song had set.
                        out.push((
                            0,
                            0,
                            0x90 | (cc << 5) | u16::from(volumes[channel as usize]),
                        ));
                        synthesised = true;
                    }
                }
                // A synthesised volume latch re-pointed the chip's latch. If the
                // song's last latch was a tone register, a data byte may still be
                // coming for it, so re-point the latch to where the song left it.
                // A volume latch needs no data byte, so it needs no re-point.
                if synthesised && *last_latch & 0x80 != 0 && *last_latch & 0x10 == 0 {
                    out.push((0, 0, u16::from(*last_latch)));
                }
            }
        }
        self.mask = mask;
    }

    /// The action for one write, updating any shadow state the chip keeps
    /// (latches, levels) whether or not the channel is muted.
    pub fn filter(&mut self, _port: u8, addr: u16, data: u16) -> GateAction {
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
            } => {
                // Address 1 is the Game Gear stereo mask; only address 0 carries
                // the command byte the latches ride.
                if addr != 0 {
                    return GateAction::Pass;
                }
                let byte = data as u8;
                if byte & 0x80 != 0 {
                    // Latch byte `1cctdddd`: cc = channel, t = tone(0)/volume(1).
                    *last_latch = byte;
                    if byte & 0x10 != 0 {
                        let channel = usize::from((byte >> 5) & 0x03);
                        // Shadow the real level even while muted, so unmute can
                        // restore it.
                        volumes[channel] = byte & 0x0F;
                        if mask & (1u32 << channel) != 0 {
                            // Muted: force silent (`vvvv = 0xF`).
                            return GateAction::Replace(u16::from(byte | 0x0F));
                        }
                    }
                }
                // A data byte `0-dddddd` extends the last-latched *tone* register;
                // a volume is a single 4-bit latch, so a data byte never carries a
                // level and always passes.
                GateAction::Pass
            }
        }
    }
}

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

    fn opm() -> ChannelGate {
        ChannelGate::new(ChipKind::Ym2151).expect("YM2151 is gated")
    }

    fn psg() -> ChannelGate {
        ChannelGate::new(ChipKind::Sn76489).expect("SN76489 is gated")
    }

    #[test]
    fn only_the_covered_chips_are_gated() {
        assert!(ChannelGate::exists(ChipKind::Ym2151));
        assert!(ChannelGate::exists(ChipKind::Sn76489));
        // Not yet covered: honestly reports no gate rather than pretending.
        assert!(!ChannelGate::exists(ChipKind::Ym2612));
        assert!(ChannelGate::new(ChipKind::Ym2612).is_none());
    }

    // -- OPM (DROP) ----------------------------------------------------------

    #[test]
    fn opm_drops_a_muted_channels_key_and_passes_the_rest() {
        let mut gate = opm();
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out); // mute channel 2
        // The mute edge keys channel 2 off (0x08 with no slot bits).
        assert_eq!(out, [(0, 0x08, 0x02)]);

        // A key-on for channel 2 (slots set) is dropped...
        assert_eq!(gate.filter(0, 0x08, 0x78 | 0x02), GateAction::Drop);
        // ...while channel 3's passes untouched, and non-key writes always pass.
        assert_eq!(gate.filter(0, 0x08, 0x78 | 0x03), GateAction::Pass);
        assert_eq!(gate.filter(0, 0x28, 0x0A), GateAction::Pass);
    }

    #[test]
    fn opm_restating_the_same_mask_emits_nothing() {
        let mut gate = opm();
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out);
        out.clear();
        // apply_mix restates the mask after every seek/pan change: no edge, no
        // write.
        gate.set_mask(0b0000_0100, &mut out);
        assert!(out.is_empty(), "a no-edge restatement is silent");
    }

    #[test]
    fn opm_unmuting_re_keys_nothing() {
        let mut gate = opm();
        let mut out = Vec::new();
        gate.set_mask(0b0000_0100, &mut out);
        out.clear();
        gate.set_mask(0, &mut out); // unmute everything
        assert!(
            out.is_empty(),
            "a muted channel rejoins at its next natural key-on, not on unmute"
        );
        // And its key-ons pass again.
        assert_eq!(gate.filter(0, 0x08, 0x78 | 0x02), GateAction::Pass);
    }

    // -- SN76489 (VOLUME, latch-aware) ---------------------------------------

    #[test]
    fn psg_forces_a_muted_channels_volume_silent_and_shadows_the_real_one() {
        let mut gate = psg();
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out); // mute channel 0
        // Mute edge: force channel 0's attenuation to 0xF (byte 0x9F).
        assert_eq!(out, [(0, 0, 0x9F)]);

        // The song setting channel 0 to full volume (0x90) is forced to silent...
        assert_eq!(gate.filter(0, 0, 0x90), GateAction::Replace(0x9F));
        // ...but channel 1's volume passes...
        assert_eq!(gate.filter(0, 0, 0xB0), GateAction::Pass);
        // ...and channel 0's tone-frequency latch passes (it is silent anyway).
        assert_eq!(gate.filter(0, 0, 0x80), GateAction::Pass);
    }

    #[test]
    fn psg_unmuting_restores_the_shadowed_volume() {
        let mut gate = psg();
        // The song sets channel 0 to attenuation 0x3 while it plays.
        assert_eq!(gate.filter(0, 0, 0x93), GateAction::Pass);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out); // mute channel 0
        out.clear();
        gate.set_mask(0, &mut out); // unmute it
        // Its real level (0x3, byte 0x93) is restored, not a guess.
        assert_eq!(out, [(0, 0, 0x93)]);
    }

    #[test]
    fn psg_re_points_the_latch_after_a_synthesised_write() {
        let mut gate = psg();
        // The song latches channel 1's tone (0x80 | 1<<5 = 0xA0), expecting a
        // data byte to follow.
        assert_eq!(gate.filter(0, 0, 0xA0), GateAction::Pass);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out); // mute channel 0
        // The channel-0 volume force is followed by a re-point to the song's last
        // tone latch, so the pending data byte still lands on channel 1's tone.
        assert_eq!(out, [(0, 0, 0x9F), (0, 0, 0xA0)]);
    }

    #[test]
    fn psg_does_not_re_point_after_a_volume_latch() {
        let mut gate = psg();
        // The song's last latch was a *volume* (channel 1, 0xB0) -- no data byte
        // follows a volume, so no re-point is needed.
        assert_eq!(gate.filter(0, 0, 0xB0), GateAction::Pass);
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out); // mute channel 0
        assert_eq!(
            out,
            [(0, 0, 0x9F)],
            "no spurious re-point after a volume latch"
        );
    }

    #[test]
    fn a_mask_is_clamped_to_the_roster() {
        // The channel splitter sets u32::MAX on the instances it is not soloing;
        // only this chip's own channels may be read from it.
        let mut gate = psg(); // 4 channels
        let mut out = Vec::new();
        gate.set_mask(u32::MAX, &mut out);
        // Exactly the four real channels are forced silent -- no out-of-roster
        // bit produces a phantom write.
        assert_eq!(
            out,
            [(0, 0, 0x9F), (0, 0, 0xBF), (0, 0, 0xDF), (0, 0, 0xFF)]
        );
    }

    #[test]
    fn a_reset_clears_the_shadows_and_the_mask() {
        let mut gate = psg();
        gate.filter(0, 0, 0x93); // channel 0 -> attenuation 0x3
        let mut out = Vec::new();
        gate.set_mask(0b0000_0001, &mut out);

        gate.reset(3_579_545, false);
        out.clear();
        // Post-reset restatement with empty shadows: the mask is gone, so muting
        // channel 0 again forces silence (not a stale shadow), and restores to the
        // powered-up 0xF if unmuted.
        gate.set_mask(0b0000_0001, &mut out);
        assert_eq!(out, [(0, 0, 0x9F)]);
        out.clear();
        gate.set_mask(0, &mut out);
        assert_eq!(
            out,
            [(0, 0, 0x9F)],
            "reset restored the powered-up 0xF level"
        );
    }
}
