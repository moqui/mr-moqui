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

//! Component tools in scripts: `ec.getTool(name, ...parameters)`, `ec.factory.getTool` and
//! `ec.factory.getToolFactory(name)`. A tool is a host value ([`ToolHandle`]); each engine makes its exotic object of
//! the tool's methods from it (`runtime/native_tools.rs`, `oxide/tool.rs`).

use std::sync::Arc;

use moqui_lib::object::{HostObject, Value};

use super::{method, with_ec, Arg, Call, HostClass, HostError, Method, Path, Ret};
use crate::native::ToolHandle;

/// The paths of the tool functions.
pub(super) static PATHS: &[Path] = &[
    Path::function("ec.getTool", &GET_TOOL),
    Path::function("ec.factory.getTool", &GET_TOOL),
    Path::function("ec.factory.getToolFactory", &GET_TOOL_FACTORY),
];

/// How many parameters `getTool` passes on; a script that passes more is refused.
const MAX_PARAMETERS: usize = 8;

static GET_TOOL: Method = method!(
    "ec",
    "getTool",
    [Text, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue],
    get_tool
);

static GET_TOOL_FACTORY: Method = method!("ec.factory", "getToolFactory", [Text], None, leaf, get_tool_factory);

/// `ec.factory.getToolFactory(name)`'s object: `name` and `getInstance(...parameters)`.
pub(crate) static TOOL_FACTORY: HostClass = HostClass {
    name: "host:ToolFactory",
    methods: &[method!(
        "ToolFactory",
        "getInstance",
        [OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue],
        get_instance
    )],
    getters: &[method!("ToolFactory", "name", [], None, leaf, factory_name)],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// A tool factory's state: the name it is found by and the name it gives.
struct FactoryNames {
    lookup: String,
    name: String,
}

/// The parameters from `first` on, as the script passed them (undefined as null).
fn parameters(call: &Call<'_>, first: usize) -> Vec<Value> {
    (first..first + MAX_PARAMETERS)
        .map_while(|index| match call.arg(index) {
            Arg::Missing => None,
            Arg::Value(value) => Some(value.clone()),
            _ => Some(Value::Null),
        })
        .collect()
}

/// The tool `name` with `parameters`, as a host value.
fn tool(name: &str, parameters: &[Value]) -> Result<Ret, HostError> {
    let tool =
        with_ec(|ec| ec.factory().get_tool(name, parameters))?.map_err(|error| HostError::message(error.message()))?;
    Ok(Ret::Value(Value::Object(HostObject(Arc::new(ToolHandle(tool)) as _))))
}

fn get_tool(call: &mut Call<'_>) -> Result<Ret, HostError> {
    tool(&call.text_or_empty(0), &parameters(call, 1))
}

/// `ec.factory.getToolFactory(name)`: `{ name, getInstance(...parameters) }`, or null when no tool factory has that name.
fn get_tool_factory(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let lookup = call.text_or_empty(0);
    let found = with_ec(|ec| ec.factory().get_tool_factory(&lookup))?;
    Ok(match found {
        Some(factory) => Ret::Object(
            HostObject::new(FactoryNames {
                name: factory.name().to_string(),
                lookup,
            }),
            &TOOL_FACTORY,
        ),
        None => Ret::Value(Value::Null),
    })
}

fn factory_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(call.this::<FactoryNames>()?.name.clone()))
}

fn get_instance(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let lookup = call.this::<FactoryNames>()?.lookup.clone();
    tool(&lookup, &parameters(call, 0))
}
