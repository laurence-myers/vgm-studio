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
pub(crate) const APP_NOTHING_MARKED: &str =
    "Mark a region first -- the loop markers cover the whole song.";
pub(crate) const APP_TARGET_ANY_DELAY: &str = "a delay";

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
pub(crate) const APP_STATUS_EXPORT_CANCELLED: &str = "Export cancelled; the zip was not saved.";
pub(crate) const APP_STATUS_DROP_SINGLE: &str = "Drop a single file at a time.";
pub(crate) const APP_STATUS_DROP_DIALOG_OPEN: &str = "Close the open dialog before dropping a file.";
pub(crate) const APP_STATUS_ALREADY_SPLITTING_CHANNELS: &str = "Already splitting channels.";
pub(crate) const APP_STATUS_ALREADY_SPLITTING: &str = "Already splitting.";
pub(crate) const APP_STATUS_NOTHING_TO_UNDO: &str = "Nothing to undo.";
pub(crate) const APP_STATUS_NOTHING_TO_REDO: &str = "Nothing to redo.";
pub(crate) const APP_STATUS_OPEN_SONG_FIRST: &str = "Please open a song first.";
pub(crate) const APP_STATUS_DRO_INFO_VGM: &str =
    "DRO Info applies to DRO files; use Edit VGM Metadata.";
pub(crate) const APP_STATUS_ONLY_VGM_TAG: &str = "Only VGMs support tag editing";
pub(crate) const APP_STATUS_NOT_VGM: &str = "Song is not a VGM";
pub(crate) const APP_STATUS_ALREADY_VGM: &str = "File is already in VGM format";
pub(crate) const APP_STATUS_CONVERTED_VGM: &str = "Successfully converted to VGM";
pub(crate) const APP_STATUS_CONVERTED_DRO1: &str = "Successfully converted to DRO v1";
pub(crate) const APP_STATUS_HEADER_AGREES: &str = "The header already agrees with the stream.";
pub(crate) const APP_STATUS_HEADER_FIXED_ONE: &str = "Corrected 1 header field. Remember to save.";
pub(crate) const APP_STATUS_ONLY_VGM_OPTIMIZE: &str = "Only VGMs can be optimized";
pub(crate) const APP_STATUS_NOTHING_TO_OPTIMIZE: &str =
    "Nothing to optimize -- the VGM is already compact";
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
pub(crate) const APP_STATUS_NO_MORE_DELAYS: &str = "No more delays found.";
pub(crate) const APP_STATUS_SETTINGS_SAVED: &str = "Settings saved.";
pub(crate) const APP_STATUS_NEEDS_OPL: &str = "This needs an OPL song.";
pub(crate) const APP_STATUS_NOTHING_TO_PLAY: &str = "There is nothing here this app can play.";
pub(crate) const APP_STATUS_OPEN_FILE_FIRST: &str = "Please open a file first.";
pub(crate) const APP_STATUS_HEADER_AGREES_NOTHING: &str =
    "The header agrees with the stream; nothing to fix.";
pub(crate) const APP_STATUS_ALREADY_RENDERING: &str = "Already rendering a WAV.";
pub(crate) const APP_STATUS_RENDERING_WAV: &str = "Rendering to WAV...";
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
pub(crate) const APP_NOT_VGM_BODY: &str = "Only a VGM file stores loop points. Convert the song to VGM first \
                                    (File > Convert > Convert to VGM).";
pub(crate) const APP_SONGS_EXPORTED_TITLE: &str = "Songs exported";
pub(crate) const APP_FIX_HEADER_TITLE: &str = "Fix Header";
pub(crate) const APP_AUDIT_HEADER_INTRO: &str =
    "This file's header disagrees with its own music:\n\n";
pub(crate) const APP_AUDIT_HEADER_OUTRO: &str = "\nCorrect them? The stream is taken as the truth.";

pub(crate) fn app_about_text(
    version: impl std::fmt::Display,
    credits: impl std::fmt::Display,
    optimize_credit: impl std::fmt::Display,
) -> String {
    format!(
        "VGM Studio v{}\n\
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
         Emulator cores in this build:\n\
         {}\
         {}\n\
         RetroWave OPL3 output links the serialport crate, used under\n\
         the MPL-2.0. Its source: https://github.com/serialport/serialport-rs",
        version, credits, optimize_credit,
    )
}

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

pub(crate) fn app_status_loop_candidates(count: impl std::fmt::Display) -> String {
    format!("Found {count} loop candidate(s).")
}

pub(crate) fn app_status_scanned_volumes(count: impl std::fmt::Display) -> String {
    format!("Scanned {count} track volume(s).")
}

pub(crate) fn app_status_optimizing(name: &str) -> String {
    format!("Optimizing {name}...")
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
    format!("Loop saved: {start} - {end}. Other players loop the whole tail until it is trimmed.")
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
pub(crate) const PACK_APPLY_TIP_SCANNED: &str = "Write volume modifiers to each track";
pub(crate) const PACK_APPLY_TIP_UNSCANNED: &str =
    "Scan volumes first -- there is no peak to level from yet";
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
pub(crate) const PACK_READINESS_TIP_NOTE: &str = "Worth a look, but nothing here blocks an export";
pub(crate) const PACK_VIEW_CHECKLIST_TIP: &str = "Open the submission checklist";
pub(crate) const PACK_EXPORT_ZIP_TIP: &str =
    "Build the submission zip (songs, screenshot, description, playlist)";
pub(crate) const PACK_SAVE_DOCS_TIP: &str = "Write Game Name.txt and Game Name.m3u into the folder";
pub(crate) const PACK_SAVE_ARCHIVE_TIP: &str =
    "Save this zip pack: re-export the archive (optimized, gzipped) back to a .zip";
pub(crate) const PACK_OPT_TIP: &str =
    "Strip redundant register writes from each VGM before packing (vgm_cmp-style)";
pub(crate) const PACK_VGZ_TIP: &str = "Gzip each .vgm to .vgz on export -- the VGMRips convention";
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
pub(crate) const SETTINGS_RESAMPLING_HOVER: &str = "How non-OPL chips are resampled. Band-limited is accurate; linear is \
     aliased but crunchy, like VGMPlay.";
pub(crate) const SETTINGS_FREQUENCY_HOVER: &str = "49716 Hz is the OPL3's native rate";
pub(crate) const SETTINGS_BUFFER_SIZE_HOVER: &str = "Frames per audio callback. Smaller seeks and mutes sooner; larger \
     avoids dropouts.";
pub(crate) const SETTINGS_BIT_DEPTH_HOVER: &str = "WAV export only";
pub(crate) const SETTINGS_TAIL_LENGTH_HOVER: &str =
    "How much the \"play last X seconds\" button plays";
pub(crate) const SETTINGS_THEME_HOVER: &str = "The case colour.";
pub(crate) const SETTINGS_PAD_STYLE_HOVER: &str = "The keycap colour.";
pub(crate) const SETTINGS_DECK_STYLE_HOVER: &str = "The panel the pads sit on.";
pub(crate) const SETTINGS_INVALID_TITLE: &str = "Invalid settings";
pub(crate) const SETTINGS_INVALID_NUMBERS: &str = "Check that the entered values are numbers.";

// ============================================================================
// dialogs/split.rs
// ============================================================================

pub(crate) const SPLIT_WAV_ONLY: &str = "Each chip channel is rendered to its own WAV.";
pub(crate) const SPLIT_WRITE_EACH_AS: &str = "Write each channel as:";
pub(crate) const SPLIT_AUDIO_HOVER: &str = "Render each channel on its own";
pub(crate) const SPLIT_SONG_HOVER: &str = "Re-record each channel in the song's own format";
pub(crate) const SPLIT_ISOLATE_PERCUSSION_HOVER: &str =
    "Splits the percussion channel per drum, not as one";
pub(crate) const SPLIT_SKIPPED_NOTE: &str =
    "Silent channels are skipped; existing files are overwritten.";

// ============================================================================
// dialogs/split_songs.rs
// ============================================================================

pub(crate) const SPLIT_SONGS_PREVIEW_UNAVAILABLE: &str =
    "No core for this file's chips yet -- can't audition, but export still works.";
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
pub(crate) const FIND_LOOP_QUALITY_IDEAL: &str =
    "ends at the song's end and is a clean repeat -- the ideal loop";
pub(crate) const FIND_LOOP_QUALITY_TO_END: &str = "the repeat runs to the end of the song";
pub(crate) const FIND_LOOP_QUALITY_CLEAN: &str = "a clean repeat, but not at the song's end";
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
pub(crate) const VGM_METADATA_INVALID_TITLE: &str = "Invalid VGM metadata";
pub(crate) const VGM_METADATA_LOOP_END_AFTER_START: &str =
    "Loop end must come after the loop start.";
pub(crate) const VGM_METADATA_UPDATE_ERROR: &str =
    "Error updating VGM metadata, check that the entered values are correct.";

pub(crate) fn vgm_metadata_loop_start_message(song_len: usize) -> String {
    format!("Loop start must be an instruction index below {song_len}.")
}
pub(crate) fn vgm_metadata_loop_end_message(song_len: usize) -> String {
    format!(
        "Loop end must be an instruction index of {song_len} or less, or empty for the end of the song."
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

pub(crate) const UNWALKABLE_VGM_BODY: &str = "Can't read this file's command stream, so there are no rows. Open the folder as a pack to edit its tags.";
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
// widgets/channels.rs
// ============================================================================

pub(crate) const CHANNELS_UNMUTE_ALL: &str =
    "Unmute every channel and drum (panning is left alone)";
pub(crate) const CHANNELS_ORIGINAL_DUAL_OPL2: &str = "Original: chip 1 left, chip 2 right";
pub(crate) const CHANNELS_ORIGINAL_OPL3: &str = "Original: the song's own panning";
pub(crate) const CHANNELS_ORIGINAL_MONO: &str = "Original: mono";
pub(crate) const CHANNELS_SPREAD: &str = "Stereo spread: mono at centre, wide at the extremes";
pub(crate) const CHANNELS_RESET: &str = "Reset panning to this song type's default (Original mode)";
pub(crate) const CHANNELS_PERCUSSION_LOW: &str = "Percussion (low bank)";
pub(crate) const CHANNELS_PERCUSSION_HIGH: &str = "Percussion (high bank)";

pub(crate) fn channels_pan_label(channel: usize, bank_name: &str) -> String {
    format!("Pan {} ({bank_name} bank)", channel + 1)
}
pub(crate) fn channels_channel_hover(index: usize) -> String {
    format!(
        "Channel {} ({} bank). Left-click mutes, right-click solos.",
        index % 9 + 1,
        if index < 9 { "low" } else { "high" },
    )
}
pub(crate) fn channels_percussion_hover(hover: &str) -> String {
    format!(
        "{hover}. Drums sound through channels 7-9's pans. \
         Left-click mutes, right-click solos."
    )
}

// ============================================================================
// widgets/chip_channels.rs
// ============================================================================

pub(crate) const CHIP_CHANNELS_CUSTOM: &str =
    "Custom: the pan knobs drive the output. Original: the chip's own image.";
pub(crate) const CHIP_CHANNELS_UNMUTE_ALL: &str = "Unmute every channel (panning is left alone)";
pub(crate) const CHIP_CHANNELS_RESET: &str = "Reset panning to centred (Original mode: the chip's own image)";

pub(crate) const CHIP_CHANNELS_MUTE_UNAVAILABLE: &str = "This chip's core can't mute individual channels. Pick the libvgm core for it in Settings > Output to enable muting.";

pub(crate) const CHIP_PANELS_MUTE_CHIP: &str =
    "Mute this whole chip. Works with every core; the channel toggles keep their pattern.";
pub(crate) const CHIP_PANELS_SOLO_CHIP: &str =
    "Solo this chip: mute every other chip in the file. Press again to bring them back.";

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
pub(crate) const BOOST_STEPPER_AT_LIMIT: &str =
    "At this song's clipping limit -- lower the volume to go quieter";
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

// ============================================================================
// widgets/waveform.rs
// ============================================================================

pub(crate) fn waveform_hover(ms: u32) -> String {
    format!("{ms} ms ({})", ms_to_timestr(ms))
}

// ============================================================================
// widgets/chip_output.rs
// ============================================================================

pub(crate) const CHIP_OUTPUT_NO_CORE: &str = "no core yet";

// ============================================================================
// widgets/position_panel.rs
// ============================================================================

pub(crate) fn position_panel_loop_progress(iteration: u32, count: LoopCount) -> String {
    match count {
        LoopCount::Infinite => format!("Loop {}", iteration + 1),
        LoopCount::Times(times) => format!("Loop {} / {}", iteration + 1, times.max(1)),
    }
}
