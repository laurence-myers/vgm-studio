// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebPackService`]: builds a pack release zip in a Web Worker.
//!
//! The export is the wasm-portable half of the native pipeline
//! ([`crate::pack_zip`]): songs optimised through `vgms_core`'s built-in pass and
//! optionally gzipped, PNGs kept as-is (oxipng is native-only). It runs in a
//! dedicated Worker so a multi-megabyte pack does not jank the frame; `cancel`
//! is `Worker.terminate()`, the browser's honest analogue of dropping the job.
//!
//! [`optimize`](PackService::optimize) -- the standalone screenshot recompress --
//! stays the honest "not available" error: that is oxipng, which does not come to
//! the browser. [`today`](PackService::today) is a pure date the history line
//! wants.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;

use vgms_ui::platform::{OptimizedImage, PackJobOutcome, PackJobRequest, PackService};

/// The Worker that runs [`crate::worker::vgms_web_run_pack_job`].
const WORKER_URL: &str = "./pack_worker.js";

/// The message the standalone PNG-optimise action answers with on the web.
const NO_PNG_OPTIMISE: &str = "PNG optimisation is not available in this browser.";

/// Builds release packs in a Worker, and answers the standalone PNG optimise
/// honestly.
pub struct WebPackService {
    jobs: Rc<RefCell<VecDeque<PackJobOutcome>>>,
    optimized: RefCell<VecDeque<Result<OptimizedImage, String>>>,
    busy: Rc<Cell<bool>>,
    worker: Option<web_sys::Worker>,
    _on_message: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    notify: Rc<dyn Fn()>,
}

impl std::fmt::Debug for WebPackService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebPackService").finish_non_exhaustive()
    }
}

impl WebPackService {
    /// Builds the service. `notify` wakes the egui loop when the export finishes,
    /// so its outcome is polled up without waiting for input.
    pub fn new(notify: impl Fn() + 'static) -> Self {
        Self {
            jobs: Rc::new(RefCell::new(VecDeque::new())),
            optimized: RefCell::new(VecDeque::new()),
            busy: Rc::new(Cell::new(false)),
            worker: None,
            _on_message: None,
            notify: Rc::new(notify),
        }
    }

    /// Terminates and forgets the running Worker, if any.
    fn terminate(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.terminate();
        }
        self._on_message = None;
        self.busy.set(false);
    }
}

impl PackService for WebPackService {
    fn submit(&mut self, request: PackJobRequest) {
        // A new job supersedes any running one, matching the native generation
        // counter's "latest submit wins".
        self.terminate();
        let bytes = crate::codec::encode_pack_job(&request);

        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        let worker = match web_sys::Worker::new_with_options(WORKER_URL, &options) {
            Ok(worker) => worker,
            Err(error) => {
                web_sys::console::error_1(&error);
                self.jobs.borrow_mut().push_back(PackJobOutcome::Failed(
                    "could not start the pack worker".to_owned(),
                ));
                (self.notify)();
                return;
            }
        };

        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new({
            let jobs = Rc::clone(&self.jobs);
            let busy = Rc::clone(&self.busy);
            let notify = Rc::clone(&self.notify);
            move |event: web_sys::MessageEvent| {
                let data = event.data();
                let outcome = if let Ok(array) = data.clone().dyn_into::<js_sys::Uint8Array>() {
                    crate::codec::decode_pack_outcome(&array.to_vec()).unwrap_or_else(|error| {
                        PackJobOutcome::Failed(format!(
                            "could not decode the pack outcome: {error}"
                        ))
                    })
                } else {
                    // A thrown error from the Worker arrives as `{ error }`.
                    let message = js_sys::Reflect::get(&data, &JsValue::from_str("error"))
                        .ok()
                        .and_then(|value| value.as_string())
                        .unwrap_or_else(|| "the pack worker failed".to_owned());
                    PackJobOutcome::Failed(message)
                };
                jobs.borrow_mut().push_back(outcome);
                busy.set(false);
                notify();
            }
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let message: JsValue = js_sys::Uint8Array::from(bytes.as_slice()).into();
        if let Err(error) = worker.post_message(&message) {
            web_sys::console::error_1(&error);
        }

        self.busy.set(true);
        self.worker = Some(worker);
        self._on_message = Some(on_message);
    }

    fn poll(&mut self) -> Option<PackJobOutcome> {
        self.jobs.borrow_mut().pop_front()
    }

    fn is_busy(&self) -> bool {
        self.busy.get()
    }

    fn cancel(&mut self) {
        self.terminate();
    }

    fn optimize(&mut self, _name: String, _bytes: Vec<u8>) {
        self.optimized
            .borrow_mut()
            .push_back(Err(NO_PNG_OPTIMISE.to_owned()));
    }

    fn poll_optimized(&mut self) -> Option<Result<OptimizedImage, String>> {
        self.optimized.borrow_mut().pop_front()
    }

    fn today(&self) -> Option<(i32, u8, u8)> {
        // JS months are 0-based; the app wants 1-12 to match the native chrono
        // path. `get_date` is the 1-31 day of month.
        let date = js_sys::Date::new_0();
        let year = date.get_full_year() as i32;
        let month = (date.get_month() + 1) as u8;
        let day = date.get_date() as u8;
        Some((year, month, day))
    }
}
