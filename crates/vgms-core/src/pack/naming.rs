//! What a pack's files are called. The VGMRips renamer `vgm_ren` is the
//! reference for a submission's file names, so [`vgm_ren_title`] reproduces its
//! replacement table byte-for-byte; the rest build on it -- [`track_file_name`]
//! numbers a title, [`tag_file_name`] computes the rename a track's GD3 tag
//! implies, [`doc_file_stem`] names the description/playlist, and
//! [`title_from_filename`] reads a title back out of a `NN Title.ext` name.

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
