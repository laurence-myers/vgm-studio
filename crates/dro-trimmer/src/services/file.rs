//! The native `FileService`: rfd dialogs + `std::fs`.
//!
//! The dialogs block the UI thread, exactly as wx's modal dialogs did; the
//! result is stashed and handed over on the next poll.

use std::fs;
use std::path::{Path, PathBuf};

use dro_ui::{FileService, PickedFile, SaveOutcome, SaveRequest};

/// The wx wildcard string, as rfd filters.
const FILTERS: [(&str, &[&str]); 4] = [
    ("DRO, VGM (*.dro;*.vgm;*.vgz)", &["dro", "vgm", "vgz"]),
    ("DRO files (*.dro)", &["dro"]),
    ("VGM files (*.vgm;*.vgz)", &["vgm", "vgz"]),
    ("All Files", &["*"]),
];

#[derive(Debug, Default)]
pub struct NativeFileService {
    picked: Option<Result<PickedFile, String>>,
    saved: Option<SaveOutcome>,
}

impl NativeFileService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&mut self, path: PathBuf) {
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
        let mut dialog = rfd::FileDialog::new().set_title("Open DRO");
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
        self.saved = Some(match request {
            SaveRequest::InPlace { path, bytes } => write_outcome(path, &bytes),
            SaveRequest::Dialog {
                suggested_name,
                bytes,
            } => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Save DRO file")
                    .set_file_name(&suggested_name);
                for (name, extensions) in FILTERS {
                    dialog = dialog.add_filter(name, extensions);
                }
                match dialog.save_file() {
                    Some(path) => write_outcome(path, &bytes),
                    None => SaveOutcome::Cancelled,
                }
            }
        });
    }

    fn poll_saved(&mut self) -> Option<SaveOutcome> {
        self.saved.take()
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
