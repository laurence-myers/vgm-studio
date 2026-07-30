//! The embedded DOS text-mode font, and the system CJK fallback behind it.
//!
//! "Px437 IBM VGA 8x16" (int10h Oldschool PC Font Pack, CC BY-SA 4.0 -- see
//! `assets/fonts/LICENSE-Px437-IBM-VGA.txt`) is a faithful trace of the IBM VGA
//! ROM font at a 16px em. It becomes the primary face for both the proportional
//! and monospace families; egui's built-in fonts stay behind it as a fallback
//! for glyphs outside CP437.
//!
//! Neither Px437 nor egui's built-ins carry a single CJK glyph, so a GD3 tag's
//! (overwhelmingly Japanese) original-language fields would render as tofu
//! squares. A native build therefore appends the first CJK-capable font it finds
//! on the system (never embedded: a Japanese face is megabytes, and every desktop
//! ships one) as the *last* fallback, so it serves exactly the glyphs nothing
//! above it has and Latin text stays pixel-DOS.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};

/// The bitmap-traced VGA font, embedded verbatim.
const PX437_IBM_VGA: &[u8] = include_bytes!("../../assets/fonts/Px437_IBM_VGA_8x16.ttf");

/// The internal name the font is registered under.
const FAMILY_NAME: &str = "Px437";

/// The internal name the system CJK fallback is registered under.
const CJK_FAMILY_NAME: &str = "cjk-fallback";

/// The em size the 8x16 face was traced at; integer multiples stay pixel-sharp.
const DOS_FONT_SIZE: f32 = 16.0;

/// egui's default fonts, with Px437 inserted ahead of both families and a
/// system CJK face (when one is found) appended behind them.
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
    if let Some(cjk) = system_cjk_font() {
        defs.font_data.insert(CJK_FAMILY_NAME.to_owned(), cjk);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families
                .entry(family)
                .or_default()
                .push(CJK_FAMILY_NAME.to_owned());
        }
    }
    defs
}

/// The first CJK-capable font the system offers, or `None` (wasm, or a very
/// bare install) -- those glyphs then fall through to the replacement box.
///
/// The candidates are each platform's stock Japanese faces first (GD3's
/// original-language fields are overwhelmingly Japanese), then the pan-CJK
/// Noto/Source Han collections. A `.ttc` index picks the face within a
/// collection; 0 is the family's own canonical face in each listed file.
fn system_cjk_font() -> Option<Arc<FontData>> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
        let candidates: [(std::path::PathBuf, u32); 8] = [
            // Windows: MS Gothic, Yu Gothic, Meiryo.
            (std::path::Path::new(&windir).join("Fonts\\msgothic.ttc"), 0),
            (std::path::Path::new(&windir).join("Fonts\\YuGothM.ttc"), 0),
            (std::path::Path::new(&windir).join("Fonts\\meiryo.ttc"), 0),
            // macOS: Hiragino.
            (
                std::path::PathBuf::from("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"),
                0,
            ),
            (
                std::path::PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
                0,
            ),
            // Linux: Noto CJK under its two common roots.
            (
                std::path::PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
                0,
            ),
            (
                std::path::PathBuf::from("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"),
                0,
            ),
            (
                std::path::PathBuf::from(
                    "/usr/share/fonts/opentype/source-han-sans/SourceHanSans-Regular.ttc",
                ),
                0,
            ),
        ];
        for (path, index) in candidates {
            if let Ok(bytes) = std::fs::read(&path) {
                log::debug!("CJK fallback font: {}", path.display());
                let mut data = FontData::from_owned(bytes);
                data.index = index;
                return Some(Arc::new(data));
            }
        }
        log::debug!("no system CJK font found; CJK text will show as boxes");
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A GD3 tag's Japanese original-language fields must resolve through the
    /// fallback rather than showing as tofu boxes. Skipped (trivially passing) on
    /// a machine with no system CJK font at all.
    #[test]
    fn japanese_glyphs_resolve_when_the_system_has_a_cjk_font() {
        let defs = font_definitions();
        if !defs.font_data.contains_key(CJK_FAMILY_NAME) {
            return;
        }
        let mut fonts =
            egui::epaint::text::Fonts::new(egui::epaint::text::TextOptions::default(), defs);
        let font_id = FontId::new(DOS_FONT_SIZE, FontFamily::Proportional);
        assert!(
            fonts.has_glyphs(&font_id, "\u{6c34}\u{3072}\u{30ab}"),
            "kanji, hiragana and katakana must all resolve"
        );
        // And the DOS face still serves Latin text.
        assert!(fonts.has_glyphs(&font_id, "DRO Trimmer 0123"));
    }
}
