//! The embedded DOS text-mode font.
//!
//! "Px437 IBM VGA 8x16" (int10h Oldschool PC Font Pack, CC BY-SA 4.0 -- see
//! `assets/fonts/LICENSE-Px437-IBM-VGA.txt`) is a faithful trace of the IBM VGA
//! ROM font at a 16px em. It becomes the primary face for both the proportional
//! and monospace families; egui's built-in fonts stay behind it as a fallback
//! for glyphs outside CP437.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};

/// The bitmap-traced VGA font, embedded verbatim.
const PX437_IBM_VGA: &[u8] = include_bytes!("../../assets/fonts/Px437_IBM_VGA_8x16.ttf");

/// The internal name the font is registered under.
const FAMILY_NAME: &str = "Px437";

/// The em size the 8x16 face was traced at; integer multiples stay pixel-sharp.
const DOS_FONT_SIZE: f32 = 16.0;

/// egui's default fonts, with Px437 inserted ahead of both families.
#[must_use]
pub(crate) fn font_definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    defs.font_data.insert(
        FAMILY_NAME.to_owned(),
        Arc::new(FontData::from_static(PX437_IBM_VGA)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        defs.families
            .entry(family)
            .or_default()
            .insert(0, FAMILY_NAME.to_owned());
    }
    defs
}

/// One DOS size for every text style. The proportional/monospace split is kept
/// (both resolve to Px437 today) so a second face can be dropped in later.
#[must_use]
pub(crate) fn text_styles() -> BTreeMap<TextStyle, FontId> {
    [
        (
            TextStyle::Small,
            FontId::new(DOS_FONT_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(DOS_FONT_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(DOS_FONT_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(DOS_FONT_SIZE, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(DOS_FONT_SIZE, FontFamily::Monospace),
        ),
    ]
    .into()
}
