// SPDX-License-Identifier: GPL-2.0-or-later
//! The `extern "C"` skin over [`crate::player`], and nothing else.
//!
//! Every export is `#[unsafe(no_mangle)] pub extern "C"`, prefixed `vgmsw_`, and
//! becomes a wasm module export the `worklet-processor.js` host calls. The host
//! moves bytes in and out through [`vgmsw_alloc`] / [`vgmsw_free`] buffers in the
//! module's own linear memory -- there is no wasm-bindgen here, because
//! `AudioWorkletGlobalScope` has no `TextDecoder`/`TextEncoder` for its glue.
//!
//! All the real work lives in [`crate::player`] as safe Rust that the native test
//! suite drives directly; this file is only the pointer plumbing, kept as thin as
//! it can be.

use vgms_core::vgm::ChipKind;
use vgms_synth::{LoopConfig, Muting, Panning};

use crate::player;

/// Allocates `len` bytes in the module's linear memory and returns the pointer,
/// for the host to write a song / name / slug into before a call. Pair every
/// call with [`vgmsw_free`].
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_alloc(len: usize) -> *mut u8 {
    // `vec![0; len]` allocates exactly `len` capacity, which `vgmsw_free`'s
    // `from_raw_parts(ptr, len, len)` relies on.
    let mut buf = vec![0u8; len];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Frees a buffer from [`vgmsw_alloc`]. `len` must be the length that was
/// allocated.
///
/// # Safety
/// `ptr`/`len` must be a buffer returned by [`vgmsw_alloc`] and not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // Safety: `ptr`/`len` came from `vgmsw_alloc`, whose capacity equals `len`.
    unsafe {
        drop(Vec::from_raw_parts(ptr, len, len));
    }
}

/// Views `len` bytes at `ptr` as a slice. Empty (and pointer-independent) when
/// `len` is zero, so a null pointer with zero length is safe.
///
/// # Safety
/// `ptr` must point to at least `len` initialised bytes that outlive `'a`.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        // Safety: the caller guarantees `ptr..ptr+len` is valid and initialised.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

/// Views `len` bytes at `ptr` as a UTF-8 string, or `None` if they are not UTF-8.
///
/// # Safety
/// As [`bytes`].
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    // Safety: forwarded to the caller's contract on `bytes`.
    std::str::from_utf8(unsafe { bytes(ptr, len) }).ok()
}

/// Installs the web core registry. Call once before the first [`vgmsw_load`].
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_init() {
    player::install_web_cores();
}

/// Records a `core.<slug> = <id>` choice, applied to the next [`vgmsw_load`].
///
/// # Safety
/// Both `(ptr, len)` pairs must describe valid initialised buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_set_core_choice(
    slug_ptr: *const u8,
    slug_len: usize,
    id_ptr: *const u8,
    id_len: usize,
) {
    // Safety: the host passes valid alloc'd buffers for both strings.
    let (Some(slug), Some(id)) = (unsafe { text(slug_ptr, slug_len) }, unsafe {
        text(id_ptr, id_len)
    }) else {
        return;
    };
    player::set_core_choice(slug, id);
}

/// Parses `name`'s `bytes` and loads them for playback at `sample_rate`, with
/// `resample` (`1` Linear, else Sinc). Returns `0` on success, negative on
/// failure -- fetch the reason with [`vgmsw_error_len`] / [`vgmsw_error_copy`].
///
/// # Safety
/// The name and byte `(ptr, len)` pairs must describe valid initialised buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_load(
    name_ptr: *const u8,
    name_len: usize,
    bytes_ptr: *const u8,
    bytes_len: usize,
    sample_rate: u32,
    resample: u32,
) -> i32 {
    // Safety: the host passes valid alloc'd buffers for the name and the song.
    let Some(name) = (unsafe { text(name_ptr, name_len) }) else {
        player::set_last_error("the file name was not valid UTF-8".to_owned());
        return -1;
    };
    let data = unsafe { bytes(bytes_ptr, bytes_len) };
    match player::load(
        name,
        data,
        sample_rate,
        player::resample_from_code(resample),
    ) {
        Ok(()) => 0,
        Err(message) => {
            player::set_last_error(message);
            -2
        }
    }
}

/// The byte length of the most recent load error (0 after a success).
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_error_len() -> usize {
    player::last_error().len()
}

/// Copies up to `cap` bytes of the most recent load error into `out`, returning
/// the number written. The host sizes `out` with [`vgmsw_error_len`] first.
///
/// # Safety
/// `out` must point to at least `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_error_copy(out: *mut u8, cap: usize) -> usize {
    let message = player::last_error();
    let n = message.len().min(cap);
    if n > 0 && !out.is_null() {
        // Safety: the host guarantees `out` holds at least `cap` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(message.as_ptr(), out, n);
        }
    }
    n
}

/// Renders one quantum of `frames` into the planar f32 buffers at `left`/`right`
/// (each `frames` long). Returns the number of frames the engine sounded.
///
/// # Safety
/// `left` and `right` must each point to at least `frames` writable f32s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_render(left: *mut f32, right: *mut f32, frames: usize) -> usize {
    if frames == 0 || left.is_null() || right.is_null() {
        return 0;
    }
    // Safety: the host owns `frames`-long f32 buffers at both pointers.
    let (left, right) = unsafe {
        (
            std::slice::from_raw_parts_mut(left, frames),
            std::slice::from_raw_parts_mut(right, frames),
        )
    };
    player::render(left, right)
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_seek_ms(ms: u32) {
    player::seek_ms(ms);
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_seek_pos(pos: usize) {
    player::seek_pos(pos);
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_rewind() {
    player::rewind();
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_set_boost(boost: f32) {
    player::set_boost(boost);
}

/// Sets (or, with `enabled == 0`, clears) the loop region. The count is the
/// `(tag, times)` pair [`crate::player::loop_count`] decodes; `start_frames` is
/// the frame position of `start`, carried as `f64` because it crosses as a JS
/// number.
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_set_loop(
    enabled: u32,
    start: usize,
    end: usize,
    count_tag: u32,
    count_times: u32,
    start_frames: f64,
) {
    let config = (enabled != 0).then(|| LoopConfig {
        start,
        end,
        count: player::loop_count(count_tag, count_times),
        start_frames: start_frames.max(0.0) as u64,
    });
    player::set_loop(config);
}

/// Replaces the OPL muting from its two raw primitives (a no-op on a non-OPL
/// song). See [`Muting::from_raw`].
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_set_muting(channels: u32, percussion_low: u8, percussion_high: u8) {
    player::set_muting(Muting::from_raw(
        channels,
        [percussion_low, percussion_high],
    ));
}

/// Replaces the OPL panning: `mode == 1` reads 18 pan bytes at `pans_ptr` as a
/// `Custom` image, anything else is `Original` (a no-op on a non-OPL song).
///
/// # Safety
/// When `mode == 1`, `pans_ptr` must point to at least 18 initialised bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_set_panning(mode: u32, pans_ptr: *const u8, pans_len: usize) {
    let panning = if mode == 1 && pans_len >= 18 {
        // Safety: the host passes an 18-byte buffer when mode is Custom.
        let pans = unsafe { bytes(pans_ptr, 18) };
        let mut array = [0u8; 18];
        array.copy_from_slice(&pans[..18]);
        Panning::Custom(array)
    } else {
        Panning::Original
    };
    player::set_panning(panning);
}

/// Sets one chip instance's channel mute mask (the generic engine's muting; a
/// no-op on an OPL song). `slug` names the chip, e.g. `sn76489`.
///
/// # Safety
/// `(slug_ptr, slug_len)` must describe a valid initialised buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_set_chip_mute(
    slug_ptr: *const u8,
    slug_len: usize,
    instance: u8,
    mask: u32,
) {
    // Safety: the host passes a valid slug buffer.
    let Some(kind) = (unsafe { text(slug_ptr, slug_len) }).and_then(ChipKind::from_slug) else {
        return;
    };
    player::set_chip_mute(kind, instance, mask);
}

/// Sets one chip instance's channel pan positions (the generic engine's panning;
/// a no-op on an OPL song). `pans` is `len` little-endian `i16`s.
///
/// # Safety
/// Both `(ptr, len)` pairs must describe valid initialised buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vgmsw_set_chip_pan(
    slug_ptr: *const u8,
    slug_len: usize,
    instance: u8,
    pans_ptr: *const u8,
    pans_len: usize,
) {
    // Safety: the host passes a valid slug and a `pans_len`-byte pan buffer.
    let Some(kind) = (unsafe { text(slug_ptr, slug_len) }).and_then(ChipKind::from_slug) else {
        return;
    };
    let raw = unsafe { bytes(pans_ptr, pans_len) };
    let pans: Vec<i16> = raw
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    player::set_chip_pan(kind, instance, pans);
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_position_frames() -> f64 {
    player::position_frames()
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_position_ms() -> u32 {
    player::position_ms()
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_position_row() -> u32 {
    player::position_row()
}

#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_loop_iteration() -> u32 {
    player::loop_iteration()
}

/// `1` if the loaded song has played to its end, else `0`.
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_is_finished() -> u32 {
    u32::from(player::is_finished())
}

/// The loudest post-limiter peak on `channel` (0 left, else right) since the last
/// call, `0.0..=1.0`. Destructive: reported once.
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_take_peak(channel: u32) -> f32 {
    player::take_peak(channel)
}

/// `1` if the limiter engaged since the last call, else `0`. Destructive.
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_take_limited() -> u32 {
    u32::from(player::take_limited())
}

/// The lowest boost at which the limiter has engaged since the song loaded, or
/// `0.0` for "never".
#[unsafe(no_mangle)]
pub extern "C" fn vgmsw_min_engaged_boost() -> f32 {
    player::min_engaged_boost()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives a load and render entirely through the raw `extern "C"` surface,
    /// proving the pointer plumbing (alloc, copy-in, render into caller buffers)
    /// works end to end -- not just the safe layer the other tests exercise.
    #[test]
    fn a_song_loads_and_renders_through_the_raw_abi() {
        const OPL_VGM: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
        vgmsw_init();

        // Copy the name and the bytes into module memory, as the host would.
        let name = b"lsl3_score_up.vgm";
        let name_ptr = vgmsw_alloc(name.len());
        let bytes_ptr = vgmsw_alloc(OPL_VGM.len());
        // Safety: fresh buffers of exactly these lengths; render into buffers we
        // own; free each buffer once. Every call meets the ABI's contract.
        unsafe {
            std::ptr::copy_nonoverlapping(name.as_ptr(), name_ptr, name.len());
            std::ptr::copy_nonoverlapping(OPL_VGM.as_ptr(), bytes_ptr, OPL_VGM.len());

            let code = vgmsw_load(name_ptr, name.len(), bytes_ptr, OPL_VGM.len(), 48_000, 0);
            assert_eq!(code, 0, "load succeeds (error: {})", player::last_error());

            vgmsw_free(name_ptr, name.len());
            vgmsw_free(bytes_ptr, OPL_VGM.len());

            // Render a second of audio into planar buffers and prove it sounded.
            let frames = 128usize;
            let left = vgmsw_alloc(frames * 4).cast::<f32>();
            let right = vgmsw_alloc(frames * 4).cast::<f32>();
            let mut peak = 0.0f32;
            for _ in 0..(48_000 / frames) {
                vgmsw_render(left, right, frames);
                let l = std::slice::from_raw_parts(left, frames);
                let r = std::slice::from_raw_parts(right, frames);
                for &s in l.iter().chain(r.iter()) {
                    peak = peak.max(s.abs());
                }
            }
            assert!(peak > 0.01, "the raw ABI rendered audible output");
            assert!(vgmsw_position_frames() > 0.0, "the position advanced");

            vgmsw_free(left.cast::<u8>(), frames * 4);
            vgmsw_free(right.cast::<u8>(), frames * 4);
        }
    }
}
