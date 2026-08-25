// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebFileService`]: opening files with a hidden `<input type=file>`, saving
//! them as browser downloads, and -- where the browser offers it -- reading and
//! writing pack folders through the File System Access API (wt-7).
//!
//! Every asynchronous result lands in a shared slot that the trait's `poll_*`
//! methods drain -- the same polled-never-awaited shape the native service has,
//! which is why the app's update loop needs no web-specific code.
//!
//! Pack folders stay `PathBuf` tokens: a picked directory is registered in the
//! JS side of `pack_fs.js` under an opaque name, and Rust round-trips it as a
//! virtual `/<token>` path. `folder.join(name)` / `path.parent()` therefore
//! reach the right directory handle with no change to the pack machinery above
//! the service boundary. Folder/save/rename/delete are async (the FSA is
//! Promise-based), so they run in `spawn_local` tasks that drop their outcome
//! into a slot; saves are serialised through one queue so their outcomes stay
//! strictly FIFO, which the pack executor and the doc-save pair rely on.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;

use vgms_ui::platform::{
    ArchiveBackend, FileService, PickedFile, PickedFolder, SaveOutcome, SaveRequest,
};

// The File System Access helpers. The directory handles live on the JS side,
// keyed by the token Rust passes back as a `/<token>` path. See `pack_fs.js`.
#[wasm_bindgen(module = "/pack_fs.js")]
extern "C" {
    #[wasm_bindgen(js_name = pickPackFolder, catch)]
    async fn pick_pack_folder_js() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = pickOutputFolder, catch)]
    async fn pick_output_folder_js() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = rescanPackFolder, catch)]
    async fn rescan_pack_folder_js(token: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = writePackFile, catch)]
    async fn write_pack_file_js(token: &str, name: &str, bytes: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = deletePackFile, catch)]
    async fn delete_pack_file_js(token: &str, name: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = renamePackFile, catch)]
    async fn rename_pack_file_js(token: &str, from: &str, to: &str) -> Result<(), JsValue>;
}

type Picked = Rc<RefCell<Option<Result<PickedFile, String>>>>;

/// A pending save, processed in submission order so outcomes stay FIFO.
enum SaveJob {
    /// A Save As / export: a browser download; there is nowhere to save back to.
    Download { name: String, bytes: Vec<u8> },
    /// An in-place write to a held pack folder (`token` + bare `name`).
    Write {
        token: String,
        name: String,
        path: PathBuf,
        bytes: Vec<u8>,
    },
}

/// Opens and saves files in the browser.
pub struct WebFileService {
    picked: Picked,
    picked_image: Picked,
    saved: Rc<RefCell<VecDeque<SaveOutcome>>>,
    save_queue: Rc<RefCell<VecDeque<SaveJob>>>,
    save_busy: Rc<Cell<bool>>,
    folder: Rc<RefCell<Option<Result<PickedFolder, String>>>>,
    renamed: Rc<RefCell<Option<Result<(), String>>>>,
    deleted: Rc<RefCell<Option<Result<(), String>>>>,
    output_folder: Rc<RefCell<Option<Option<PathBuf>>>>,
    /// The Render to WAV "destination": on the web just the download name,
    /// resolved at once (the write is a browser download).
    save_path: Rc<RefCell<Option<Option<PathBuf>>>>,
    /// Zip-opened packs (wt-8): their edits stay in memory here, and are served
    /// synchronously, ahead of the async File System Access path below.
    archives: ArchiveBackend,
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
            picked_image: Rc::new(RefCell::new(None)),
            saved: Rc::new(RefCell::new(VecDeque::new())),
            save_queue: Rc::new(RefCell::new(VecDeque::new())),
            save_busy: Rc::new(Cell::new(false)),
            folder: Rc::new(RefCell::new(None)),
            renamed: Rc::new(RefCell::new(None)),
            deleted: Rc::new(RefCell::new(None)),
            output_folder: Rc::new(RefCell::new(None)),
            save_path: Rc::new(RefCell::new(None)),
            archives: ArchiveBackend::new(),
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

/// Extracts a message from a thrown JS value, preferring an `Error.message`,
/// and falling back to the value's debug form.
fn js_error(value: JsValue) -> String {
    crate::js::message(&value).unwrap_or_else(|| format!("{value:?}"))
}

/// Reads a string property off a JS object.
fn get_string(object: &JsValue, key: &str) -> Result<String, String> {
    js_sys::Reflect::get(object, &JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_string())
        .ok_or_else(|| format!("pack folder result missing {key:?}"))
}

/// Turns the JS `{ token, name, files: [{ name, bytes }] }` into a [`PickedFolder`]
/// with virtual `/<token>` paths. `Ok(None)` means the picker was dismissed.
fn parse_folder(value: &JsValue) -> Result<Option<PickedFolder>, String> {
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    let token = get_string(value, "token")?;
    let name = get_string(value, "name")?;
    let token_path = PathBuf::from(format!("/{token}"));

    let files_value = js_sys::Reflect::get(value, &JsValue::from_str("files"))
        .map_err(|_| "pack folder result missing files".to_owned())?;
    let files_array: js_sys::Array = files_value
        .dyn_into()
        .map_err(|_| "pack folder files is not an array".to_owned())?;

    let mut files = Vec::with_capacity(files_array.length() as usize);
    for item in files_array.iter() {
        let file_name = get_string(&item, "name")?;
        let bytes_value = js_sys::Reflect::get(&item, &JsValue::from_str("bytes"))
            .map_err(|_| "pack file missing bytes".to_owned())?;
        let bytes = bytes_value
            .dyn_into::<js_sys::Uint8Array>()
            .map_err(|_| "pack file bytes is not a Uint8Array".to_owned())?
            .to_vec();
        files.push(PickedFile {
            name: file_name.clone(),
            path: Some(token_path.join(&file_name)),
            bytes,
        });
    }

    Ok(Some(PickedFolder {
        name,
        path: Some(token_path),
        files,
    }))
}

/// The `(token, bare-name)` a virtual `/<token>/<name>` path resolves to. Built
/// with `Path::parent`/`file_name` so it is the exact inverse of the
/// `folder.join(name)` the app builds paths with, whatever the wasm separator.
fn split_token(path: &Path) -> Option<(String, String)> {
    let name = path.file_name()?.to_str()?.to_owned();
    let token = path.parent()?.file_name()?.to_str()?.to_owned();
    Some((token, name))
}

/// Processes queued saves in order: downloads finish inline, writes are awaited
/// one at a time so their outcomes reach `saved` strictly FIFO.
fn pump_saves(
    queue: Rc<RefCell<VecDeque<SaveJob>>>,
    busy: Rc<Cell<bool>>,
    saved: Rc<RefCell<VecDeque<SaveOutcome>>>,
    notify: Rc<dyn Fn()>,
) {
    if busy.get() {
        return;
    }
    loop {
        let job = queue.borrow_mut().pop_front();
        let Some(job) = job else {
            return;
        };
        match job {
            SaveJob::Download { name, bytes } => {
                let outcome = match download(&name, &bytes) {
                    Ok(()) => SaveOutcome::Saved { name, path: None },
                    Err(message) => SaveOutcome::Failed(message),
                };
                saved.borrow_mut().push_back(outcome);
                notify();
            }
            SaveJob::Write {
                token,
                name,
                path,
                bytes,
            } => {
                busy.set(true);
                let queue = Rc::clone(&queue);
                let busy_inner = Rc::clone(&busy);
                let saved_inner = Rc::clone(&saved);
                let notify_inner = Rc::clone(&notify);
                spawn_local(async move {
                    let outcome = match write_pack_file_js(&token, &name, &bytes).await {
                        Ok(()) => SaveOutcome::Saved {
                            name,
                            path: Some(path),
                        },
                        Err(error) => SaveOutcome::Failed(js_error(error)),
                    };
                    saved_inner.borrow_mut().push_back(outcome);
                    busy_inner.set(false);
                    notify_inner();
                    pump_saves(queue, busy_inner, saved_inner, notify_inner);
                });
                return;
            }
        }
    }
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
        // A write to a zip-opened pack goes to the in-memory archive, synchronously.
        if let SaveRequest::InPlace { path, bytes } = &request
            && self.archives.holds_file(path)
        {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file")
                .to_owned();
            let outcome = match self.archives.write(path, bytes.clone()) {
                Ok(()) => SaveOutcome::Saved {
                    name,
                    path: Some(path.clone()),
                },
                Err(message) => SaveOutcome::Failed(message),
            };
            self.saved.borrow_mut().push_back(outcome);
            (self.notify)();
            return;
        }
        let job = match request {
            SaveRequest::InPlace { path, bytes } => {
                // An in-place save on the web is a write-back to a held pack /
                // split folder (its token is the virtual path's parent). If the
                // path carries no token -- it never should -- fall back to a
                // download so the bytes are not silently lost.
                match split_token(&path) {
                    Some((token, name)) => SaveJob::Write {
                        token,
                        name,
                        path,
                        bytes,
                    },
                    None => SaveJob::Download {
                        name: path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("download")
                            .to_owned(),
                        bytes,
                    },
                }
            }
            SaveRequest::Dialog {
                suggested_name,
                bytes,
            } => SaveJob::Download {
                name: suggested_name,
                bytes,
            },
        };
        self.save_queue.borrow_mut().push_back(job);
        pump_saves(
            Rc::clone(&self.save_queue),
            Rc::clone(&self.save_busy),
            Rc::clone(&self.saved),
            Rc::clone(&self.notify),
        );
    }

    fn poll_saved(&mut self) -> Option<SaveOutcome> {
        self.saved.borrow_mut().pop_front()
    }

    fn pick_image(&mut self) {
        open_picker(
            ".png,image/png",
            Rc::clone(&self.picked_image),
            Rc::clone(&self.notify),
        );
    }

    fn poll_picked_image(&mut self) -> Option<Result<PickedFile, String>> {
        self.picked_image.borrow_mut().take()
    }

    fn delete(&mut self, path: PathBuf) {
        if self.archives.holds_file(&path) {
            *self.deleted.borrow_mut() = Some(self.archives.delete(&path));
            (self.notify)();
            return;
        }
        let Some((token, name)) = split_token(&path) else {
            *self.deleted.borrow_mut() = Some(Err(format!("cannot delete {}", path.display())));
            (self.notify)();
            return;
        };
        let deleted = Rc::clone(&self.deleted);
        let notify = Rc::clone(&self.notify);
        spawn_local(async move {
            *deleted.borrow_mut() = Some(match delete_pack_file_js(&token, &name).await {
                Ok(()) => Ok(()),
                Err(error) => Err(js_error(error)),
            });
            notify();
        });
    }

    fn poll_deleted(&mut self) -> Option<Result<(), String>> {
        self.deleted.borrow_mut().take()
    }

    fn pick_pack_zip(&mut self) {
        // The picked-file channel, like `pick_open`: the app routes a `.zip`
        // there to the in-memory pack open, prompting first if the current pack
        // is dirty.
        open_picker(
            ".zip,application/zip",
            Rc::clone(&self.picked),
            Rc::clone(&self.notify),
        );
    }

    fn pick_folder(&mut self) {
        let folder = Rc::clone(&self.folder);
        let notify = Rc::clone(&self.notify);
        spawn_local(async move {
            match pick_pack_folder_js().await {
                Ok(value) => match parse_folder(&value) {
                    // Dismissed: leave the slot idle so the app simply does nothing.
                    Ok(None) => {}
                    Ok(Some(picked)) => *folder.borrow_mut() = Some(Ok(picked)),
                    Err(message) => *folder.borrow_mut() = Some(Err(message)),
                },
                Err(error) => *folder.borrow_mut() = Some(Err(js_error(error))),
            }
            notify();
        });
    }

    fn open_pack_archive(&mut self, name: String, bytes: Vec<u8>) {
        *self.folder.borrow_mut() = Some(self.archives.open(&name, &bytes));
        (self.notify)();
    }

    fn open_folder_path(&mut self, path: PathBuf) {
        // A zip pack rescan re-lists the in-memory archive; nothing async.
        if self.archives.holds_folder(&path) {
            *self.folder.borrow_mut() = self
                .archives
                .folder(&path)
                .map(Ok)
                .or_else(|| Some(Err(format!("lost the zip pack {}", path.display()))));
            (self.notify)();
            return;
        }
        let Some(token) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            *self.folder.borrow_mut() = Some(Err(format!("cannot reopen {}", path.display())));
            (self.notify)();
            return;
        };
        let folder = Rc::clone(&self.folder);
        let notify = Rc::clone(&self.notify);
        spawn_local(async move {
            match rescan_pack_folder_js(&token).await {
                Ok(value) => match parse_folder(&value) {
                    Ok(None) => {}
                    Ok(Some(picked)) => *folder.borrow_mut() = Some(Ok(picked)),
                    Err(message) => *folder.borrow_mut() = Some(Err(message)),
                },
                Err(error) => *folder.borrow_mut() = Some(Err(js_error(error))),
            }
            notify();
        });
    }

    fn poll_folder(&mut self) -> Option<Result<PickedFolder, String>> {
        self.folder.borrow_mut().take()
    }

    fn rename(&mut self, from: PathBuf, to_name: String) {
        if self.archives.holds_file(&from) {
            *self.renamed.borrow_mut() = Some(self.archives.rename(&from, &to_name));
            (self.notify)();
            return;
        }
        let Some((token, from_name)) = split_token(&from) else {
            *self.renamed.borrow_mut() = Some(Err(format!("cannot rename {}", from.display())));
            (self.notify)();
            return;
        };
        let renamed = Rc::clone(&self.renamed);
        let notify = Rc::clone(&self.notify);
        spawn_local(async move {
            *renamed.borrow_mut() = Some(
                match rename_pack_file_js(&token, &from_name, &to_name).await {
                    Ok(()) => Ok(()),
                    Err(error) => Err(js_error(error)),
                },
            );
            notify();
        });
    }

    fn poll_renamed(&mut self) -> Option<Result<(), String>> {
        self.renamed.borrow_mut().take()
    }

    fn pick_output_folder(&mut self) {
        let output = Rc::clone(&self.output_folder);
        let notify = Rc::clone(&self.notify);
        spawn_local(async move {
            let resolved = match pick_output_folder_js().await {
                Ok(value) if value.is_null() || value.is_undefined() => None,
                Ok(value) => match get_string(&value, "token") {
                    Ok(token) => Some(PathBuf::from(format!("/{token}"))),
                    Err(message) => {
                        log::error!("pick output folder: {message}");
                        None
                    }
                },
                Err(error) => {
                    log::error!("pick output folder: {}", js_error(error));
                    None
                }
            };
            *output.borrow_mut() = Some(resolved);
            notify();
        });
    }

    fn poll_output_folder(&mut self) -> Option<Option<PathBuf>> {
        self.output_folder.borrow_mut().take()
    }

    fn pick_save_path(&mut self, suggested_name: String) {
        // The web has no writable path: the "destination" is just the download
        // name. Resolve it at once so the shared render flow proceeds, and the
        // later in-place write to this token-less path falls through to a
        // browser download (see `save`).
        *self.save_path.borrow_mut() = Some(Some(PathBuf::from(suggested_name)));
        (self.notify)();
    }

    fn poll_save_path(&mut self) -> Option<Option<PathBuf>> {
        self.save_path.borrow_mut().take()
    }
}
