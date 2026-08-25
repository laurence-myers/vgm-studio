//! The seam for OS media-transport keys.
//!
//! egui/egui-winit surface no media keys, and the OS routes them system-wide to
//! whichever app owns the media session -- not to a focused window. So the
//! platform shells drive them from outside the UI loop: the native binary
//! through the Windows System Media Transport Controls (`souvlaki`), the web
//! build through `navigator.mediaSession`. Both push a [`TransportCommand`] into
//! a [`MediaKeys`] sink, which the app drains each frame and turns into an
//! ordinary [`PlaybackAction`](crate::action::PlaybackAction) -- so a media key
//! runs the exact transport path a button click does.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::action::{Action, PlaybackAction};

/// A transport command from outside the UI loop. Trivially `Copy`/`Send`, so a
/// callback on any thread (or a browser event) can post one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCommand {
    /// Start (or resume) playback.
    Play,
    /// Pause playback. Mapped to the app's Stop, which pauses (and rewinds), the
    /// same thing toggling playback off does.
    Pause,
    /// Stop playback.
    Stop,
    /// Toggle between playing and stopped -- the MediaPlayPause key.
    Toggle,
}

impl TransportCommand {
    /// The in-app playback action this command runs, so a media key takes the
    /// same path a transport button does.
    #[must_use]
    fn to_action(self) -> Action {
        Action::Playback(match self {
            Self::Play => PlaybackAction::Play,
            // Pause and Stop both land on the app's Stop (pause + rewind) --
            // there is no separate resume-in-place transport, and this matches
            // toggling playback off.
            Self::Pause | Self::Stop => PlaybackAction::Stop,
            Self::Toggle => PlaybackAction::TogglePlayback,
        })
    }
}

/// A cloneable, thread-safe sink the platform pushes [`TransportCommand`]s into.
/// The app drains it each frame with [`MediaKeys::take_actions`].
#[derive(Clone, Default)]
pub struct MediaKeys(Arc<Mutex<VecDeque<TransportCommand>>>);

impl std::fmt::Debug for MediaKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaKeys").finish_non_exhaustive()
    }
}

impl MediaKeys {
    /// Posts a command. Called from an OS media-key callback (any thread) or a
    /// browser media-session handler; a poisoned lock drops the command rather
    /// than panicking a callback the UI cannot see.
    pub fn send(&self, command: TransportCommand) {
        if let Ok(mut queue) = self.0.lock() {
            queue.push_back(command);
        }
    }

    /// Drains the queued commands into their [`Action`]s, oldest first. Called by
    /// the app at the top of each frame.
    pub(crate) fn take_actions(&self) -> Vec<Action> {
        let commands: Vec<TransportCommand> = self
            .0
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default();
        commands
            .into_iter()
            .map(TransportCommand::to_action)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_map_to_the_transport_actions() {
        let keys = MediaKeys::default();
        keys.send(TransportCommand::Play);
        keys.send(TransportCommand::Pause);
        keys.send(TransportCommand::Stop);
        keys.send(TransportCommand::Toggle);
        assert_eq!(
            keys.take_actions(),
            vec![
                Action::Playback(PlaybackAction::Play),
                Action::Playback(PlaybackAction::Stop),
                Action::Playback(PlaybackAction::Stop),
                Action::Playback(PlaybackAction::TogglePlayback),
            ]
        );
        // Draining leaves the sink empty.
        assert!(keys.take_actions().is_empty());
    }

    #[test]
    fn a_clone_shares_the_same_queue() {
        let keys = MediaKeys::default();
        let handle = keys.clone();
        handle.send(TransportCommand::Toggle);
        assert_eq!(
            keys.take_actions(),
            vec![Action::Playback(PlaybackAction::TogglePlayback)]
        );
    }
}
