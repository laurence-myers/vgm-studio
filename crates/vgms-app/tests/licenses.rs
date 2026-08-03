//! Every workspace crate must be named somewhere in the licence split, and its
//! manifest's `license` field must say what the split says it is -- so the
//! two-halves table stays honest as crates are added, removed, or relicensed.
//! This is the only thing that stops a crate silently escaping the licence
//! record (dd-4).

use std::path::Path;

/// The licence the split assigns each crate. The reusable pair is permissive,
/// the Nuked provider is upstream's LGPL, and everything else is the app tier's
/// GPL -- the same three-way rule `licenses/README.md` documents. A new crate
/// not named here defaults to the app tier, matching the README's "everything
/// else" row.
fn documented_license(name: &str) -> &'static str {
    match name {
        "vgms-core" | "vgms-synth" => "MIT OR Apache-2.0",
        "vgms-cores-nuked" => "LGPL-2.1-or-later",
        _ => "GPL-2.0-or-later",
    }
}

#[test]
fn every_crate_is_named_in_the_licence_split() {
    // This test crate lives at <root>/crates/vgms-app.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root is two levels up");
    let readme = std::fs::read_to_string(root.join("licenses/README.md"))
        .expect("licenses/README.md must exist");

    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/ must exist")
        .flatten()
    {
        let manifest_path = entry.path().join("Cargo.toml");
        if !manifest_path.is_file() {
            continue;
        }
        // Crate directory names match crate names here, and the table lists each
        // in backticks.
        let name = entry.file_name().to_string_lossy().into_owned();
        if !readme.contains(&format!("`{name}`")) {
            missing.push(name.clone());
        }

        // And the manifest must agree with the split -- being *named* in the
        // README is worthless if the `license` field says something else.
        let manifest = std::fs::read_to_string(&manifest_path).expect("a readable Cargo.toml");
        let declared = manifest
            .lines()
            .find_map(|line| {
                line.strip_prefix("license")
                    .and_then(|rest| rest.split_once('='))
                    .map(|(_, value)| value.trim().trim_matches('"').to_owned())
            })
            .unwrap_or_else(|| panic!("{name}/Cargo.toml declares no license field"));
        let documented = documented_license(&name);
        if declared != documented {
            mismatched.push(format!("{name}: manifest {declared:?}, split says {documented:?}"));
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "these crates are not named in licenses/README.md: {missing:?}"
    );
    mismatched.sort();
    assert!(
        mismatched.is_empty(),
        "these crates' license fields disagree with the documented split:\n{}",
        mismatched.join("\n")
    );
}
