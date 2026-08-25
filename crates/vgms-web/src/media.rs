//! Web media-session integration: `navigator.mediaSession` drives the same
//! media-key seam the native SMTC build does.
//!
//! The browser routes media keys to the page's media session, not to the canvas,
//! so the app cannot see them as ordinary key events. Register play/pause/stop
//! action handlers that post a [`TransportCommand`] into the app's [`MediaKeys`]
//! sink, and publish the playback state each frame so the browser keeps the
//! session active and routes its buttons. A small inline-JS shim avoids pulling
//! extra `web-sys` features for an API used in exactly one place.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use vgms_ui::{TransportCommand, VgmStudioApp};

#[wasm_bindgen(inline_js = r#"
export function vgms_install_media_session(play, pause, stop) {
    if (typeof navigator === 'undefined' || !('mediaSession' in navigator)) {
        return false;
    }
    try {
        navigator.mediaSession.setActionHandler('play', play);
        navigator.mediaSession.setActionHandler('pause', pause);
        navigator.mediaSession.setActionHandler('stop', stop);
        return true;
    } catch (e) {
        return false;
    }
}
export function vgms_set_playback_state(state) {
    if (typeof navigator !== 'undefined' && 'mediaSession' in navigator) {
        try { navigator.mediaSession.playbackState = state; } catch (e) {}
    }
}
"#)]
extern "C" {
    fn vgms_install_media_session(
        play: &js_sys::Function,
        pause: &js_sys::Function,
        stop: &js_sys::Function,
    ) -> bool;
    fn vgms_set_playback_state(state: &str);
}

/// A [`VgmStudioApp`] with the browser media session wired to its media-key sink.
/// Holds the action-handler closures alive and mirrors playback state each frame.
pub(crate) struct MediaSessionApp {
    inner: VgmStudioApp,
    /// The registered action handlers, kept alive for the page's lifetime.
    _handlers: Vec<Closure<dyn FnMut()>>,
    /// The playback state last published, so it is only re-sent on a change.
    playing: bool,
}

impl std::fmt::Debug for MediaSessionApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaSessionApp")
            .field("playing", &self.playing)
            .finish_non_exhaustive()
    }
}

impl MediaSessionApp {
    /// Wraps `app`, registering the media-session handlers. Returns the wrapper
    /// when the session was set up, or the plain app boxed otherwise (a browser
    /// without `mediaSession`, or one that refused).
    #[must_use]
    pub(crate) fn attach(app: VgmStudioApp, ctx: eframe::egui::Context) -> Box<dyn eframe::App> {
        let sink = app.media_keys();
        // Each handler posts its command and wakes the paused egui loop.
        let handler = |command: TransportCommand| {
            let sink = sink.clone();
            let ctx = ctx.clone();
            Closure::<dyn FnMut()>::new(move || {
                sink.send(command);
                ctx.request_repaint();
            })
        };
        let play = handler(TransportCommand::Play);
        let pause = handler(TransportCommand::Pause);
        let stop = handler(TransportCommand::Stop);
        let installed = vgms_install_media_session(
            play.as_ref().unchecked_ref(),
            pause.as_ref().unchecked_ref(),
            stop.as_ref().unchecked_ref(),
        );
        if !installed {
            return Box::new(app);
        }
        Box::new(Self {
            inner: app,
            _handlers: vec![play, pause, stop],
            playing: false,
        })
    }
}

impl eframe::App for MediaSessionApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        self.inner.ui(ui, frame);
        // Keep the browser's session state in step so it shows the controls and
        // routes their buttons.
        let playing = self.inner.is_playing();
        if playing != self.playing {
            self.playing = playing;
            vgms_set_playback_state(if playing { "playing" } else { "paused" });
        }
    }

    fn on_exit(&mut self) {
        self.inner.on_exit();
    }
}
