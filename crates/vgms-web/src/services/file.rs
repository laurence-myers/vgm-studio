// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebFileService`]: opening files with a hidden `<input type=file>`, and
//! saving them as browser downloads.
//!
//! Every asynchronous result lands in a shared slot that the trait's `poll_*`
//! methods drain -- the same polled-never-awaited shape the native service has,
//! which is why the app's update loop needs no web-specific code. Pack folder
//! operations (folder/rename/image) are not reachable in the browser until the
//! File System Access and zip-pack backends land, so they answer the honest
//! "not available" error through the existing channel rather than pretending.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use vgms_ui::platform::{FileService, PickedFile, PickedFolder, SaveOutcome, SaveRequest};

/// The message the folder/rename channels answer with until pack backends exist.
const NO_PACKS_YET: &str = "Pack folders are not available in this browser yet.";

type Picked = Rc<RefCell<Option<Result<PickedFile, String>>>>;

/// Opens and saves files in the browser.
pub struct WebFileService {
    picked: Picked,
    saved: Rc<RefCell<VecDeque<SaveOutcome>>>,
    folder: Rc<RefCell<Option<Result<PickedFolder, String>>>>,
    renamed: Rc<RefCell<Option<Result<(), String>>>>,
    notify: Rc<dyn Fn()>,
}

impl std::fmt::Debug for WebFileService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebFileService").finish_non_exhaustive()
    }
}

impl WebFileService {
    /// Builds the service. `notify` is called when an async result lands, so the
    /// egui loop repaints and polls it up (the browser has no frame clock of its
    /// own while idle).
    pub fn new(notify: impl Fn() + 'static) -> Self {
        Self {
            picked: Rc::new(RefCell::new(None)),
            saved: Rc::new(RefCell::new(VecDeque::new())),
            folder: Rc::new(RefCell::new(None)),
            renamed: Rc::new(RefCell::new(None)),
            notify: Rc::new(notify),
        }
    }
}

/// Opens a file chooser and, once a file is picked, reads its bytes and drops a
/// [`PickedFile`] into `slot`. `accept` is the input's filter, e.g. `.dro,.vgm`.
fn open_picker(accept: &str, slot: Picked, notify: Rc<dyn Fn()>) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        *slot.borrow_mut() = Some(Err("no document to open a file picker in".to_owned()));
        notify();
        return;
    };
    let input = match document.create_element("input").and_then(|element| {
        element
            .dyn_into::<web_sys::HtmlInputElement>()
            .map_err(Into::into)
    }) {
        Ok(input) => input,
        Err(_) => {
            *slot.borrow_mut() = Some(Err("could not create a file input".to_owned()));
            notify();
            return;
        }
    };
    input.set_type("file");
    input.set_accept(accept);

    // On change, read the first file's bytes through a FileReader and post the
    // result into the slot. The closures outlive this function -- the picker's
    // whole point -- so they are `forget()`-leaked; a file open is a rare,
    // user-driven event, so the handful of bytes leaked per open never matters.
    let on_change = Closure::<dyn FnMut()>::new({
        let input = input.clone();
        move || {
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let name = file.name();
            let reader = match web_sys::FileReader::new() {
                Ok(reader) => reader,
                Err(_) => {
                    *slot.borrow_mut() = Some(Err("could not read the file".to_owned()));
                    notify();
                    return;
                }
            };
            let on_load = Closure::<dyn FnMut()>::new({
                let reader = reader.clone();
                let slot = Rc::clone(&slot);
                let notify = Rc::clone(&notify);
                let name = name.clone();
                move || {
                    let result = reader
                        .result()
                        .ok()
                        .map(|buffer| js_sys::Uint8Array::new(&buffer).to_vec());
                    *slot.borrow_mut() = Some(match result {
                        Some(bytes) => Ok(PickedFile {
                            name: name.clone(),
                            path: None,
                            bytes,
                        }),
                        None => Err("could not read the file bytes".to_owned()),
                    });
                    notify();
                }
            });
            reader.set_onloadend(Some(on_load.as_ref().unchecked_ref()));
            let _ = reader.read_as_array_buffer(&file);
            on_load.forget();
        }
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    input.click();
}

/// Triggers a browser download of `bytes` under `name`, via a temporary object
/// URL on a synthetic `<a download>`.
fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)
        .map_err(|_| "could not build the download blob".to_owned())?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| "could not create the download URL".to_owned())?;
    let anchor = document
        .create_element("a")
        .and_then(|element| {
            element
                .dyn_into::<web_sys::HtmlAnchorElement>()
                .map_err(Into::into)
        })
        .map_err(|_| "could not create the download link".to_owned())?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

impl FileService for WebFileService {
    fn pick_open(&mut self) {
        open_picker(
            ".dro,.vgm,.vgz,.zip",
            Rc::clone(&self.picked),
            Rc::clone(&self.notify),
        );
    }

    fn open_path(&mut self, _path: PathBuf) {
        // The browser has no paths: drag-and-drop delivers bytes, which the app
        // routes without this call. Reached only by an impossible code path, so
        // it does nothing rather than inventing a file.
    }

    fn poll_picked(&mut self) -> Option<Result<PickedFile, String>> {
        self.picked.borrow_mut().take()
    }

    fn save(&mut self, request: SaveRequest) {
        let (name, bytes) = match request {
            // The web has no in-place path, but honour one if the app ever makes
            // it: download under the path's file name.
            SaveRequest::InPlace { path, bytes } => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("download")
                    .to_owned();
                (name, bytes)
            }
            SaveRequest::Dialog {
                suggested_name,
                bytes,
            } => (suggested_name, bytes),
        };
        // A download cannot be cancelled or reported as failed by the browser, so
        // the outcome is always `Saved` (with no path -- there is nowhere to save
        // back to). A blob-build failure is the one thing we can see.
        let outcome = match download(&name, &bytes) {
            Ok(()) => SaveOutcome::Saved { name, path: None },
            Err(message) => SaveOutcome::Failed(message),
        };
        self.saved.borrow_mut().push_back(outcome);
        (self.notify)();
    }

    fn poll_saved(&mut self) -> Option<SaveOutcome> {
        self.saved.borrow_mut().pop_front()
    }

    fn pick_folder(&mut self) {
        *self.folder.borrow_mut() = Some(Err(NO_PACKS_YET.to_owned()));
        (self.notify)();
    }

    fn open_folder_path(&mut self, _path: PathBuf) {
        *self.folder.borrow_mut() = Some(Err(NO_PACKS_YET.to_owned()));
        (self.notify)();
    }

    fn poll_folder(&mut self) -> Option<Result<PickedFolder, String>> {
        self.folder.borrow_mut().take()
    }

    fn rename(&mut self, _from: PathBuf, _to_name: String) {
        *self.renamed.borrow_mut() = Some(Err(NO_PACKS_YET.to_owned()));
        (self.notify)();
    }

    fn poll_renamed(&mut self) -> Option<Result<(), String>> {
        self.renamed.borrow_mut().take()
    }
}
