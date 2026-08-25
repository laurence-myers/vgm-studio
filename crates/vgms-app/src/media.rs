//! Windows System Media Transport Controls: OS media-transport keys drive the
//! app's transport, even while it is unfocused.
//!
//! [`souvlaki`] receives the keys through the media session (so it never steals
//! them from another player) and needs the playback *status* published back, or
//! the OS neither shows the controls nor routes their buttons. So this wraps the
//! app: it keeps the [`MediaControls`] alive, posts each transport key into the
//! app's [`MediaKeys`] sink, and mirrors playback status to the OS each frame.
//!
//! Windows-only; the web build drives the same [`MediaKeys`] sink through
//! `navigator.mediaSession`.

use std::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use souvlaki::{MediaControlEvent, MediaControls, MediaPlayback, PlatformConfig};
use vgms_ui::{MediaKeys, TransportCommand, VgmStudioApp};

/// A [`VgmStudioApp`] with the Windows media session attached.
pub struct MediaKeyApp {
    inner: VgmStudioApp,
    controls: MediaControls,
    /// The playback status last published to the OS, so it is only re-sent on a
    /// change rather than every frame.
    playing: bool,
}

impl std::fmt::Debug for MediaKeyApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `MediaControls` is not Debug (it wraps OS handles).
        f.debug_struct("MediaKeyApp")
            .field("playing", &self.playing)
            .finish_non_exhaustive()
    }
}

impl MediaKeyApp {
    /// Wraps `app` with the media session bound to `window`, returning a boxed
    /// `eframe::App` either way: the wrapper when the session was created, or the
    /// plain app when it could not be (then the app simply has no media keys).
    ///
    /// `repaint` wakes the paused event loop when a key arrives.
    #[must_use]
    pub fn attach(
        app: VgmStudioApp,
        window: &impl HasWindowHandle,
        repaint: impl Fn() + Send + 'static,
    ) -> Box<dyn eframe::App> {
        // Build the session against the app's sink before moving the app, so a
        // failure simply leaves the plain app to run without media keys.
        match make_controls(window, app.media_keys(), repaint) {
            Some(controls) => Box::new(Self {
                inner: app,
                controls,
                playing: false,
            }),
            None => Box::new(app),
        }
    }
}

/// Creates the media session, attaching `sink` as its key handler. `None` if
/// there is no Win32 window handle or `souvlaki` refuses.
fn make_controls(
    window: &impl HasWindowHandle,
    sink: MediaKeys,
    repaint: impl Fn() + Send + 'static,
) -> Option<MediaControls> {
    let config = PlatformConfig {
        dbus_name: "vgmstudio",
        display_name: "VGM Studio",
        hwnd: Some(hwnd_of(window)?),
    };
    let mut controls = MediaControls::new(config).ok()?;
    // The callback fires on the OS message pump; it only touches the thread-safe
    // sink and wakes the loop -- never the app directly.
    controls
        .attach(move |event| {
            if let Some(command) = to_command(&event) {
                sink.send(command);
                repaint();
            }
        })
        .ok()?;
    // Publish an initial status so the OS shows the controls and enables the Play
    // button; the per-frame reconcile keeps it in step afterwards.
    let _ = controls.set_playback(MediaPlayback::Paused { progress: None });
    Some(controls)
}

/// The transport command an OS media button maps to, or `None` for the events
/// this transport-only integration ignores (Next/Previous, seek, and the rest).
fn to_command(event: &MediaControlEvent) -> Option<TransportCommand> {
    match event {
        MediaControlEvent::Play => Some(TransportCommand::Play),
        MediaControlEvent::Pause => Some(TransportCommand::Pause),
        MediaControlEvent::Stop => Some(TransportCommand::Stop),
        MediaControlEvent::Toggle => Some(TransportCommand::Toggle),
        _ => None,
    }
}

/// The raw `HWND` behind a window handle, or `None` if it is not a Win32 one.
fn hwnd_of(window: &impl HasWindowHandle) -> Option<*mut c_void> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
        _ => None,
    }
}

impl eframe::App for MediaKeyApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        self.inner.ui(ui, frame);
        // Mirror playback status to the OS so the media overlay reads right and
        // the correct button (Play vs Pause) is the active one.
        let playing = self.inner.is_playing();
        if playing != self.playing {
            self.playing = playing;
            let status = if playing {
                MediaPlayback::Playing { progress: None }
            } else {
                MediaPlayback::Paused { progress: None }
            };
            let _ = self.controls.set_playback(status);
        }
    }

    fn on_exit(&mut self) {
        self.inner.on_exit();
    }
}
