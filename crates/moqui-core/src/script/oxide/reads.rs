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

//! Whether the context values a text reads are ones the oxide bindings hold, asked by the router before a run that may
//! make a side effect and then read them (plans/Script-Engine-Oxide.md section 3). Such a run cannot fall back to
//! QuickJS after the side effect, so a text whose names hold a value the bindings do not have yet (a tools map, a Set,
//! a host object the bindings pass through opaque, whose members fall back) runs on QuickJS from the start. The answer follows [`super::convert`]'s `to_js` and [`super::names`]' entity
//! reads: a value they convert is held here.

use moqui_lib::object::Value as HostValue;

use crate::script::run::STACK;
use crate::entity::{EntityFind, EntityList};
use crate::script::host::entities::{EntityCell, ListRow, ScriptList};
use crate::script::runtime::{SET_KEY, WRAP_KEY};

/// How deep a value is looked into; past it the conversion fails with an error of its own, which is no fallback.
const MAX_DEPTH: usize = crate::script::runtime::MAX_NESTING + 56;

/// Whether every name of `reads` holds a value the oxide bindings convert; with `all_context` (a text that may read any
/// name through `context`), every visible name. A name the stack does not hold reads as null. (`members`, the
/// `name.property` reads, needs no look since entity relationships and methods are bound on oxide.)
pub(crate) fn reads_are_held(reads: &[String], _members: &[(String, String)], all_context: bool) -> bool {
    STACK.with(|stack| {
        let stack = stack.borrow();
        if all_context
            && !stack
                .combined_keys()
                .iter()
                .all(|name| stack.with_value(name, |value| held(value, 0)).unwrap_or(true))
        {
            return false;
        }
        reads
            .iter()
            .all(|name| stack.with_value(name, |value| held(value, 0)).unwrap_or(true))
    })
}

/// Whether [`super::convert`] converts `value` to an object the bindings bind (not an opaque one, whose members fall
/// back).
fn held(value: &HostValue, depth: usize) -> bool {
    if depth >= MAX_DEPTH {
        return true;
    }
    match value {
        HostValue::List(items) => items.iter().all(|item| held(item, depth + 1)),
        HostValue::Map(entries) => {
            // a tools map converts through the tools shim's wrapper, as on QuickJS, unless its `__wrap` is not a name
            if entries.len() == 1 && entries.contains_key(SET_KEY)
                || entries
                    .get(WRAP_KEY)
                    .is_some_and(|name| !matches!(name, HostValue::String(_)))
            {
                return false;
            }
            entries.values().all(|item| held(item, depth + 1))
        }
        // the host objects `bind::entities` and `bind::tool` bind
        HostValue::Object(object) => {
            object.downcast::<EntityCell>().is_some()
                || object.downcast::<ListRow>().is_some()
                || object.downcast::<ScriptList>().is_some()
                || object.downcast::<EntityList>().is_some()
                || object.downcast::<EntityFind>().is_some()
                || object.downcast::<crate::web::multipart::UploadedFile>().is_some()
                || object.downcast::<crate::script::host::StreamBytes>().is_some()
                || object.downcast::<crate::web::error_info::ErrorInfo>().is_some()
                || object.downcast::<moqui_db::sql::Condition>().is_some()
                || object.downcast::<crate::native::ToolHandle>().is_some()
        }
        _ => true,
    }
}
