//! Which channels of which chips are muted or panned: the multichip
//! counterpart of the OPL [`Muting`](crate::engine::Muting) /
//! [`Panning`](crate::engine::Panning) pair.
//!
//! Where the OPL engine gates register writes, these travel to the cores:
//! [`ChipCore::set_channel_mutes`](crate::ChipCore::set_channel_mutes) and
//! [`set_channel_pans`](crate::ChipCore::set_channel_pans) apply them inside
//! each emulator's own mixer. Bit `i` and pan entry `i` mean entry `i` of
//! [`dro_core::vgm::channels_of`] -- the app's canonical channel order --
//! and a provider whose emulator numbers differently remaps on its side.
//!
//! State is keyed per chip *instance*, because a dual-chip file has two of
//! the same kind and a user mutes one of them, not the kind.

use dro_core::vgm::ChipKind;

/// The centre pan position, and the hard extremes: positions are
/// `-0x100 ..= 0x100` for full left through full right, the same span
/// libvgm's equal-power pan takes.
pub const PAN_CENTER: i16 = 0;
pub const PAN_LEFT: i16 = -0x100;
pub const PAN_RIGHT: i16 = 0x100;

/// One chip instance's mute mask: bit `i` set means channel `i` is muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipMuteEntry {
    pub kind: ChipKind,
    pub instance: u8,
    pub muted: u32,
}

/// Every chip instance's channel mutes. Absent means nothing muted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChipMuting {
    entries: Vec<ChipMuteEntry>,
}

impl ChipMuting {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the mask for one chip instance, replacing whatever it had.
    pub fn set(&mut self, kind: ChipKind, instance: u8, muted: u32) {
        match self
            .entries
            .iter_mut()
            .find(|entry| entry.kind == kind && entry.instance == instance)
        {
            Some(entry) => entry.muted = muted,
            None => self.entries.push(ChipMuteEntry {
                kind,
                instance,
                muted,
            }),
        }
    }

    /// The mask for one chip instance. Zero -- nothing muted -- when unset.
    #[must_use]
    pub fn mask_for(&self, kind: ChipKind, instance: u8) -> u32 {
        self.entries
            .iter()
            .find(|entry| entry.kind == kind && entry.instance == instance)
            .map_or(0, |entry| entry.muted)
    }

    /// Whether every mask is zero -- the state a fresh panel is in, and the
    /// one the render path can skip applying.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        self.entries.iter().all(|entry| entry.muted == 0)
    }
}

/// One chip instance's pan positions, entry `i` for channel `i`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipPanEntry {
    pub kind: ChipKind,
    pub instance: u8,
    pub pans: Vec<i16>,
}

/// Every chip instance's channel pans. Absent means the chip's own image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChipPanning {
    entries: Vec<ChipPanEntry>,
}

impl ChipPanning {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the pan array for one chip instance, replacing whatever it had.
    pub fn set(&mut self, kind: ChipKind, instance: u8, pans: Vec<i16>) {
        match self
            .entries
            .iter_mut()
            .find(|entry| entry.kind == kind && entry.instance == instance)
        {
            Some(entry) => entry.pans = pans,
            None => self.entries.push(ChipPanEntry {
                kind,
                instance,
                pans,
            }),
        }
    }

    /// The pan array for one chip instance, or `None` for "leave the chip's
    /// own image alone".
    #[must_use]
    pub fn pans_for(&self, kind: ChipKind, instance: u8) -> Option<&[i16]> {
        self.entries
            .iter()
            .find(|entry| entry.kind == kind && entry.instance == instance)
            .map(|entry| entry.pans.as_slice())
    }

    /// Whether no chip has a pan image set.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_are_per_instance_and_replace() {
        let mut muting = ChipMuting::new();
        assert!(muting.is_neutral());
        muting.set(ChipKind::Sn76489, 0, 0b0011);
        muting.set(ChipKind::Sn76489, 1, 0b0100);
        assert_eq!(muting.mask_for(ChipKind::Sn76489, 0), 0b0011);
        assert_eq!(muting.mask_for(ChipKind::Sn76489, 1), 0b0100);
        assert_eq!(muting.mask_for(ChipKind::Ym2612, 0), 0, "unset is unmuted");
        assert!(!muting.is_neutral());
        muting.set(ChipKind::Sn76489, 0, 0);
        muting.set(ChipKind::Sn76489, 1, 0);
        assert!(muting.is_neutral(), "cleared masks read as neutral");
    }

    #[test]
    fn pans_are_per_instance_and_absent_means_own_image() {
        let mut panning = ChipPanning::new();
        assert!(panning.is_neutral());
        panning.set(ChipKind::Ay8910, 0, vec![PAN_LEFT, PAN_CENTER, PAN_RIGHT]);
        assert_eq!(
            panning.pans_for(ChipKind::Ay8910, 0),
            Some([PAN_LEFT, PAN_CENTER, PAN_RIGHT].as_slice())
        );
        assert_eq!(panning.pans_for(ChipKind::Ay8910, 1), None);
    }
}
