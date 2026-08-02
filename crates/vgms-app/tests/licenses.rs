//! Every workspace crate must be named somewhere in the licence split, so the
//! two-halves table stays honest as crates are added or removed. This is the
//! only thing that stops a new crate silently escaping the licence record (dd-4).

use std::path::Path;

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
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/ must exist")
        .flatten()
    {
        if !entry.path().join("Cargo.toml").is_file() {
            continue;
        }
        // Crate directory names match crate names here, and the table lists each
        // in backticks.
        let name = entry.file_name().to_string_lossy().into_owned();
        if !readme.contains(&format!("`{name}`")) {
            missing.push(name);
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "these crates are not named in licenses/README.md: {missing:?}"
    );
}
