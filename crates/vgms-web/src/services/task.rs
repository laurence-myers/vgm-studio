// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WorkerTaskService`]: background tasks as dedicated Web Workers.
//!
//! `run_task` was written platform-independent ("the web implementation inside a
//! Worker"). Here that Worker is real: one per running [`TaskKind`] (kinds run
//! concurrently, as the native service's do), each loading the app module and
//! running the task off the main thread. The request crosses as
//! [`crate::codec`] bytes; every emitted result posts back the same way and lands
//! in a queue [`poll`](WorkerTaskService::poll) drains. Cancellation is
//! `Worker.terminate()` -- the browser's honest analogue of dropping the thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

use vgms_ui::tasks::{TaskKind, TaskRequest, TaskResult, TaskService};

/// Where the Worker bootstrap script lives, relative to the page. The build lays
/// it beside the app module.
const WORKER_URL: &str = "./task_worker.js";

/// The string a Worker posts once its task has emitted everything, so the main
/// thread can mark that kind idle. Any other message is result bytes.
const DONE: &str = "done";

/// A running Worker and the state the main thread reads about it.
struct WorkerSlot {
    worker: web_sys::Worker,
    /// Kept alive for as long as the Worker: dropping it would unhook the handler.
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    /// Set by the handler when the Worker posts [`DONE`]; a done Worker is idle
    /// and gets reaped on the next poll.
    done: Rc<Cell<bool>>,
}

/// A debounced request waiting for its quiet period to elapse.
struct Pending {
    bytes: Vec<u8>,
    deadline_ms: f64,
}

/// Schedules [`TaskRequest`]s onto Web Workers.
pub struct WorkerTaskService {
    workers: HashMap<TaskKind, WorkerSlot>,
    pending: HashMap<TaskKind, Pending>,
    results: Rc<RefCell<Vec<TaskResult>>>,
    notify: Rc<dyn Fn()>,
}

impl std::fmt::Debug for WorkerTaskService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerTaskService")
            .field("workers", &self.workers.len())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl WorkerTaskService {
    /// Builds the service. `notify` fires when a result lands or a debounce is due,
    /// so the egui loop keeps ticking while work is in flight.
    pub fn new(notify: impl Fn() + 'static) -> Self {
        Self {
            workers: HashMap::new(),
            pending: HashMap::new(),
            results: Rc::new(RefCell::new(Vec::new())),
            notify: Rc::new(notify),
        }
    }

    /// `performance.now()` in milliseconds, or `0.0` if unavailable (which only
    /// makes every debounce fire on the next poll -- harmless).
    fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|window| window.performance())
            .map_or(0.0, |performance| performance.now())
    }

    /// Spawns a Worker for `kind`, posts `bytes`, and wires its results into the
    /// shared queue. Any Worker already running that kind is terminated first.
    fn spawn(&mut self, kind: TaskKind, bytes: Vec<u8>) {
        self.terminate(kind);

        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        let worker = match web_sys::Worker::new_with_options(WORKER_URL, &options) {
            Ok(worker) => worker,
            Err(error) => {
                web_sys::console::error_1(&error);
                return;
            }
        };

        let done = Rc::new(Cell::new(false));
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new({
            let results = Rc::clone(&self.results);
            let notify = Rc::clone(&self.notify);
            let done = Rc::clone(&done);
            move |event: web_sys::MessageEvent| {
                let data = event.data();
                // A Worker signals completion with the DONE string; everything
                // else is a Uint8Array of result bytes.
                if data.as_string().as_deref() == Some(DONE) {
                    done.set(true);
                    notify();
                    return;
                }
                let Ok(array) = data.dyn_into::<js_sys::Uint8Array>() else {
                    web_sys::console::error_1(&JsValue::from_str(
                        "vgms-web: a Worker posted a non-buffer message",
                    ));
                    return;
                };
                match crate::codec::decode_result(&array.to_vec()) {
                    Ok(result) => {
                        results.borrow_mut().push(result);
                        notify();
                    }
                    Err(error) => web_sys::console::error_1(&JsValue::from_str(&format!(
                        "vgms-web: could not decode a task result: {error}"
                    ))),
                }
            }
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let message: JsValue = js_sys::Uint8Array::from(bytes.as_slice()).into();
        if let Err(error) = worker.post_message(&message) {
            web_sys::console::error_1(&error);
        }

        self.workers.insert(
            kind,
            WorkerSlot {
                worker,
                _on_message: on_message,
                done,
            },
        );
    }

    /// Terminates and forgets the Worker running `kind`, if any.
    fn terminate(&mut self, kind: TaskKind) {
        if let Some(slot) = self.workers.remove(&kind) {
            slot.worker.terminate();
        }
    }
}

impl TaskService for WorkerTaskService {
    fn submit(&mut self, request: TaskRequest, debounce: Option<Duration>) {
        let kind = request.kind();
        let bytes = match crate::codec::encode_request(&request) {
            Ok(bytes) => bytes,
            Err(error) => {
                web_sys::console::error_1(&JsValue::from_str(&format!(
                    "vgms-web: could not encode a task request: {error}"
                )));
                return;
            }
        };
        // Submitting supersedes any running or pending task of the same kind.
        self.terminate(kind);
        match debounce {
            Some(delay) => {
                self.pending.insert(
                    kind,
                    Pending {
                        bytes,
                        deadline_ms: Self::now_ms() + delay.as_secs_f64() * 1000.0,
                    },
                );
                (self.notify)();
            }
            None => self.spawn(kind, bytes),
        }
    }

    fn cancel(&mut self, kind: TaskKind) {
        self.terminate(kind);
        self.pending.remove(&kind);
    }

    fn poll(&mut self) -> Vec<TaskResult> {
        // Start any debounced task whose quiet period has elapsed.
        let now = Self::now_ms();
        let due: Vec<TaskKind> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline_ms <= now)
            .map(|(kind, _)| *kind)
            .collect();
        for kind in due {
            if let Some(pending) = self.pending.remove(&kind) {
                self.spawn(kind, pending.bytes);
            }
        }
        // Reap Workers that finished, freeing their threads.
        let finished: Vec<TaskKind> = self
            .workers
            .iter()
            .filter(|(_, slot)| slot.done.get())
            .map(|(kind, _)| *kind)
            .collect();
        for kind in finished {
            self.terminate(kind);
        }
        std::mem::take(&mut *self.results.borrow_mut())
    }

    fn is_busy(&self) -> bool {
        !self.pending.is_empty() || self.workers.values().any(|slot| !slot.done.get())
    }

    fn is_busy_kind(&self, kind: TaskKind) -> bool {
        self.pending.contains_key(&kind)
            || self.workers.get(&kind).is_some_and(|slot| !slot.done.get())
    }

    fn shutdown(&mut self) {
        for (_, slot) in self.workers.drain() {
            slot.worker.terminate();
        }
        self.pending.clear();
    }
}
