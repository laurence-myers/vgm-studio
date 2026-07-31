// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebPackService`]: builds a pack release zip in a Web Worker.
//!
//! The export is the wasm-portable pipeline in [`crate::pack_zip`]: songs
//! optimised through the vgmtools wasip1 modules (or `vgms_core`'s built-in
//! pass when they are unavailable) and optionally gzipped, PNGs kept as-is
//! (oxipng is native-only). It runs in a dedicated Worker so a multi-megabyte
//! pack does not jank the frame; `cancel` is `Worker.terminate()`, the
//! browser's honest analogue of dropping the job.
//!
//! **The watchdog.** Native gives every tool run a 120 s deadline and kills the
//! child at it; the web cannot pre-empt a running wasm instance, so the page
//! stands in: the Worker posts a heartbeat per pack entry, and a job that goes
//! [`WATCHDOG_MS`] without one is terminated and reported as failed. This is
//! the backstop for the hangs the ROM-size guard does not know about.
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

/// How long the export may go without a heartbeat before it is presumed hung.
///
/// Generous: heartbeats arrive per entry, and one entry is at most a few
/// seconds of optimising plus a gzip. Minutes of silence is a tool spinning,
/// which is exactly what the terminate is for.
const WATCHDOG_MS: i32 = 180_000;

/// The timeout closure, shared so both the service and the message handler can
/// re-arm the timer with it.
type TimeoutClosure = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

/// The Worker and its watchdog timer, together because they die together:
/// terminating the Worker clears the timer, and the timer firing terminates
/// the Worker.
struct RunningJob {
    worker: web_sys::Worker,
    timer: Option<i32>,
}

impl RunningJob {
    fn clear_timer(&mut self) {
        if let Some(id) = self.timer.take()
            && let Some(window) = web_sys::window()
        {
            window.clear_timeout_with_handle(id);
        }
    }

    fn terminate(mut self) {
        self.clear_timer();
        self.worker.terminate();
    }
}

/// Builds release packs in a Worker, and answers the standalone PNG optimise
/// honestly.
pub struct WebPackService {
    jobs: Rc<RefCell<VecDeque<PackJobOutcome>>>,
    optimized: RefCell<VecDeque<Result<OptimizedImage, String>>>,
    busy: Rc<Cell<bool>>,
    running: Rc<RefCell<Option<RunningJob>>>,
    _on_message: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    // Kept alive for as long as any timer may fire it.
    _on_timeout: TimeoutClosure,
    notify: Rc<dyn Fn()>,
}

impl std::fmt::Debug for WebPackService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebPackService").finish_non_exhaustive()
    }
}

/// (Re)arms the inactivity timer on the running job, replacing any previous
/// deadline. A free function over the shared cells so the message closure can
/// call it too.
fn rearm_watchdog(running: &Rc<RefCell<Option<RunningJob>>>, on_timeout: &TimeoutClosure) {
    let mut slot = running.borrow_mut();
    let Some(job) = slot.as_mut() else {
        return;
    };
    job.clear_timer();
    let (Some(window), Some(callback)) = (web_sys::window(), on_timeout.borrow().as_ref().map(
        |closure| closure.as_ref().unchecked_ref::<js_sys::Function>().clone(),
    )) else {
        return;
    };
    job.timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(&callback, WATCHDOG_MS)
        .ok();
}

impl WebPackService {
    /// Builds the service. `notify` wakes the egui loop when the export finishes,
    /// so its outcome is polled up without waiting for input.
    pub fn new(notify: impl Fn() + 'static) -> Self {
        Self {
            jobs: Rc::new(RefCell::new(VecDeque::new())),
            optimized: RefCell::new(VecDeque::new()),
            busy: Rc::new(Cell::new(false)),
            running: Rc::new(RefCell::new(None)),
            _on_message: None,
            _on_timeout: Rc::new(RefCell::new(None)),
            notify: Rc::new(notify),
        }
    }

    /// Terminates and forgets the running Worker and its watchdog, if any.
    fn terminate(&mut self) {
        if let Some(job) = self.running.borrow_mut().take() {
            job.terminate();
        }
        self._on_message = None;
        *self._on_timeout.borrow_mut() = None;
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

        // The watchdog: fires only when the Worker has posted nothing --
        // heartbeat or result -- for WATCHDOG_MS. Terminate, report, wake.
        let on_timeout = Closure::<dyn FnMut()>::new({
            let running = Rc::clone(&self.running);
            let jobs = Rc::clone(&self.jobs);
            let busy = Rc::clone(&self.busy);
            let notify = Rc::clone(&self.notify);
            move || {
                let Some(job) = running.borrow_mut().take() else {
                    return;
                };
                job.terminate();
                jobs.borrow_mut().push_back(PackJobOutcome::Failed(format!(
                    "the pack export made no progress for {} minutes and was stopped",
                    WATCHDOG_MS / 60_000
                )));
                busy.set(false);
                notify();
            }
        });
        *self._on_timeout.borrow_mut() = Some(on_timeout);

        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new({
            let running = Rc::clone(&self.running);
            let on_timeout = Rc::clone(&self._on_timeout);
            let jobs = Rc::clone(&self.jobs);
            let busy = Rc::clone(&self.busy);
            let notify = Rc::clone(&self.notify);
            move |event: web_sys::MessageEvent| {
                let data = event.data();
                // A heartbeat only feeds the watchdog; the job is still going.
                if js_sys::Reflect::get(&data, &JsValue::from_str("heartbeat"))
                    .is_ok_and(|flag| flag.as_bool() == Some(true))
                {
                    rearm_watchdog(&running, &on_timeout);
                    return;
                }

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
                if let Some(job) = running.borrow_mut().take() {
                    job.terminate();
                }
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
        *self.running.borrow_mut() = Some(RunningJob {
            worker,
            timer: None,
        });
        rearm_watchdog(&self.running, &self._on_timeout);
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
