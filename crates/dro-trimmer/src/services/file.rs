//! The native `FileService`: rfd dialogs + `std::fs`.
//!
//! The dialogs block the UI thread; the result is stashed and handed over on
//! the next poll.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use dro_ui::{FileService, PickedFile, PickedFolder, SaveOutcome, SaveRequest};

/// The file-type wildcard string, as rfd filters.
const FILTERS: [(&str, &[&str]); 4] = [
    ("DRO, VGM (*.dro;*.vgm;*.vgz)", &["dro", "vgm", "vgz"]),
    ("DRO files (*.dro)", &["dro"]),
    ("VGM files (*.vgm;*.vgz)", &["vgm", "vgz"]),
    ("All Files", &["*"]),
];

/// Extensions a pack project folder scan keeps, lower-cased.
const PACK_EXTENSIONS: [&str; 4] = ["vgm", "vgz", "png", "txt"];

#[derive(Debug, Default)]
pub struct NativeFileService {
    picked: Option<Result<PickedFile, String>>,
    /// A screenshot picked for the open pack, on its own channel so it is never
    /// mistaken for a song to open.
    picked_image: Option<Result<PickedFile, String>>,
    deleted: Option<Result<(), String>>,
    /// One outcome per `save`, oldest first: pack mode saves the description and
    /// the playlist back to back and correlates the outcomes by this order.
    saved: VecDeque<SaveOutcome>,
    folder: Option<Result<PickedFolder, String>>,
    renamed: Option<Result<(), String>>,
    /// Where a split should write, once chosen. The inner `None` is a dismissed
    /// picker, which the app reports rather than treating as an error.
    output_folder: Option<Option<PathBuf>>,
}

impl NativeFileService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `path`. A directory is scanned as a pack project folder instead --
    /// this is what makes dropping a folder (or `drotrim <folder>`) open pack
    /// mode, without `dro-ui` ever touching the filesystem.
    fn read(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.folder = Some(scan_folder(&path));
            return;
        }
        self.picked = Some(match fs::read(&path) {
            Ok(bytes) => Ok(PickedFile {
                name: file_name(&path),
                path: Some(path),
                bytes,
            }),
            Err(error) => Err(format!("{}: {error}", path.display())),
        });
    }
}

impl FileService for NativeFileService {
    fn pick_open(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Open file");
        for (name, extensions) in FILTERS {
            dialog = dialog.add_filter(name, extensions);
        }
        if let Some(path) = dialog.pick_file() {
            self.read(path);
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        self.read(path);
    }

    fn poll_picked(&mut self) -> Option<Result<PickedFile, String>> {
        self.picked.take()
    }

    fn pick_image(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Add screenshot")
            .add_filter("PNG images (*.png)", &["png"])
            .pick_file()
        else {
            return; // dismissed; nothing to report
        };
        self.picked_image = Some(match fs::read(&path) {
            Ok(bytes) => Ok(PickedFile {
                name: file_name(&path),
                path: Some(path),
                bytes,
            }),
            Err(error) => Err(format!("{}: {error}", path.display())),
        });
    }

    fn poll_picked_image(&mut self) -> Option<Result<PickedFile, String>> {
        self.picked_image.take()
    }

    fn delete(&mut self, path: PathBuf) {
        self.deleted = Some(match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        });
    }

    fn poll_deleted(&mut self) -> Option<Result<(), String>> {
        self.deleted.take()
    }

    fn save(&mut self, request: SaveRequest) {
        let outcome = match request {
            SaveRequest::InPlace { path, bytes } => write_outcome(path, &bytes),
            SaveRequest::Dialog {
                suggested_name,
                bytes,
            } => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save file")
                    .set_file_name(&suggested_name);
                for &(name, extensions) in save_filters(&suggested_name) {
                    dialog = dialog.add_filter(name, extensions);
                }
                match dialog.save_file() {
                    Some(path) if changes_song_format(&suggested_name, &path) => {
                        SaveOutcome::Failed(
                            "Save As can't change the file format. Use Convert to VGM instead."
                                .to_owned(),
                        )
                    }
                    Some(path) => write_outcome(path, &bytes),
                    None => SaveOutcome::Cancelled,
                }
            }
        };
        self.saved.push_back(outcome);
    }

    fn poll_saved(&mut self) -> Option<SaveOutcome> {
        self.saved.pop_front()
    }

    fn pick_folder(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open pack project folder")
            .pick_folder()
        {
            self.folder = Some(scan_folder(&path));
        }
    }

    fn open_folder_path(&mut self, path: PathBuf) {
        self.folder = Some(scan_folder(&path));
    }

    fn poll_folder(&mut self) -> Option<Result<PickedFolder, String>> {
        self.folder.take()
    }

    fn rename(&mut self, from: PathBuf, to_name: String) {
        self.renamed = Some(rename_in_place(&from, &to_name));
    }

    fn poll_renamed(&mut self) -> Option<Result<(), String>> {
        self.renamed.take()
    }

    fn pick_output_folder(&mut self) {
        // Unlike `pick_folder`, nothing is read: the split only needs somewhere
        // to put its files.
        self.output_folder = Some(
            rfd::FileDialog::new()
                .set_title("Choose where to write the split files")
                .pick_folder(),
        );
    }

    fn poll_output_folder(&mut self) -> Option<Option<PathBuf>> {
        self.output_folder.take()
    }
}

/// Reads a folder's pack-relevant files (non-recursive), sorted by lower-cased
/// name. An empty result is still `Ok`; the UI validates the contents.
fn scan_folder(path: &Path) -> Result<PickedFolder, String> {
    let entries = fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut files: Vec<PickedFile> = Vec::new();
    for entry in entries.flatten() {
        let file_path = entry.path();
        if !file_path.is_file() {
            continue;
        }
        let is_relevant = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                PACK_EXTENSIONS
                    .iter()
                    .any(|want| ext.eq_ignore_ascii_case(want))
            });
        if !is_relevant {
            continue;
        }
        match fs::read(&file_path) {
            Ok(bytes) => files.push(PickedFile {
                name: file_name(&file_path),
                path: Some(file_path),
                bytes,
            }),
            // One unreadable file must not abort the whole folder open -- skip it
            // with a warning, the way a song that fails to parse becomes an
            // "unreadable" row rather than an error (ux-19).
            Err(error) => log::warn!("skipping unreadable {}: {error}", file_path.display()),
        }
    }
    files.sort_by_key(|file| file.name.to_lowercase());
    Ok(PickedFolder {
        name: folder_name(path),
        path: Some(path.to_path_buf()),
        files,
    })
}

/// Renames `from` to the bare `to_name` in the same directory, refusing to
/// clobber an existing file (`std::fs::rename` would replace it silently on
/// Windows).
fn rename_in_place(from: &Path, to_name: &str) -> Result<(), String> {
    let parent = from.parent().unwrap_or_else(|| Path::new(""));
    let dest = parent.join(to_name);
    if dest == from {
        return Ok(());
    }
    // A case-only rename ("01 intro" -> "01 Intro"): on NTFS `dest.exists()` is
    // true (it *is* this file) and a direct rename won't update the stored case,
    // so it must bounce through a temp name -- but it isn't a clobber.
    let same_file_case_only = from
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(to_name));
    if same_file_case_only {
        return rename_via_temp(from, &dest);
    }
    if dest.exists() {
        return Err(format!("{to_name} already exists"));
    }
    fs::rename(from, &dest).map_err(|error| format!("{}: {error}", from.display()))
}

/// Renames `from` to `dest` via a throwaway intermediate name in the same
/// directory, so a case-only rename actually updates the on-disk case (which a
/// direct `fs::rename` won't on a case-insensitive volume).
fn rename_via_temp(from: &Path, dest: &Path) -> Result<(), String> {
    let parent = from.parent().unwrap_or_else(|| Path::new(""));
    let base = dest.file_name().and_then(|n| n.to_str()).unwrap_or("track");
    let mut temp = parent.join(format!("{base}.rename-tmp"));
    let mut counter = 0u32;
    while temp.exists() {
        counter += 1;
        temp = parent.join(format!("{base}.rename-tmp{counter}"));
    }
    fs::rename(from, &temp).map_err(|error| format!("{}: {error}", from.display()))?;
    fs::rename(&temp, dest).map_err(|error| {
        // Best effort: undo the first leg so the file isn't stranded under the
        // temp name if the second fails.
        let _ = fs::rename(&temp, from);
        format!("{}: {error}", dest.display())
    })
}

/// Save-dialog filters chosen from the suggested name's extension, so the zip
/// export and the description/playlist saves do not offer DRO/VGM filters.
fn save_filters(suggested_name: &str) -> &'static [(&'static str, &'static [&'static str])] {
    let extension = Path::new(suggested_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("zip") => &[("Zip archive (*.zip)", &["zip"])],
        Some("txt") => &[("Text file (*.txt)", &["txt"])],
        Some("m3u") => &[("Playlist (*.m3u)", &["m3u"])],
        Some("wav") => &[("WAV audio (*.wav)", &["wav"])],
        // A song is offered only its own format, so Save As cannot pick an
        // extension the already-serialised bytes don't match (M5/ux-2).
        Some("dro") => &[("DRO files (*.dro)", &["dro"])],
        Some("vgm" | "vgz") => &[("VGM files (*.vgm;*.vgz)", &["vgm", "vgz"])],
        _ => &FILTERS,
    }
}

/// The song-format class of an extension, for the Save As guard: `vgm` and `vgz`
/// are one format (they differ only in compression), `dro` another. `None` for
/// anything that isn't a song.
fn song_format_class(extension: Option<&str>) -> Option<&'static str> {
    match extension {
        Some(ext) if ext.eq_ignore_ascii_case("dro") => Some("dro"),
        Some(ext) if ext.eq_ignore_ascii_case("vgm") || ext.eq_ignore_ascii_case("vgz") => {
            Some("vgm")
        }
        _ => None,
    }
}

/// Whether saving under `chosen` would change a song's format away from what its
/// own `suggested` name implies -- e.g. DRO bytes written to a `.vgm`, which the
/// app then can't reopen. Fires only when both are recognised song formats, so a
/// `.zip`/`.txt` (or an extension-less) target never trips it.
fn changes_song_format(suggested: &str, chosen: &Path) -> bool {
    let suggested = song_format_class(Path::new(suggested).extension().and_then(|e| e.to_str()));
    let chosen = song_format_class(chosen.extension().and_then(|e| e.to_str()));
    matches!((suggested, chosen), (Some(s), Some(c)) if s != c)
}

fn write_outcome(path: PathBuf, bytes: &[u8]) -> SaveOutcome {
    match fs::write(&path, bytes) {
        Ok(()) => SaveOutcome::Saved {
            name: file_name(&path),
            path: Some(path),
        },
        Err(error) => SaveOutcome::Failed(format!("{}: {error}", path.display())),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// A unique temp directory under the OS temp root, created fresh.
    fn temp_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("drotrim-file-test-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn folder_scan_keeps_relevant_files_sorted_and_read() {
        let dir = temp_dir("scan");
        write_file(&dir, "02 Second.vgz", b"two");
        write_file(&dir, "01 First.vgm", b"one");
        write_file(&dir, "Game.txt", b"desc");
        write_file(&dir, "Game.png", b"img");
        write_file(&dir, "notes.md", b"ignored"); // wrong extension
        fs::create_dir(dir.join("subdir")).unwrap(); // directories skipped

        let mut service = NativeFileService::new();
        service.open_folder_path(dir.clone());
        let folder = service.poll_folder().unwrap().unwrap();

        let names: Vec<&str> = folder.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["01 First.vgm", "02 Second.vgz", "Game.png", "Game.txt"]
        );
        assert_eq!(folder.files[0].bytes, b"one");
        assert_eq!(folder.path.as_deref(), Some(dir.as_path()));
        assert!(service.poll_folder().is_none(), "the slot is drained once");
    }

    #[test]
    fn open_path_on_a_directory_scans_it_as_a_folder() {
        let dir = temp_dir("dir-route");
        write_file(&dir, "01 Song.vgz", b"x");
        let mut service = NativeFileService::new();
        service.open_path(dir.clone());
        assert!(
            service.poll_picked().is_none(),
            "a directory is not a picked file"
        );
        assert!(
            service.poll_folder().unwrap().is_ok(),
            "it routes to the folder slot"
        );
    }

    #[test]
    fn saves_deliver_one_outcome_each_in_order() {
        let dir = temp_dir("save-order");
        let mut service = NativeFileService::new();
        service.save(SaveRequest::InPlace {
            path: dir.join("a.txt"),
            bytes: b"a".to_vec(),
        });
        service.save(SaveRequest::InPlace {
            path: dir.join("b.m3u"),
            bytes: b"b".to_vec(),
        });
        match service.poll_saved().unwrap() {
            SaveOutcome::Saved { name, .. } => assert_eq!(name, "a.txt"),
            other => panic!("expected a.txt saved, got {other:?}"),
        }
        match service.poll_saved().unwrap() {
            SaveOutcome::Saved { name, .. } => assert_eq!(name, "b.m3u"),
            other => panic!("expected b.m3u saved, got {other:?}"),
        }
        assert!(service.poll_saved().is_none());
        assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"a");
    }

    #[test]
    fn rename_moves_the_file_but_refuses_to_clobber() {
        let dir = temp_dir("rename");
        let from = write_file(&dir, "01 Old.vgz", b"song");
        let mut service = NativeFileService::new();

        service.rename(from.clone(), "01 New.vgz".to_owned());
        assert!(service.poll_renamed().unwrap().is_ok());
        assert!(dir.join("01 New.vgz").exists());
        assert!(!from.exists());

        // A second file, then a rename onto it, must fail rather than overwrite.
        let other = write_file(&dir, "02 Other.vgz", b"other");
        service.rename(dir.join("01 New.vgz"), "02 Other.vgz".to_owned());
        assert!(service.poll_renamed().unwrap().is_err());
        assert_eq!(
            fs::read(&other).unwrap(),
            b"other",
            "the target is untouched"
        );
    }

    #[test]
    fn a_case_only_rename_updates_the_on_disk_case() {
        // M1/ux-9: "01 intro" -> "01 Intro" must succeed, not fail as a clobber.
        let dir = temp_dir("rename-case");
        let from = write_file(&dir, "01 intro.vgz", b"song");
        let mut service = NativeFileService::new();

        service.rename(from, "01 Intro.vgz".to_owned());
        assert!(
            service.poll_renamed().unwrap().is_ok(),
            "a case-only rename succeeds"
        );

        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.contains(&"01 Intro.vgz".to_owned()),
            "the on-disk name took the new case, got {names:?}"
        );
        assert_eq!(fs::read(dir.join("01 Intro.vgz")).unwrap(), b"song");
    }

    #[cfg(windows)]
    #[test]
    fn an_unreadable_file_is_skipped_not_fatal() {
        // ux-19: one unreadable file must not abort the whole folder scan.
        use std::os::windows::fs::OpenOptionsExt as _;
        let dir = temp_dir("scan-unreadable");
        write_file(&dir, "01 Good.vgz", b"good");
        let locked = write_file(&dir, "02 Locked.vgz", b"locked");
        // Hold the file with no sharing, so scan_folder's fs::read fails on it.
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();

        let folder = scan_folder(&dir).expect("the folder still scans");
        let names: Vec<&str> = folder.files.iter().map(|file| file.name.as_str()).collect();
        assert!(
            names.contains(&"01 Good.vgz"),
            "the readable file is kept, got {names:?}"
        );
        assert!(
            !names.contains(&"02 Locked.vgz"),
            "the locked file is skipped, not fatal"
        );
    }

    #[test]
    fn save_filters_narrow_a_song_to_its_own_format() {
        assert_eq!(save_filters("song.dro")[0].1.to_vec(), ["dro"]);
        assert_eq!(
            save_filters("song.dro").len(),
            1,
            "no combined DRO/VGM filter"
        );
        assert_eq!(save_filters("song.vgm")[0].1.to_vec(), ["vgm", "vgz"]);
        assert_eq!(save_filters("song.vgz")[0].1.to_vec(), ["vgm", "vgz"]);
        assert_eq!(save_filters("Game.zip")[0].1.to_vec(), ["zip"]);
        // A rendered WAV is not a song, and must not be offered song filters.
        assert_eq!(save_filters("song.dro.wav")[0].1.to_vec(), ["wav"]);
    }

    /// The Save As format guard must not fire on a rendered WAV: `.wav` is not a
    /// song format, so saving one under any name is fine.
    #[test]
    fn a_rendered_wav_is_never_a_format_change() {
        assert!(!changes_song_format("song.dro.wav", Path::new("mix.wav")));
        assert!(!changes_song_format("song.dro.wav", Path::new("mix.dro")));
    }

    #[test]
    fn changes_song_format_flags_cross_format_saves_only() {
        assert!(changes_song_format("song.dro", Path::new("song.vgm")));
        assert!(changes_song_format("song.vgm", Path::new("song.dro")));
        assert!(
            !changes_song_format("song.vgm", Path::new("song.vgz")),
            "vgm<->vgz is the same format (compression only)"
        );
        assert!(!changes_song_format("song.dro", Path::new("song.dro")));
        assert!(
            !changes_song_format("song.dro", Path::new("song")),
            "an unrecognised target extension is not a format change"
        );
    }
}
