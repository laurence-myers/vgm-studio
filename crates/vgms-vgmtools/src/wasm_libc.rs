// SPDX-License-Identifier: GPL-2.0-or-later
//! The libc surface the vgmtools optimisers expect, provided on
//! `wasm32-unknown-unknown`.
//!
//! The target has no libc, so `shim/wasm-libc/` declares what the tools include
//! and this module supplies the symbols the C references but the compiler does
//! not:
//!
//! - **The allocator family** forwards to Rust's own allocator. C's `free`
//!   arrives without a size and Rust's `dealloc` demands one, so every
//!   allocation carries a 16-byte header holding its size -- 16 rather than 8 so
//!   the pointer handed to C keeps `malloc`'s 16-byte alignment. `chip_srom.c`
//!   reallocs its ~50 ROM buffers hard; this is the real allocator the plan
//!   leans on rather than a bump arena.
//! - **The `str*` family** is implemented directly. The `mem*` family needs
//!   nothing here -- `compiler_builtins` carries it on every target.
//! - **printf/sprintf** live in `shim/wasm_printf.c` and the **`FILE*`** layer in
//!   `shim/memfile.c` (variadics and struct layout are cleaner in C).
//!
//! This is `vgms-cores-libvgm/src/wasm_libc.rs` trimmed to the tools' needs
//! (no `math`, no `rand`) and grown by `strchr`/`strrchr`/`strcasecmp`/
//! `strncasecmp`, which the tools and `zshim.c` use and libvgm's cores did not.
//!
//! Everything is `#[unsafe(no_mangle)]` C ABI, resolved when the tool `.wasm`
//! links. Nothing else on the target defines these names: Rust's own wasm
//! allocator keeps its dlmalloc internal and exports no C symbols.

// `pub` documents that these are an exported ABI surface, which
// `unreachable_pub` cannot see.
#![allow(unreachable_pub)]

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc};
use std::ffi::{c_char, c_int};

/// Bytes reserved ahead of every allocation for its size, preserving the
/// 16-byte alignment C is entitled to assume.
const HEADER: usize = 16;

/// The layout for a C allocation of `size` payload bytes.
fn layout_for(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HEADER)?, HEADER).ok()
}

/// # Safety
/// C ABI `malloc`: the returned pointer is only valid until [`free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    let Some(layout) = layout_for(size) else {
        return std::ptr::null_mut();
    };
    // SAFETY: the layout is non-zero (HEADER is added) and valid.
    let base = unsafe { alloc(layout) };
    if base.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `base` is at least HEADER + size bytes; the header stores the
    // payload size for `free`/`realloc` to rebuild the layout.
    unsafe {
        base.cast::<usize>().write(size);
        base.add(HEADER)
    }
}

/// # Safety
/// C ABI `calloc`, zero-filled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(count: usize, size: usize) -> *mut u8 {
    let Some(total) = count.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    let Some(layout) = layout_for(total) else {
        return std::ptr::null_mut();
    };
    // SAFETY: as in `malloc`, but zeroed -- the header is rewritten after.
    let base = unsafe { alloc_zeroed(layout) };
    if base.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: as in `malloc`.
    unsafe {
        base.cast::<usize>().write(total);
        base.add(HEADER)
    }
}

/// # Safety
/// C ABI `free`: `ptr` must be null or something this module handed out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: `ptr` came from `malloc`/`calloc`/`realloc` above, so the size
    // header sits HEADER bytes before it and the layout reconstructs exactly.
    unsafe {
        let base = ptr.sub(HEADER);
        let size = base.cast::<usize>().read();
        let layout = layout_for(size).unwrap_or_else(|| unreachable!("layout was valid at alloc"));
        dealloc(base, layout);
    }
}

/// # Safety
/// C ABI `realloc`: `ptr` must be null or something this module handed out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        // SAFETY: forwarding to our own `malloc`.
        return unsafe { malloc(size) };
    }
    // SAFETY: the size header, as in `free`.
    let old_size = unsafe { ptr.sub(HEADER).cast::<usize>().read() };
    // SAFETY: our own allocator entry points; the copy stays within both
    // allocations' payloads.
    unsafe {
        let fresh = malloc(size);
        if !fresh.is_null() {
            std::ptr::copy_nonoverlapping(ptr, fresh, old_size.min(size));
            free(ptr);
        }
        fresh
    }
}

/// # Safety
/// C ABI `strlen`: `s` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strlen(s: *const c_char) -> usize {
    let mut len = 0usize;
    // SAFETY: the caller guarantees termination.
    while unsafe { s.add(len).read() } != 0 {
        len += 1;
    }
    len
}

/// # Safety
/// C ABI `strcmp`: both arguments must be NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcmp(a: *const c_char, b: *const c_char) -> c_int {
    let mut at = 0usize;
    loop {
        // SAFETY: the caller guarantees termination of both.
        let (x, y) = unsafe { (a.add(at).read() as u8, b.add(at).read() as u8) };
        if x != y || x == 0 {
            return c_int::from(x) - c_int::from(y);
        }
        at += 1;
    }
}

/// # Safety
/// C ABI `strncmp`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int {
    for at in 0..n {
        // SAFETY: the caller guarantees `n` readable bytes or termination.
        let (x, y) = unsafe { (a.add(at).read() as u8, b.add(at).read() as u8) };
        if x != y || x == 0 {
            return c_int::from(x) - c_int::from(y);
        }
    }
    0
}

/// ASCII lowercase, for the case-insensitive comparisons.
const fn lower(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

/// # Safety
/// C ABI `strcasecmp` (`common.h`'s `stricmp`): NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcasecmp(a: *const c_char, b: *const c_char) -> c_int {
    let mut at = 0usize;
    loop {
        // SAFETY: the caller guarantees termination of both.
        let (x, y) = unsafe { (lower(a.add(at).read() as u8), lower(b.add(at).read() as u8)) };
        if x != y || x == 0 {
            return c_int::from(x) - c_int::from(y);
        }
        at += 1;
    }
}

/// # Safety
/// C ABI `strncasecmp` (`common.h`'s `strnicmp`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strncasecmp(a: *const c_char, b: *const c_char, n: usize) -> c_int {
    for at in 0..n {
        // SAFETY: the caller guarantees `n` readable bytes or termination.
        let (x, y) = unsafe { (lower(a.add(at).read() as u8), lower(b.add(at).read() as u8)) };
        if x != y || x == 0 {
            return c_int::from(x) - c_int::from(y);
        }
    }
    0
}

/// # Safety
/// C ABI `strcpy`: `dest` must have room for `src` and its terminator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcpy(dest: *mut c_char, src: *const c_char) -> *mut c_char {
    let mut at = 0usize;
    // SAFETY: per the C contract on both pointers.
    unsafe {
        loop {
            let byte = src.add(at).read();
            dest.add(at).write(byte);
            if byte == 0 {
                return dest;
            }
            at += 1;
        }
    }
}

/// # Safety
/// C ABI `strncpy`, padding with NULs as the standard demands.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strncpy(dest: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
    let mut at = 0usize;
    // SAFETY: per the C contract on both pointers.
    unsafe {
        while at < n {
            let byte = src.add(at).read();
            dest.add(at).write(byte);
            at += 1;
            if byte == 0 {
                break;
            }
        }
        while at < n {
            dest.add(at).write(0);
            at += 1;
        }
    }
    dest
}

/// # Safety
/// C ABI `strcat`: `dest` must be NUL-terminated with room for `src`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strcat(dest: *mut c_char, src: *const c_char) -> *mut c_char {
    // SAFETY: per the C contract; both calls are to our own functions.
    unsafe {
        let end = strlen(dest);
        strcpy(dest.add(end), src);
    }
    dest
}

/// # Safety
/// C ABI `strdup`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strdup(s: *const c_char) -> *mut c_char {
    // SAFETY: `s` is NUL-terminated per the C contract; the copy fits the
    // fresh allocation by construction.
    unsafe {
        let len = strlen(s) + 1;
        let fresh = malloc(len).cast::<c_char>();
        if !fresh.is_null() {
            std::ptr::copy_nonoverlapping(s, fresh, len);
        }
        fresh
    }
}

/// # Safety
/// C ABI `strchr`: `s` must be NUL-terminated. Returns a pointer to the first
/// `c`, or null. Matching the terminator (`c == 0`) returns the terminator, as
/// C requires and `zshim.c`'s mode sniffing relies on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strchr(s: *const c_char, c: c_int) -> *mut c_char {
    let needle = c as u8;
    let mut at = 0usize;
    // SAFETY: the caller guarantees termination.
    unsafe {
        loop {
            let byte = s.add(at).read() as u8;
            if byte == needle {
                return s.add(at) as *mut c_char;
            }
            if byte == 0 {
                return std::ptr::null_mut();
            }
            at += 1;
        }
    }
}

/// # Safety
/// C ABI `strrchr`: the *last* `c`, or null. Used to split a file extension.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strrchr(s: *const c_char, c: c_int) -> *mut c_char {
    let needle = c as u8;
    let mut at = 0usize;
    let mut found: *mut c_char = std::ptr::null_mut();
    // SAFETY: the caller guarantees termination.
    unsafe {
        loop {
            let byte = s.add(at).read() as u8;
            if byte == needle {
                found = s.add(at) as *mut c_char;
            }
            if byte == 0 {
                return found;
            }
            at += 1;
        }
    }
}
