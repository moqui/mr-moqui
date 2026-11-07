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

//! `ec.resource.theme`: the theme composition adapter (`crate::script::theme`, `docs/ui/Themes.md`) for services.
//!
//! ```text
//! ec.resource.theme.validate(package, {requiredPageKinds, apiVersion, allowedOrigins, limits})
//!     -> {ok, engine, diagnostics: [{severity, code, file, line, message, section, block}]}
//! ec.resource.theme.render(package, {template | pageKind, view, components, references, media, sources,
//!         assetUrls, assetBase, allowedOrigins, releaseId, limits})
//!     -> {ok, html, diagnostics, omitted: [{section, block, reason}], failedRequired, dependencies}
//! ec.resource.theme.engines()            -> ["js-fancy"]
//! ec.resource.theme.engineInfo()         -> [{id: "js-fancy", rendererVersion: 1, viewApiVersions: [1]}]
//! ec.resource.theme.limits()             -> the operator's limits (conf theme-limits)
//! ec.resource.theme.sanitizeSvg(text)    -> the SVG cleaned to the safe set
//! ec.resource.theme.sanitizeRichText(html) -> the HTML cleaned to the safe set
//! ```
//!
//! `package` is `{files: {path: text}}`. The limits of a call can only lower the operator's.

use std::sync::Arc;

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostClass, HostError, Ret};
use crate::script::theme::{self, Limits, Package, RenderRequest, ValidateOptions};

pub(super) static THEME: HostClass = HostClass {
    name: "host:Theme",
    methods: &[
        method!(
            "ec.resource.theme",
            "validate",
            [Value, OptValue],
            None,
            validate
        ),
        method!("ec.resource.theme", "render", [Value, Value], None, render),
        method!("ec.resource.theme", "engines", [], engines),
        method!("ec.resource.theme", "engineInfo", [], engine_info),
        method!("ec.resource.theme", "limits", [], limits),
        method!("ec.resource.theme", "sanitizeSvg", [Text], sanitize_svg),
        method!("ec.resource.theme", "sanitizeRichText", [Text], sanitize_rich_text),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// A script value as plain JSON, as the locked renderer's snapshot is made.
fn json_of(value: Option<&Value>) -> serde_json::Value {
    match value {
        Some(value) => crate::script::template::unwrap_json(value),
        None => serde_json::Value::Null,
    }
}

fn from_json(json: &serde_json::Value) -> Ret {
    Ret::Value(moqui_lib::json::from_json(json))
}

fn operator_limits() -> Result<Arc<Limits>, HostError> {
    let factory = with_ec(|ec| Arc::clone(ec.factory()))?;
    Ok(Limits::operator(&factory))
}

fn validate(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let package = Package::from_json(&json_of(Some(&call.value_or_null(0))));
    let options = ValidateOptions::from_json(&json_of(call.value(1)), &*operator_limits()?);
    Ok(from_json(&theme::validate(&package, &options).to_json()))
}

fn render(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let package = Package::from_json(&json_of(Some(&call.value_or_null(0))));
    let request = RenderRequest::from_json(&json_of(Some(&call.value_or_null(1))), &*operator_limits()?);
    Ok(from_json(&theme::render(&package, &request).to_json()))
}

fn engines(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::List(
        theme::engine_ids().into_iter().map(Value::string).collect(),
    )))
}

fn engine_info(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(from_json(&theme::engine_info()))
}

fn limits(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(from_json(&operator_limits()?.to_json()))
}

fn sanitize_svg(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(theme::sanitize_svg(call.text(0).unwrap_or_default()).into())
}

fn sanitize_rich_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(theme::sanitize_rich_text(call.text(0).unwrap_or_default()).into())
}
