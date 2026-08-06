//! Submission readiness: the checks a pack must pass (or be warned about) before
//! it is fit to upload. [`readiness`] runs them all and returns a flat list of
//! [`ReadinessItem`]s, each tiered by [`Severity`] and filed under a
//! [`ReadinessCategory`]; the UI feeds one list to both the export gate and the
//! checklist so the two can never disagree.

use std::collections::BTreeSet;

use crate::Gd3Tag;

use super::PackMeta;
use super::naming::{tag_file_name, title_from_filename, vgm_ren_title};

/// The severity tier of a [`ReadinessItem`], matching the export gate: an
/// [`Error`](Severity::Error) blocks export, a [`Warning`](Severity::Warning)
/// prompts an "export anyway?" confirm, and a [`Note`](Severity::Note) is shown
/// in the checklist but never gates (genuinely-optional things, like a track that
/// legitimately never loops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// The checklist group a [`ReadinessItem`] belongs to, so the submission
/// checklist can show one line per group (a tick when the group is clean). The
/// content checks here fill the middle three groups and [`Loops`](Self::Loops);
/// the app fills [`Files`](Self::Files) with its file-level shape checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessCategory {
    /// Package header fields (creator, date, authors, history, game name).
    PackInfo,
    /// Per-track GD3 tag completeness.
    TrackTags,
    /// Per-track GD3 fields that must agree with the pack meta.
    Consistency,
    /// Tracks with no loop point.
    Loops,
    /// File-level shape: readable songs, `NN Title` naming, screenshot.
    Files,
}

impl ReadinessCategory {
    /// The categories in checklist display order.
    pub const ALL: [ReadinessCategory; 5] = [
        ReadinessCategory::PackInfo,
        ReadinessCategory::TrackTags,
        ReadinessCategory::Consistency,
        ReadinessCategory::Loops,
        ReadinessCategory::Files,
    ];

    /// The group's heading in the checklist.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ReadinessCategory::PackInfo => "Package info",
            ReadinessCategory::TrackTags => "Track tags",
            ReadinessCategory::Consistency => "Consistency with the pack",
            ReadinessCategory::Loops => "Loops",
            ReadinessCategory::Files => "Files & naming",
        }
    }
}

/// A package-metadata form field a [`ReadinessItem`] can point at, so the UI can
/// scroll to and focus it. Only the fields the checks actually flag are modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaField {
    GameName,
    MusicAuthors,
    ReleaseDate,
    Creator,
    History,
}

/// What a [`ReadinessItem`] points at, so the checklist can navigate straight to
/// the fix: a metadata field to focus, a track (by 0-based pack index) to open in
/// quick-edit, or the pack as a whole (an item tied to no single field or track).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessTarget {
    Meta(MetaField),
    Track(usize),
    Pack,
}

/// One submission-readiness finding: its severity, the checklist group it falls
/// under, the message shown to the user, and the field or track it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessItem {
    pub severity: Severity,
    pub category: ReadinessCategory,
    pub target: ReadinessTarget,
    pub message: String,
}

/// A slim, UI-free view of one song for [`readiness`]: no egui, no app types, so
/// the checks stay wasm-clean and table-testable. The UI builds these from its
/// loaded tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackFacts {
    /// The file's current name on disk, `NN Title.ext`.
    pub file_name: String,
    /// The parsed GD3 tag, or `None` when the song carries none.
    pub tag: Option<Gd3Tag>,
    /// Whether the song has a loop point.
    pub loops: bool,
    /// Whether the song parsed. An unreadable track is skipped by every content
    /// check here -- the file-level "could not be read" warning covers it.
    pub readable: bool,
}

/// Whether `s` is a VGMRips-style release date: an all-digit, hyphen-separated
/// `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`. This is the wiki convention (the rerip
/// guide converts slashes to hyphens); slashes, dots and free text all fail.
/// Field ranges are not checked -- only the shape.
#[must_use]
pub fn is_pack_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let widths: &[usize] = match parts.len() {
        1 => &[4],
        2 => &[4, 2],
        3 => &[4, 2, 2],
        _ => return false,
    };
    parts
        .iter()
        .zip(widths)
        .all(|(part, &width)| part.len() == width && part.bytes().all(|b| b.is_ascii_digit()))
}

/// If `date` is a slash-separated date that becomes a valid [`is_pack_date`] just
/// by swapping slashes for hyphens (e.g. `1994/03/01` -> `1994-03-01`), the
/// hyphenated form; otherwise `None`. This is the one mechanical date fix the
/// checklist offers -- it deliberately does not pad or reformat, so it can only
/// ever produce a valid date and never guesses.
#[must_use]
pub fn hyphenate_date(date: &str) -> Option<String> {
    if !date.contains('/') {
        return None;
    }
    let hyphenated = date.replace('/', "-");
    is_pack_date(&hyphenated).then_some(hyphenated)
}

/// The submission-readiness checks the VGMRips wiki wants verified before a pack
/// is submitted, beyond the file-level shape [`crate::pack`]'s callers already
/// check: complete and consistent GD3 tags, hyphen-separated dates, update notes,
/// and loops.
///
/// Pure over [`PackMeta`] and per-track [`TrackFacts`] -- no filesystem, no egui --
/// so it is driven entirely by table tests and runs on the web too. Each returned
/// [`ReadinessItem`] carries its severity, a message, and the field or track to
/// navigate to. A `Track(index)` target is the 0-based position in `tracks`, so
/// pass one `TrackFacts` per pack track in order and the index maps straight back.
#[must_use]
pub fn readiness(meta: &PackMeta, tracks: &[TrackFacts]) -> Vec<ReadinessItem> {
    let mut items = Vec::new();
    check_pack_meta(meta, &mut items);
    for (index, facts) in tracks.iter().enumerate() {
        if facts.readable {
            check_track(index, facts, meta, &mut items);
        }
    }
    check_author_consistency(meta, tracks, &mut items);
    check_loops(tracks, &mut items);
    items
}

fn warn(
    items: &mut Vec<ReadinessItem>,
    category: ReadinessCategory,
    target: ReadinessTarget,
    message: String,
) {
    items.push(ReadinessItem {
        severity: Severity::Warning,
        category,
        target,
        message,
    });
}

fn note(
    items: &mut Vec<ReadinessItem>,
    category: ReadinessCategory,
    target: ReadinessTarget,
    message: String,
) {
    items.push(ReadinessItem {
        severity: Severity::Note,
        category,
        target,
        message,
    });
}

/// P1-P4: the pack-level header fields a submission must fill.
fn check_pack_meta(meta: &PackMeta, items: &mut Vec<ReadinessItem>) {
    let cat = ReadinessCategory::PackInfo;
    if meta.creator.trim().is_empty() {
        warn(
            items,
            cat,
            ReadinessTarget::Meta(MetaField::Creator),
            "Package created by (the ripper) is empty.".to_owned(),
        );
    }
    let date = meta.release_date.trim();
    if date.is_empty() {
        warn(
            items,
            cat,
            ReadinessTarget::Meta(MetaField::ReleaseDate),
            "Game release date is empty.".to_owned(),
        );
    } else if !is_pack_date(date) {
        warn(
            items,
            cat,
            ReadinessTarget::Meta(MetaField::ReleaseDate),
            format!(
                "Game release date \"{date}\" should be a hyphen-separated date \
                 (YYYY, YYYY-MM or YYYY-MM-DD)."
            ),
        );
    }
    if meta.music_authors.trim().is_empty() {
        warn(
            items,
            cat,
            ReadinessTarget::Meta(MetaField::MusicAuthors),
            "Music author is empty.".to_owned(),
        );
    }
    if meta.history.trim().is_empty() {
        warn(
            items,
            cat,
            ReadinessTarget::Meta(MetaField::History),
            "Package history (update notes) is empty.".to_owned(),
        );
    }
}

/// T1-T5 and C1-C4 for one readable track.
fn check_track(index: usize, facts: &TrackFacts, meta: &PackMeta, items: &mut Vec<ReadinessItem>) {
    let label = track_label(index, facts);
    let target = ReadinessTarget::Track(index);

    // T1: no GD3 tag at all -- nothing more to check on this track.
    let Some(tag) = facts.tag.as_ref() else {
        warn(
            items,
            ReadinessCategory::TrackTags,
            target,
            format!("{label}: has no GD3 tag."),
        );
        return;
    };

    // T2 + T3: every required field that is empty, gathered into one line.
    let required = [
        ("Track Name", tag.track_name_en.as_str()),
        ("Game Name", tag.game_name_en.as_str()),
        ("System", tag.system_name_en.as_str()),
        ("Composer", tag.track_author_en.as_str()),
        ("Release Date", tag.release_date.as_str()),
        ("Ripper", tag.creator.as_str()),
    ];
    let missing: Vec<&str> = required
        .into_iter()
        .filter(|(_, value)| value.trim().is_empty())
        .map(|(name, _)| name)
        .collect();
    if !missing.is_empty() {
        warn(
            items,
            ReadinessCategory::TrackTags,
            target,
            format!("{label}: missing {}.", missing.join(", ")),
        );
    }

    // T4: a present release date must be hyphen-separated.
    let date = tag.release_date.trim();
    if !date.is_empty() && !is_pack_date(date) {
        warn(
            items,
            ReadinessCategory::TrackTags,
            target,
            format!("{label}: release date \"{date}\" should be hyphen-separated."),
        );
    }

    // T5: the on-disk file name must still match the Track Name it derives from.
    // Compare the *titles* -- through `vgm_ren`'s replacements, so a name that
    // tool would have written is never read as drift -- rather than the whole
    // file names, leaving a wrong track number to the numbering check alone.
    let track_name = tag.track_name_en.trim();
    if let Some(expected) = tag_file_name(index + 1, track_name, &facts.file_name) {
        let file_title = title_from_filename(&facts.file_name).trim();
        if vgm_ren_title(track_name) != file_title {
            warn(
                items,
                ReadinessCategory::TrackTags,
                target,
                format!(
                    "{label}: file name doesn't match the Track Name \
                     (expected \"{expected}\" -- rename from the tag, or re-tag)."
                ),
            );
        }
    }

    // C1-C4: fields that must agree with the pack meta -- only when both sides are
    // set (an empty side is already covered by the missing-field checks and P1-P4).
    let consistency = [
        (
            "game name",
            tag.game_name_en.as_str(),
            meta.game_name.as_str(),
        ),
        ("system", tag.system_name_en.as_str(), meta.system.as_str()),
        ("ripper", tag.creator.as_str(), meta.creator.as_str()),
        (
            "release date",
            tag.release_date.as_str(),
            meta.release_date.as_str(),
        ),
    ];
    for (what, track_value, meta_value) in consistency {
        let track_value = track_value.trim();
        let meta_value = meta_value.trim();
        if !track_value.is_empty() && !meta_value.is_empty() && track_value != meta_value {
            warn(
                items,
                ReadinessCategory::Consistency,
                target,
                format!(
                    "{label}: {what} \"{track_value}\" differs from the pack's \"{meta_value}\"."
                ),
            );
        }
    }
}

/// C5 (note): the union of the tracks' GD3 composers vs the pack's Music author
/// list. Track authors vary legitimately, so this is only ever a note.
fn check_author_consistency(
    meta: &PackMeta,
    tracks: &[TrackFacts],
    items: &mut Vec<ReadinessItem>,
) {
    let pack: BTreeSet<String> = split_authors(&meta.music_authors).collect();
    if pack.is_empty() {
        return; // an empty Music author field is P3's business
    }
    let mut tracked: BTreeSet<String> = BTreeSet::new();
    for facts in tracks {
        if facts.readable
            && let Some(tag) = &facts.tag
        {
            tracked.extend(split_authors(&tag.track_author_en));
        }
    }
    if !tracked.is_empty() && tracked != pack {
        note(
            items,
            ReadinessCategory::Consistency,
            ReadinessTarget::Meta(MetaField::MusicAuthors),
            "The tracks' composers don't all match the pack's Music author list.".to_owned(),
        );
    }
}

/// L1 (note): the readable tracks with no loop point, one per line. Jingles
/// legitimately never loop, so this only asks the packager to verify.
///
/// One item listing many tracks rather than one item each: the packager checks
/// them as a set ("are these all jingles?"), and a pack of twenty loopless
/// tracks would otherwise bury every other finding.
fn check_loops(tracks: &[TrackFacts], items: &mut Vec<ReadinessItem>) {
    let loopless: Vec<String> = tracks
        .iter()
        .enumerate()
        .filter(|(_, facts)| facts.readable && !facts.loops)
        .map(|(index, facts)| track_label(index, facts))
        .collect();
    if !loopless.is_empty() {
        note(
            items,
            ReadinessCategory::Loops,
            ReadinessTarget::Pack,
            format!(
                "No loop point (verify these are meant to play once):\n{}",
                loopless.join("\n")
            ),
        );
    }
}

/// The `NN Title` label for a track in a message: its 1-based position and the
/// GD3 English track name, falling back to the file name's title when untagged.
fn track_label(index: usize, facts: &TrackFacts) -> String {
    let title = facts
        .tag
        .as_ref()
        .map(|tag| tag.track_name_en.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| title_from_filename(&facts.file_name));
    format!("{:02} {title}", index + 1)
}

/// Splits a `Music author` / composer string into individual names on commas and
/// ampersands, trimmed, dropping empties.
fn split_authors(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split([',', '&'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}
