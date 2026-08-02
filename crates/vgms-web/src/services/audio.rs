// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebAudioService`]: playback through an `AudioWorklet`.
//!
//! The audio-thread half lives in `worklet-processor.js`, which hosts the
//! `vgms-synth-worklet` wasm module and renders each quantum. This is the
//! main-thread half: it opens one `AudioContext`, adds the processor module once,
//! and creates one `AudioWorkletNode` per loaded song. Control (play, seek, mute)
//! posts command messages to the node's port; the node posts playback state back,
//! which the trait's `position`/`take_peaks`/... read from the last snapshot.
//!
//! `load` is optimistic: the context-open, module-add and node-create are async,
//! so it spawns that work and returns `Ok` at once. A failure along the way
//! surfaces through [`last_error`](AudioService::last_error) -- the trait's channel
//! for faults away from a call -- exactly as a device unplugged mid-song does
//! natively. Commands that arrive before the node exists are queued and flushed
//! when it does.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::{JsFuture, spawn_local};

use vgms_core::config::AudioConfig;
use vgms_synth::resample::ResampleMode;
use vgms_synth::{
    AudioSource, ChipMuting, ChipPanning, LoopConfig, LoopCount, Muting, Panning, Position,
};
use vgms_ui::AudioService;

/// The processor module, laid beside the app module by the build.
const PROCESSOR_URL: &str = "./worklet-processor.js";
/// The worklet wasm the processor instantiates. Fetched once and cached.
const WORKLET_WASM_URL: &str = "./vgms_synth_worklet.wasm";

/// The most recent playback state the processor posted.
#[derive(Default)]
struct StateSnapshot {
    frames: f64,
    ms: u32,
    row: usize,
    loop_iteration: u32,
    finished: bool,
    min_engaged_boost: f32,
}

/// Everything the service shares between the trait methods and the async setup /
/// the port's message handler.
struct Inner {
    context: Option<web_sys::AudioContext>,
    node: Option<web_sys::AudioWorkletNode>,
    /// The `add_module` promise, stored so every concurrent setup awaits the same
    /// one instead of racing a flag across the await and adding it twice.
    module_promise: Option<js_sys::Promise>,
    /// Bumped on every load and unload. A setup captures it and, if it no longer
    /// matches when the setup finishes, knows a newer load has superseded it.
    epoch: u64,
    ready: bool,
    /// Commands posted before the node existed, replayed in order once it does.
    pending: Vec<JsValue>,
    playing: bool,
    output_rate: Option<u32>,
    state: StateSnapshot,
    /// Peaks accumulated across state posts until the UI takes them (a
    /// destructive read), so a transient between two UI polls survives.
    accum_peak: [f32; 2],
    limited: bool,
    last_error: Option<String>,
    /// Kept alive for as long as the node: dropping it unhooks the port handler.
    _on_message: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
    notify: Rc<dyn Fn()>,
}

/// Plays a song through an `AudioWorklet`.
pub struct WebAudioService {
    inner: Rc<RefCell<Inner>>,
}

impl std::fmt::Debug for WebAudioService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebAudioService").finish_non_exhaustive()
    }
}

impl WebAudioService {
    /// Builds the service. `notify` fires when a state message lands, so the egui
    /// loop repaints and reads the new position / meter.
    pub fn new(notify: impl Fn() + 'static) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                context: None,
                node: None,
                module_promise: None,
                epoch: 0,
                ready: false,
                pending: Vec::new(),
                playing: false,
                output_rate: None,
                state: StateSnapshot::default(),
                accum_peak: [0.0, 0.0],
                limited: false,
                last_error: None,
                _on_message: None,
                notify: Rc::new(notify),
            })),
        }
    }

    /// Posts a command to the processor, or queues it until the node is ready.
    fn post(&self, command: JsValue) {
        let mut inner = self.inner.borrow_mut();
        if inner.ready {
            if let Some(node) = &inner.node
                && let Ok(port) = node.port()
            {
                let _ = port.post_message(&command);
            }
        } else {
            inner.pending.push(command);
        }
    }
}

/// Tears the current node down: dispose the processor so it stops rendering,
/// unhook its port handler so a late message cannot fire a dropped closure
/// (~43/s throw storm otherwise), and disconnect it from the graph.
fn teardown_node(inner: &mut Inner) {
    if let Some(node) = inner.node.take() {
        if let Ok(port) = node.port() {
            let _ = port.post_message(&command("dispose", &[]));
            port.set_onmessage(None);
        }
        let _ = node.disconnect();
    }
    inner._on_message = None;
}

/// Tears the node down and resets the playback state, bumping the epoch so an
/// in-flight setup knows it has been superseded.
fn reset(inner: &mut Inner) {
    teardown_node(inner);
    inner.ready = false;
    inner.playing = false;
    inner.state = StateSnapshot::default();
    inner.accum_peak = [0.0, 0.0];
    inner.limited = false;
    inner.epoch = inner.epoch.wrapping_add(1);
}

/// Builds a command object `{ cmd, ...fields }` for the processor.
fn command(cmd: &str, fields: &[(&str, JsValue)]) -> JsValue {
    let object = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&object, &"cmd".into(), &cmd.into());
    for (key, value) in fields {
        let _ = js_sys::Reflect::set(&object, &(*key).into(), value);
    }
    object.into()
}

fn num(value: f64) -> JsValue {
    JsValue::from_f64(value)
}

/// The `(name, file-bytes)` for a source, via its own writer -- the same bytes the
/// worklet reads back by name.
fn source_bytes(source: &AudioSource) -> Result<(String, Vec<u8>), String> {
    match source {
        AudioSource::Opl(song) => Ok((
            song.name.clone(),
            vgms_core::io::write_song(song).map_err(|error| error.to_string())?,
        )),
        AudioSource::Vgm(file) => Ok((
            file.name.clone(),
            vgms_core::vgm::file::write(file).map_err(|error| error.to_string())?,
        )),
    }
}

impl AudioService for WebAudioService {
    fn load(&mut self, source: AudioSource, config: &AudioConfig) -> Result<(), String> {
        let (name, bytes) = source_bytes(&source)?;
        let sample_rate = config.frequency;
        let resample = match ResampleMode::from_slug(&config.resampling).unwrap_or_default() {
            ResampleMode::Sinc => 0u32,
            ResampleMode::Linear => 1,
        };
        let choices: Vec<(String, String)> = config
            .cores
            .iter()
            .map(|(slug, id)| (slug.clone(), id.clone()))
            .collect();

        // A fresh node supersedes any current one: tear the old node down
        // (disconnect, dispose, unhook its handler), reset the state, and bump the
        // epoch so a still-running previous setup knows it has been superseded.
        let epoch = {
            let mut inner = self.inner.borrow_mut();
            reset(&mut inner);
            inner.epoch
        };

        // Open the AudioContext *now*, synchronously, not inside the async setup.
        // A browser only lets `resume()` start audio from within a user gesture,
        // and Play is that gesture -- but if the context does not yet exist when
        // Play fires (the async node setup is still running), the resume is a
        // no-op and playback stays silent. Creating it here means `play`'s resume
        // always has a context to act on, on the click that reached it. A failure
        // here is synchronous with this call, so return it rather than deferring
        // it to `last_error`.
        ensure_context(&self.inner, sample_rate)?;

        let inner = Rc::clone(&self.inner);
        spawn_local(async move {
            if let Err(message) =
                setup(&inner, name, bytes, sample_rate, resample, choices, epoch).await
            {
                // Only the current load reports a fault; a superseded one is silent.
                let notify = {
                    let mut b = inner.borrow_mut();
                    if b.epoch != epoch {
                        return;
                    }
                    b.last_error = Some(message);
                    Rc::clone(&b.notify)
                };
                notify();
            }
        });
        Ok(())
    }

    fn unload(&mut self) {
        let mut inner = self.inner.borrow_mut();
        reset(&mut inner);
        inner.pending.clear();
    }

    fn play(&mut self) -> Result<(), String> {
        // Resume the context on the click gesture that reached us.
        if let Some(context) = self.inner.borrow().context.clone() {
            let _ = context.resume();
        }
        self.inner.borrow_mut().playing = true;
        self.post(command("play", &[]));
        Ok(())
    }

    fn pause(&mut self) {
        self.inner.borrow_mut().playing = false;
        self.post(command("pause", &[]));
    }

    fn seek_ms(&mut self, ms: u32) {
        self.post(command("seekMs", &[("ms", num(f64::from(ms)))]));
    }

    fn seek_pos(&mut self, pos: usize) {
        self.post(command("seekPos", &[("pos", num(pos as f64))]));
    }

    fn rewind(&mut self) {
        self.post(command("rewind", &[]));
    }

    fn set_muting(&mut self, muting: Muting) {
        let [perc0, perc1] = muting.percussion_raw();
        self.post(command(
            "setMuting",
            &[
                ("channels", num(f64::from(muting.channels_raw()))),
                ("perc0", num(f64::from(perc0))),
                ("perc1", num(f64::from(perc1))),
            ],
        ));
    }

    fn set_panning(&mut self, panning: Panning) {
        let command = match panning {
            Panning::Original => command("setPanning", &[("mode", num(0.0))]),
            Panning::Custom(pans) => {
                let array = js_sys::Uint8Array::from(&pans[..]);
                command("setPanning", &[("mode", num(1.0)), ("pans", array.into())])
            }
        };
        self.post(command);
    }

    fn set_chip_muting(&mut self, muting: ChipMuting) {
        for (kind, instance, mask) in muting.entries() {
            self.post(command(
                "setChipMute",
                &[
                    ("slug", JsValue::from_str(kind.slug())),
                    ("instance", num(f64::from(instance))),
                    ("mask", num(f64::from(mask))),
                ],
            ));
        }
    }

    fn set_chip_panning(&mut self, panning: ChipPanning) {
        for (kind, instance, pans) in panning.entries() {
            let array = js_sys::Int16Array::from(pans);
            self.post(command(
                "setChipPan",
                &[
                    ("slug", JsValue::from_str(kind.slug())),
                    ("instance", num(f64::from(instance))),
                    ("pans", array.into()),
                ],
            ));
        }
    }

    fn set_boost(&mut self, boost: f32) {
        self.post(command("setBoost", &[("boost", num(f64::from(boost)))]));
    }

    fn set_loop(&mut self, config: Option<LoopConfig>) {
        let command = match config {
            None => command("setLoop", &[("enabled", num(0.0))]),
            Some(loop_config) => {
                let (tag, times) = match loop_config.count {
                    LoopCount::Infinite => (0.0, 0.0),
                    LoopCount::Times(n) => (1.0, f64::from(n)),
                };
                command(
                    "setLoop",
                    &[
                        ("enabled", num(1.0)),
                        ("start", num(loop_config.start as f64)),
                        ("end", num(loop_config.end as f64)),
                        ("countTag", num(tag)),
                        ("countTimes", num(times)),
                        ("startFrames", num(loop_config.start_frames as f64)),
                    ],
                )
            }
        };
        self.post(command);
    }

    fn is_playing(&self) -> bool {
        let inner = self.inner.borrow();
        inner.playing && !inner.state.finished
    }

    fn is_finished(&self) -> bool {
        self.inner.borrow().state.finished
    }

    fn position(&self) -> Option<Position> {
        let inner = self.inner.borrow();
        inner.node.as_ref().map(|_| Position {
            frames_rendered: inner.state.frames as u64,
            elapsed_ms: inner.state.ms,
            next_instruction: inner.state.row,
            loop_iteration: inner.state.loop_iteration,
        })
    }

    fn take_peaks(&mut self) -> Option<[f32; 2]> {
        let mut inner = self.inner.borrow_mut();
        inner.node.as_ref()?;
        Some(std::mem::take(&mut inner.accum_peak))
    }

    fn output_rate(&self) -> Option<u32> {
        self.inner.borrow().output_rate
    }

    fn min_engaged_boost(&self) -> Option<f32> {
        let boost = self.inner.borrow().state.min_engaged_boost;
        (boost > 0.0).then_some(boost)
    }

    fn take_limited(&mut self) -> bool {
        let mut inner = self.inner.borrow_mut();
        std::mem::take(&mut inner.limited)
    }

    fn last_error(&mut self) -> Option<String> {
        self.inner.borrow_mut().last_error.take()
    }
}

/// The async node setup: open the context, add the processor module (once), fetch
/// the worklet wasm (once cached would be nicer, but a fetch hits the HTTP cache),
/// create the node, wire its port, and connect it.
async fn setup(
    inner: &Rc<RefCell<Inner>>,
    name: String,
    song_bytes: Vec<u8>,
    sample_rate: u32,
    resample: u32,
    choices: Vec<(String, String)>,
    epoch: u64,
) -> Result<(), String> {
    let context = ensure_context(inner, sample_rate)?;

    // Add the processor module once. The promise is stored, so a second load that
    // arrives before this one finishes awaits the same add rather than adding the
    // module twice (which throws).
    let module_promise = {
        let mut b = inner.borrow_mut();
        match b.module_promise.clone() {
            Some(promise) => promise,
            None => {
                let worklet = context
                    .audio_worklet()
                    .map_err(|_| "this browser has no AudioWorklet".to_owned())?;
                let promise = worklet
                    .add_module(PROCESSOR_URL)
                    .map_err(|_| "could not add the audio processor module".to_owned())?;
                b.module_promise = Some(promise.clone());
                promise
            }
        }
    };
    if let Err(error) = JsFuture::from(module_promise).await {
        // Let a later load retry the add rather than awaiting a rejected promise.
        inner.borrow_mut().module_promise = None;
        let _ = error;
        return Err("the audio processor module failed to load".to_owned());
    }
    if inner.borrow().epoch != epoch {
        return Ok(()); // superseded while adding the module
    }

    let wasm_bytes = fetch_bytes(WORKLET_WASM_URL).await?;
    if inner.borrow().epoch != epoch {
        return Ok(()); // superseded while fetching the worklet wasm
    }

    let options = node_options(
        &wasm_bytes,
        &name,
        &song_bytes,
        sample_rate,
        resample,
        &choices,
    );
    let node = web_sys::AudioWorkletNode::new_with_options(&context, "vgms-engine", &options)
        .map_err(|_| "could not create the audio node".to_owned())?;

    let notify = Rc::clone(&inner.borrow().notify);
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new({
        let inner = Rc::clone(inner);
        move |event: web_sys::MessageEvent| handle_message(&inner, &event)
    });
    let port = node
        .port()
        .map_err(|_| "the audio node has no port".to_owned())?;
    port.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    node.connect_with_audio_node(&context.destination())
        .map_err(|_| "could not connect the audio node".to_owned())?;

    {
        let mut inner = inner.borrow_mut();
        // A newer load may have superseded this one while the node was being
        // built. If so, dispose the node we just made and install nothing.
        if inner.epoch != epoch {
            let _ = port.post_message(&command("dispose", &[]));
            port.set_onmessage(None);
            let _ = node.disconnect();
            return Ok(());
        }
        inner.output_rate = Some(context.sample_rate() as u32);
        inner._on_message = Some(on_message);
        inner.ready = true;
        // Flush any commands that arrived before the node existed.
        for command in inner.pending.drain(..).collect::<Vec<_>>() {
            let _ = port.post_message(&command);
        }
        inner.node = Some(node);
    }
    notify();
    Ok(())
}

/// The service's `AudioContext`, opening one at `sample_rate` if there is none.
fn ensure_context(
    inner: &Rc<RefCell<Inner>>,
    sample_rate: u32,
) -> Result<web_sys::AudioContext, String> {
    if let Some(context) = inner.borrow().context.clone() {
        return Ok(context);
    }
    let options = web_sys::AudioContextOptions::new();
    options.set_sample_rate(sample_rate as f32);
    let context = web_sys::AudioContext::new_with_context_options(&options)
        .map_err(|_| "could not open an AudioContext".to_owned())?;
    inner.borrow_mut().context = Some(context.clone());
    Ok(context)
}

/// Fetches `url` and returns its bytes.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response_value = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| format!("could not fetch {url}"))?;
    let response: web_sys::Response = response_value
        .dyn_into()
        .map_err(|_| "fetch did not return a Response".to_owned())?;
    let buffer = JsFuture::from(
        response
            .array_buffer()
            .map_err(|_| "the response had no body".to_owned())?,
    )
    .await
    .map_err(|_| "could not read the response body".to_owned())?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// Builds the `AudioWorkletNodeOptions`, packing the module bytes and the song
/// into `processorOptions` for the processor's constructor.
fn node_options(
    wasm_bytes: &[u8],
    name: &str,
    song_bytes: &[u8],
    sample_rate: u32,
    resample: u32,
    choices: &[(String, String)],
) -> web_sys::AudioWorkletNodeOptions {
    let processor_options = js_sys::Object::new();
    let set = |key: &str, value: &JsValue| {
        let _ = js_sys::Reflect::set(&processor_options, &key.into(), value);
    };
    set("wasmBytes", &js_sys::Uint8Array::from(wasm_bytes));
    set("songName", &JsValue::from_str(name));
    set("songBytes", &js_sys::Uint8Array::from(song_bytes));
    set("sampleRate", &num(f64::from(sample_rate)));
    set("resampleMode", &num(f64::from(resample)));
    let core_choices = js_sys::Array::new();
    for (slug, id) in choices {
        core_choices.push(&js_sys::Array::of2(
            &JsValue::from_str(slug),
            &JsValue::from_str(id),
        ));
    }
    set("coreChoices", &core_choices);

    let options = web_sys::AudioWorkletNodeOptions::new();
    options.set_number_of_inputs(0);
    options.set_number_of_outputs(1);
    let output_channels = js_sys::Array::of1(&num(2.0));
    options.set_output_channel_count(&output_channels);
    options.set_processor_options(Some(&processor_options));
    options
}

/// Updates the shared state from one processor message.
fn handle_message(inner: &Rc<RefCell<Inner>>, event: &web_sys::MessageEvent) {
    let data = event.data();
    let kind = get_string(&data, "type");
    match kind.as_deref() {
        Some("state") => {
            let mut inner = inner.borrow_mut();
            inner.state.frames = get_f64(&data, "frames");
            inner.state.ms = get_f64(&data, "ms") as u32;
            inner.state.row = get_f64(&data, "row") as usize;
            inner.state.loop_iteration = get_f64(&data, "loopIteration") as u32;
            inner.state.finished = get_bool(&data, "finished");
            inner.state.min_engaged_boost = get_f64(&data, "minEngagedBoost") as f32;
            let peak_l = get_f64(&data, "peakL") as f32;
            let peak_r = get_f64(&data, "peakR") as f32;
            inner.accum_peak[0] = inner.accum_peak[0].max(peak_l);
            inner.accum_peak[1] = inner.accum_peak[1].max(peak_r);
            if get_bool(&data, "limited") {
                inner.limited = true;
            }
        }
        Some("error") => {
            inner.borrow_mut().last_error = get_string(&data, "message");
        }
        _ => {}
    }
    let notify = Rc::clone(&inner.borrow().notify);
    notify();
}

fn get(object: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn get_f64(object: &JsValue, key: &str) -> f64 {
    get(object, key).as_f64().unwrap_or(0.0)
}

fn get_bool(object: &JsValue, key: &str) -> bool {
    get(object, key).as_bool().unwrap_or(false)
}

fn get_string(object: &JsValue, key: &str) -> Option<String> {
    get(object, key).as_string()
}
