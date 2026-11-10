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

//! `errorThrowable` for the html error pages ([`ERROR_INFO`]), with the calls `error/*.xml` makes on it. Each engine
//! makes an object of the class from a host value that holds an [`ErrorInfo`].

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::{method, Call, HostClass, HostError, Ret};
use crate::web::error_info::ErrorInfo;

/// `getArtifactInfo()` as Java's: the denied artifact's `name`, `actionDescription` and `typeDescription`, with their
/// getters and `toString`, or null.
const ERROR_INFO_JS: &str = r#"(function (proto, hidden) {
    const info = hidden.artifactInfo;
    Object.defineProperty(proto, "getArtifactInfo", { writable: true, configurable: true, value: function getArtifactInfo() {
        const found = info.call(this);
        if (found === null) return null;
        const { name, actionDescription, typeDescription, text } = found;
        return { name, actionDescription, typeDescription, getName: () => name,
            getActionDescription: () => actionDescription, getTypeDescription: () => typeDescription,
            toString: () => text };
    } });
})"#;

pub(crate) static ERROR_INFO: HostClass = HostClass {
    name: "host:ErrorInfo",
    methods: &[
        method!("ErrorInfo", "getMessage", [], None, leaf, message),
        method!("ErrorInfo", "getArtifactStack", [], None, leaf, artifact_stack),
        method!("ErrorInfo", "getStackTrace", [], None, leaf, stack_trace),
    ],
    getters: &[
        method!("ErrorInfo", "message", [], None, leaf, message),
        method!("ErrorInfo", "pathFromLastScreen", [], None, leaf, path_from_last_screen),
        method!("ErrorInfo", "fullPathNameList", [], None, leaf, full_path_name_list),
        method!("ErrorInfo", "resourceLocation", [], None, leaf, resource_location),
        method!("ErrorInfo", "lastSd", [], None, leaf, last_sd),
    ],
    hidden: &[method!("ErrorInfo", "artifactInfo", [], None, leaf, artifact_info)],
    shim_js: Some(ERROR_INFO_JS),
    shim_members: &["getArtifactInfo"],
};

fn info(call: &Call<'_>) -> Result<std::sync::Arc<ErrorInfo>, HostError> {
    call.this::<ErrorInfo>()
}

fn message(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(info(call)?.message.clone()))
}

fn path_from_last_screen(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(info(call)?.path_from_last_screen.clone()))
}

fn full_path_name_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(info(call)?.full_path.clone()))
}

fn resource_location(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(String::new()))
}

fn last_sd(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::Null))
}

fn artifact_stack(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(Vec::<String>::new()))
}

fn stack_trace(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(info(call)?.trace.clone()))
}

/// The denied artifact's parts and its text, for the shim's `getArtifactInfo()`; null when no artifact was denied.
fn artifact_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let info = info(call)?;
    let Some(artifact) = info.artifact.as_ref() else {
        return Ok(Ret::Value(Value::Null));
    };
    let (name, action, kind) = (
        artifact.name.clone(),
        artifact.action_description(),
        artifact.type_description(),
    );
    let text = format!("{kind} {name} ({action})");
    let mut map = IndexMap::default();
    map.insert("name".to_string(), Value::String(name));
    map.insert("actionDescription".to_string(), Value::String(action));
    map.insert("typeDescription".to_string(), Value::String(kind));
    map.insert("text".to_string(), Value::String(text));
    Ok(Ret::Value(Value::Map(map)))
}
