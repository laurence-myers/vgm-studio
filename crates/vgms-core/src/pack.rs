//! Preparing a VGMRips submission ("pack") from a folder of VGM/VGZ files.
//!
//! A submission is a flat zip of `NN Track Title.vgz` songs plus three text
//! artefacts: a `Game Name.txt` *description*, a `Game Name.m3u` *playlist*, and
//! (elsewhere) a `Game Name.png` screenshot. This module owns the two things that
//! are pure data transforms -- generating and parsing the description, and
//! generating the playlist -- so they stay wasm-clean and testable without a
//! filesystem. Folder scanning, zip building and PNG optimisation are native-only
//! and live in `vgms-app`.
//!
//! The description layout is not ours to invent: it matches the official template
//! and `vgm_stat`'s output byte-for-byte (validated against five real packs). The
//! load-bearing quirks -- CRLF endings, the 47-column width, header values wrapped
//! at 26 columns, track titles wrapped at 35, the time block right-aligned to end
//! at column 47, and a non-looping track's `" -   "` dash landing at column 44 --
//! are all reproduced here and pinned by the tests.

use std::collections::BTreeSet;

use crate::Gd3Tag;
use crate::error::{Error, Result};
use crate::song::{OplType, Song};
use crate::vgm::VgmFile;

/// The system name a fresh PC pack defaults to.
pub const DEFAULT_SYSTEM: &str = "IBM PC/AT";
/// The OS a fresh PC pack defaults to.
pub const DEFAULT_OS: &str = "DOS";
/// The song-list heading a fresh pack uses; real packs vary the wording.
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
/// `Label: value` lines in [`PackMeta::extra_fields`] so re-saving does not lose
/// them. Empty known fields are omitted when generating, which is how legacy
/// packs without an `OS:` line round-trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMeta {
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

impl Default for PackMeta {
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

    /// Derives an entry from a VGM whose commands this app cannot decode.
    ///
    /// The timings come from the header rather than from the stream, which is
    /// what `vgm_stat` reads too. They are only as honest as the file is -- but
    /// nothing here can edit such a file's music, so they cannot go stale.
    #[must_use]
    pub fn from_vgm_file(file: &VgmFile) -> Self {
        let title = file
            .tag
            .as_ref()
            .map(|tag| tag.track_name_en.trim())
            .filter(|name| !name.is_empty())
            .map_or_else(|| title_from_filename(&file.name).to_owned(), str::to_owned);
        Self {
            title,
            total_samples: u64::from(file.total_samples()),
            loop_samples: file.loop_samples().map(u64::from),
            plays: vgm_play_count(file.header.loop_base(), file.header.loop_modifier()),
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

/// Formats a byte count with thousands separators, so a six-figure screenshot
/// can be read at a glance rather than counted.
#[must_use]
pub fn format_byte_count(bytes: usize) -> String {
    let digits = bytes.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// What a PNG's header says about the image.
///
/// Read straight from the IHDR chunk rather than by decoding the file: a PNG
/// always carries IHDR first, at a fixed offset, so the four facts a pack cares
/// about cost no decoder and no dependency (and stay wasm-clean).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    /// Bits per sample (1, 2, 4, 8 or 16), per the PNG spec.
    pub bit_depth: u8,
    /// PNG colour type: 0 greyscale, 2 truecolour, 3 palette, 4 greyscale+alpha,
    /// 6 truecolour+alpha.
    pub colour_type: u8,
}

/// The PC display modes a DOS-era screenshot is likely to have been taken in.
/// Naming the mode is how you spot a screenshot that has been rescaled: a
/// 640x400 shot of a mode 13h game is an upscale, not a capture.
const DISPLAY_MODES: &[(u32, u32, &str)] = &[
    (320, 200, "VGA mode 13h"),
    (320, 240, "VGA mode X"),
    (360, 240, "VGA mode X"),
    (640, 200, "CGA/EGA"),
    (640, 350, "EGA"),
    (640, 400, "VGA"),
    (640, 480, "VGA"),
    (720, 400, "VGA text"),
    (800, 600, "SVGA"),
];

impl PngInfo {
    /// Reads the header of `bytes`, or `None` if it is not a PNG.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        // 8 signature + 8 chunk header + 13 IHDR bytes; the fields read below sit
        // at 16..26.
        if bytes.len() < 26 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
            return None;
        }
        let word = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let (width, height) = (word(16), word(20));
        // A zero dimension is illegal, and would divide by zero in `aspect`.
        (width > 0 && height > 0).then_some(Self {
            width,
            height,
            bit_depth: bytes[24],
            colour_type: bytes[25],
        })
    }

    /// The colour format in words, e.g. `"8-bit palette"` or `"24-bit RGB"`.
    /// Bit depth is per *sample*, so a truecolour depth is multiplied out to the
    /// per-pixel figure people actually quote.
    #[must_use]
    pub fn colour(&self) -> String {
        let depth = u32::from(self.bit_depth);
        match self.colour_type {
            0 => format!("{depth}-bit greyscale"),
            2 => format!("{}-bit RGB", depth * 3),
            3 => format!("{depth}-bit palette"),
            4 => format!("{}-bit greyscale + alpha", depth * 2),
            6 => format!("{}-bit RGBA", depth * 4),
            other => format!("colour type {other}"),
        }
    }

    /// The width:height ratio in lowest terms.
    #[must_use]
    pub fn aspect(&self) -> (u32, u32) {
        let divisor = gcd(self.width, self.height);
        (self.width / divisor, self.height / divisor)
    }

    /// The PC display mode these dimensions are, when they are a familiar one.
    #[must_use]
    pub fn display_mode(&self) -> Option<&'static str> {
        DISPLAY_MODES
            .iter()
            .find(|(width, height, _)| *width == self.width && *height == self.height)
            .map(|(_, _, name)| *name)
    }
}

/// Greatest common divisor, for reducing an aspect ratio.
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A one-click fill for the System / OS / Music hardware fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackPreset {
    /// The button label, e.g. `"OPL-2"`.
    pub name: &'static str,
    pub system: &'static str,
    pub os: &'static str,
    pub music_hardware: &'static str,
}

/// The chip presets, in OPL order. All PC packs share the system and OS; the
/// hardware line names the chip.
pub const PRESETS: [PackPreset; 3] = [
    PackPreset {
        name: "OPL-2",
        system: DEFAULT_SYSTEM,
        os: DEFAULT_OS,
        music_hardware: "AdLib/Sound Blaster (YM3812)",
    },
    PackPreset {
        name: "Dual OPL-2",
        system: DEFAULT_SYSTEM,
        os: DEFAULT_OS,
        music_hardware: "Dual OPL2 (2x YM3812)",
    },
    PackPreset {
        name: "OPL-3",
        system: DEFAULT_SYSTEM,
        os: DEFAULT_OS,
        music_hardware: "Sound Blaster Pro 2 (YMF262)",
    },
];

/// One-click fills for the common non-OPL systems, in rough console-era order.
/// The OS line is blank for the cartridge consoles (they have none), which
/// [`generate_description`] omits rather than printing empty. Every field stays
/// editable after a click; these are a starting point, not a lock.
pub const CONSOLE_PRESETS: [PackPreset; 8] = [
    PackPreset {
        name: "Mega Drive",
        system: "Sega Mega Drive / Genesis",
        os: "",
        music_hardware: "YM2612, SN76489",
    },
    PackPreset {
        name: "Master System",
        system: "Sega Master System",
        os: "",
        music_hardware: "SN76489",
    },
    PackPreset {
        name: "NES",
        system: "Nintendo Famicom / NES",
        os: "",
        music_hardware: "RP2A03",
    },
    PackPreset {
        name: "Game Boy",
        system: "Nintendo Game Boy",
        os: "",
        music_hardware: "LR35902",
    },
    PackPreset {
        name: "PC Engine",
        system: "NEC PC Engine / TurboGrafx-16",
        os: "",
        music_hardware: "HuC6280",
    },
    PackPreset {
        name: "Neo Geo",
        system: "SNK Neo Geo",
        os: "",
        music_hardware: "YM2610",
    },
    PackPreset {
        name: "X68000",
        system: "Sharp X68000",
        os: "Human68k",
        music_hardware: "YM2151, MSM6258",
    },
    PackPreset {
        name: "PC-98",
        system: "NEC PC-9801",
        os: "DOS",
        music_hardware: "YM2608",
    },
];

/// The preset matching a chip type.
#[must_use]
pub const fn preset_for(opl: OplType) -> &'static PackPreset {
    match opl {
        OplType::Opl2 => &PRESETS[0],
        OplType::DualOpl2 => &PRESETS[1],
        OplType::Opl3 => &PRESETS[2],
    }
}

/// A suggested (editable) `Music hardware:` value for the chip a pack targets.
#[must_use]
pub fn music_hardware_suggestion(opl: OplType) -> &'static str {
    preset_for(opl).music_hardware
}

/// A GD3 track title rewritten the way `vgm_ren` (the VGMRips renamer) writes it
/// into a file name. That tool is the reference for what a pack's files are
/// called, so both the file-name check and the rename-from-tag fix follow its
/// table exactly rather than inventing one:
///
/// ```text
/// "  ->  '        ?  ->  [removed]     |  ->  -
/// :  ->  " - "    !  ->  [removed]     <  ->  (
/// /  ->  ", "     \  ->  ", "          >  ->  )
/// ```
///
/// `:`, `/` and `\` also swallow the spaces that follow them, and `/` and `\`
/// drop the spaces already written before them -- so `"Hard / Soft"` becomes
/// `"Hard, Soft"`, not `"Hard , Soft"`. Trailing dots are then dropped, and
/// trailing spaces after them (that order is `vgm_ren`'s, and it is why a title
/// ending `". ."` keeps its last dot).
///
/// Note that `:` is replaced by `" - "` *unconditionally*: `vgm_ren` trims the
/// spaces before a comma but not before a dash, so `"Foo : Bar"` really does
/// become `"Foo  - Bar"`. Reproduced rather than corrected, so a folder already
/// named by `vgm_ren` never reads as drifted.
///
/// Two deliberate departures from the C: leading whitespace is trimmed (rather
/// than leaving a file called `"01  Title.vgz"`), and `*` -- which `vgm_ren`
/// passes through even though no Windows file name may hold it -- becomes `_`,
/// as do control characters.
#[must_use]
pub fn vgm_ren_title(title: &str) -> String {
    /// Drops the spaces `vgm_ren` eats after a `:`, `/` or `\`.
    fn skip_spaces(chars: &mut core::iter::Peekable<core::str::Chars<'_>>) {
        while chars.next_if_eq(&' ').is_some() {}
    }

    let mut out = String::with_capacity(title.len());
    let mut chars = title.trim_start().chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push('\''),
            ':' => {
                out.push_str(" - ");
                skip_spaces(&mut chars);
            }
            '?' | '!' => {}
            '/' | '\\' => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(", ");
                skip_spaces(&mut chars);
            }
            '|' => out.push('-'),
            '<' => out.push('('),
            '>' => out.push(')'),
            '*' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    while out.ends_with('.') {
        out.pop();
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Builds a VGMRips track file name from its 1-based `number`, `title`, and
/// `ext` (the extension without the dot, e.g. `"vgz"`): `"NN Title.ext"`, the
/// title rewritten by [`vgm_ren_title`]. Shared by the pack quick-edit dialog
/// (which derives the name from the GD3 tag) and reordering.
#[must_use]
pub fn track_file_name(number: usize, title: &str, ext: &str) -> String {
    format!("{number:02} {}.{ext}", vgm_ren_title(title))
}

/// The file name a track *should* carry: its 1-based pack position, its GD3
/// track name through [`vgm_ren_title`], and the extension it already has (so a
/// rename never turns a `.vgz` into a `.vgm`).
///
/// `None` when the title yields nothing a file can be named after -- an empty
/// tag, or one made only of characters `vgm_ren` removes (`"?!"`) -- since there
/// is then no name to check against or rename to.
#[must_use]
pub fn tag_file_name(number: usize, track_name: &str, current_file_name: &str) -> Option<String> {
    if vgm_ren_title(track_name).is_empty() {
        return None;
    }
    let ext = current_file_name
        .rsplit_once('.')
        .map_or("vgz", |(_, ext)| ext);
    Some(track_file_name(number, track_name, ext))
}

/// A file-name-safe stem for the `.txt`/`.m3u`/`.zip`, from the game name.
///
/// The same [`vgm_ren_title`] replacements the tracks get, so a subtitled game
/// reads as `Doom II - Hell on Earth.zip` beside its `NN Doom II - ...vgz`
/// tracks rather than picking up an underscore the songs never had. Empty when
/// the game name leaves nothing behind, which is what
/// [`crate::pack`]'s callers gate a save on.
#[must_use]
pub fn doc_file_stem(game_name: &str) -> String {
    vgm_ren_title(game_name)
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

/// One header field: the label [`generate_description`] prints (with its colon),
/// the lowercased labels [`parse_description`] accepts (canonical first), and typed
/// access to its [`PackMeta`] slot. One ordered table drives both directions, so the
/// generated labels and the parsed aliases can never drift apart.
struct HeaderField {
    label: &'static str,
    aliases: &'static [&'static str],
    get: fn(&PackMeta) -> &str,
    set: fn(&mut PackMeta) -> &mut String,
}

/// The header fields in file order, split into the three blank-separated groups the
/// template uses (identity, credits, packaging). Any unrecognised `Label: value`
/// lines trail the last group as [`PackMeta::extra_fields`].
const HEADER_GROUPS: [&[HeaderField]; 3] = [
    &[
        HeaderField {
            label: "Game name:",
            aliases: &["game name"],
            get: |m| m.game_name.as_str(),
            set: |m| &mut m.game_name,
        },
        HeaderField {
            label: "System:",
            aliases: &["system"],
            get: |m| m.system.as_str(),
            set: |m| &mut m.system,
        },
        HeaderField {
            label: "OS:",
            aliases: &["os"],
            get: |m| m.os.as_str(),
            set: |m| &mut m.os,
        },
        HeaderField {
            label: "Music hardware:",
            aliases: &["music hardware"],
            get: |m| m.music_hardware.as_str(),
            set: |m| &mut m.music_hardware,
        },
    ],
    &[
        HeaderField {
            label: "Music author:",
            aliases: &["music author", "music authors"],
            get: |m| m.music_authors.as_str(),
            set: |m| &mut m.music_authors,
        },
        HeaderField {
            label: "Game developer:",
            aliases: &["game developer"],
            get: |m| m.developer.as_str(),
            set: |m| &mut m.developer,
        },
        HeaderField {
            label: "Game publisher:",
            aliases: &["game publisher"],
            get: |m| m.publisher.as_str(),
            set: |m| &mut m.publisher,
        },
        HeaderField {
            label: "Game release date:",
            aliases: &["game release date"],
            get: |m| m.release_date.as_str(),
            set: |m| &mut m.release_date,
        },
    ],
    &[
        HeaderField {
            label: "Package created by:",
            aliases: &["package created by"],
            get: |m| m.creator.as_str(),
            set: |m| &mut m.creator,
        },
        HeaderField {
            label: "Package version:",
            aliases: &["package version"],
            get: |m| m.version.as_str(),
            set: |m| &mut m.version,
        },
    ],
];

/// Renders the full description file for `meta` and `tracks`.
///
/// Header values are greedily word-wrapped at the value column. A value that a
/// packager hand-formatted as a multi-line block with its own alignment (some
/// elaborate multi-author credits do this, with varying indentation and internal
/// alignment spaces) is normalised to that greedy wrap on save: every word is
/// preserved, but manual whitespace alignment is not. The result is stable --
/// saving it again is a no-op.
#[must_use]
pub fn generate_description(meta: &PackMeta, tracks: &[TrackEntry]) -> String {
    // Banner.
    let mut lines: Vec<String> = vec![
        "*".repeat(LINE_WIDTH),
        banner_line("* VGM music package"),
        banner_line("* http://vgmrips.net/"),
        "*".repeat(LINE_WIDTH),
    ];

    // Header field groups, blank-separated as in the template; the unrecognised
    // extra fields trail the last group, before its blank.
    for (index, group) in HEADER_GROUPS.iter().enumerate() {
        for field in *group {
            push_field(&mut lines, field.label, (field.get)(meta));
        }
        if index == HEADER_GROUPS.len() - 1 {
            for (label, value) in &meta.extra_fields {
                push_field(&mut lines, &format!("{label}:"), value);
            }
        }
        lines.push(String::new());
    }

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

/// Parses a description back into [`PackMeta`], tolerantly.
///
/// Accepts CRLF or LF, any banner URL, missing fields, the compact one-line song
/// header some old packs use, and a single blank line before the section markers.
/// The song-list block is skipped -- track timings are recomputed from the files.
/// Returns [`Error::File`] only when the text carries no recognisable field or
/// section at all.
pub fn parse_description(text: &str) -> Result<PackMeta> {
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

    let mut meta = PackMeta::default();
    let mut matched_known = false;

    for (label, value) in parse_header_fields(&lines[..header_end]) {
        let label_lc = label.to_ascii_lowercase();
        if let Some(field) = HEADER_GROUPS
            .iter()
            .flat_map(|group| group.iter())
            .find(|field| field.aliases.contains(&label_lc.as_str()))
        {
            *(field.set)(&mut meta) = value;
            matched_known = true;
        } else {
            meta.extra_fields.push((label, value));
        }
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
        for (index, chunk) in wrap_line(line, LINE_WIDTH, continuation_width)
            .into_iter()
            .enumerate()
        {
            if index == 0 {
                lines.push(chunk);
            } else {
                lines.push(format!("{continuation_prefix}{chunk}"));
            }
        }
    }
}

/// Greedily word-wraps a single logical line (no embedded newlines): the first
/// output chunk fits `first_width`, every continuation chunk `continuation_width`.
/// Breaks at spaces, hard-splitting any word wider than its line's width; never
/// hyphenates (unlike titles). Returns one string per output line -- no chunks at
/// all when the input holds no words, which each caller handles as it needs.
fn wrap_line(text: &str, first_width: usize, continuation_width: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<char> = Vec::new();
    let width_for = |emitted: usize| {
        if emitted == 0 {
            first_width
        } else {
            continuation_width
        }
    };
    for word in text.split_whitespace() {
        let mut chars: Vec<char> = word.chars().collect();
        loop {
            let width = width_for(chunks.len());
            if current.is_empty() {
                if chars.len() <= width {
                    current = chars;
                    break;
                }
                // A word wider than a whole line (a URL): hard-split it.
                let rest = chars.split_off(width);
                chunks.push(chars.iter().collect());
                chars = rest;
            } else if current.len() + 1 + chars.len() <= width {
                current.push(' ');
                current.extend(chars);
                break;
            } else {
                chunks.push(current.drain(..).collect());
            }
        }
    }
    if !current.is_empty() {
        chunks.push(current.iter().collect());
    }
    chunks
}

/// Greedy word-wrap for header values at a uniform width, always yielding at least
/// one (possibly empty) chunk so an all-whitespace value still emits its label row.
fn wrap_value(value: &str, width: usize) -> Vec<String> {
    let mut chunks = wrap_line(value, width, width);
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Pushes a `head`…`block` row with the time `block` right-aligned to end at column
/// [`LINE_WIDTH`]; the padding between is trimmed off the stored line.
fn push_aligned_row(lines: &mut Vec<String>, head: &str, block: &str) {
    let pad = LINE_WIDTH.saturating_sub(head.chars().count() + block.chars().count());
    lines.push(
        format!("{head}{}{block}", " ".repeat(pad))
            .trim_end()
            .to_owned(),
    );
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
            push_aligned_row(lines, &head, &block);
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
    push_aligned_row(lines, "Total Length", &block);
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

// -- submission readiness ----------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Gd3Tag;
    use crate::io;

    const FIXTURE: &str = include_str!("../../../tests/description_vgm151_PC.txt");
    const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    /// A PNG header (signature + IHDR) with the given shape. Only the first 26
    /// bytes matter to [`PngInfo::parse`], so the rest of the file is elided.
    fn png_header(width: u32, height: u32, bit_depth: u8, colour_type: u8) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.push(bit_depth);
        bytes.push(colour_type);
        bytes
    }

    #[test]
    fn png_headers_yield_the_facts_a_screenshot_is_judged_on() {
        let info = PngInfo::parse(&png_header(320, 200, 8, 3)).expect("a PNG header");
        assert_eq!((info.width, info.height), (320, 200));
        assert_eq!(info.colour(), "8-bit palette");
        assert_eq!(info.aspect(), (8, 5));
        assert_eq!(info.display_mode(), Some("VGA mode 13h"));

        // Truecolour depth is per sample, so it multiplies out per pixel.
        let truecolour = PngInfo::parse(&png_header(640, 480, 8, 2)).expect("a PNG header");
        assert_eq!(truecolour.colour(), "24-bit RGB");
        assert_eq!(truecolour.aspect(), (4, 3));
        assert_eq!(truecolour.display_mode(), Some("VGA"));

        // An unfamiliar size still reports its facts, just without a mode name.
        let odd = PngInfo::parse(&png_header(1024, 640, 8, 6)).expect("a PNG header");
        assert_eq!(odd.colour(), "32-bit RGBA");
        assert_eq!(odd.aspect(), (8, 5));
        assert_eq!(odd.display_mode(), None);
    }

    #[test]
    fn a_non_png_or_degenerate_header_is_rejected() {
        assert!(PngInfo::parse(b"not a png at all, not even close").is_none());
        assert!(PngInfo::parse(&[]).is_none());
        // Truncated before the IHDR fields.
        assert!(PngInfo::parse(&png_header(320, 200, 8, 3)[..20]).is_none());
        // A zero dimension is illegal, and would divide by zero reducing aspect.
        assert!(PngInfo::parse(&png_header(320, 0, 8, 3)).is_none());
    }

    #[test]
    fn byte_counts_are_grouped_in_threes() {
        assert_eq!(format_byte_count(0), "0");
        assert_eq!(format_byte_count(999), "999");
        assert_eq!(format_byte_count(1_000), "1,000");
        assert_eq!(format_byte_count(24_806), "24,806");
        assert_eq!(format_byte_count(1_234_567), "1,234,567");
    }

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
        let meta = PackMeta {
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
            ..PackMeta::default()
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
    fn track_file_name_formats_number_title_and_extension() {
        assert_eq!(track_file_name(1, "Intro", "vgz"), "01 Intro.vgz");
        assert_eq!(
            track_file_name(12, "Boss Battle", "vgm"),
            "12 Boss Battle.vgm"
        );
        // Forbidden characters follow vgm_ren's table; the title is trimmed.
        assert_eq!(track_file_name(3, "  A/B:C  ", "vgz"), "03 A, B - C.vgz");
    }

    #[test]
    fn vgm_ren_title_follows_the_renamer_replacement_table() {
        assert_eq!(vgm_ren_title("He said \"hi\""), "He said 'hi'");
        assert_eq!(
            vgm_ren_title("Chapter 1: The Start"),
            "Chapter 1 - The Start"
        );
        assert_eq!(vgm_ren_title("Really?! Yes!"), "Really Yes");
        assert_eq!(vgm_ren_title("Hard / Soft"), "Hard, Soft");
        assert_eq!(vgm_ren_title("Hard\\Soft"), "Hard, Soft");
        assert_eq!(vgm_ren_title("A|B"), "A-B");
        assert_eq!(vgm_ren_title("<Unused>"), "(Unused)");
        // `*` is not in vgm_ren's table but no file name may hold it.
        assert_eq!(vgm_ren_title("Star*Field"), "Star_Field");
    }

    #[test]
    fn vgm_ren_title_handles_the_whitespace_and_trailing_rules() {
        // Spaces after a colon/slash are eaten, and those before a comma dropped.
        assert_eq!(vgm_ren_title("Doom:   E1M1"), "Doom - E1M1");
        assert_eq!(vgm_ren_title("A   /   B"), "A, B");
        // ...but not the space *before* a colon: vgm_ren really does double it up.
        assert_eq!(vgm_ren_title("Foo : Bar"), "Foo  - Bar");
        // Trailing dots go first, then trailing spaces -- so ". ." keeps a dot.
        assert_eq!(vgm_ren_title("The End..."), "The End");
        assert_eq!(vgm_ren_title("The End. ."), "The End.");
        assert_eq!(vgm_ren_title("  Padded  "), "Padded");
        // A title made only of removed characters leaves nothing to name a file.
        assert!(vgm_ren_title("?!").is_empty());
    }

    #[test]
    fn tag_file_name_numbers_the_title_and_keeps_the_extension() {
        assert_eq!(
            tag_file_name(7, "Boss: Round 2", "07 Old Name.vgm").as_deref(),
            Some("07 Boss - Round 2.vgm")
        );
        assert_eq!(
            tag_file_name(1, "Intro", "whatever").as_deref(),
            Some("01 Intro.vgz"), // no extension to keep: the pack default
        );
        assert_eq!(tag_file_name(1, "   ", "01 X.vgz"), None);
        assert_eq!(tag_file_name(1, "!?", "01 X.vgz"), None);
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
        let meta = PackMeta {
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
            ..PackMeta::default()
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
        let meta = PackMeta {
            game_name: "Legacy".to_owned(),
            system: "PC / DOS".to_owned(),
            music_authors: "Someone".to_owned(),
            version: "1.00".to_owned(),
            ..PackMeta::default()
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
        let out = generate_description(&PackMeta::default(), &tracks);
        assert!(
            out.contains("\r\n001 Track with a fairly long title"),
            "three-digit prefix"
        );
        // Continuation lines indent by num_width + 1 == 4 spaces.
        assert!(out.contains("\r\n    "), "four-space continuation indent");
    }

    #[test]
    fn long_notes_and_history_lines_wrap_at_the_file_width() {
        let meta = PackMeta {
            game_name: "G".to_owned(),
            notes: "This pack was made using DOSBox and a whole lot of patience, because \
                    the game only plays each song once per boot and refuses to loop."
                .to_owned(),
            history: "1.00 2026-07-16 Someone: Initial release, with a remark long enough \
                      that it has to wrap onto a continuation line."
                .to_owned(),
            ..PackMeta::default()
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
        let meta = PackMeta {
            game_name: "G".to_owned(),
            notes: format!("See {}", "x".repeat(60)),
            ..PackMeta::default()
        };
        let text = generate_description(&meta, &[]);
        for line in text.split("\r\n") {
            assert!(line.chars().count() <= 47, "line too long: {line:?}");
        }
    }

    #[test]
    fn wrap_line_honours_first_and_continuation_widths() {
        // Uniform width, as header values use it.
        assert_eq!(wrap_line("a b c", 3, 3), vec!["a b", "c"]);
        // A narrower continuation width breaks the later lines sooner.
        assert_eq!(wrap_line("aa bb cc dd", 5, 2), vec!["aa bb", "cc", "dd"]);
        // A word wider than a line is hard-split at each line's own width.
        assert_eq!(wrap_line("xxxxxxx", 3, 2), vec!["xxx", "xx", "xx"]);
        // No words -> no chunks; callers supply their own fallback.
        assert!(wrap_line("   ", 5, 5).is_empty());
    }

    #[test]
    fn console_presets_are_named_and_fill_the_hardware_fields() {
        // Each has a button label and a music-hardware line; the cartridge
        // consoles carry no OS, and generate_description omits an empty one.
        assert_eq!(CONSOLE_PRESETS.len(), 8);
        let names: Vec<&str> = CONSOLE_PRESETS.iter().map(|preset| preset.name).collect();
        assert_eq!(
            names,
            [
                "Mega Drive",
                "Master System",
                "NES",
                "Game Boy",
                "PC Engine",
                "Neo Geo",
                "X68000",
                "PC-98",
            ]
        );
        for preset in &CONSOLE_PRESETS {
            assert!(!preset.system.is_empty(), "{} has a system", preset.name);
            assert!(
                !preset.music_hardware.is_empty(),
                "{} names its hardware",
                preset.name
            );
        }
        // The cartridge consoles have no OS; the computers do.
        assert_eq!(CONSOLE_PRESETS[0].os, "", "Mega Drive has no OS");
        assert_eq!(CONSOLE_PRESETS[6].os, "Human68k", "X68000 runs Human68k");
        assert_eq!(CONSOLE_PRESETS[7].os, "DOS", "PC-98 runs DOS");
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

    // -- submission readiness --------------------------------------------------

    fn facts(file_name: &str, tag: Option<Gd3Tag>, loops: bool, readable: bool) -> TrackFacts {
        TrackFacts {
            file_name: file_name.to_owned(),
            tag,
            loops,
            readable,
        }
    }

    /// A track that passes every content check against [`full_meta`].
    fn full_tag() -> Gd3Tag {
        Gd3Tag {
            track_name_en: "Intro".to_owned(),
            game_name_en: "Cool Game".to_owned(),
            system_name_en: "IBM PC/AT".to_owned(),
            track_author_en: "Ada".to_owned(),
            release_date: "1994-03-01".to_owned(),
            creator: "Ripper".to_owned(),
            ..Gd3Tag::default()
        }
    }

    fn full_meta() -> PackMeta {
        PackMeta {
            game_name: "Cool Game".to_owned(),
            system: "IBM PC/AT".to_owned(),
            music_authors: "Ada".to_owned(),
            release_date: "1994-03-01".to_owned(),
            creator: "Ripper".to_owned(),
            history: "1.00 1994-03-01 Ripper: Initial release.".to_owned(),
            ..PackMeta::default()
        }
    }

    fn messages(items: &[ReadinessItem], severity: Severity) -> Vec<&str> {
        items
            .iter()
            .filter(|item| item.severity == severity)
            .map(|item| item.message.as_str())
            .collect()
    }

    fn has(items: &[ReadinessItem], severity: Severity, needle: &str) -> bool {
        messages(items, severity)
            .iter()
            .any(|message| message.contains(needle))
    }

    #[test]
    fn is_pack_date_accepts_hyphenated_dates_and_rejects_the_rest() {
        for good in ["1994", "1994-03", "1994-03-01", "0000", "2026-12-31"] {
            assert!(is_pack_date(good), "{good:?} should be a valid pack date");
        }
        for bad in [
            "",
            "199",
            "94",
            "1994-3",
            "1994-03-1",
            "1994-",
            "1994/03/01",
            "1994.03.01",
            "March 1994",
            "1994-03-01-01",
            "199a",
        ] {
            assert!(!is_pack_date(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn hyphenate_date_only_fixes_convertible_slash_dates() {
        assert_eq!(hyphenate_date("1994/03/01").as_deref(), Some("1994-03-01"));
        assert_eq!(hyphenate_date("1994/03").as_deref(), Some("1994-03"));
        // Already hyphenated (or year-only): nothing to convert.
        assert_eq!(hyphenate_date("1994-03-01"), None);
        assert_eq!(hyphenate_date("1994"), None);
        // Slashes that would not yield a valid pack date are left for a manual fix.
        assert_eq!(hyphenate_date("1994/3/1"), None);
        assert_eq!(hyphenate_date("March/1994"), None);
        assert_eq!(hyphenate_date(""), None);
    }

    #[test]
    fn a_complete_pack_reports_nothing() {
        let tracks = [facts("01 Intro.vgz", Some(full_tag()), true, true)];
        assert!(
            readiness(&full_meta(), &tracks).is_empty(),
            "a clean pack yields no items"
        );
    }

    #[test]
    fn empty_pack_meta_warns_on_every_required_header_field() {
        // Default meta: no creator, date, author or history. One readable, fully
        // tagged track keeps the per-track checks quiet so only P1-P4 show.
        let meta = PackMeta::default();
        let tracks = [facts("01 Intro.vgz", Some(full_tag()), true, true)];
        let items = readiness(&meta, &tracks);
        assert!(has(&items, Severity::Warning, "Package created by"));
        assert!(has(&items, Severity::Warning, "release date is empty"));
        assert!(has(&items, Severity::Warning, "Music author is empty"));
        assert!(has(&items, Severity::Warning, "Package history"));
    }

    #[test]
    fn a_slash_separated_pack_date_warns_but_a_hyphenated_one_does_not() {
        let mut meta = full_meta();
        meta.release_date = "1994/03/01".to_owned();
        let tracks = [facts("01 Intro.vgz", Some(full_tag()), true, true)];
        let items = readiness(&meta, &tracks);
        assert!(has(&items, Severity::Warning, "hyphen-separated"));
        assert_eq!(
            items[0].target,
            ReadinessTarget::Meta(MetaField::ReleaseDate)
        );

        meta.release_date = "1994-03-01".to_owned();
        assert!(
            readiness(
                &meta,
                &[facts("01 Intro.vgz", Some(full_tag()), true, true)]
            )
            .is_empty()
        );
    }

    #[test]
    fn an_untagged_readable_track_is_flagged() {
        let tracks = [facts("01 Intro.vgz", None, true, true)];
        let items = readiness(&full_meta(), &tracks);
        assert!(has(&items, Severity::Warning, "has no GD3 tag"));
        // And the target is the track itself, for click-to-fix.
        let tagless: Vec<_> = items
            .iter()
            .filter(|item| item.message.contains("no GD3 tag"))
            .collect();
        assert_eq!(tagless[0].target, ReadinessTarget::Track(0));
    }

    #[test]
    fn missing_required_gd3_fields_are_listed_in_one_line() {
        let tag = Gd3Tag {
            track_name_en: "Intro".to_owned(),
            game_name_en: "Cool Game".to_owned(),
            system_name_en: "IBM PC/AT".to_owned(),
            // no composer, no ripper
            release_date: "1994-03-01".to_owned(),
            ..Gd3Tag::default()
        };
        let items = readiness(
            &full_meta(),
            &[facts("01 Intro.vgz", Some(tag), true, true)],
        );
        let missing: Vec<&str> = messages(&items, Severity::Warning)
            .into_iter()
            .filter(|message| message.contains("missing"))
            .collect();
        assert_eq!(missing.len(), 1, "one combined missing-fields line");
        assert!(missing[0].contains("Composer"));
        assert!(missing[0].contains("Ripper"));
        assert!(!missing[0].contains("Game Name"), "present fields omitted");
    }

    #[test]
    fn a_track_release_date_with_slashes_warns() {
        let tag = Gd3Tag {
            release_date: "1994/03".to_owned(),
            ..full_tag()
        };
        let items = readiness(
            &full_meta(),
            &[facts("01 Intro.vgz", Some(tag), true, true)],
        );
        assert!(has(&items, Severity::Warning, "should be hyphen-separated"));
    }

    #[test]
    fn a_file_name_that_drifts_from_the_track_name_warns() {
        let items = readiness(
            &full_meta(),
            &[facts("01 Wrong Name.vgz", Some(full_tag()), true, true)],
        );
        assert!(has(
            &items,
            Severity::Warning,
            "doesn't match the Track Name"
        ));
    }

    #[test]
    fn a_vgm_ren_renamed_file_is_not_a_false_file_name_mismatch() {
        // The names vgm_ren would have written are the ones a pack carries, so
        // none of its replacements may read as drift.
        for (track_name, file_name) in [
            ("A/B", "01 A, B.vgz"),
            ("Doom II: Hell on Earth", "01 Doom II - Hell on Earth.vgz"),
            ("Who?!", "01 Who.vgz"),
            ("\"Quoted\"", "01 'Quoted'.vgz"),
            ("<Unused>|Alt", "01 (Unused)-Alt.vgz"),
        ] {
            let tag = Gd3Tag {
                track_name_en: track_name.to_owned(),
                ..full_tag()
            };
            let items = readiness(&full_meta(), &[facts(file_name, Some(tag), true, true)]);
            assert!(
                !has(&items, Severity::Warning, "doesn't match the Track Name"),
                "{track_name:?} is correctly named {file_name:?}"
            );
        }
    }

    #[test]
    fn a_file_name_mismatch_names_the_file_it_expected() {
        // The message has to carry the fix: the exact name the rename produces.
        let tag = Gd3Tag {
            track_name_en: "Doom II: Hell on Earth".to_owned(),
            ..full_tag()
        };
        let items = readiness(
            &full_meta(),
            &[facts("01 Doom 2.vgz", Some(tag), true, true)],
        );
        assert!(has(
            &items,
            Severity::Warning,
            "expected \"01 Doom II - Hell on Earth.vgz\""
        ));
    }

    #[test]
    fn an_unnameable_track_name_is_not_a_file_name_mismatch() {
        // "?!" leaves nothing for a file to be named after, so there is no name
        // to demand -- the missing-fields check is not this check's job either.
        let tag = Gd3Tag {
            track_name_en: "?!".to_owned(),
            ..full_tag()
        };
        let items = readiness(&full_meta(), &[facts("01 Huh.vgz", Some(tag), true, true)]);
        assert!(!has(
            &items,
            Severity::Warning,
            "doesn't match the Track Name"
        ));
    }

    #[test]
    fn gd3_fields_inconsistent_with_the_pack_are_flagged_per_track() {
        let tag = Gd3Tag {
            game_name_en: "Different Game".to_owned(),
            creator: "Other Ripper".to_owned(),
            ..full_tag()
        };
        let items = readiness(
            &full_meta(),
            &[facts("01 Intro.vgz", Some(tag), true, true)],
        );
        assert!(has(&items, Severity::Warning, "game name"));
        assert!(has(&items, Severity::Warning, "differs from the pack's"));
        assert!(has(&items, Severity::Warning, "ripper"));
    }

    #[test]
    fn a_matching_gd3_field_raises_no_consistency_warning() {
        // Same game name as the pack: no C1 warning (the only difference here is a
        // composer, which never triggers consistency -- that is a note, C5).
        let tag = Gd3Tag {
            track_author_en: "Bob".to_owned(),
            ..full_tag()
        };
        let items = readiness(
            &full_meta(),
            &[facts("01 Intro.vgz", Some(tag), true, true)],
        );
        assert!(!has(&items, Severity::Warning, "differs from the pack's"));
    }

    #[test]
    fn composer_set_mismatch_is_a_note_not_a_warning() {
        // The pack credits Ada & Bob; the tracks only ever credit Ada.
        let mut meta = full_meta();
        meta.music_authors = "Ada & Bob".to_owned();
        let items = readiness(
            &meta,
            &[facts("01 Intro.vgz", Some(full_tag()), true, true)],
        );
        assert!(has(&items, Severity::Note, "composers don't all match"));
        assert!(!has(&items, Severity::Warning, "composers"));
    }

    #[test]
    fn a_matching_composer_set_raises_no_note() {
        // Split on both comma and ampersand, order-insensitive.
        let mut meta = full_meta();
        meta.music_authors = "Bob & Ada".to_owned();
        let tracks = [
            facts(
                "01 Intro.vgz",
                Some(Gd3Tag {
                    track_author_en: "Ada".to_owned(),
                    ..full_tag()
                }),
                true,
                true,
            ),
            facts(
                "02 Boss.vgz",
                Some(Gd3Tag {
                    track_name_en: "Boss".to_owned(),
                    track_author_en: "Bob".to_owned(),
                    ..full_tag()
                }),
                true,
                true,
            ),
        ];
        assert!(!has(
            &readiness(&meta, &tracks),
            Severity::Note,
            "composers"
        ));
    }

    #[test]
    fn loopless_readable_tracks_get_a_single_note() {
        let tracks = [
            facts("01 Intro.vgz", Some(full_tag()), false, true),
            facts(
                "02 Loop.vgz",
                Some(Gd3Tag {
                    track_name_en: "Loop".to_owned(),
                    ..full_tag()
                }),
                true,
                true,
            ),
        ];
        let items = readiness(&full_meta(), &tracks);
        let loop_notes: Vec<&str> = messages(&items, Severity::Note)
            .into_iter()
            .filter(|message| message.contains("No loop point"))
            .collect();
        assert_eq!(loop_notes.len(), 1, "one aggregate loop note");
        assert!(loop_notes[0].contains("01 Intro"));
        assert!(
            !loop_notes[0].contains("02 Loop"),
            "the looping track is out"
        );
    }

    #[test]
    fn unreadable_tracks_are_skipped_and_track_targets_keep_their_index() {
        // A readable-but-broken track sits at index 2, behind an unreadable one at
        // index 1. Its Track target must be 2 (the pack position), so click-to-fix
        // opens the right track.
        let broken = Gd3Tag {
            game_name_en: "Wrong Game".to_owned(),
            ..full_tag()
        };
        let tracks = [
            facts("01 Intro.vgz", Some(full_tag()), true, true),
            facts("02 Broken.vgz", None, false, false),
            facts("03 Boss.vgz", Some(broken), true, true),
        ];
        let items = readiness(&full_meta(), &tracks);
        // Nothing points at the unreadable track...
        assert!(
            !items
                .iter()
                .any(|item| item.target == ReadinessTarget::Track(1)),
            "the unreadable track is not content-checked"
        );
        // ...and the game-name mismatch points at index 2.
        let mismatch = items
            .iter()
            .find(|item| item.message.contains("game name"))
            .expect("a consistency warning");
        assert_eq!(mismatch.target, ReadinessTarget::Track(2));
    }

    #[test]
    fn items_are_filed_under_the_right_checklist_category() {
        // A game name to compare against and a two-composer credit, but empty
        // creator/date/history so PackInfo still fires.
        let meta = PackMeta {
            game_name: "Cool Game".to_owned(),
            music_authors: "Ada & Bob".to_owned(),
            ..PackMeta::default()
        };
        let tag = Gd3Tag {
            game_name_en: "Different".to_owned(),
            ..full_tag()
        };
        let items = readiness(&meta, &[facts("01 Wrong.vgz", Some(tag), false, true)]);

        let category_of = |needle: &str| {
            items
                .iter()
                .find(|item| item.message.contains(needle))
                .map(|item| item.category)
        };
        assert_eq!(
            category_of("Game release date is empty"),
            Some(ReadinessCategory::PackInfo)
        );
        assert_eq!(
            category_of("differs from the pack's"),
            Some(ReadinessCategory::Consistency)
        );
        assert_eq!(
            category_of("doesn't match the Track Name"),
            Some(ReadinessCategory::TrackTags)
        );
        assert_eq!(category_of("No loop point"), Some(ReadinessCategory::Loops));
        // The five display categories are distinct and labelled.
        assert_eq!(ReadinessCategory::ALL.len(), 5);
        assert_eq!(ReadinessCategory::TrackTags.label(), "Track tags");
    }
}
