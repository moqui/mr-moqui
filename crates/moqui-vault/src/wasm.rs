/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The browser module's raw `extern "C"` ABI (plans/archive/Vault-Instances.md §3.6), modelled on moqui-browser's exports:
//! the worker copies a JSON request into a buffer from [`moqui_vault_alloc`], calls [`moqui_vault_call`], reads the
//! answer by its handle ([`moqui_vault_result_ptr`], [`moqui_vault_result_len`]) and frees it
//! ([`moqui_vault_result_free`], which zeroes it first). Randomness is the page's `crypto.getRandomValues`, imported
//! as `env.moqui_vault_random`. This file is the crate's only `unsafe`: the pointers the worker passes in.

use std::cell::RefCell;
use std::collections::BTreeMap;

use zeroize::Zeroize as _;

use crate::ops::Session;
use crate::rng::Rng;

#[link(wasm_import_module = "env")]
extern "C" {
    /// Fills `len` bytes at `ptr` from `crypto.getRandomValues`.
    fn moqui_vault_random(ptr: *mut u8, len: usize);
}

/// The page's random source.
struct PageRng;

impl Rng for PageRng {
    fn fill(&mut self, out: &mut [u8]) {
        // SAFETY: `out` is a live, writable slice of `out.len()` bytes in this module's memory, which is all the
        // import writes; it returns before anything else touches the slice.
        unsafe { moqui_vault_random(out.as_mut_ptr(), out.len()) }
    }
}

#[derive(Default)]
struct Results {
    next: u32,
    held: BTreeMap<u32, Vec<u8>>,
}

thread_local! {
    static SESSION: RefCell<Session> = RefCell::new(Session::default());
    static RESULTS: RefCell<Results> = RefCell::new(Results::default());
}

/// A buffer of `len` bytes for a request. Free it with [`moqui_vault_free`].
#[no_mangle]
pub extern "C" fn moqui_vault_alloc(len: usize) -> *mut u8 {
    let mut buffer = vec![0_u8; len.max(1)].into_boxed_slice();
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Zeroes and frees a buffer from [`moqui_vault_alloc`]: a request may hold a passphrase.
///
/// # Safety
///
/// `ptr` and `len` must be exactly what [`moqui_vault_alloc`] returned and was given.
// SAFETY: an `unsafe fn` because it takes back a buffer by its raw pointer and length; the worker calls it only with
// what moqui_vault_alloc returned and was given, once per buffer.
#[no_mangle]
pub unsafe extern "C" fn moqui_vault_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr came from moqui_vault_alloc(len), which leaked a boxed slice of len.max(1) bytes.
    let mut buffer = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len.max(1))) };
    buffer.zeroize();
}

/// Answers the JSON request at `ptr`; returns the handle of the JSON answer.
///
/// # Safety
///
/// `ptr` must point at `len` readable bytes, or `len` must be 0.
// SAFETY: an `unsafe fn` because it reads the request through a raw pointer and length; the worker calls it with a
// buffer from moqui_vault_alloc that it wrote `len` bytes into.
#[no_mangle]
pub unsafe extern "C" fn moqui_vault_call(ptr: *const u8, len: usize) -> u32 {
    let request: &[u8] = if len == 0 || ptr.is_null() {
        &[]
    } else {
        // SAFETY: the caller wrote `len` bytes at `ptr`, from moqui_vault_alloc.
        unsafe { std::slice::from_raw_parts(ptr, len) }
    };
    let answer = match std::str::from_utf8(request) {
        Ok(text) => SESSION.with(|session| session.borrow_mut().call(text, &mut PageRng)),
        Err(_) => r#"{"ok":false,"error":"bad format: the request is not UTF-8"}"#.to_string(),
    };
    RESULTS.with(|results| {
        let mut results = results.borrow_mut();
        results.next = results.next.wrapping_add(1).max(1);
        let handle = results.next;
        results.held.insert(handle, answer.into_bytes());
        handle
    })
}

/// Where the answer for `handle` starts, or null for an unknown handle.
#[no_mangle]
pub extern "C" fn moqui_vault_result_ptr(handle: u32) -> *const u8 {
    RESULTS.with(|results| {
        results
            .borrow()
            .held
            .get(&handle)
            .map_or(std::ptr::null(), |bytes| bytes.as_ptr())
    })
}

/// How many bytes the answer for `handle` holds.
#[no_mangle]
pub extern "C" fn moqui_vault_result_len(handle: u32) -> usize {
    RESULTS.with(|results| results.borrow().held.get(&handle).map_or(0, Vec::len))
}

/// Zeroes and releases the answer for `handle`: an answer may hold an item's body.
#[no_mangle]
pub extern "C" fn moqui_vault_result_free(handle: u32) {
    RESULTS.with(|results| {
        if let Some(mut bytes) = results.borrow_mut().held.remove(&handle) {
            bytes.zeroize();
        }
    });
}
