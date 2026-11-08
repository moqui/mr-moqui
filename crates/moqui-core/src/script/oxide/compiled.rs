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

//! Scripts compiled once per oxide runtime, kept by their text's hash ([`crate::script::text::hash_text`]), and their
//! code images kept for the process ([`IMAGES`]; the fork's F1 step 1, plans/archive/Script-Engine-Oxide.md section 6): a
//! runtime that has not compiled a text loads its image (`load_image`, no parse) instead of compiling it.
//!
//! quickjs-oxide keeps a script compiled the default way for the runtime's life, in its static code store, where the
//! loop runs it fastest; transient code is freed once nothing names it, at about 5% more instructions. So the first
//! [`MAX_STATIC_UNITS`] texts (and [`MAX_STATIC_BYTES`] of source) are compiled static and kept for good, and later
//! texts transient, in a bounded cache: a text built anew on every call cannot grow the runtime without bound.
//!
//! An image is made at a text's second sighting in the process, so a text that runs once (one built anew on each call)
//! makes none: the second run of a static text in a runtime encodes the code that runtime compiled (`encode_image`,
//! no parse), and a runtime that compiles a text another runtime compiled before makes the image as it compiles. A
//! loaded image is static code; a transient text is neither imaged nor loaded.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, LazyLock, Mutex};

use quickjs_oxide::engine::api::core::{CodeImage, CompileOptions, Context, FunctionBytecodeRef, RuntimeError};

use crate::script::BoundedMap;

/// Texts compiled to static code at most, and their source bytes.
const MAX_STATIC_UNITS: usize = 4096;
const MAX_STATIC_BYTES: usize = 4 * 1024 * 1024;
/// Transient texts kept, past the static ones.
const MAX_TRANSIENT: usize = 512;
const MAX_TRANSIENT_BYTES: usize = 1024 * 1024;

/// The name a compiled expression has in stack traces, as on QuickJS.
pub(super) const FILE_NAME: &str = "eval_script";

/// The images the process keeps, by the hash of their text plus strictness, as `runtime/compiled.rs` keys its
/// `SHARED_BYTECODE`: a hit compares the text and file name. The byte cap is that cache's 64 MiB.
type Images = BoundedMap<(u64, bool), (Arc<str>, Arc<str>, CodeImage)>;

const MAX_IMAGES: usize = 4 * MAX_STATIC_UNITS;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

static IMAGES: LazyLock<Mutex<Images>> =
    LazyLock::new(|| Mutex::new(BoundedMap::with_bytes(MAX_IMAGES, MAX_IMAGE_BYTES)));

/// The texts some runtime compiled once and no image holds yet, by the same key: a second sighting makes the image.
static SEEN: LazyLock<Mutex<BoundedMap<(u64, bool), ()>>> = LazyLock::new(|| Mutex::new(BoundedMap::new(64 * 1024)));

/// Oxide compiles run sloppy, as QuickJS's expressions and actions do; the key keeps the strictness as QuickJS's does.
const STRICT: bool = false;

/// What the process's images have done, for the engine stats.
pub(super) mod counts {
    use super::{AtomicU64, AtomicUsize};
    /// Images made (encoded) and put in [`super::IMAGES`].
    pub(crate) static MADE: AtomicU64 = AtomicU64::new(0);
    /// Images a runtime loaded instead of compiling: each a parse avoided.
    pub(crate) static LOADS: AtomicU64 = AtomicU64::new(0);
    /// Images that did not encode or load, after which the text was compiled.
    pub(crate) static FAILED: AtomicU64 = AtomicU64::new(0);
    /// The bytes [`super::IMAGES`] holds, as last changed.
    pub(crate) static BYTES: AtomicUsize = AtomicUsize::new(0);
}

/// The process's images: made, loaded instead of a compile, failed, and the bytes kept now.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ImageStats {
    pub(crate) made: u64,
    pub(crate) loads: u64,
    pub(crate) failed: u64,
    pub(crate) bytes: usize,
}

pub(crate) fn image_stats() -> ImageStats {
    ImageStats {
        made: counts::MADE.load(Relaxed),
        loads: counts::LOADS.load(Relaxed),
        failed: counts::FAILED.load(Relaxed),
        bytes: counts::BYTES.load(Relaxed),
    }
}

/// A static text of this runtime: its code, and whether the process has its image (or this runtime tried to make one).
struct Fixed {
    text: Arc<str>,
    /// The name it has in stack traces: a hit compares it with the text, so code and image keep their name.
    file_name: Arc<str>,
    compiled: FunctionBytecodeRef,
    imaged: bool,
}

pub(super) struct Compiled {
    fixed: HashMap<u64, Fixed>,
    fixed_bytes: usize,
    /// By hash: the text, its file name and its code.
    transient: BoundedMap<u64, (Arc<str>, Arc<str>, FunctionBytecodeRef)>,
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

    /// The compiled form of `text`, from the cache, loaded from the process's image, or compiled now.
    pub(super) fn get_or_compile(
        &mut self,
        context: &Context,
        text: &str,
        hash: u64,
        file_name: &str,
    ) -> Result<FunctionBytecodeRef, RuntimeError> {
        if let Some(fixed) = self.fixed.get_mut(&hash) {
            if *fixed.text == *text && *fixed.file_name == *file_name {
                if !fixed.imaged {
                    // the second run of the text: its image, from the code compiled already
                    fixed.imaged = true;
                    publish(context, &fixed.compiled, text, hash, file_name);
                }
                return Ok(fixed.compiled.clone());
            }
        }
        if let Some((kept, name, compiled)) = self.transient.get(&hash) {
            if *kept == *text && *name == *file_name {
                return Ok(compiled);
            }
        }
        let fixed = self.fixed.len() < MAX_STATIC_UNITS
            && self.fixed_bytes + text.len() <= MAX_STATIC_BYTES
            && !self.fixed.contains_key(&hash);
        if !fixed {
            self.compiles += 1;
            let compiled = context.compile_with_options(text, &CompileOptions::new(file_name).transient())?;
            self.transient
                .insert_sized(hash, (Arc::from(text), Arc::from(file_name), compiled.clone()), text.len());
            return Ok(compiled);
        }
        let (compiled, imaged) = match load(context, text, hash, file_name) {
            Some(loaded) => (loaded, true),
            None => {
                self.compiles += 1;
                let compiled = context.compile_with_options(text, &CompileOptions::new(file_name))?;
                // a text another runtime compiled before: this is its second sighting
                let seen = moqui_lib::sync::lock(&SEEN).get(&(hash, STRICT)).is_some();
                if seen {
                    publish(context, &compiled, text, hash, file_name);
                } else {
                    moqui_lib::sync::lock(&SEEN).insert((hash, STRICT), ());
                }
                (compiled, seen)
            }
        };
        self.fixed_bytes += text.len();
        self.fixed.insert(
            hash,
            Fixed {
                text: Arc::from(text),
                file_name: Arc::from(file_name),
                compiled: compiled.clone(),
                imaged,
            },
        );
        Ok(compiled)
    }

    /// Texts kept, and their source bytes.
    pub(super) fn len(&self) -> usize {
        self.fixed.len() + self.transient.len()
    }

    pub(super) fn bytes(&self) -> usize {
        self.fixed_bytes + self.transient.bytes()
    }

    /// Static texts and their source bytes, transient texts and theirs, for the heap log.
    pub(super) fn split(&self) -> (usize, usize, usize, usize) {
        (self.fixed.len(), self.fixed_bytes, self.transient.len(), self.transient.bytes())
    }

    /// Texts compiled by this runtime (not those loaded from an image).
    pub(super) fn compiles(&self) -> u64 {
        self.compiles
    }

    /// Let the transient code go, for a runtime short of memory. The static code stays in the runtime whatever the
    /// cache holds, so its entries stay too.
    pub(super) fn clear_transient(&mut self) {
        self.transient.clear();
    }
}

/// The process's image of `text`, loaded into this runtime; `None` when there is none or it did not load.
fn load(context: &Context, text: &str, hash: u64, file_name: &str) -> Option<FunctionBytecodeRef> {
    let image = moqui_lib::sync::lock(&IMAGES)
        .get(&(hash, STRICT))
        .filter(|(kept, name, _)| **kept == *text && **name == *file_name)
        .map(|(_, _, image)| image)?;
    match context.load_image(&image) {
        Ok(loaded) => {
            counts::LOADS.fetch_add(1, Relaxed);
            Some(loaded)
        }
        Err(err) => {
            counts::FAILED.fetch_add(1, Relaxed);
            tracing::warn!(error = ?err, file_name, "a quickjs-oxide code image did not load; compiling the text");
            None
        }
    }
}

/// Encode `compiled` (this runtime's code of `text`) and keep the image for the process, unless one is kept already.
fn publish(context: &Context, compiled: &FunctionBytecodeRef, text: &str, hash: u64, file_name: &str) {
    let held = moqui_lib::sync::lock(&IMAGES)
        .get(&(hash, STRICT))
        .is_some_and(|(kept, name, _)| *kept == *text && *name == *file_name);
    if held {
        return;
    }
    let image = match context.encode_image(compiled) {
        Ok(image) => image,
        Err(err) => {
            counts::FAILED.fetch_add(1, Relaxed);
            tracing::debug!(error = ?err, file_name, "a quickjs-oxide script did not encode as an image");
            return;
        }
    };
    let weight = image.bytes() + text.len() + file_name.len();
    let mut images = moqui_lib::sync::lock(&IMAGES);
    images.insert_sized((hash, STRICT), (Arc::from(text), Arc::from(file_name), image), weight);
    counts::BYTES.store(images.bytes(), Relaxed);
    drop(images);
    counts::MADE.fetch_add(1, Relaxed);
    moqui_lib::sync::lock(&SEEN).remove(&(hash, STRICT));
}
