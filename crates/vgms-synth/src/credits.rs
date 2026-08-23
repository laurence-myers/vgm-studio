//! Who to credit for each emulator core, and under what terms.
//!
//! The runtime face of the provenance policy: `crates/vgms-synth/PROVENANCE.md`
//! is the long form a maintainer reads, and this is the short form the About
//! dialog shows a user. Both must name every core that is compiled in, because
//! the app links copyleft cores and their licenses require the notice.
//!
//! **It is derived, not written.** The list comes from the
//! [registry](crate::registry), so a provider crate that registers a core
//! credits itself, and a core absent from a build -- a wasm one, say, where a
//! native-only provider was never linked -- is absent from the notice too. The
//! only way to ship a core uncredited is to ship it unregistered, which is also
//! the only way to ship one unusable.

use crate::registry::{self, CoreInfo};

/// One emulator core, as a user should see it credited.
///
/// A view of a [`CoreInfo`] with the per-chip rows collapsed: the registry
/// carries one entry per (core, chip) because that is how a core is *chosen*,
/// but a credit names each core once and lists the chips it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreCredit {
    /// What the core is called, as the Settings picker names it.
    pub label: String,
    /// The chips it serves, as display names.
    pub chips: String,
    /// Who wrote it. `"this project"` for a core written here.
    pub authors: String,
    /// SPDX expression.
    pub license: String,
    /// Where the source lives. Empty for a core with no upstream.
    pub upstream: String,
}

/// Every core registered in this build, each credited once.
///
/// Order follows registration, so the default core for a chip is credited
/// before its alternatives.
#[must_use]
pub fn credits() -> Vec<CoreCredit> {
    let mut out: Vec<CoreCredit> = Vec::new();
    for info in registry::registry().all() {
        // The registry keys on (core, chip); a credit keys on the core. Same id
        // means same core, so its chips join the row already there.
        if let Some(existing) = out.iter_mut().find(|credit| credit.label == info.label) {
            existing.chips.push_str(", ");
            existing.chips.push_str(info.chip.name());
        } else {
            out.push(credit_from(info));
        }
    }
    out
}

fn credit_from(info: &CoreInfo) -> CoreCredit {
    CoreCredit {
        label: info.label.to_owned(),
        chips: info.chip.name().to_owned(),
        authors: info.authors.to_owned(),
        license: info.license.to_owned(),
        upstream: info.upstream.to_owned(),
    }
}

/// Condenses a core's SPDX license for the compact About table.
///
/// libvgm ships no explicit grant, so its terms are recorded in `PROVENANCE.md`
/// and its license string is the long "see PROVENANCE.md -- upstream publishes
/// no grant". In the table that whole clause is just noise repeated down the
/// column, so it shows as "see PROVENANCE.md". Every other license is a short
/// SPDX expression already and passes through unchanged.
#[must_use]
pub fn short_license(license: &str) -> &str {
    if license.starts_with("see PROVENANCE.md") {
        "see PROVENANCE.md"
    } else {
        license
    }
}

/// The credits as an aligned table: one row per chip and a core that serves it,
/// ordered by chip name, with the core's source library and license.
///
/// The About dialog is a text alert and the app's font is fixed-width, so the
/// columns line up from space padding alone. The formatting lives here rather
/// than in the UI crate -- keeping it beside the data means a provider crate
/// added later cannot be credited in one place and forgotten in the other.
#[must_use]
pub fn credits_text() -> String {
    // One row per (chip, core): the registry already keys on that pair, so a
    // core serving several chips lists under each, which is what "ordered by
    // chip" wants.
    let mut rows: Vec<(&'static str, &'static str, &'static str)> = registry::registry()
        .all()
        .map(|info| (info.chip.name(), info.label, short_license(info.license)))
        .collect();
    // By chip name, then by the source library within a chip.
    rows.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(b.1)));

    const HEAD: (&str, &str, &str) = ("Chip", "Source library", "License");
    let chip_w = rows
        .iter()
        .map(|row| row.0.len())
        .max()
        .unwrap_or(0)
        .max(HEAD.0.len());
    let src_w = rows
        .iter()
        .map(|row| row.1.len())
        .max()
        .unwrap_or(0)
        .max(HEAD.1.len());
    // The last column is not padded, but its rule spans the widest entry so the
    // header underline reaches across the whole column.
    let license_w = rows
        .iter()
        .map(|row| row.2.len())
        .max()
        .unwrap_or(0)
        .max(HEAD.2.len());

    let mut out = String::new();
    out.push_str(&format!(
        "  {:<chip_w$}  {:<src_w$}  {}\n",
        HEAD.0, HEAD.1, HEAD.2
    ));
    out.push_str(&format!(
        "  {}  {}  {}\n",
        "-".repeat(chip_w),
        "-".repeat(src_w),
        "-".repeat(license_w),
    ));
    for (chip, src, license) in rows {
        out.push_str(&format!("  {chip:<chip_w$}  {src:<src_w$}  {license}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_core_is_creditable() {
        // A credit missing its license or its authors is worse than no panel at
        // all -- it looks like the notice was given when it was not.
        let credits = credits();
        assert!(!credits.is_empty(), "a build with no cores at all");
        for core in &credits {
            assert!(!core.label.is_empty(), "a core with no name");
            assert!(!core.chips.is_empty(), "{}: no chips", core.label);
            assert!(!core.authors.is_empty(), "{}: no authors", core.label);
            assert!(!core.license.is_empty(), "{}: no license", core.label);
        }
    }

    #[cfg(feature = "nuked-opl")]
    #[test]
    fn a_core_serving_several_chips_is_credited_once() {
        // The OPL core is registered four times -- OPL2, OPL3, YM3526, Y8950 --
        // because a core is *chosen* per chip. Crediting it four times would
        // read as four emulators.
        let credits = credits();
        let opl: Vec<_> = credits
            .iter()
            .filter(|c| c.license == "LGPL-2.1-or-later")
            .collect();
        assert_eq!(opl.len(), 1, "one OPL core, one credit");
        assert!(opl[0].chips.contains("YMF262"));
        assert!(opl[0].chips.contains("YM3812"));
    }

    #[cfg(feature = "nuked-opl")]
    #[test]
    fn the_copyleft_core_names_its_upstream() {
        // LGPL section 6 wants the user pointed at the source. A permissive
        // clean-room core has no upstream to point at, and says so with "".
        let credits = credits();
        let nuked = credits
            .iter()
            .find(|c| c.license == "LGPL-2.1-or-later")
            .expect("the OPL core is compiled into every build");
        assert!(nuked.upstream.starts_with("https://"));
    }

    #[test]
    fn the_text_form_mentions_every_core() {
        let text = credits_text();
        for core in credits() {
            assert!(
                text.contains(&core.label),
                "{} missing from the text",
                core.label
            );
            // The table shows the condensed license (libvgm's long clause becomes
            // "see PROVENANCE.md"); every core's terms are still named.
            assert!(
                text.contains(short_license(&core.license)),
                "{}: license missing",
                core.label
            );
        }
    }

    #[test]
    fn libvgm_terms_condense_but_real_spdx_passes_through() {
        assert_eq!(
            short_license("see PROVENANCE.md -- upstream publishes no grant"),
            "see PROVENANCE.md"
        );
        assert_eq!(short_license("LGPL-2.1-or-later"), "LGPL-2.1-or-later");
        assert_eq!(short_license("MIT OR Apache-2.0"), "MIT OR Apache-2.0");
    }
}
