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

//! What a host value's conversion to a script value treats apart.

use super::*;

/// How deep a script value may nest, in arrays, objects and Sets, when it becomes a host value.
/// Context data rarely nests past a few levels and parsed JSON stops at 128; the limit keeps a
/// deep or cyclic value from overflowing the native stack.
pub(crate) const MAX_NESTING: usize = 200;

/// Dates reach a script as text, as Java `toString` would give. Assigning one back stores the text,
/// and the entity layer converts it to the field type on write.
pub(crate) fn temporal_text(value: &Value) -> String {
    let text = moqui_lib::object::basic_convert(value, "String").value;
    match text.as_str() {
        Some(text) => text.to_string(),
        None => moqui_lib::object::to_plain_string(value),
    }
}

/// Largest integer a JavaScript number holds exactly.
pub(crate) const SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The error for a value deeper than [`MAX_NESTING`], which a cyclic value always is.
pub(crate) const TOO_DEEP: &str = "value is cyclic or nested too deeply";

/// Whether converting `value` to a script value runs script code: a tools map is wrapped by a script function.
pub(crate) fn converts_with_script(value: &Value) -> bool {
    match value {
        Value::List(items) => items.iter().any(converts_with_script),
        Value::Map(entries) => {
            (has_special_key(entries) && entries.contains_key(WRAP_KEY)) || entries.values().any(converts_with_script)
        }
        _ => false,
    }
}

/// Whether a map may hold one of the names a conversion treats apart (`__set__`, `__wrap`, `__proto__`): one starting
/// `__`. A scan of the keys' first bytes, cheaper than a hashed lookup of each name in the small maps a script reads.
pub(crate) fn has_special_key(entries: &indexmap::IndexMap<String, Value>) -> bool {
    entries.keys().any(|key| key.as_bytes().starts_with(b"__"))
}

/// A script `Set` is held on the context stack as a one-key map holding its items, so a Set one
/// action block makes in a context name is a Set again in the next block.
pub(crate) const SET_KEY: &str = "__set__";

/// The key of a map whose JavaScript form is a tools object: the name of the wrapper that adds its methods.
pub(crate) const WRAP_KEY: &str = "__wrap";
