//! The bulk-tag model: a GD3 [`BulkTagOverlay`] that writes only its checked
//! fields onto each track's existing tag, plus the pack-metadata seeding that
//! pre-fills it. The dialog that edits an overlay lives in
//! [`crate::dialogs::bulk_tag`]; this is the headless part it operates on.

use vgms_core::Gd3Tag;
use vgms_core::pack::PackMeta;
use vgms_core::vgm::data::GD3_FIELD_COUNT;

// GD3 field indices (file order), for the bulk-tag seeding below. The "native"
// fields are GD3's original-language variants, paired with their English siblings.
mod gd3_index {
    pub(super) const GAME_NAME_EN: usize = 2;
    pub(super) const GAME_NAME_NATIVE: usize = 3;
    pub(super) const SYSTEM_NAME_EN: usize = 4;
    pub(super) const SYSTEM_NAME_NATIVE: usize = 5;
    pub(super) const TRACK_AUTHOR_EN: usize = 6;
    pub(super) const TRACK_AUTHOR_NATIVE: usize = 7;
    pub(super) const RELEASE_DATE: usize = 8;
    pub(super) const CREATOR: usize = 9;
}

/// A bulk GD3 edit: which of the eleven fields to write, and the value for each.
///
/// Applying it overlays only the *checked* fields onto a track's existing tag,
/// so every unchecked field keeps that track's own value. That is the whole
/// point of a bulk edit: correct the composer on half the tracks, or stamp the
/// shared game name onto all of them, without disturbing anything else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkTagOverlay {
    /// Per field, in GD3 file order: whether `values[i]` is written.
    pub apply: [bool; GD3_FIELD_COUNT],
    /// Per field, in GD3 file order: the value written when `apply[i]` is set.
    pub values: [String; GD3_FIELD_COUNT],
}

impl BulkTagOverlay {
    /// The tag that results from writing the checked fields onto `base`, leaving
    /// the unchecked fields at their existing values.
    #[must_use]
    pub fn apply_to(&self, base: &Gd3Tag) -> Gd3Tag {
        let mut fields = base.fields().map(str::to_owned);
        for (slot, (on, value)) in fields.iter_mut().zip(self.apply.iter().zip(&self.values)) {
            if *on {
                slot.clone_from(value);
            }
        }
        Gd3Tag::from_fields(fields)
    }

    /// Whether any field is checked. With none, a bulk edit has nothing to do.
    #[must_use]
    pub fn writes_anything(&self) -> bool {
        self.apply.iter().any(|&on| on)
    }
}

/// Seeds a bulk edit from the package metadata: the GD3 fields a pack typically
/// shares across every track.
///
/// Game, system, composer, release date and ripper are pre-filled and
/// pre-checked when present, so opening the dialog on a filled-in pack and
/// hitting Apply writes them to every track with no extra clicks. The
/// original-language ("orig") variants of game, system and composer are seeded
/// with the same values -- the pack metadata holds no separate native names,
/// and this app's PC/AT games rarely have one, so mirroring the English value
/// keeps both variants filled. The two track-name fields are never seeded: a
/// title is per-track by definition. To tag a subset with a different value
/// (say, the half of the pack a second composer wrote), edit the value and
/// deselect the tracks it does not apply to.
#[must_use]
pub fn seed_from_meta(meta: &PackMeta) -> BulkTagOverlay {
    let mut overlay = BulkTagOverlay::default();
    let seeds = [
        (gd3_index::GAME_NAME_EN, &meta.game_name),
        (gd3_index::GAME_NAME_NATIVE, &meta.game_name),
        (gd3_index::SYSTEM_NAME_EN, &meta.system),
        (gd3_index::SYSTEM_NAME_NATIVE, &meta.system),
        (gd3_index::TRACK_AUTHOR_EN, &meta.music_authors),
        (gd3_index::TRACK_AUTHOR_NATIVE, &meta.music_authors),
        (gd3_index::RELEASE_DATE, &meta.release_date),
        (gd3_index::CREATOR, &meta.creator),
    ];
    for (index, value) in seeds {
        overlay.values[index] = value.clone();
        overlay.apply[index] = !value.trim().is_empty();
    }
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(game: &str, author: &str, creator: &str) -> Gd3Tag {
        Gd3Tag {
            game_name_en: game.to_owned(),
            track_author_en: author.to_owned(),
            creator: creator.to_owned(),
            release_date: "1994".to_owned(),
            ..Gd3Tag::default()
        }
    }

    #[test]
    fn overlay_writes_only_the_checked_fields() {
        let base = tag("Old Game", "Ada", "Old Ripper");
        let mut overlay = BulkTagOverlay::default();
        // Check game name and creator; leave author and everything else alone.
        overlay.apply[gd3_index::GAME_NAME_EN] = true;
        overlay.values[gd3_index::GAME_NAME_EN] = "New Game".to_owned();
        overlay.apply[gd3_index::CREATOR] = true;
        overlay.values[gd3_index::CREATOR] = "New Ripper".to_owned();
        // A value present but unchecked must not be written.
        overlay.values[gd3_index::TRACK_AUTHOR_EN] = "Zoe".to_owned();

        let merged = overlay.apply_to(&base);
        assert_eq!(merged.game_name_en, "New Game", "checked field written");
        assert_eq!(merged.creator, "New Ripper", "checked field written");
        assert_eq!(merged.track_author_en, "Ada", "unchecked field kept");
        assert_eq!(merged.release_date, "1994", "untouched field kept");
    }

    #[test]
    fn overlay_can_clear_a_field_by_checking_an_empty_value() {
        let base = tag("Game", "Ada", "Ripper");
        let mut overlay = BulkTagOverlay::default();
        overlay.apply[gd3_index::TRACK_AUTHOR_EN] = true; // empty value
        assert_eq!(overlay.apply_to(&base).track_author_en, "");
    }

    #[test]
    fn writes_anything_reflects_the_checkboxes() {
        let mut overlay = BulkTagOverlay::default();
        assert!(!overlay.writes_anything());
        overlay.apply[gd3_index::SYSTEM_NAME_EN] = true;
        assert!(overlay.writes_anything());
    }

    #[test]
    fn seed_prechecks_every_shared_field_including_the_composer() {
        let meta = PackMeta {
            game_name: "Cool Game".to_owned(),
            system: "IBM PC/AT".to_owned(),
            release_date: "1994-03-01".to_owned(),
            creator: "Ripper".to_owned(),
            music_authors: "Ada, Bob".to_owned(),
            ..PackMeta::default()
        };
        let overlay = seed_from_meta(&meta);

        // Every shared pack field -- composer and the orig variants included --
        // is pre-filled and pre-checked, so "apply to all" needs no extra clicks.
        for index in [
            gd3_index::GAME_NAME_EN,
            gd3_index::GAME_NAME_NATIVE,
            gd3_index::SYSTEM_NAME_EN,
            gd3_index::SYSTEM_NAME_NATIVE,
            gd3_index::TRACK_AUTHOR_EN,
            gd3_index::TRACK_AUTHOR_NATIVE,
            gd3_index::RELEASE_DATE,
            gd3_index::CREATOR,
        ] {
            assert!(overlay.apply[index], "field {index} pre-checked");
        }
        // The orig variants mirror their English siblings' pack values.
        assert_eq!(overlay.values[gd3_index::TRACK_AUTHOR_EN], "Ada, Bob");
        assert_eq!(overlay.values[gd3_index::TRACK_AUTHOR_NATIVE], "Ada, Bob");
        assert_eq!(overlay.values[gd3_index::GAME_NAME_NATIVE], "Cool Game");
        // Neither track-name field is ever seeded (EN index 0, orig index 1).
        for index in [0, 1] {
            assert!(overlay.values[index].is_empty() && !overlay.apply[index]);
        }
    }

    #[test]
    fn seed_leaves_empty_pack_fields_unchecked() {
        let overlay = seed_from_meta(&PackMeta::default());
        assert!(
            !overlay.writes_anything(),
            "a blank pack pre-checks nothing"
        );
    }
}
