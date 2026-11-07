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

//! `ec.web.sessionAttributes`, which writes through to the session. `ec.user` is in the host table (`script/host`).

use super::*;

/// A view of the session attributes. Reading a name gives its text; assigning, `put`, and
/// `remove` change the session. `ec.web.sessionAttributes.remove(name)` is the Java idiom the
/// screens use.
const SESSION_ATTRIBUTES: &str = r#"(function (snapshot, write) {
    const methods = {
        get: (name) => snapshot[name],
        containsKey: (name) => Object.prototype.hasOwnProperty.call(snapshot, name),
        put: (name, value) => {
            const old = snapshot[name];
            if (value == null) delete snapshot[name]; else snapshot[name] = String(value);
            write(String(name), value == null ? null : String(value));
            return old;
        },
        remove: (name) => {
            const old = snapshot[name];
            delete snapshot[name];
            write(String(name), null);
            return old;
        }
    };
    return new Proxy(snapshot, {
        get(target, key) {
            if (typeof key === "string" && Object.prototype.hasOwnProperty.call(methods, key)) return methods[key];
            return target[key];
        },
        set(target, key, value) {
            methods.put(key, value);
            return true;
        },
        deleteProperty(target, key) {
            methods.remove(key);
            return true;
        }
    });
})"#;

/// `ec.web.sessionAttributes` over the current session.
pub(super) fn session_attributes_js<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    let mut map = indexmap::IndexMap::new();
    for (key, value) in crate::user::session::session_attributes() {
        map.insert(key, Value::string(value));
    }
    let snapshot = host_to_js(ctx, Value::Map(map))?;
    let write = Function::new(ctx.clone(), write_session_attribute)?;
    let make: Function =
        eval_js(ctx, SESSION_ATTRIBUTES).map_err(|err| Exception::throw_message(ctx, err.message()))?;
    make.call((snapshot, write))
}

fn write_session_attribute(ctx: Ctx<'_>, name: String, value: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let text = optional_text(&ctx, value.0.as_ref())?;
        crate::user::session::set_session_attribute(&name, text.as_deref());
        Ok(())
    })
}
