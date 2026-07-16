//! Preparing a VGMRips submission ("rip") from a folder of VGM/VGZ files.
//!
//! A submission is a flat zip of `NN Track Title.vgz` songs plus three text
//! artefacts: a `Game Name.txt` *description*, a `Game Name.m3u` *playlist*, and
//! (elsewhere) a `Game Name.png` screenshot. This module owns the two things that
//! are pure data transforms -- generating and parsing the description, and
//! generating the playlist -- so they stay wasm-clean and testable without a
//! filesystem. Folder scanning, zip building and PNG optimisation are native-only
//! and live in `dro-trimmer`.
//!
//! The description layout is not ours to invent: it matches the official template
//! and `vgm_stat`'s output byte-for-byte (validated against five real packs). The
//! load-bearing quirks -- CRLF endings, the 47-column width, header values wrapped
//! at 26 columns, track titles wrapped at 35, the time block right-aligned to end
//! at column 47, and a non-looping track's `" -   "` dash landing at column 44 --
//! are all reproduced here and pinned by the tests.

use crate::error::{Error, Result};
use crate::song::{OplType, Song};

/// The system name a fresh PC rip defaults to.
pub const DEFAULT_SYSTEM: &str = "IBM PC/AT";
/// The OS a fresh PC rip defaults to.
pub const DEFAULT_OS: &str = "DOS";
/// The song-list heading a fresh rip uses; real packs vary the wording.
pub const DEFAULT_SONG_LIST_HEADING: &str = "Song list, in approximate game order:";

/// Total line width of the description file.
const LINE_WIDTH: usize = 47;
/// Header values start in this column (0-based 21), so the label pads to 21.
const LABEL_WIDTH: usize = 21;
/// Header values wrap at this width (`LINE_WIDTH - LABEL_WIDTH`).
const HEADER_VALUE_WIDTH: usize = LINE_WIDTH - LABEL_WIDTH;
/// Track titles (including the `NN ` number prefix) wrap at this width.
const TITLE_WIDTH: usize = 35;
/// The loop string for a non-looping track. Right-aligned in the 6-wide loop
/// field it becomes `"  -   "`, landing the dash at column 44 -- exactly where
/// `vgm_stat` puts it. The trailing spaces are trimmed off the finished line.
const NO_LOOP: &str = " -   ";

/// Editable package metadata: everything in the description's header, plus the
/// free-text Notes and Package-history sections.
///
/// Parsing a description fills known fields and preserves any unrecognised
/// `Label: value` lines in [`RipMeta::extra_fields`] so re-saving does not lose
/// them. Empty known fields are omitted when generating, which is how legacy
/// packs without an `OS:` line round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RipMeta {
    pub game_name: String,
    pub system: String,
    pub os: String,
    pub music_hardware: String,
    /// The value of the (singular) `Music author:` field, verbatim.
    pub music_authors: String,
    pub developer: String,
    pub publisher: String,
    pub release_date: String,
    /// `Package created by:` -- the ripper.
    pub creator: String,
    /// `Package version:`, e.g. `"1.00"`.
    pub version: String,
    /// The heading above the song list, captured through its first `:`.
    pub song_list_heading: String,
    /// Unrecognised header `Label: value` lines, in file order.
    pub extra_fields: Vec<(String, String)>,
    /// The Notes section, verbatim: interior lines byte-exact, no trailing newline.
    pub notes: String,
    /// The Package-history section, verbatim.
    pub history: String,
}

impl Default for RipMeta {
    fn default() -> Self {
        Self {
            game_name: String::new(),
            system: String::new(),
            os: String::new(),
            music_hardware: String::new(),
            music_authors: String::new(),
            developer: String::new(),
            publisher: String::new(),
            release_date: String::new(),
            creator: String::new(),
            version: String::new(),
            song_list_heading: DEFAULT_SONG_LIST_HEADING.to_owned(),
            extra_fields: Vec::new(),
            notes: String::new(),
            history: String::new(),
        }
    }
}

/// One row of the song list: a title and its timings, in samples at 44100 Hz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackEntry {
    pub title: String,
    pub total_samples: u64,
    /// Samples in one loop, or `None` for a non-looping track.
    pub loop_samples: Option<u64>,
    /// Play-throughs (`>= 1`). The Total Length loop column adds
    /// `loop_samples * (plays - 1)` per track, matching `vgm_stat`.
    pub plays: u32,
}

impl TrackEntry {
    /// Derives an entry from a loaded song and its file name.
    ///
    /// The title is the GD3 English track name, falling back to the file name's
    /// stem (minus any `NN ` prefix) when there is no tag. Timings come from the
    /// command stream, so they stay correct after trimming.
    #[must_use]
    pub fn from_song(song: &Song, file_name: &str) -> Self {
        let title = song
            .vgm_meta()
            .and_then(|meta| meta.tag.as_ref())
            .map(|tag| tag.track_name_en.trim())
            .filter(|name| !name.is_empty())
            .map_or_else(|| title_from_filename(file_name).to_owned(), str::to_owned);
        let plays = song
            .vgm_meta()
            .map_or(1, |meta| vgm_play_count(meta.loop_base, meta.loop_modifier));
        Self {
            title,
            total_samples: u64::from(song.total_delay_samples()),
            loop_samples: song.loop_num_samples().map(u64::from),
            plays,
        }
    }
}

/// The title carried by a file named `NN Title.ext`: the stem with the leading
/// two-or-more digit number and its trailing space removed.
#[must_use]
pub fn title_from_filename(file_name: &str) -> &str {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let digits = stem.bytes().take_while(u8::is_ascii_digit).count();
    match stem.as_bytes().get(digits) {
        Some(b' ') if digits > 0 => &stem[digits + 1..],
        _ => stem,
    }
}

/// The number of times a VGM plays, from its loop-base and loop-modifier header
/// fields. This is `vgm_stat`'s formula; the modifier is treated as `0x10` when
/// zero, and the base is a signed byte.
#[must_use]
pub fn vgm_play_count(loop_base: u8, loop_modifier: u8) -> u32 {
    let modifier = if loop_modifier == 0 {
        0x10
    } else {
        i32::from(loop_modifier)
    };
    let base = i32::from(loop_base as i8);
    let plays = (2 * modifier + 0x08) / 0x10 - base;
    u32::try_from(plays.max(1)).expect("plays is clamped to >= 1")
}

/// Formats a sample count as `M:SS` (or `H:MM:SS` past an hour), rounding to the
/// nearest second as `vgm_stat` does. The leading unit is never zero-padded.
///
/// This is deliberately *not* [`crate::util::ms_to_timestr`], which zero-pads
/// minutes and truncates rather than rounds -- a different format for a different
/// file.
#[must_use]
pub fn format_track_time(samples: u64) -> String {
    let seconds = (samples + 22_050) / 44_100;
    let (hours, minutes, secs) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

/// A one-click fill for the System / OS / Music hardware fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RipPreset {
    /// The button label, e.g. `"OPL-2"`.
    pub name: &'static str,
    pub system: &'static str,
    pub os: &'static str,
    pub music_hardware: &'static str,
}

/// The chip presets, in OPL order. All PC rips share the system and OS; the
/// hardware line names the chip.
pub const PRESETS: [RipPreset; 3] = [
    RipPreset {
        name: "OPL-2",
        system: DEFAULT_SYSTEM,
        os: DEFAULT_OS,
        music_hardware: "AdLib/Sound Blaster (YM3812)",
    },
    RipPreset {
        name: "Dual OPL-2",
        system: DEFAULT_SYSTEM,
        os: DEFAULT_OS,
        music_hardware: "Dual OPL2 (2x YM3812)",
    },
    RipPreset {
        name: "OPL-3",
        system: DEFAULT_SYSTEM,
        os: DEFAULT_OS,
        music_hardware: "Sound Blaster Pro 2 (YMF262)",
    },
];

/// The preset matching a chip type.
#[must_use]
pub const fn preset_for(opl: OplType) -> &'static RipPreset {
    match opl {
        OplType::Opl2 => &PRESETS[0],
        OplType::DualOpl2 => &PRESETS[1],
        OplType::Opl3 => &PRESETS[2],
    }
}

/// A suggested (editable) `Music hardware:` value for the chip a rip targets.
#[must_use]
pub fn music_hardware_suggestion(opl: OplType) -> &'static str {
    preset_for(opl).music_hardware
}

/// A file-name-safe stem for the `.txt`/`.m3u`/`.zip`, from the game name.
#[must_use]
pub fn doc_file_stem(game_name: &str) -> String {
    let sanitised: String = game_name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    sanitised.trim().to_owned()
}

/// Builds an `.m3u` playlist: one file name per line, CRLF-terminated, no header.
#[must_use]
pub fn generate_m3u(file_names: &[String]) -> String {
    let mut out = String::new();
    for name in file_names {
        out.push_str(name);
        out.push_str("\r\n");
    }
    out
}

/// Renders the full description file for `meta` and `tracks`.
///
/// Header values are greedily word-wrapped at the value column. A value that a
/// packager hand-formatted as a multi-line block with its own alignment (some
/// elaborate multi-author credits do this, with varying indentation and internal
/// alignment spaces) is normalised to that greedy wrap on save: every word is
/// preserved, but manual whitespace alignment is not. The result is stable --
/// saving it again is a no-op.
#[must_use]
pub fn generate_description(meta: &RipMeta, tracks: &[TrackEntry]) -> String {
    // Banner.
    let mut lines: Vec<String> = vec![
        "*".repeat(LINE_WIDTH),
        banner_line("* VGM music package"),
        banner_line("* http://vgmrips.net/"),
        "*".repeat(LINE_WIDTH),
    ];

    // Header field groups, blank-separated as in the template.
    push_field(&mut lines, "Game name:", &meta.game_name);
    push_field(&mut lines, "System:", &meta.system);
    push_field(&mut lines, "OS:", &meta.os);
    push_field(&mut lines, "Music hardware:", &meta.music_hardware);
    lines.push(String::new());
    push_field(&mut lines, "Music author:", &meta.music_authors);
    push_field(&mut lines, "Game developer:", &meta.developer);
    push_field(&mut lines, "Game publisher:", &meta.publisher);
    push_field(&mut lines, "Game release date:", &meta.release_date);
    lines.push(String::new());
    push_field(&mut lines, "Package created by:", &meta.creator);
    push_field(&mut lines, "Package version:", &meta.version);
    for (label, value) in &meta.extra_fields {
        push_field(&mut lines, &format!("{label}:"), value);
    }
    lines.push(String::new());

    // Song list.
    lines.push(meta.song_list_heading.trim_end().to_owned());
    lines.push(format!("{:<width$}Length:", "Song name", width = 36));
    lines.push(format!("{:<width$}Total  Loop", "", width = 36));
    let num_width = num_width(tracks.len());
    for (index, track) in tracks.iter().enumerate() {
        push_track_rows(&mut lines, index, num_width, track);
    }
    lines.push(String::new());
    push_total_row(&mut lines, tracks);

    // Notes and history: lines that fit pass through byte-exact; anything longer
    // (a paragraph typed without manual newlines) is wrapped to the file width.
    lines.push(String::new());
    lines.push(String::new());
    lines.push("Notes:".to_owned());
    push_wrapped_block(&mut lines, &meta.notes, "");
    lines.push(String::new());
    lines.push(String::new());
    lines.push("Package history:".to_owned());
    // The history convention indents continuation lines by one space.
    push_wrapped_block(&mut lines, &meta.history, " ");

    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

/// Parses a description back into [`RipMeta`], tolerantly.
///
/// Accepts CRLF or LF, any banner URL, missing fields, the compact one-line song
/// header some old packs use, and a single blank line before the section markers.
/// The song-list block is skipped -- track timings are recomputed from the files.
/// Returns [`Error::File`] only when the text carries no recognisable field or
/// section at all.
pub fn parse_description(text: &str) -> Result<RipMeta> {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalised.split('\n').collect();

    let song_list_idx = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Song list"));
    let notes_idx = lines.iter().position(|line| line.trim_end() == "Notes:");
    let history_idx = lines
        .iter()
        .position(|line| line.trim_end() == "Package history:");

    let header_end = [song_list_idx, notes_idx, history_idx]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(lines.len());

    let mut meta = RipMeta::default();
    let mut matched_known = false;

    for (label, value) in parse_header_fields(&lines[..header_end]) {
        let slot = match label.to_ascii_lowercase().as_str() {
            "game name" => &mut meta.game_name,
            "system" => &mut meta.system,
            "os" => &mut meta.os,
            "music hardware" => &mut meta.music_hardware,
            "music author" | "music authors" => &mut meta.music_authors,
            "game developer" => &mut meta.developer,
            "game publisher" => &mut meta.publisher,
            "game release date" => &mut meta.release_date,
            "package created by" => &mut meta.creator,
            "package version" => &mut meta.version,
            _ => {
                meta.extra_fields.push((label, value));
                continue;
            }
        };
        *slot = value;
        matched_known = true;
    }

    if let Some(idx) = song_list_idx
        && let Some(colon) = lines[idx].find(':')
    {
        meta.song_list_heading = lines[idx][..=colon].trim_start().to_owned();
    }

    if let Some(idx) = notes_idx {
        let end = history_idx.filter(|&h| h > idx).unwrap_or(lines.len());
        meta.notes = verbatim_block(&lines[idx + 1..end]);
    }
    if let Some(idx) = history_idx {
        meta.history = verbatim_block(&lines[idx + 1..]);
    }

    if !matched_known && notes_idx.is_none() && history_idx.is_none() && song_list_idx.is_none() {
        return Err(Error::file("not a VGMRips description file"));
    }
    Ok(meta)
}

// -- generation helpers ------------------------------------------------------

fn banner_line(text: &str) -> String {
    format!("{text:<width$}*", width = LINE_WIDTH - 1)
}

fn num_width(track_count: usize) -> usize {
    track_count.to_string().len().max(2)
}

/// Emits a header field, wrapped and blank-omitting; strips trailing spaces.
fn push_field(lines: &mut Vec<String>, label: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    for (index, chunk) in wrap_value(value, HEADER_VALUE_WIDTH).iter().enumerate() {
        let line = if index == 0 {
            format!("{label:<width$}{chunk}", width = LABEL_WIDTH)
        } else {
            format!("{:<width$}{chunk}", "", width = LABEL_WIDTH)
        };
        lines.push(line.trim_end().to_owned());
    }
}

/// Emits a Notes/Package-history block. A line already within the file width is
/// kept byte-exact (existing packs stay verbatim, trailing spaces and all); a
/// longer one -- prose typed without manual newlines -- is greedily word-wrapped,
/// with continuation lines prefixed (the history convention is one space).
fn push_wrapped_block(lines: &mut Vec<String>, text: &str, continuation_prefix: &str) {
    let continuation_width = LINE_WIDTH - continuation_prefix.chars().count();
    for line in text.split('\n') {
        if line.chars().count() <= LINE_WIDTH {
            lines.push(line.to_owned());
            continue;
        }
        let mut first = true;
        let mut current: Vec<char> = Vec::new();
        let flush = |current: &mut Vec<char>, first: &mut bool, lines: &mut Vec<String>| {
            let content: String = current.drain(..).collect();
            if *first {
                lines.push(content);
                *first = false;
            } else {
                lines.push(format!("{continuation_prefix}{content}"));
            }
        };
        for word in line.split_whitespace() {
            let mut chars: Vec<char> = word.chars().collect();
            loop {
                let width = if first {
                    LINE_WIDTH
                } else {
                    continuation_width
                };
                if current.is_empty() {
                    if chars.len() <= width {
                        current = chars;
                        break;
                    }
                    // A word longer than a whole line (a URL): hard-split it.
                    let rest = chars.split_off(width);
                    current = chars;
                    flush(&mut current, &mut first, lines);
                    chars = rest;
                } else if current.len() + 1 + chars.len() <= width {
                    current.push(' ');
                    current.extend(chars);
                    break;
                } else {
                    flush(&mut current, &mut first, lines);
                }
            }
        }
        if !current.is_empty() {
            flush(&mut current, &mut first, lines);
        }
    }
}

/// Greedy word-wrap for header values: breaks at spaces, hard-splitting any word
/// longer than `width`. No hyphenation (unlike titles).
fn wrap_value(value: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current: Vec<char> = Vec::new();
    for word in value.split_whitespace() {
        let mut chars: Vec<char> = word.chars().collect();
        while chars.len() > width {
            if !current.is_empty() {
                lines.push(current.iter().collect());
                current.clear();
            }
            let rest = chars.split_off(width);
            lines.push(chars.iter().collect());
            chars = rest;
        }
        if chars.is_empty() {
            continue;
        }
        if current.is_empty() {
            current = chars;
        } else if current.len() + 1 + chars.len() <= width {
            current.push(' ');
            current.extend(chars);
        } else {
            lines.push(current.iter().collect());
            current = chars;
        }
    }
    if !current.is_empty() {
        lines.push(current.iter().collect());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Emits one track's row(s): the wrapped title, with the time block right-aligned
/// to end at column 47 on the final line.
fn push_track_rows(lines: &mut Vec<String>, index: usize, num_width: usize, track: &TrackEntry) {
    let prefix = format!("{number:0width$} ", number = index + 1, width = num_width);
    let indent = " ".repeat(num_width + 1);
    let avail = TITLE_WIDTH - (num_width + 1);
    let chunks = break_title(&track.title, avail);

    let total = format_track_time(track.total_samples);
    let loop_str = track
        .loop_samples
        .map_or_else(|| NO_LOOP.to_owned(), format_track_time);
    let block = format!("{total:>5} {loop_str:>6}");

    let last = chunks.len() - 1;
    for (row, chunk) in chunks.iter().enumerate() {
        let head = format!("{}{chunk}", if row == 0 { &prefix } else { &indent });
        if row == last {
            let pad = LINE_WIDTH.saturating_sub(head.chars().count() + block.chars().count());
            lines.push(
                format!("{head}{}{block}", " ".repeat(pad))
                    .trim_end()
                    .to_owned(),
            );
        } else {
            lines.push(head.trim_end().to_owned());
        }
    }
}

/// Emits the `Total Length` summary row. Its loop column is always a time (the
/// sum of every track's total plus one extra play of each loop), never a dash.
fn push_total_row(lines: &mut Vec<String>, tracks: &[TrackEntry]) {
    let total: u64 = tracks.iter().map(|track| track.total_samples).sum();
    let extra_loops: u64 = tracks
        .iter()
        .filter_map(|track| Some(track.loop_samples? * u64::from(track.plays.saturating_sub(1))))
        .sum();
    let total_str = format_track_time(total);
    let loop_str = format_track_time(total + extra_loops);
    let block = format!("{total_str:>5} {loop_str:>6}");
    let head = "Total Length";
    let pad = LINE_WIDTH.saturating_sub(head.chars().count() + block.chars().count());
    lines.push(
        format!("{head}{}{block}", " ".repeat(pad))
            .trim_end()
            .to_owned(),
    );
}

/// Splits a title into chunks of at most `avail` chars, following `vgm_stat`:
/// break at the last space, else after the last non-alphanumeric, else hyphenate.
fn break_title(title: &str, avail: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut rest: Vec<char> = title.chars().collect();
    if avail == 0 {
        return vec![title.to_owned()];
    }
    loop {
        if rest.len() <= avail {
            chunks.push(rest.iter().collect());
            return chunks;
        }
        if let Some(pos) = (0..=avail).rev().find(|&i| rest[i] == ' ') {
            chunks.push(rest[..pos].iter().collect());
            rest.drain(..=pos);
        } else if let Some(pos) = (0..avail).rev().find(|&i| !rest[i].is_alphanumeric()) {
            chunks.push(rest[..=pos].iter().collect());
            rest.drain(..=pos);
        } else {
            let mut chunk: String = rest[..avail - 1].iter().collect();
            chunk.push('-');
            chunks.push(chunk);
            rest.drain(..avail - 1);
        }
    }
}

// -- parsing helpers ---------------------------------------------------------

/// Collapses a header region into ordered `(label, value)` pairs, merging
/// leading-whitespace continuation lines into the preceding value.
fn parse_header_fields(lines: &[&str]) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.starts_with('*') || line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = fields.last_mut() {
                let trimmed = line.trim();
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(trimmed);
            }
            continue;
        }
        if let Some(colon) = line.find(':') {
            fields.push((
                line[..colon].trim().to_owned(),
                line[colon + 1..].trim().to_owned(),
            ));
        }
    }
    fields
}

/// Joins a section's lines verbatim, dropping only wholly-blank leading and
/// trailing lines. Interior blank lines and trailing spaces are preserved.
fn verbatim_block(lines: &[&str]) -> String {
    let start = lines.iter().position(|line| !line.trim().is_empty());
    let Some(start) = start else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("a non-blank line exists once start is found");
    lines[start..=end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Gd3Tag;
    use crate::io;

    const FIXTURE: &str = include_str!("../../../tests/description_vgm151_PC.txt");
    const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    /// Samples that render as exactly `secs` seconds (`secs * 44100` rounds to
    /// itself under the +22050 rounding).
    fn secs(secs: u64) -> u64 {
        secs * 44_100
    }

    #[test]
    fn the_fixture_is_checked_out_with_lf_endings() {
        // The golden comparisons below assume it; a CRLF-mangling checkout would
        // silently invalidate them.
        assert!(!FIXTURE.contains('\r'), "fixture must keep its LF endings");
    }

    #[test]
    fn parses_the_official_template() {
        let meta = parse_description(FIXTURE).unwrap();
        assert_eq!(meta.game_name, "Game-With-Cool-Music");
        assert_eq!(meta.system, "IBM PC/AT");
        assert_eq!(meta.os, "DOS");
        assert_eq!(meta.music_hardware, "Soundcard Name/OPL? (YM????)");
        assert_eq!(meta.music_authors, "I.P. Freely, another one, next author");
        assert_eq!(meta.developer, "AM2; Sonic Team");
        assert_eq!(meta.publisher, "Sega / Co-Publisher");
        assert_eq!(meta.release_date, "YYYY-MM-DD");
        assert_eq!(meta.creator, "Al Koholic");
        assert_eq!(meta.version, "1.00");
        assert_eq!(
            meta.song_list_heading,
            "Song list, in approximate game order:"
        );
        assert!(meta.extra_fields.is_empty());
        assert_eq!(
            meta.notes,
            "Feel free to write comments about anything\nrelated to this pack."
        );
        // The history's last line keeps its meaningful trailing space ("lost ").
        assert!(
            meta.history
                .starts_with("1.00 1999-12-31 Al Koholic: Initial release.")
        );
        assert!(meta.history.contains("Added lost \n song."));
    }

    #[test]
    fn regenerates_the_template_header_block_byte_for_byte() {
        let meta = parse_description(FIXTURE).unwrap();
        let generated = generate_description(&meta, &[]);
        let gen_lines: Vec<&str> = generated.split("\r\n").collect();
        let fixture_lines: Vec<&str> = FIXTURE.split('\n').collect();
        // Lines 1-20: banner, wrapped header fields, blank, song-list heading.
        for i in 0..20 {
            assert_eq!(gen_lines[i], fixture_lines[i], "line {} differs", i + 1);
        }
    }

    #[test]
    fn generates_a_full_description_with_wrapping_and_totals() {
        let meta = RipMeta {
            game_name: "Test Game".to_owned(),
            system: "IBM PC/AT".to_owned(),
            os: "DOS".to_owned(),
            music_hardware: "AdLib (YM3812)".to_owned(),
            music_authors: "Me".to_owned(),
            developer: "Dev".to_owned(),
            publisher: "Pub".to_owned(),
            release_date: "2020".to_owned(),
            creator: "Ripper".to_owned(),
            version: "1.00".to_owned(),
            notes: "Line one.\nLine two.".to_owned(),
            history: "1.00 2020-01-01 Ripper: Initial release.".to_owned(),
            ..RipMeta::default()
        };
        let tracks = [
            TrackEntry {
                title: "Intro".to_owned(),
                total_samples: secs(60),
                loop_samples: None,
                plays: 1,
            },
            TrackEntry {
                title: "Looping Tune".to_owned(),
                total_samples: secs(120),
                loop_samples: Some(secs(120)),
                plays: 2,
            },
            TrackEntry {
                title: "A very long title that keeps going and going".to_owned(),
                total_samples: secs(30),
                loop_samples: None,
                plays: 1,
            },
        ];
        let expected = [
            "***********************************************",
            "* VGM music package                           *",
            "* http://vgmrips.net/                         *",
            "***********************************************",
            "Game name:           Test Game",
            "System:              IBM PC/AT",
            "OS:                  DOS",
            "Music hardware:      AdLib (YM3812)",
            "",
            "Music author:        Me",
            "Game developer:      Dev",
            "Game publisher:      Pub",
            "Game release date:   2020",
            "",
            "Package created by:  Ripper",
            "Package version:     1.00",
            "",
            "Song list, in approximate game order:",
            "Song name                           Length:",
            "                                    Total  Loop",
            "01 Intro                            1:00   -",
            "02 Looping Tune                     2:00   2:00",
            "03 A very long title that keeps",
            "   going and going                  0:30   -",
            "",
            "Total Length                        3:30   5:30",
            "",
            "",
            "Notes:",
            "Line one.",
            "Line two.",
            "",
            "",
            "Package history:",
            "1.00 2020-01-01 Ripper: Initial release.",
        ]
        .join("\r\n")
            + "\r\n";
        assert_eq!(generate_description(&meta, &tracks), expected);
    }

    #[test]
    fn track_row_dash_lands_at_column_44() {
        let mut lines = Vec::new();
        push_track_rows(
            &mut lines,
            0,
            2,
            &TrackEntry {
                title: "Intro".to_owned(),
                total_samples: secs(142),
                loop_samples: None,
                plays: 1,
            },
        );
        let row = &lines[0];
        // "2:22" total ends at column 40, the non-looping dash sits at column 44.
        assert_eq!(row.char_indices().nth(43).map(|(_, c)| c), Some('-'));
        assert_eq!(&row[36..40], "2:22");
    }

    #[test]
    fn total_length_row_overflows_gracefully_for_hour_long_packs() {
        let mut lines = Vec::new();
        // A pack whose totals exceed an hour: the H:MM:SS values overflow their
        // fields and squeeze the row left, still ending at column 47.
        push_total_row(
            &mut lines,
            &[TrackEntry {
                title: "x".to_owned(),
                total_samples: secs(4682), // 1:18:02
                loop_samples: Some(secs(4682)),
                plays: 2,
            }],
        );
        assert_eq!(lines[0], "Total Length                    1:18:02 2:36:04");
        assert_eq!(lines[0].chars().count(), 47);
    }

    #[test]
    fn format_track_time_rounds_to_the_nearest_second() {
        assert_eq!(format_track_time(0), "0:00");
        assert_eq!(format_track_time(22_049), "0:00");
        assert_eq!(format_track_time(22_050), "0:01");
        assert_eq!(format_track_time(44_100), "0:01");
        assert_eq!(format_track_time(secs(90)), "1:30");
        assert_eq!(format_track_time(secs(3600)), "1:00:00");
        assert_eq!(format_track_time(secs(6528)), "1:48:48");
    }

    #[test]
    fn vgm_play_count_matches_vgm_stat() {
        assert_eq!(vgm_play_count(0, 0), 2);
        assert_eq!(vgm_play_count(0, 0x10), 2);
        assert_eq!(vgm_play_count(0, 0x20), 4);
        assert_eq!(vgm_play_count(1, 0), 1);
        assert_eq!(vgm_play_count(255, 0), 3); // loop_base -1 as i8
        assert_eq!(vgm_play_count(10, 0), 1); // clamped to at least one play
    }

    #[test]
    fn title_from_filename_strips_number_prefix_and_extension() {
        assert_eq!(title_from_filename("01 Foo Bar.vgz"), "Foo Bar");
        assert_eq!(title_from_filename("117 X.vgm"), "X");
        assert_eq!(title_from_filename("Waltz No. 1.vgz"), "Waltz No. 1");
        assert_eq!(title_from_filename("NoNumber.vgm"), "NoNumber");
        assert_eq!(title_from_filename("03.vgm"), "03");
    }

    #[test]
    fn generates_crlf_m3u_without_header() {
        let names = ["01 A.vgz".to_owned(), "02 B.vgz".to_owned()];
        assert_eq!(generate_m3u(&names), "01 A.vgz\r\n02 B.vgz\r\n");
        assert_eq!(generate_m3u(&[]), "");
    }

    #[test]
    fn parses_legacy_packs_without_an_os_line_or_column_headers() {
        // An old mdscene-era pack: different banner URL, System holds the OS, a
        // compact one-line song header, a single blank before the markers, and no
        // trailing newline.
        let legacy = "***********************************************\r\n\
            * VGM music package                           *\r\n\
            * http://vgm.mdscene.net/                     *\r\n\
            ***********************************************\r\n\
            Game name:           Doom\r\n\
            System:              PC / DOS\r\n\
            Music hardware:      AdLib/OPL (YM3812)\r\n\
            \r\n\
            Music author:        Robert Prince\r\n\
            \r\n\
            Package created by:  RichterEX2\r\n\
            Package version:     1.00\r\n\
            \r\n\
            Song list:                          Total  Loop\r\n\
            01 Title Screen                     0:09   -\r\n\
            \r\n\
            Total Length                        0:09   0:09\r\n\
            Notes:\r\n\
            Thanks!\r\n\
            \r\n\
            Package history:\r\n\
            1.00 2012-05-27 RichterEX2: Initial release.";
        let meta = parse_description(legacy).unwrap();
        assert_eq!(meta.game_name, "Doom");
        assert_eq!(meta.system, "PC / DOS");
        assert_eq!(meta.os, "", "legacy pack has no OS line");
        assert_eq!(meta.song_list_heading, "Song list:");
        assert_eq!(meta.notes, "Thanks!");
        assert_eq!(meta.history, "1.00 2012-05-27 RichterEX2: Initial release.");

        // Regenerating omits the empty OS line and modernises the layout.
        let generated = generate_description(&meta, &[]);
        assert!(!generated.contains("OS:"));
        assert!(generated.contains("* http://vgmrips.net/"));
    }

    #[test]
    fn preserves_and_round_trips_unknown_header_fields() {
        let text = "Game name:           Some Game\r\n\
            Custom label:        a custom value\r\n\
            \r\n\
            Notes:\r\n\
            -\r\n\
            \r\n\
            Package history:\r\n\
            1.00 2020-01-01 Me: Initial release.\r\n";
        let meta = parse_description(text).unwrap();
        assert_eq!(
            meta.extra_fields,
            vec![("Custom label".to_owned(), "a custom value".to_owned())]
        );
        // The unknown field survives a generate/parse cycle.
        let round_tripped = parse_description(&generate_description(&meta, &[])).unwrap();
        assert_eq!(round_tripped, meta);
    }

    #[test]
    fn parse_of_generate_is_the_identity_on_meta() {
        let meta = RipMeta {
            game_name: "Commander Keen in Goodbye, Galaxy! Episode IV: Secret of the Oracle"
                .to_owned(),
            system: "IBM PC/AT".to_owned(),
            os: "DOS".to_owned(),
            music_hardware: "Sound Blaster 16/OPL2 (YM3812)".to_owned(),
            music_authors: "Robert Prince".to_owned(),
            developer: "id Software".to_owned(),
            publisher: "Apogee Software".to_owned(),
            release_date: "1991-12-15".to_owned(),
            creator: "The Green Herring".to_owned(),
            version: "1.50".to_owned(),
            notes: "First line.\n\nThird line after a blank.".to_owned(),
            history: "1.00 2014-08-24 The Green Herring: Initial\n release.".to_owned(),
            ..RipMeta::default()
        };
        let tracks = [TrackEntry {
            title: "Some Track".to_owned(),
            total_samples: secs(21),
            loop_samples: Some(secs(21)),
            plays: 2,
        }];
        let round_tripped = parse_description(&generate_description(&meta, &tracks)).unwrap();
        assert_eq!(round_tripped, meta);
    }

    #[test]
    fn empty_os_round_trips() {
        let meta = RipMeta {
            game_name: "Legacy".to_owned(),
            system: "PC / DOS".to_owned(),
            music_authors: "Someone".to_owned(),
            version: "1.00".to_owned(),
            ..RipMeta::default()
        };
        let round_tripped = parse_description(&generate_description(&meta, &[])).unwrap();
        assert_eq!(round_tripped.os, "");
        assert_eq!(round_tripped, meta);
    }

    #[test]
    fn hundred_tracks_use_three_digit_numbers_and_indent() {
        let tracks: Vec<TrackEntry> = (0..100)
            .map(|i| TrackEntry {
                title: format!("Track with a fairly long title number {i}"),
                total_samples: secs(60),
                loop_samples: None,
                plays: 1,
            })
            .collect();
        let out = generate_description(&RipMeta::default(), &tracks);
        assert!(
            out.contains("\r\n001 Track with a fairly long title"),
            "three-digit prefix"
        );
        // Continuation lines indent by num_width + 1 == 4 spaces.
        assert!(out.contains("\r\n    "), "four-space continuation indent");
    }

    #[test]
    fn long_notes_and_history_lines_wrap_at_the_file_width() {
        let meta = RipMeta {
            game_name: "G".to_owned(),
            notes: "This pack was made using DOSBox and a whole lot of patience, because \
                    the game only plays each song once per boot and refuses to loop."
                .to_owned(),
            history: "1.00 2026-07-16 Someone: Initial release, with a remark long enough \
                      that it has to wrap onto a continuation line."
                .to_owned(),
            ..RipMeta::default()
        };
        let text = generate_description(&meta, &[]);
        let lines: Vec<&str> = text.split("\r\n").collect();

        // Nothing emitted exceeds the file width.
        for line in &lines {
            assert!(line.chars().count() <= 47, "line too long: {line:?}");
        }
        // History continuations carry the conventional one-space indent.
        let history_idx = lines.iter().position(|l| *l == "Package history:").unwrap();
        let continuation = lines[history_idx + 2];
        assert!(
            continuation.starts_with(' ') && !continuation.starts_with("  "),
            "one-space continuation, got {continuation:?}"
        );
        // Notes continuations do not.
        let notes_idx = lines.iter().position(|l| *l == "Notes:").unwrap();
        assert!(!lines[notes_idx + 2].starts_with(' '));

        // The wrap is a fixed point: saving again changes nothing further.
        let once = parse_description(&text).unwrap();
        let twice = parse_description(&generate_description(&once, &[])).unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn an_unbreakable_word_is_hard_split_rather_than_overflowing() {
        let meta = RipMeta {
            game_name: "G".to_owned(),
            notes: format!("See {}", "x".repeat(60)),
            ..RipMeta::default()
        };
        let text = generate_description(&meta, &[]);
        for line in text.split("\r\n") {
            assert!(line.chars().count() <= 47, "line too long: {line:?}");
        }
    }

    #[test]
    fn presets_cover_the_three_chips_and_match_the_suggestions() {
        assert_eq!(PRESETS.len(), 3);
        for (opl, preset) in [
            (OplType::Opl2, &PRESETS[0]),
            (OplType::DualOpl2, &PRESETS[1]),
            (OplType::Opl3, &PRESETS[2]),
        ] {
            assert_eq!(preset_for(opl), preset);
            assert_eq!(music_hardware_suggestion(opl), preset.music_hardware);
            assert_eq!(preset.system, DEFAULT_SYSTEM);
            assert_eq!(preset.os, DEFAULT_OS);
        }
    }

    #[test]
    fn hand_aligned_multiline_author_is_normalised_and_then_stable() {
        // A real quirk (Illusion Blaze, Wordtris, ...): the Music author field is
        // a hand-formatted block with varying indentation and internal alignment
        // spaces. We normalise it to a greedy wrap -- words kept, alignment not --
        // and, crucially, the normalised form round-trips unchanged thereafter.
        let source = [
            "Music author:        D.A.C.",
            " in particular:      Seung-Hwan Ro,", // one-space indent, aligned sub-entry
            "                     Myung-Jin Ahn",  // twenty-one-space indent
            "",
            "Package history:",
            "1.00 2015-06-13 X: Initial release.",
        ]
        .join("\r\n");
        let meta = parse_description(&source).unwrap();
        // Parsing keeps the block's interior alignment spaces verbatim.
        assert_ne!(
            meta.music_authors,
            "D.A.C. in particular: Seung-Hwan Ro, Myung-Jin Ahn"
        );
        // The first save normalises it to a greedy, single-spaced wrap...
        let once = parse_description(&generate_description(&meta, &[])).unwrap();
        assert_eq!(
            once.music_authors,
            "D.A.C. in particular: Seung-Hwan Ro, Myung-Jin Ahn"
        );
        // ...and every save after that is a fixed point.
        let twice = parse_description(&generate_description(&once, &[])).unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn rejects_unrelated_text() {
        assert!(parse_description("just some\r\nrandom text\r\n").is_err());
    }

    #[test]
    fn track_entry_from_song_prefers_gd3_then_falls_back_to_filename() {
        // The VGM fixture carries no GD3 tag, so the title falls back to the stem.
        let song = io::read_song("01 Fallback Title.vgm", VGM_FIXTURE).unwrap();
        let entry = TrackEntry::from_song(&song, "01 Fallback Title.vgm");
        assert_eq!(entry.title, "Fallback Title");
        assert_eq!(entry.total_samples, u64::from(song.total_delay_samples()));
        assert_eq!(entry.loop_samples, song.loop_num_samples().map(u64::from));

        // With a GD3 English track name, that wins.
        let mut tagged = io::read_song("01 Fallback Title.vgm", VGM_FIXTURE).unwrap();
        if let Some(meta) = tagged.vgm_meta_mut() {
            meta.tag = Some(Gd3Tag {
                track_name_en: "Real Title".to_owned(),
                ..Gd3Tag::default()
            });
        }
        assert_eq!(
            TrackEntry::from_song(&tagged, "01 Fallback Title.vgm").title,
            "Real Title"
        );
    }
}
