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

//! Expressions compiled once per oxide runtime, kept by their text's hash ([`crate::script::text::hash_text`]) until
//! F1 makes the cache process-wide (plans/Script-Engine-Oxide.md section 6).
//!
//! quickjs-oxide keeps a script compiled the default way for the runtime's life, in its static code store, where the
//! loop runs it fastest; transient code is freed once nothing names it, at about 5% more instructions. So the first
//! [`MAX_STATIC_UNITS`] texts (and [`MAX_STATIC_BYTES`] of source) are compiled static and kept for good, and later
//! texts transient, in a bounded cache: a text built anew on every call cannot grow the runtime without bound.

use std::collections::HashMap;
use std::sync::Arc;

use quickjs_oxide::engine::api::core::{CompileOptions, Context, FunctionBytecodeRef, RuntimeError};

use crate::script::BoundedMap;

/// Texts compiled to static code at most, and their source bytes.
const MAX_STATIC_UNITS: usize = 4096;
const MAX_STATIC_BYTES: usize = 4 * 1024 * 1024;
/// Transient texts kept, past the static ones.
const MAX_TRANSIENT: usize = 512;
const MAX_TRANSIENT_BYTES: usize = 1024 * 1024;

/// The name a compiled expression has in stack traces, as on QuickJS.
pub(super) const FILE_NAME: &str = "eval_script";

pub(super) struct Compiled {
    fixed: HashMap<u64, (Arc<str>, FunctionBytecodeRef)>,
    fixed_bytes: usize,
    transient: BoundedMap<u64, (Arc<str>, FunctionBytecodeRef)>,
    compiles: u64,
}

impl Compiled {
    pub(super) fn new() -> Self {
        Self {
            fixed: HashMap::new(),
            fixed_bytes: 0,
            transient: BoundedMap::with_bytes(MAX_TRANSIENT, MAX_TRANSIENT_BYTES),
            compiles: 0,
        }
    }

    /// The compiled form of `text`, from the cache or compiled now.
    pub(super) fn get_or_compile(
        &mut self,
        context: &Context,
        text: &str,
        hash: u64,
        file_name: &str,
    ) -> Result<FunctionBytecodeRef, RuntimeError> {
        if let Some((kept, compiled)) = self.fixed.get(&hash) {
            if **kept == *text {
                return Ok(compiled.clone());
            }
        }
        if let Some((kept, compiled)) = self.transient.get(&hash) {
            if *kept == *text {
                return Ok(compiled);
            }
        }
        self.compiles += 1;
        let fixed = self.fixed.len() < MAX_STATIC_UNITS
            && self.fixed_bytes + text.len() <= MAX_STATIC_BYTES
            && !self.fixed.contains_key(&hash);
        let options = CompileOptions::new(file_name);
        let options = if fixed { options } else { options.transient() };
        let compiled = context.compile_with_options(text, &options)?;
        if fixed {
            self.fixed_bytes += text.len();
            self.fixed.insert(hash, (Arc::from(text), compiled.clone()));
        } else {
            self.transient
                .insert_sized(hash, (Arc::from(text), compiled.clone()), text.len());
        }
        Ok(compiled)
    }

    /// Texts kept, and their source bytes.
    pub(super) fn len(&self) -> usize {
        self.fixed.len() + self.transient.len()
    }

    pub(super) fn bytes(&self) -> usize {
        self.fixed_bytes + self.transient.bytes()
    }

    /// Texts compiled by this runtime.
    pub(super) fn compiles(&self) -> u64 {
        self.compiles
    }

    /// Let the transient code go, for a runtime short of memory. The static code stays in the runtime whatever the
    /// cache holds, so its entries stay too.
    pub(super) fn clear_transient(&mut self) {
        self.transient.clear();
    }
}
