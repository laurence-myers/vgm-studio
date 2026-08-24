//! Pack mode: preparing a VGMRips submission from a folder of songs.
//!
//! [`PackState`] is the headless core -- the loaded folder, the editable package
//! metadata, and the derived track list -- with no egui, so it is testable
//! without a window (like [`crate::editor::Editor`]). [`show`] draws the view.
//!
//! The description file *is* the project: opening a folder re-parses any
//! `Game Name.txt` back into the form, so a pack can be reopened and updated.
//! When there is no description (a fresh pack), the fields are prefilled from the
//! songs' GD3 tags.
//!
//! The module is split into three files: `state` (the headless model and its
//! tests), `tags` (the bulk-tag overlay model) and `view` (the egui view). The
//! items each file exports keep their original `pack::` paths through the
//! re-exports below, so nothing downstream moves.

mod state;
mod tags;
mod view;

pub use state::{
    PackImage, PackMutation, PackSection, PackSong, PackState, PackTrack, PackTransaction,
    PackValidations, TrackOptimizeStatus, reorder_renames,
};
pub use tags::{BulkTagOverlay, seed_from_meta};
pub use view::{deck, show};
