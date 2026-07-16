//! The native `FileService`: rfd dialogs + `std::fs`.
//!
//! The dialogs block the UI thread, exactly as wx's modal dialogs did; the
//! result is stashed and handed over on the next poll.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use dro_ui::{FileService, PickedFile, PickedFolder, SaveOutcome, SaveRequest};

/// The wx wildcard string, as rfd filters.
const FILTERS: [(&str, &[&str]); 4] = [
    ("DRO, VGM (*.dro;*.vgm;*.vgz)", &["dro", "vgm", "vgz"]),
    ("DRO files (*.dro)", &["dro"]),
    ("VGM files (*.vgm;*.vgz)", &["vgm", "vgz"]),
    ("All Files", &["*"]),
];

/// Extensions a rip project folder scan keeps, lower-cased.
const RIP_EXTENSIONS: [&str; 4] = ["vgm", "vgz", "png", "txt"];

#[derive(Debug, Default)]
pub struct NativeFileService {
    picked: Option<Result<PickedFile, String>>,
    /// One outcome per `save`, oldest first: rip mode saves the description and
    /// the playlist back to back and correlates the outcomes by this order.
    saved: VecDeque<SaveOutcome>,
    folder: Option<Result<PickedFolder, String>>,
    renamed: Option<Result<(), String>>,
}

impl NativeFileService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `path`. A directory is scanned as a rip project folder instead --
    /// this is what makes dropping a folder (or `drotrim <folder>`) open rip
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
            .set_title("Open rip project folder")
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
}

/// Reads a folder's rip-relevant files (non-recursive), sorted by lower-cased
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
                RIP_EXTENSIONS
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
            Err(error) => return Err(format!("{}: {error}", file_path.display())),
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
    if dest.exists() {
        return Err(format!("{to_name} already exists"));
    }
    fs::rename(from, &dest).map_err(|error| format!("{}: {error}", from.display()))
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
        _ => &FILTERS,
    }
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
}
