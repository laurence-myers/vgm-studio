// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebPackService`]: present because the app takes one, but inert until a pack
//! backend exists.
//!
//! Pack mode is unreachable in the browser until the File System Access and
//! zip-pack backends land (the pack export and the PNG optimiser then arrive with
//! them). Until then this answers every job with the honest "not available"
//! outcome rather than pretending to build a zip. Only [`today`](PackService::today)
//! does real work -- it is a pure date and the prefilled history line wants it.

use std::cell::RefCell;
use std::collections::VecDeque;

use vgms_ui::platform::{OptimizedImage, PackJobOutcome, PackJobRequest, PackService};

/// The message pack jobs answer with until a pack backend exists.
const NO_PACKS_YET: &str = "Building release packs is not available in this browser yet.";

/// A no-op pack service that reports jobs as unavailable and answers `today`.
#[derive(Debug, Default)]
pub struct WebPackService {
    jobs: RefCell<VecDeque<PackJobOutcome>>,
    optimized: RefCell<VecDeque<Result<OptimizedImage, String>>>,
}

impl WebPackService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl PackService for WebPackService {
    fn submit(&mut self, _request: PackJobRequest) {
        self.jobs
            .borrow_mut()
            .push_back(PackJobOutcome::Failed(NO_PACKS_YET.to_owned()));
    }

    fn poll(&mut self) -> Option<PackJobOutcome> {
        self.jobs.borrow_mut().pop_front()
    }

    fn is_busy(&self) -> bool {
        false
    }

    fn cancel(&mut self) {}

    fn optimize(&mut self, _name: String, _bytes: Vec<u8>) {
        self.optimized
            .borrow_mut()
            .push_back(Err(NO_PACKS_YET.to_owned()));
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
