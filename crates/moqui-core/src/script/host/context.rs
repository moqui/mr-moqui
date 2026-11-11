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

//! The frame methods of the context stack (`context`, which `ec.context` is too) and the method forms of `ec`'s
//! members, as Java's `ContextStack` and `ExecutionContext` have them (docs/dev/JS-Core-API.md).
//!
//! ```text
//! context.push()  context.push(map)  context.pop() -> the popped frame's entries
//! context.pushContext()  context.popContext()      an isolated empty stack, and back
//! context.getRootMap()  context.addRootMap(map)  context.clear()   (clear empties the top frame)
//! ec.getUser(), ec.getMessage(), ... ec.getContext()   each member of ec by its Java getter
//! ec.contextRoot, ec.getContextRoot()                  the root frame's entries
//! ec.getNotificationMessages(topic)                    the user's notification messages, as plain objects
//! ```
//!
//! The Map methods that need no host call (`get`, `put`, `size`, `entrySet`, ...) are the stack object's own
//! (`oxide/context.rs`). The functions here are hidden functions of the tools shim; `context_js!` puts them on the
//! stack's methods, which the engine lends the shims as `__moquiContextMethods` while they install.

use std::cell::RefCell;
use std::rc::Rc;

use moqui_lib::object::Value;

use super::{method, Call, HostError, Method, Ret};
use crate::script::run::STACK;

/// `(function (ec, host, global) { ... })`: the frame methods on the stack's methods object, then `ec`'s own.
macro_rules! context_js {
    () => {
        r#"(function (ec, host, global) {
    const methods = global.__moquiContextMethods;
    if (methods) {
        methods.push = (map) => { host.contextPush(map); return ec.context; };
        methods.pop = () => host.contextPop();
        methods.pushContext = () => { host.contextPushContext(); return ec.context; };
        methods.popContext = () => { host.contextPopContext(); return ec.context; };
        methods.getRootMap = () => host.contextRootMap();
        methods.addRootMap = (map) => { host.contextAddRootMap(map || {}); };
        methods.clear = () => { host.contextClear(); };
    }
    Object.defineProperty(ec, "contextRoot", { get: () => host.contextRootMap(), enumerable: false, configurable: true });
    // Java's getNotificationMessages: the signed-in user's messages, of one topic or all, whatever the caller may view
    ec.getNotificationMessages = (topic) => {
        const userId = ec.user.userId;
        if (!userId) return [];
        return ec.artifactExecution.runAsSystem(() => {
            const find = ec.entity.find("moqui.security.user.NotificationMessageByUser").condition("userId", userId);
            if (topic) find.condition("topic", String(topic));
            return Array.from(find.list(), (row) => row.getMap());
        });
    };
})"#
    };
}
pub(super) use context_js;

/// The context functions of the tools shim. Each may change what a name reads as, so the run's tracked values are
/// written back and released first.
pub(super) static METHODS: &[Method] = &[
    method!("tools", "contextPush", [OptValue], Release, leaf, push),
    method!("tools", "contextPop", [], Release, leaf, pop),
    method!("tools", "contextPushContext", [], Release, leaf, push_context),
    method!("tools", "contextPopContext", [], Release, leaf, pop_context),
    method!("tools", "contextRootMap", [], Keep, leaf, root_map),
    method!("tools", "contextAddRootMap", [Value], Release, leaf, add_root_map),
    method!("tools", "contextClear", [], Release, leaf, clear),
];

fn stack_error(err: impl std::fmt::Display) -> HostError {
    HostError::message(err.to_string())
}

fn push(call: &mut Call<'_>) -> Result<Ret, HostError> {
    match call.value(0) {
        Some(Value::Map(map)) => {
            let map = map.clone();
            STACK
                .with(|stack| stack.borrow_mut().push_entries(map))
                .map_err(stack_error)?;
        }
        Some(Value::Null) | None => STACK.with(|stack| stack.borrow_mut().push()),
        Some(_) => return Err(HostError::message("context.push takes a map")),
    }
    Ok(Ret::Undefined)
}

/// The popped frame's entries. The last frame stays, as in Java.
fn pop(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let frame = STACK.with(|stack| stack.borrow_mut().pop()).map_err(stack_error)?;
    // a popped frame is the stack's alone unless a saved context holds it too: then it is copied
    let entries = frame
        .map(|frame| Rc::try_unwrap(frame).map_or_else(|shared| shared.borrow().clone(), RefCell::into_inner))
        .unwrap_or_default();
    Ok(Ret::Value(Value::Map(entries)))
}

fn push_context(_: &mut Call<'_>) -> Result<Ret, HostError> {
    STACK.with(|stack| stack.borrow_mut().push_context());
    Ok(Ret::Undefined)
}

fn pop_context(_: &mut Call<'_>) -> Result<Ret, HostError> {
    STACK
        .with(|stack| stack.borrow_mut().pop_context())
        .map_err(stack_error)?;
    Ok(Ret::Undefined)
}

fn root_map(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let entries = STACK.with(|stack| {
        stack
            .borrow()
            .root_map()
            .map(|frame| frame.borrow().clone())
            .unwrap_or_default()
    });
    Ok(Ret::Value(Value::Map(entries)))
}

fn add_root_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Some(Value::Map(map)) = call.value(0) else {
        return Err(HostError::message("context.addRootMap takes a map"));
    };
    let frame = Rc::new(RefCell::new(map.clone()));
    STACK
        .with(|stack| stack.borrow_mut().add_root_map(frame))
        .map_err(stack_error)?;
    Ok(Ret::Undefined)
}

fn clear(_: &mut Call<'_>) -> Result<Ret, HostError> {
    STACK.with(|stack| stack.borrow_mut().clear());
    Ok(Ret::Undefined)
}
