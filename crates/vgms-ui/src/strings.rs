// SPDX-License-Identifier: GPL-2.0-or-later
//! Central home for the GUI's user-facing prose: tooltips, dialog and alert
//! text, status-line messages, empty states, and validation errors. Edit
//! wording here rather than at the call sites.
//!
//! Terse chrome -- button captions, menu items, field and column labels -- is
//! left inline where it reads naturally with the layout code.
//!
//! Static strings are `&str` constants; strings that interpolate a value are
//! small functions returning `String`. Names are prefixed by the module they
//! serve (`APP_`, `PACK_`, `SETTINGS_`, ...).
#![allow(clippy::doc_markdown)]

use vgms_core::util::ms_to_timestr;
use vgms_synth::LoopCount;

// ============================================================================
// app.rs
// ============================================================================

pub(crate) const APP_AUTO_TRIM_TITLE: &str = "DRO auto-trimmed";
pub(crate) const APP_AUTO_TRIM_TEXT: &str =
    "Removed a bogus delay at the start of the DRO. Remember to save.";
pub(crate) const APP_MISMATCH_TITLE: &str = "DRO timing mismatch";
pub(crate) const APP_MISMATCH_PREFIX_TRIMMED: &str = "Despite auto-trimming, t";
pub(crate) const APP_MISMATCH_PREFIX_PLAIN: &str = "T";
pub(crate) const APP_MISMATCH_ADVICE_V1: &str =
    "Please re-save the file to use the calculated value.";
pub(crate) const APP_MISMATCH_ADVICE_V2: &str = "Please enable \"Allow editing in DRO Info\" in the\n\
                                          Settings dialog, then edit the song length on\n\
                                          the DRO Info screen.";
pub(crate) const APP_NOTHING_MARKED: &str = "Mark a loop region first.";
pub(crate) const APP_TARGET_ANY_DELAY: &str = "a delay";
pub(crate) const APP_TARGET_ANY_WRITE: &str = "a register write";
pub(crate) const APP_TARGET_BANK_SWITCH: &str = "a bank switch";

pub(crate) const APP_TIP_REWIND: &str = "Rewind to the start";
pub(crate) const APP_TIP_DELETE: &str = "Delete the selected instruction(s)";
pub(crate) const APP_TIP_PLAY: &str = "Play the song from the current position";
pub(crate) const APP_TIP_STOP: &str = "Stop playback";
pub(crate) const APP_TIP_LOOP: &str = "Repeat the marked region. Shift+click / Shift+right-click \
                                the waveform to mark start and end; [ and ] use the selected row.";

pub(crate) const APP_BUSY_WORKING: &str = "Working...";
pub(crate) const APP_BUSY_RENDER_WAV: &str = "Rendering WAV...";
pub(crate) const APP_BUSY_RENDER_WAVEFORM: &str = "Rendering waveform...";

pub(crate) const APP_EMPTY_STATE: &str = "Open a DRO, VGM or VGZ file (File > Open Song..., or drop it \
                                   here).";

pub(crate) const APP_ERR_OPEN_FILE_TITLE: &str = "Failed to open file";
pub(crate) const APP_ERR_READ_IMAGE_TITLE: &str = "Failed to read image";
pub(crate) const APP_ERR_OPEN_FOLDER_TITLE: &str = "Failed to open folder";
pub(crate) const APP_ERR_RENAME_TITLE: &str = "Rename failed";
pub(crate) const APP_ERR_PACK_EXPORT_TITLE: &str = "Pack export failed";
pub(crate) const APP_ERR_OPTIMISE_TITLE: &str = "Optimize failed";
pub(crate) const APP_ERR_SAVE_FILE_TITLE: &str = "Failed to save file";
pub(crate) const APP_ERR_LOAD_FILE_TITLE: &str = "Failed to load file";
pub(crate) const APP_ERR_TRACK_OP_TITLE: &str = "Track operation failed";
pub(crate) const APP_ERR_NEED_GAME_NAME: &str =
    "Enter a game name before saving the package files.";

pub(crate) const APP_STATUS_RENAMED_TRACK: &str = "Renamed track; pack folder rescanned.";
pub(crate) const APP_STATUS_PACK_ZIP_BUILT: &str = "Built the pack zip -- choose where to save it.";
pub(crate) const APP_STATUS_PACK_EXPORT_FAILED: &str = "Pack export failed.";
pub(crate) const APP_STATUS_SCREENSHOT_OPT_FAILED: &str = "Screenshot optimize failed.";
pub(crate) const APP_STATUS_WAV_RENDER_FAILED: &str = "The WAV render failed.";
pub(crate) const APP_STATUS_MEASURING_VOLUME: &str = "Measuring volume...";
pub(crate) const APP_STATUS_MEASURING_PEAK: &str = "Measuring peak...";
pub(crate) const APP_STATUS_SONG_SILENT: &str = "The song is silent; volume left unchanged.";
pub(crate) const APP_STATUS_PACKAGE_SAVE_FAILED: &str =
    "Some package files could not be saved; changes kept.";
pub(crate) const APP_STATUS_PACKAGE_SAVED: &str = "Saved the package .txt and .m3u.";
pub(crate) const APP_MSG_SAVE_CANCELLED: &str = "The save was cancelled.";
pub(crate) const APP_STATUS_EXPORT_CANCELLED: &str = "Export cancelled.";
pub(crate) const APP_STATUS_DROP_SINGLE: &str = "Drop a single file at a time.";
pub(crate) const APP_DROP_HINT: &str = "Drop to open";
pub(crate) const APP_STATUS_DROP_DIALOG_OPEN: &str =
    "Close the open dialog before dropping a file.";
pub(crate) const APP_STATUS_ALREADY_SPLITTING_CHANNELS: &str = "Already splitting channels.";
pub(crate) const APP_STATUS_ALREADY_SPLITTING: &str = "Already splitting.";
pub(crate) const APP_STATUS_NOTHING_TO_UNDO: &str = "Nothing to undo.";
pub(crate) const APP_STATUS_NOTHING_TO_REDO: &str = "Nothing to redo.";
pub(crate) const APP_STATUS_OPEN_SONG_FIRST: &str = "Please open a song first.";
pub(crate) const APP_STATUS_DRO_INFO_VGM: &str =
    "DRO Info applies to DRO files; use Edit VGM Metadata.";
pub(crate) const APP_STATUS_ONLY_VGM_TAG: &str = "Only VGMs support tag editing";
pub(crate) const APP_STATUS_NOT_VGM: &str = "Song is not a VGM";
pub(crate) const APP_STATUS_CONVERTED_VGM: &str = "Successfully converted to VGM";
pub(crate) const APP_STATUS_CONVERTED_DRO1: &str = "Successfully converted to DRO v1";
pub(crate) const APP_STATUS_HEADER_AGREES: &str = "The header already agrees with the stream.";
pub(crate) const APP_STATUS_HEADER_FIXED_ONE: &str = "Corrected 1 header field. Remember to save.";
pub(crate) const APP_STATUS_ONLY_VGM_OPTIMIZE: &str = "Only VGMs can be optimized";
pub(crate) const APP_STATUS_NOTHING_TO_OPTIMIZE: &str = "Nothing to optimize";
pub(crate) const APP_STATUS_LOOP_RESET: &str = "Loop markers reset to the whole song.";
pub(crate) const APP_STATUS_LOOP_SEARCH_CANCELLED: &str = "Loop search cancelled.";
pub(crate) const APP_STATUS_VGM_METADATA_UPDATED: &str = "Updated VGM metadata.";
pub(crate) const APP_STATUS_SONG_CLOSED: &str = "Closed the song.";
pub(crate) const APP_STATUS_CLICK_TRACK_FIRST: &str =
    "Click a track first, then Alt+Up / Alt+Down to move it.";
pub(crate) const APP_STATUS_TRACK_OP_RUNNING: &str = "A track operation is still running.";
pub(crate) const APP_STATUS_PACK_CLOSED: &str = "Closed the pack project.";
pub(crate) const APP_STATUS_BUILDING_ZIP: &str = "Building pack zip...";
pub(crate) const APP_STATUS_NO_TAGGABLE: &str = "No readable tracks to tag.";
pub(crate) const APP_STATUS_BULK_TAG_NOOP: &str = "Bulk tag: nothing changed.";
pub(crate) const APP_STATUS_NO_RENDERABLE_TRACKS: &str = "No tracks this app can render to scan.";
pub(crate) const APP_STATUS_STILL_SCANNING: &str = "Still scanning volumes...";
pub(crate) const APP_STATUS_MODIFIERS_NOOP: &str =
    "Volume modifiers: nothing to change (scan volumes first).";
pub(crate) const APP_STATUS_DATE_CONVERTED: &str = "Converted the pack date to hyphens.";
pub(crate) const APP_STATUS_NO_DATES: &str = "No slash-separated dates to convert.";
pub(crate) const APP_STATUS_NAMES_MATCH: &str = "Every file name already matches its tag.";
pub(crate) const APP_STATUS_SEARCHING_LOOPS: &str = "Searching for loops...";
pub(crate) const APP_STATUS_NO_LOOPS_FOUND: &str = "No loops found.";
pub(crate) const APP_STATUS_NO_MORE_DELAYS: &str = "No more delays found.";
pub(crate) const APP_STATUS_SETTINGS_SAVED: &str = "Settings saved.";
pub(crate) const APP_STATUS_NEEDS_DRO: &str = "This applies to DRO files.";
pub(crate) const APP_STATUS_NOTHING_TO_PLAY: &str = "There is nothing here this app can play.";
pub(crate) const APP_STATUS_OPEN_FILE_FIRST: &str = "Please open a file first.";
pub(crate) const APP_STATUS_HEADER_AGREES_NOTHING: &str = "The header agrees with the stream.";
pub(crate) const APP_STATUS_ALREADY_RENDERING: &str = "Already rendering a WAV.";
pub(crate) const APP_STATUS_RENDERING_WAV: &str = "Rendering to WAV...";
pub(crate) const APP_STATUS_RENDER_CHOOSE_PATH: &str = "Choose where to save the WAV...";
pub(crate) const APP_STATUS_NOTHING_TO_SPLIT: &str = "There is nothing here to split.";
pub(crate) const APP_STATUS_SPLITTING_CHANNELS: &str = "Splitting channels...";
pub(crate) const APP_STATUS_SPLITTING_SONGS: &str = "Splitting songs...";
pub(crate) const APP_STATUS_SPLIT_CANCELLED: &str = "Split cancelled.";
pub(crate) const APP_STATUS_SPLIT_FAILED: &str = "The split failed.";
pub(crate) const APP_STATUS_NO_SONGS_SPLIT: &str = "No songs to split.";
pub(crate) const APP_STATUS_NO_CHANNELS_SPLIT: &str = "No channels to split.";
pub(crate) const APP_STATUS_SPLIT_WRITE_FAILED: &str = "Some split files could not be written.";

pub(crate) const APP_CONFIRM_DISCARD_TITLE: &str = "Discard unsaved changes?";
pub(crate) const APP_CONFIRM_QUIT_BODY: &str = "You have unsaved changes. Quit anyway?";
pub(crate) const APP_CONFIRM_DISCARD_LOAD_BODY: &str =
    "The current song has unsaved changes. Open a different file anyway?";
pub(crate) const APP_CONFIRM_CLOSE_FILE_BODY: &str =
    "The current song has unsaved changes. Close it anyway?";
pub(crate) const APP_CONFIRM_DISCARD_PACK_TITLE: &str = "Discard unsaved package details?";
pub(crate) const APP_CONFIRM_PACK_OPEN_BODY: &str =
    "This pack has unsaved changes. Open a different folder anyway?";
pub(crate) const APP_CONFIRM_PACK_CLOSE_BODY: &str =
    "This pack has unsaved changes. Close it anyway?";
pub(crate) const APP_CONFIRM_EXPORT_TITLE: &str = "Export anyway?";
pub(crate) const APP_CONFIRM_DELETE_SCREENSHOT_TITLE: &str = "Delete screenshot?";

pub(crate) const APP_LOOP_CLEARED_TITLE: &str = "Loop point cleared";
pub(crate) const APP_LOOP_CLEARED_BODY: &str =
    "The loop start was past the end of the song and has been cleared.";
pub(crate) const APP_DESC_NOT_PARSED_TITLE: &str = "Description not parsed";
pub(crate) const APP_NOT_PNG_TITLE: &str = "Not a PNG";
pub(crate) const APP_NOT_VGM_TITLE: &str = "Not a VGM";
pub(crate) const APP_NOT_VGM_BODY: &str =
    "Convert the song to VGM first (File > Convert > Convert to VGM) to store loop points.";
pub(crate) const APP_SONGS_EXPORTED_TITLE: &str = "Songs exported";
pub(crate) const APP_FIX_HEADER_TITLE: &str = "Fix Header";
pub(crate) const APP_AUDIT_HEADER_INTRO: &str =
    "This file's header disagrees with its own music:\n\n";
pub(crate) const APP_AUDIT_HEADER_OUTRO: &str = "\nCorrect them to the stream's values?";

/// The OS window title: the open file (with a `*` while it has unsaved changes)
/// ahead of the app name, or the app name and tagline when nothing is open.
pub(crate) fn app_window_title(name: Option<&str>, dirty: bool) -> String {
    match name {
        Some(name) => {
            let marker = if dirty { " *" } else { "" };
            format!("{name}{marker} \u{2014} VGM Studio")
        }
        // Matches the startup title in bin/vgmstudio.rs so the empty state does
        // not flash a different string on the first frame. The workspace version
        // is shared, so this crate's CARGO_PKG_VERSION is the app's.
        None => format!(
            "VGM Studio v{} \u{2014} Vintage Groove Mangler",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

/// The About body: identity and the binary's licensing. The per-core credit
/// table and the serialport note live in the separate Licenses dialog now (they
/// are a table, not prose), reachable by a button in About.
pub(crate) fn app_about_text(
    version: impl std::fmt::Display,
    optimize_credit: impl std::fmt::Display,
) -> String {
    format!(
        "VGM Studio v{version}\n\
         Vintage Groove Mangler\n\
         Laurence Dougal Myers\n\
         Web: http://www.jestarjokin.net/apps/drotrimmer\n\
         Web: https://github.com/laurence-myers/vgm-studio\n\
         E-Mail: jestarjokin@jestarjokin.net\n\
         \n\
         This program is licensed under the GNU General Public License,\n\
         version 2 or (at your option) any later version -- it links\n\
         emulator cores under the GPL and LGPL. Complete corresponding\n\
         source code: https://github.com/laurence-myers/vgm-studio\n\
         \n\
         The file model and playback engine (vgms-core, vgms-synth) are\n\
         separately available under MIT OR Apache-2.0; see licenses/\n\
         in the source distribution.\n\
         \n\
         {optimize_credit}\n\
         \n\
         See Licenses for the emulator cores compiled into this build."
    )
}

/// The dependency-license note shown at the foot of the Licenses dialog.
pub(crate) const APP_LICENSES_NOTE: &str = "RetroWave OPL3 output links the serialport crate, used under the MPL-2.0. \
     Its source: https://github.com/serialport/serialport-rs";

pub(crate) fn app_target_write(name: &str, inst: &str) -> String {
    format!("a write to {name}{inst}")
}

pub(crate) fn app_mismatch_body(prefix: &str, advice: &str) -> String {
    format!(
        "{prefix}here was a mismatch between\n\
         the measured length of the song in milliseconds,\n\
         and the length stored in the DRO file.\n\
         {advice}"
    )
}

pub(crate) fn app_pack_zip_built_log(log: &str) -> String {
    format!("Built the pack zip. {log} Choose where to save it.")
}

pub(crate) fn app_could_not_save_settings(error: impl std::fmt::Display) -> String {
    format!("Could not save settings: {error}")
}

pub(crate) fn app_could_not_resume(error: impl std::fmt::Display) -> String {
    format!("Could not resume playback: {error}")
}

pub(crate) fn app_playback_stopped(error: impl std::fmt::Display) -> String {
    format!("Playback stopped: {error}")
}

pub(crate) fn app_status_matched_volume(dbfs: f32, volume: f32) -> String {
    format!("Peak {dbfs:.1} dBFS \u{2192} volume {volume:.2}\u{00d7}")
}

pub(crate) fn app_status_measured_modifier(dbfs: f32, modifier: u8) -> String {
    format!("Peak {dbfs:.1} dBFS \u{2192} volume modifier {modifier}")
}

pub(crate) fn app_status_file_saved(shown: &str) -> String {
    format!("File saved to {shown}.")
}

pub(crate) fn app_status_screenshot_added(name: &str) -> String {
    format!("Added {name} to the pack folder.")
}

pub(crate) fn app_status_exported(shown: &str) -> String {
    format!("Exported {shown}.")
}

pub(crate) fn app_status_pack_saved(shown: &str) -> String {
    format!("Saved pack {shown}.")
}

pub(crate) fn app_status_rendered(shown: &str) -> String {
    format!("Rendered {shown}.")
}

pub(crate) fn app_status_unsupported_type(name: &str) -> String {
    format!("Can't open {name}: unsupported file type.")
}

pub(crate) fn app_status_undone(description: &str) -> String {
    format!("Undone: {description}")
}

pub(crate) fn app_status_redone(description: &str) -> String {
    format!("Redone: {description}")
}

pub(crate) fn app_status_header_fixed(count: impl std::fmt::Display) -> String {
    format!("Corrected {count} header fields. Remember to save.")
}

pub(crate) fn app_status_optimized(
    commands: impl std::fmt::Display,
    bytes: impl std::fmt::Display,
) -> String {
    format!("Optimized: removed {commands} command(s), saved {bytes} byte(s)")
}

/// Edit > Optimize kept the original because the render gate rejected the
/// smaller file (D-orw-4).
pub(crate) fn app_status_optimize_reverted(reason: &str) -> String {
    format!("Kept the original: {reason}.")
}

pub(crate) fn app_status_cropped(kept: impl std::fmt::Display) -> String {
    format!("Cropped to {kept} instruction(s).")
}

pub(crate) fn app_status_cropped_restored(
    kept: impl std::fmt::Display,
    n: impl std::fmt::Display,
) -> String {
    format!("Cropped to {kept} instruction(s), including {n} that restore the chip state.")
}

pub(crate) fn app_status_deleted(removed: impl std::fmt::Display) -> String {
    format!("Deleted {removed} instruction(s).")
}

pub(crate) fn app_status_deleted_bridged(
    removed: impl std::fmt::Display,
    n: impl std::fmt::Display,
) -> String {
    format!(
        "Deleted {removed} instruction(s), leaving {n} write(s) to carry the chip state across the seam."
    )
}

pub(crate) fn app_status_opened(name: &str) -> String {
    format!("Successfully opened {name}.")
}

pub(crate) fn app_status_opened_chips(name: &str, chips: impl std::fmt::Display) -> String {
    format!("Successfully opened {name} ({chips}).")
}

pub(crate) fn app_status_opened_missing(
    name: &str,
    chips: impl std::fmt::Display,
    missing: &str,
) -> String {
    format!(
        "Opened {name} ({chips}); no core yet for {missing}, which will \
         stay silent."
    )
}

pub(crate) fn app_status_opened_unsupported(name: &str, chips: impl std::fmt::Display) -> String {
    format!("Opened {name} ({chips}); playback is not supported yet.")
}

pub(crate) fn app_status_unreadable_commands(name: &str) -> String {
    format!("{name} could not be read as commands.")
}

pub(crate) fn app_status_pack_undone(label: &str) -> String {
    format!("Undone: {label}.")
}

pub(crate) fn app_status_pack_redone(label: &str) -> String {
    format!("Redone: {label}.")
}

pub(crate) fn app_status_pack_opened(name: &str) -> String {
    format!("Opened pack project: {name}.")
}

pub(crate) fn app_desc_not_parsed_body(warning: &str) -> String {
    format!("{warning}\n\nSaving the package files will overwrite it.")
}

pub(crate) fn app_export_warnings_body(listed: &str) -> String {
    format!("These submission checks did not pass:\n\n{listed}")
}

pub(crate) fn app_err_edit_gone(original_name: &str) -> String {
    format!("\"{original_name}\" is no longer in the folder; the edit was not applied.")
}

pub(crate) fn app_err_renamed_gone(original_name: &str) -> String {
    format!("\"{original_name}\" is no longer in the folder; it was not renamed.")
}

pub(crate) fn app_status_updated(new_name: &str) -> String {
    format!("Updated {new_name}.")
}

pub(crate) fn app_status_scanning_volumes(count: impl std::fmt::Display) -> String {
    format!("Scanning {count} track volume(s)...")
}

/// The pack volume scan's live progress, for the status bar's busy readout.
pub(crate) fn app_busy_scanning_volumes(done: usize, total: usize) -> String {
    format!("Scanning song {done} / {total}")
}

pub(crate) fn app_status_loop_candidates(count: impl std::fmt::Display) -> String {
    format!("Found {count} loop candidate(s).")
}

pub(crate) fn app_status_scanned_volumes(count: impl std::fmt::Display) -> String {
    format!("Scanned {count} track volume(s).")
}

pub(crate) fn app_status_optimizing(name: &str) -> String {
    format!("Optimizing {name}...")
}

/// The status line while a sweep optimises one track of several.
pub(crate) fn app_status_optimizing_track(name: &str, n: usize, total: usize) -> String {
    format!("Optimizing {name} ({n}/{total})...")
}

/// The busy-spinner readout counting an "Optimize All" sweep off track by track.
pub(crate) fn app_busy_optimizing_tracks(done: usize, total: usize) -> String {
    format!("Optimizing song {} / {total}", (done + 1).min(total.max(1)))
}

/// The one-line summary once a sweep finishes.
pub(crate) fn app_status_optimized_tracks(_done: usize, total: usize) -> String {
    format!("Finished optimizing {total} track(s).")
}

/// The status line when a per-track optimise kept the original, and why.
pub(crate) fn app_status_optimize_kept(name: &str, reason: &str) -> String {
    format!("{name}: kept original -- {reason}.")
}

pub(crate) fn app_status_already_optimal(name: &str, bytes: impl std::fmt::Display) -> String {
    format!("{name} is already optimal ({bytes} bytes).")
}

pub(crate) fn app_status_no_path(name: &str) -> String {
    format!("{name}: no file path to save to.")
}

pub(crate) fn app_status_optimized_bytes(
    name: &str,
    from: impl std::fmt::Display,
    to: impl std::fmt::Display,
) -> String {
    format!("{name}: {from} -> {to} bytes.")
}

pub(crate) fn app_not_png_body(name: &str) -> String {
    format!("{name} is not a readable PNG image.")
}

pub(crate) fn app_status_replacing(name: &str) -> String {
    format!("Replacing {name}...")
}

pub(crate) fn app_status_recompressing(file_name: &str) -> String {
    format!("Recompressing {file_name}...")
}

pub(crate) fn app_status_adding(name: &str) -> String {
    format!("Adding {name}...")
}

pub(crate) fn app_delete_screenshot_body(name: &str) -> String {
    format!("{name} will be deleted from the pack folder.")
}

pub(crate) fn app_status_loop_marked(start: usize, end: usize, count: usize) -> String {
    format!("Loop {start} - {end} ({count} instructions).")
}

pub(crate) fn app_status_loop_saved_range(start: usize, end: usize) -> String {
    format!("Loop saved: {start} - {end}.")
}

pub(crate) fn app_status_loop_saved_end(start: usize) -> String {
    format!("Loop saved: {start} - end of song.")
}

pub(crate) fn app_status_goto_invalid(text: &str) -> String {
    format!("Invalid position for goto: {text}")
}

pub(crate) fn app_status_goto_out_of_range(position: usize) -> String {
    format!("Position for goto is out of range: {position:04X}")
}

pub(crate) fn app_status_goto_gone(position: usize) -> String {
    format!("Gone to position: {position:04X}")
}

pub(crate) fn app_status_find_found(label: &str, index: usize) -> String {
    format!("Occurrence of {label} found at position {index:04X}.")
}

pub(crate) fn app_status_find_not_found(label: &str) -> String {
    format!("Could not find another occurrence of {label}.")
}

pub(crate) fn app_status_wrote_songs(written: usize, dir: impl std::fmt::Display) -> String {
    format!("Wrote {written} song(s) to {dir}.")
}

pub(crate) fn app_songs_exported_body(written: usize, dir: impl std::fmt::Display) -> String {
    format!("Wrote {written} song(s) to {dir}.\n\nOpen the folder as a pack project?")
}

pub(crate) fn app_status_wrote_files(written: usize, dir: impl std::fmt::Display) -> String {
    format!("Wrote {written} file(s) to {dir}.")
}

pub(crate) fn app_play_tail_label(value: &str, plural: &str) -> String {
    format!("Play last {value} second{plural}")
}

pub(crate) fn app_play_seam_label(tail: impl std::fmt::Display) -> String {
    format!("Play the loop join: the last {tail} of the region, repeating")
}

// ============================================================================
// pack.rs
// ============================================================================

pub(crate) const PACK_DIRTY_TIP: &str = "The package metadata has unsaved edits";
pub(crate) const PACK_SCAN_VOLUMES_TIP_SCANNING: &str = "Measuring every track's peak volume...";
pub(crate) const PACK_SCAN_VOLUMES_TIP: &str = "Measure every track's peak volume (dBFS)";
// The per-track optimise is native-only (it renders both files to compare
// them, D-orw-7), so its tooltips would be dead code on the web.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const PACK_OPTIMIZE_ALL_TIP: &str =
    "Optimize every track, verify each renders identically, and write the smaller files back";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const PACK_OPTIMIZE_DISABLED_TIP: &str =
    "This track has no file on disk to optimize in place";
pub(crate) const PACK_APPLY_TIP_SCANNED: &str = "Write volume modifiers to each track";
pub(crate) const PACK_APPLY_TIP_UNSCANNED: &str = "Scan volumes first";
pub(crate) const PACK_ALBUM_TIP: &str =
    "ON: use the loudest track's peak level.\nOFF: use each track's peak level.";
pub(crate) const PACK_BULK_TAG_TIP: &str = "Write shared GD3 fields (game, system, composer\u{2026}) to many tracks at \
     once";
pub(crate) const PACK_FIX_DATES_TIP: &str =
    "Rewrite slash-separated dates (1994/03/01 \u{2192} 1994-03-01)";
pub(crate) const PACK_FIX_FILE_NAMES_TIP: &str =
    "Rename each file to \"NN Track Name.ext\" from its GD3 tag";
pub(crate) const PACK_HARDWARE_TIP_HIDE: &str = "Hide the system, OS and music hardware fields";
pub(crate) const PACK_HARDWARE_TIP_EDIT: &str = "Edit the system, OS and music hardware fields";
/// The prompt on the closed preset dropdown -- it fills the three hardware
/// fields, and reverts to this after a pick, since the fields stay editable.
pub(crate) const PACK_PRESET_PROMPT: &str = "Choose a system...";
pub(crate) const PACK_READINESS_TIP_NONE: &str = "Every submission check passes";
pub(crate) const PACK_READINESS_TIP_ERROR: &str =
    "This must be fixed before the pack can be exported";
pub(crate) const PACK_READINESS_TIP_WARNING: &str = "Exporting will ask you to confirm these first";
pub(crate) const PACK_READINESS_TIP_NOTE: &str = "Notes to review before submitting";
pub(crate) const PACK_VIEW_CHECKLIST_TIP: &str = "Open the submission checklist";
pub(crate) const PACK_EXPORT_ZIP_TIP: &str =
    "Build the submission zip (songs, screenshot, description, playlist)";
pub(crate) const PACK_SAVE_DOCS_TIP: &str = "Write Game Name.txt and Game Name.m3u into the folder";
pub(crate) const PACK_SAVE_ARCHIVE_TIP: &str = "Save the pack back to a .zip (optimized, gzipped)";
pub(crate) const PACK_OPT_TIP: &str =
    "Strip redundant register writes from each VGM before packing";
pub(crate) const PACK_VGZ_TIP: &str = "Gzip each .vgm to .vgz on export";
pub(crate) const PACK_CHECKLIST_LINK_TIP: &str = "Click to jump to the fix";
pub(crate) const PACK_TRACK_READY_TIP: &str = "Ready for submission";
pub(crate) const PACK_TRACK_UNREADABLE_TIP: &str = "This file could not be read.";
pub(crate) const PACK_TRACKS_HEADING: &str = "Tracks (double-click to edit)";
pub(crate) const PACK_PEAK_TIP_CLIPPED: &str = "Peak reaches full scale (clipping)";
pub(crate) const PACK_PEAK_TIP: &str = "Loudest peak, in dBFS";
pub(crate) const PACK_DRAG_TIP: &str = "Drag to reorder";
pub(crate) const PACK_OPEN_DISABLED_TIP: &str =
    "This file's commands could not be read. Quick edit still changes its tags.";
pub(crate) const PACK_ADD_SCREENSHOT_TIP: &str = "Copy a .png into the pack folder";
pub(crate) const PACK_PNG_UNREADABLE: &str = "This file's header could not be read as a PNG.";
pub(crate) const PACK_PNG_NONSTANDARD: &str =
    "Not a standard PC display mode; check it was captured, not resized.";
pub(crate) const PACK_RENAME_SCREENSHOT_TIP: &str =
    "Name this screenshot after the game, or after a variant of it";
pub(crate) const PACK_RECOMPRESS_TIP: &str = "Losslessly recompress with oxipng and save in place";
pub(crate) const PACK_REPLACE_SCREENSHOT_TIP: &str =
    "Overwrite this file with another .png, keeping its name";
pub(crate) const PACK_DELETE_SCREENSHOT_TIP: &str =
    "Remove this screenshot from the pack folder (asks first)";
pub(crate) const PACK_NO_SCREENSHOT_TITLE: &str = "No screenshot in this folder";
pub(crate) const PACK_NO_SCREENSHOT_BODY: &str =
    "A submission needs a title-screen .png at the game's native resolution.";
pub(crate) const PACK_CHECK_GAME_NAME: &str =
    "Enter a game name (it names every file in the pack).";
pub(crate) const PACK_CHECK_NO_READABLE: &str = "There are no readable songs to export.";
pub(crate) const PACK_CHECK_UNREADABLE_FILES: &str =
    "Some files could not be read; they ship as-is, without a track-list entry.";
pub(crate) const PACK_CHECK_NO_SCREENSHOT: &str = "There is no screenshot (.png) in the folder.";
pub(crate) const PACK_CHECK_NAMING: &str = "Some files are not named \"NN Title.ext\".";
pub(crate) const PACK_CHECK_DUP_NUMBERS: &str = "Some track numbers are duplicated.";
pub(crate) const PACK_CHECK_NONCONTIGUOUS: &str =
    "Track numbers are not a contiguous 01, 02, 03... sequence.";
pub(crate) const PACK_READY_TO_SUBMIT: &str = "Ready to submit";

pub(crate) fn pack_check_playback(silent: &str) -> String {
    format!(
        "Playback is not supported yet for {}; those tracks export normally, but \
         preview here without them.",
        silent
    )
}

pub(crate) fn pack_playback_unsupported(chips: &str) -> String {
    format!("Playback for {chips} is not supported yet")
}

pub(crate) fn pack_add_screenshot_named(name: &str) -> String {
    format!("Copy a .png into the pack folder as \"{name}\"")
}

// ============================================================================
// dialogs/settings.rs
// ============================================================================

pub(crate) const SETTINGS_OUTPUT_CORE_APPLIES: &str = "The core used for live playback.";
pub(crate) const SETTINGS_OUTPUT_CORE_HOVER: &str =
    "Rendering, splitting and the waveform always use an emulator.";
pub(crate) const SETTINGS_DEVICE_HOVER: &str =
    "The board's serial port. Recognised boards are matched by USB ID.";
pub(crate) const SETTINGS_RESAMPLING_HOVER: &str = "How non-OPL chips are resampled. Sinc is \
     band-limited and accurate; linear is aliased but crunchy.";
pub(crate) const SETTINGS_OPTIMIZER_HOVER: &str = "Which optimiser shrinks a VGM on Edit > Optimize and pack export. \
     Automatic uses the built-in pass where it covers every chip and the \
     external vgmtools otherwise; the other choices always use the named \
     optimiser.";
pub(crate) const SETTINGS_TOOL_STAGES_HOVER: &str = "Extra stages the external tools run. They apply only when the tools run, so \
     they are greyed for the built-in optimiser.";
pub(crate) const SETTINGS_SAMPLE_ROMS_HOVER: &str =
    "Run vgm_sro to strip unused regions from sample ROMs.";
pub(crate) const SETTINGS_DAC_RUNS_HOVER: &str =
    "Run optdac to collapse long runs of identical DAC writes.";
pub(crate) const TRACK_OPTIMIZE_USE_GLOBAL_HINT: &str =
    "Drop this track's own options and fall back to the global Settings default.";
pub(crate) fn track_optimize_intro(file_name: &str) -> String {
    format!("Optimiser options for {file_name}, overriding the global Settings default:")
}
pub(crate) const SETTINGS_FREQUENCY_HOVER: &str = "49716 Hz is the OPL3's native rate";
pub(crate) const SETTINGS_BUFFER_SIZE_HOVER: &str = "Frames per audio callback. Smaller seeks and mutes sooner; larger \
     avoids dropouts.";
pub(crate) const SETTINGS_BIT_DEPTH_HOVER: &str = "WAV export only";
pub(crate) const SETTINGS_TAIL_LENGTH_HOVER: &str =
    "How much the \"play last X seconds\" button plays";
pub(crate) const SETTINGS_THEME_HOVER: &str = "The case colour.";
pub(crate) const SETTINGS_PAD_STYLE_HOVER: &str = "The keycap colour.";
pub(crate) const SETTINGS_INVALID_TITLE: &str = "Invalid settings";

/// Hover text on a folded run's summary row in the instruction table.
pub(crate) const TABLE_FOLD_EXPAND: &str = "Show this run of commands";
pub(crate) const TABLE_FOLD_COLLAPSE: &str = "Collapse this run of commands";
pub(crate) const SETTINGS_INVALID_NUMBERS: &str = "Check that the entered values are numbers.";

// ============================================================================
// dialogs/split.rs
// ============================================================================

pub(crate) const SPLIT_WAV_ONLY: &str = "Each chip channel is rendered to its own WAV.";
pub(crate) const SPLIT_WRITE_EACH_AS: &str = "Write each channel as:";
pub(crate) const SPLIT_AUDIO_HOVER: &str = "Render each channel on its own";
pub(crate) const SPLIT_SONG_HOVER: &str = "Rewrite each channel into its own VGM";
pub(crate) const SPLIT_SKIPPED_NOTE: &str =
    "Silent channels are skipped; existing files are overwritten.";
pub(crate) const SPLIT_MIX_APPLY: &str = "Apply to each stem:";
pub(crate) const SPLIT_SKIP_MUTED_HOVER: &str = "Leave out the muted channels";
pub(crate) const SPLIT_PANNING_HOVER: &str = "Place each stem where its channel's pan knob is set";
pub(crate) const SPLIT_BOOST_HOVER: &str = "Drive each stem through the peak limiter";
pub(crate) const SPLIT_CORE: &str = "Core for this split:";
pub(crate) const SPLIT_CORE_HOVER: &str =
    "The core to render each channel with, for this split only";

// ============================================================================
// dialogs/split_songs.rs
// ============================================================================

pub(crate) const SPLIT_SONGS_PREVIEW_UNAVAILABLE: &str =
    "No core for this file's chips yet; export still works.";
pub(crate) const SPLIT_SONGS_GAP_EXPLAIN: &str =
    "Songs are split where the capture goes silent for at least this long.";
pub(crate) const SPLIT_SONGS_TAIL_HOVER: &str =
    "How much of the silence after each song to keep, for release tails";
pub(crate) const SPLIT_SONGS_NONE_FOUND: &str = "No songs found at this threshold.";
pub(crate) const SPLIT_SONGS_NOTHING_TITLE: &str = "Nothing to export";
pub(crate) const SPLIT_SONGS_NOTHING_MESSAGE: &str = "Check at least one song to export.";

pub(crate) fn split_songs_found(count: usize) -> String {
    format!("{count} song(s) found")
}
pub(crate) fn split_songs_to_export(count: usize) -> String {
    format!("{count} to export")
}

// ============================================================================
// dialogs/screenshot_rename.rs
// ============================================================================

pub(crate) const SCREENSHOT_RENAME_NAME_HOVER: &str =
    "Prefilled from the game name. Add a suffix like \"(Japan)\" to keep several.";
pub(crate) const SCREENSHOT_RENAME_RECOMPRESS_HOVER: &str = "Lossless shrink, applied on import.";
pub(crate) const SCREENSHOT_RENAME_NAME_REQUIRED_TITLE: &str = "Name required";
pub(crate) const SCREENSHOT_RENAME_NAME_REQUIRED_MESSAGE: &str =
    "Enter a name for the screenshot file.";
pub(crate) const SCREENSHOT_RENAME_DUPLICATE_TITLE: &str = "Duplicate file name";

pub(crate) fn screenshot_rename_duplicate_message(name: &str) -> String {
    format!("Another screenshot in this pack is already named \"{name}\".")
}

// ============================================================================
// dialogs/render_wav.rs
// ============================================================================

pub(crate) const RENDER_WAV_APPLY: &str = "Apply to the render:";
pub(crate) const RENDER_WAV_TOGGLES_HOVER: &str =
    "Leave out the channels muted in the channel panel";
pub(crate) const RENDER_WAV_PANNING_HOVER: &str = "Place each channel where its pan knob is set";
pub(crate) const RENDER_WAV_BOOST_HOVER: &str = "Drive the signal through the peak limiter";
pub(crate) const RENDER_WAV_FREQ_NOTE: &str = "Frequency and bit depth: see Settings.";
pub(crate) const RENDER_WAV_INVALID_TITLE: &str = "Invalid boost";
pub(crate) const RENDER_WAV_CORE: &str = "Core for this render:";
pub(crate) const RENDER_WAV_CORE_HOVER: &str = "The core to render with, for this render only";

pub(crate) fn render_wav_boost_range(min: f32, max: f32) -> String {
    format!("{min}x to {max}x")
}
pub(crate) fn render_wav_boost_message(min: f32, max: f32) -> String {
    format!("The boost must be a number from {min} to {max}.")
}

// ============================================================================
// dialogs/track_edit.rs
// ============================================================================

pub(crate) const TRACK_EDIT_CURRENT_NAME_HINT: &str = "The file's name on disk right now";
pub(crate) const TRACK_EDIT_NEW_NAME_HINT: &str =
    "Derived from the track number and Track Name (EN)";
pub(crate) const TRACK_EDIT_TRACK_NAME_REQUIRED_TITLE: &str = "Track name required";
pub(crate) const TRACK_EDIT_TRACK_NAME_REQUIRED_MESSAGE: &str =
    "Enter a Track Name (EN) to derive the file name from (\"?\" and \"!\" are dropped).";
pub(crate) const TRACK_EDIT_DUPLICATE_TITLE: &str = "Duplicate file name";

pub(crate) fn track_edit_duplicate_message(name: &str) -> String {
    format!("Another track in this pack is already named \"{name}\".")
}

// ============================================================================
// dialogs/find_loop.rs
// ============================================================================

pub(crate) const FIND_LOOP_MIN_LENGTH_HELP: &str =
    "A repeated block must be at least this long to count as a loop.";
pub(crate) const FIND_LOOP_APPLY_VGM_ONLY_HINT: &str = "Only VGM files store loop points.";
pub(crate) const FIND_LOOP_SEARCHING: &str = "Searching...";
pub(crate) const FIND_LOOP_NONE_FOUND: &str = "No loops found. Try a shorter minimum length.";
pub(crate) const FIND_LOOP_PROMPT: &str = "Click Search to look for loop points.";
pub(crate) const FIND_LOOP_QUALITY_IDEAL: &str = "a clean repeat running to the song's end";
pub(crate) const FIND_LOOP_QUALITY_TO_END: &str = "the repeat runs to the end of the song";
pub(crate) const FIND_LOOP_QUALITY_CLEAN: &str = "a clean repeat ending before the song's end";
pub(crate) const FIND_LOOP_QUALITY_PARTIAL: &str = "a partial or overlapping repeat";

pub(crate) fn find_loop_searching_count(found: usize) -> String {
    format!("Searching... ({found} found)")
}
pub(crate) fn find_loop_quality_help(shape: &str, match_len: usize) -> String {
    format!("{shape} ({match_len} commands matched)")
}

// ============================================================================
// dialogs/vgm_metadata.rs
// ============================================================================

pub(crate) const VGM_METADATA_MEASURE_HINT: &str =
    "Measure the song's peak and suggest a modifier that brings it to full scale";
pub(crate) const VGM_METADATA_FROM_MEASURED_HINT: &str =
    "Fill the modifier from the volume already measured in the editor, without re-scanning";
pub(crate) const VGM_METADATA_MULTIPLIER_HINT: &str =
    "Set the modifier from a linear multiplier, floored to the nearest valid value";
pub(crate) const VGM_METADATA_INVALID_TITLE: &str = "Invalid VGM metadata";
pub(crate) const VGM_METADATA_LOOP_END_AFTER_START: &str =
    "Loop end must come after the loop start.";
pub(crate) const VGM_METADATA_UPDATE_ERROR: &str =
    "Error updating VGM metadata, check that the entered values are correct.";

pub(crate) fn vgm_metadata_loop_start_message(song_len: usize) -> String {
    format!("Loop start must be a Pos. (hex instruction index) below {song_len:#06X}.")
}
pub(crate) fn vgm_metadata_loop_end_message(song_len: usize) -> String {
    format!(
        "Loop end must be a Pos. (hex instruction index) of {song_len:#06X} or less, or empty for the end of the song."
    )
}

// ============================================================================
// dialogs/dro_info.rs
// ============================================================================

pub(crate) const DRO_INFO_EDIT_MODE_ENABLED: &str = "DRO Info edit mode enabled.";
pub(crate) const DRO_INFO_ALERT_TITLE: &str = "DRO Info";
pub(crate) const DRO_INFO_UPDATED_MESSAGE: &str = "DRO info updated.\nRemember to save the file.";
pub(crate) const DRO_INFO_ERROR_TITLE: &str = "Error";
pub(crate) const DRO_INFO_ERROR_MESSAGE: &str =
    "Error updating DRO info, check that the entered values are correct.";

// ============================================================================
// dialogs/bulk_tag.rs
// ============================================================================

pub(crate) const BULK_TAG_INTRO: &str = "Check the fields to write, then choose the tracks. Unchecked fields keep each track's own value.";
pub(crate) const BULK_TAG_NOTHING_TITLE: &str = "Nothing to write";
pub(crate) const BULK_TAG_NOTHING_MESSAGE: &str =
    "Check at least one field to write to the selected tracks.";
pub(crate) const BULK_TAG_NO_TRACKS_TITLE: &str = "No tracks selected";
pub(crate) const BULK_TAG_NO_TRACKS_MESSAGE: &str = "Select at least one track to tag.";

pub(crate) fn bulk_tag_selected_count(selected: usize, total: usize) -> String {
    format!("{selected}/{total} selected")
}

// ============================================================================
// dialogs/unwalkable_vgm.rs
// ============================================================================

pub(crate) const UNWALKABLE_VGM_BODY: &str =
    "Can't read this file's command stream. Open the folder as a pack to edit its tags.";
pub(crate) const UNWALKABLE_VGM_OPEN_PACK_HINT: &str = "Pack mode can edit this file's tags";

// ============================================================================
// dialogs/goto.rs
// ============================================================================

pub(crate) const GOTO_INPUT_LABEL: &str = "Go to instruction (hex):";

// ============================================================================
// dialogs/help.rs
// ============================================================================

pub(crate) const HELP_ADVICE: &str = "To trim a song: select the instructions to remove and press Del. On a looping capture, look for a run of instructions with no delays between them -- that is usually where the instruments are set up.";
pub(crate) const HELP_FULL_INSTRUCTIONS: &str = "Full instructions:";

// ============================================================================
// editor.rs
// ============================================================================

/// Error surfaced when a save/convert is attempted with nothing loaded.
pub(crate) const EDITOR_NO_SONG_LOADED: &str = "no song is loaded";

// ============================================================================
// alert.rs
// ============================================================================

/// The default error-alert title.
pub(crate) const ALERT_ERROR_TITLE: &str = "Error";

// ============================================================================
// tasks.rs
// ============================================================================

pub(crate) fn tasks_render_wav_failed(e: impl std::fmt::Display) -> String {
    format!("Rendering to WAV failed: {e}")
}

pub(crate) fn tasks_song_not_extracted(number: impl std::fmt::Display) -> String {
    format!("Song {number} could not be extracted.")
}

// ============================================================================
// widgets/pan_controls.rs
// ============================================================================

pub(crate) const CHANNELS_SPREAD: &str = "Stereo width: centred is mono; turn either way to widen";

// ============================================================================
// widgets/chip_channels.rs
// ============================================================================

pub(crate) const CHIP_CHANNELS_CUSTOM: &str = "Pan with the knobs (off: the chip's own image)";
pub(crate) const CHIP_CHANNELS_UNMUTE_ALL: &str = "Unmute every channel";
pub(crate) const CHIP_CHANNELS_RESET: &str = "Reset panning to the chip's own image";

pub(crate) const CHIP_CHANNELS_MUTE_UNAVAILABLE: &str =
    "To mute individual channels, pick this chip's libvgm core in Settings > Output.";

/// The chip lamp's hover text, one per play state. Left-click mutes the whole
/// chip, right-click solos it, on every core.
pub(crate) const CHIP_LAMP_PLAYING: &str =
    "Playing. Click to mute this whole chip; right-click to solo it.";
pub(crate) const CHIP_LAMP_MUTED: &str = "Muted. Click to unmute; right-click to solo.";
pub(crate) const CHIP_LAMP_SOLOED: &str = "Soloed. Right-click to unsolo; click to mute.";
pub(crate) const CHIP_LAMP_SILENCED: &str =
    "Silenced by another chip's solo. Right-click to solo this one instead; click to mute it.";

pub(crate) fn chip_channels_channel_hover(name: &str) -> String {
    format!("{}. Left-click mutes, right-click solos.", name)
}

// ============================================================================
// widgets/boost_stepper.rs
// ============================================================================

pub(crate) const BOOST_STEPPER_LOCK_ON: &str =
    "Volume is kept across songs. Click to start each song from its header modifier.";
pub(crate) const BOOST_STEPPER_LOCK_OFF: &str = "Each song starts from its header volume modifier. Click to keep \
     this volume across songs.";
pub(crate) const BOOST_STEPPER_QUIETER: &str = "Quieter";
pub(crate) const BOOST_STEPPER_LOUDER: &str = "Louder";
pub(crate) const BOOST_STEPPER_AT_LIMIT: &str = "At this song's clipping limit";
pub(crate) const BOOST_STEPPER_MEASURING: &str = "Measuring the song's peak...";
pub(crate) const BOOST_STEPPER_MATCH: &str =
    "Set the volume to bring the song's peak to full scale.";

pub(crate) fn boost_stepper_factor_readout(factor: f32, db: f32) -> String {
    format!("{factor:.2}\u{00d7} ({db:+.1} dB)")
}

// ============================================================================
// widgets/loop_stepper.rs
// ============================================================================

pub(crate) const LOOP_STEPPER_FEWER: &str = "Fewer repeats";
pub(crate) const LOOP_STEPPER_MORE: &str = "More repeats";

pub(crate) fn loop_stepper_hover(count: LoopCount) -> String {
    match count {
        LoopCount::Infinite => "Repeat the region until playback is stopped".to_owned(),
        LoopCount::Times(1) | LoopCount::Times(0) => {
            "Play the region once, then carry on into the rest of the song".to_owned()
        }
        LoopCount::Times(times) => {
            format!("Play the region {times} times, then carry on into the rest of the song")
        }
    }
}

// ============================================================================
// widgets/pan_knob.rs
// ============================================================================

pub(crate) fn pan_knob_readout(value: u8) -> String {
    use core::cmp::Ordering;
    const CENTER: u8 = 0x80;
    fn percent(num: u32, den: u32) -> u32 {
        (num * 100 + den / 2) / den
    }
    match value.cmp(&CENTER) {
        Ordering::Equal => "C".to_owned(),
        Ordering::Less => format!("L{}", percent(u32::from(CENTER - value), u32::from(CENTER))),
        Ordering::Greater => format!(
            "R{}",
            percent(u32::from(value - CENTER), u32::from(255 - CENTER))
        ),
    }
}
pub(crate) fn pan_knob_spread_readout(spread: f32) -> String {
    let pct = (spread.abs() * 100.0).round() as i32;
    if pct == 0 {
        "Mono".to_owned()
    } else {
        format!("{}{pct}%", if spread < 0.0 { "-" } else { "+" })
    }
}
/// The trim knob's hover readout: a plain percentage (`71%`), no `L`/`R`/`C`
/// prefix and no sign, since the trim only attenuates over `0..=100`%.
pub(crate) fn pan_knob_trim_readout(percent: u8) -> String {
    format!("{percent}%")
}

// ============================================================================
// widgets/waveform.rs
// ============================================================================

pub(crate) fn waveform_hover(ms: u32) -> String {
    format!("{ms} ms ({})", ms_to_timestr(ms))
}

// ============================================================================
// app/frame.rs -- the chip deck disclosure
// ============================================================================

pub(crate) const CHIP_DECK_TIP_SHOW: &str = "Show the chip's channel controls";
pub(crate) const CHIP_DECK_TIP_HIDE: &str = "Hide the chip's channel controls";

// ============================================================================
// widgets/chip_output.rs
// ============================================================================

pub(crate) const CHIP_OUTPUT_NO_CORE: &str = "no core yet";
// The OPL split control: one core for the whole family, or a core per generation.
pub(crate) const CHIP_OUTPUT_OPL_MODE: &str = "OPL cores";
pub(crate) const CHIP_OUTPUT_OPL_COMBINED: &str = "Combined";
pub(crate) const CHIP_OUTPUT_OPL_SEPARATE: &str = "Separate";
pub(crate) const CHIP_OUTPUT_OPL_COMBINED_HOVER: &str =
    "Use one core for the whole OPL family (OPL2 and OPL3).";
pub(crate) const CHIP_OUTPUT_OPL_SEPARATE_HOVER: &str = "One core for the OPL2-generation chips \
     (OPL2, YM3526, Y8950), another for OPL3. Hardware output follows the OPL3 selector for \
     the whole family.";
// The picker legend: what the accuracy badge and the speed word mean.
pub(crate) const CHIP_OUTPUT_LEGEND: &str = "What the labels mean";
pub(crate) const CHIP_OUTPUT_LEGEND_ACCURACY: &str = "Accuracy";
pub(crate) const CHIP_OUTPUT_LEGEND_SPEED: &str = "Speed";
pub(crate) const SETTINGS_AUTO_SELECT: &str = "Auto-select cores";
pub(crate) const SETTINGS_AUTO_SELECT_HOVER: &str = "Set every chip to its most authentic core \
     that still holds realtime on this machine.";
// The speed measurement is native-only (it needs a wall clock and a thread),
// so its labels would be dead code in a wasm build.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const SETTINGS_MEASURE: &str = "Measure speed";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const SETTINGS_MEASURING: &str = "Measuring\u{2026}";
/// The Measure button hover, with the last result appended once measured.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn settings_measure_hover(measured: Option<f32>) -> String {
    let base = "Render a short probe to measure this machine against the reference machine. \
         Takes about a second; sharpens every core-speed estimate.";
    match measured {
        Some(ratio) => format!("{base} This machine measured at {ratio:.2}\u{d7} the reference."),
        None => base.to_owned(),
    }
}

// ============================================================================
// widgets/position_panel.rs
// ============================================================================

pub(crate) fn position_panel_loop_progress(iteration: u32, count: LoopCount) -> String {
    match count {
        LoopCount::Infinite => format!("Loop {}", iteration + 1),
        LoopCount::Times(times) => format!("Loop {} / {}", iteration + 1, times.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_title_names_the_open_file_and_marks_dirty() {
        assert_eq!(
            app_window_title(Some("song.vgm"), false),
            "song.vgm \u{2014} VGM Studio"
        );
        assert_eq!(
            app_window_title(Some("song.vgm"), true),
            "song.vgm * \u{2014} VGM Studio",
            "an unsaved change is marked with a star"
        );
        // Nothing open: the app name and tagline, carrying the version.
        let empty = app_window_title(None, false);
        assert!(empty.starts_with("VGM Studio v"), "{empty}");
        assert!(empty.ends_with("Vintage Groove Mangler"), "{empty}");
    }
}
