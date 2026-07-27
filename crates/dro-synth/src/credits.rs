//! Who to credit for each emulator core, and under what terms.
//!
//! The runtime face of the provenance policy: `crates/dro-synth/PROVENANCE.md`
//! is the long form a maintainer reads, and this is the short form the About
//! dialog shows a user. Both must name every core that is compiled in, because
//! the app links copyleft cores and their licenses require the notice.
//!
//! **This list is temporary in shape, not in content.** cr-2 turns
//! [`core_for`](crate::chip::core_for) into a registry whose `CoreInfo` already
//! carries label, license and upstream; at that point [`credits`] is derived
//! from the registry (plus the provider crates the app registers) instead of
//! written out here, and a new core appears in the About box without anyone
//! remembering to add it. The struct below is deliberately the shape those
//! entries will take.

/// One emulator core, as a user should see it credited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreCredit {
    /// What the core is called, as the Settings picker would name it.
    pub label: &'static str,
    /// The chips it serves, as display names.
    pub chips: &'static str,
    /// Who wrote it. `"this project"` for a core written here.
    pub authors: &'static str,
    /// SPDX expression.
    pub license: &'static str,
    /// Where the source lives. Empty for a core with no upstream.
    pub upstream: &'static str,
}

/// Every core compiled into this build, in the order they should be credited.
///
/// Cores that need a C toolchain or a native device are absent from a wasm
/// build; when a provider crate is `cfg`-excluded its entries must go with it,
/// which is exactly what cr-2's registry does automatically.
#[must_use]
pub fn credits() -> &'static [CoreCredit] {
    &[
        CoreCredit {
            label: "Nuked-OPL3",
            chips: "YM3812 (OPL2), YMF262 (OPL3)",
            authors: "Nuke.YKT; Rust port by the nuked-opl3 crate authors",
            license: "LGPL-2.1-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPL3",
        },
        CoreCredit {
            label: "SN76489 (clean-room)",
            chips: "SN76489",
            authors: "this project",
            license: "MIT OR Apache-2.0",
            upstream: "",
        },
    ]
}

/// The credits as plain lines, one core per stanza.
///
/// The About dialog is a text alert, so the formatting lives here rather than
/// in the UI crate -- keeping it beside the data means a provider crate added
/// later cannot be credited in one place and forgotten in the other.
#[must_use]
pub fn credits_text() -> String {
    let mut out = String::new();
    for core in credits() {
        out.push_str(&format!("  {} -- {}\n", core.label, core.chips));
        out.push_str(&format!("    {}, {}\n", core.authors, core.license));
        if !core.upstream.is_empty() {
            out.push_str(&format!("    {}\n", core.upstream));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_is_creditable() {
        // A credit missing its license or its authors is worse than no panel at
        // all -- it looks like the notice was given when it was not.
        for core in credits() {
            assert!(!core.label.is_empty(), "a core with no name");
            assert!(!core.chips.is_empty(), "{}: no chips", core.label);
            assert!(!core.authors.is_empty(), "{}: no authors", core.label);
            assert!(!core.license.is_empty(), "{}: no license", core.label);
        }
    }

    #[test]
    fn the_copyleft_core_names_its_upstream() {
        // LGPL section 6 wants the user pointed at the source. A permissive
        // clean-room core has no upstream to point at, and says so with "".
        let nuked = credits()
            .iter()
            .find(|c| c.label == "Nuked-OPL3")
            .expect("the OPL core is compiled into every build");
        assert!(nuked.upstream.starts_with("https://"));
        assert_eq!(nuked.license, "LGPL-2.1-or-later");
    }

    #[test]
    fn the_text_form_mentions_every_core() {
        let text = credits_text();
        for core in credits() {
            assert!(
                text.contains(core.label),
                "{} missing from the text",
                core.label
            );
            assert!(
                text.contains(core.license),
                "{}: license missing",
                core.label
            );
        }
    }
}
