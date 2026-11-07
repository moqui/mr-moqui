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

use super::*;
use crate::script::theme::{self, Limits, Package, RenderRequest, ValidateOptions};

pub(super) fn add_theme<'js>(ctx: &Ctx<'js>, resource: &Object<'js>) -> Result<(), Error> {
    let object = caught(ctx, "resource.theme", Object::new(ctx.clone()))?;
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, object.set($name, function))?;
        };
    }
    method!("validate", theme_validate);
    method!("render", theme_render);
    method!("engines", theme_engines);
    method!("engineInfo", theme_engine_info);
    method!("limits", theme_limits);
    method!("sanitizeSvg", theme_sanitize_svg);
    method!("sanitizeRichText", theme_sanitize_rich_text);
    caught(ctx, "resource.theme", resource.set("theme", object))
}

/// A script value as plain JSON, as the locked renderer's snapshot is made.
fn json_of(ctx: &Ctx<'_>, value: Option<&JsValue<'_>>) -> rquickjs::Result<serde_json::Value> {
    match value.filter(|value| !value.is_undefined() && !value.is_null()) {
        Some(value) => Ok(crate::script::template::unwrap_json(&js_to_host(ctx, value)?)),
        None => Ok(serde_json::Value::Null),
    }
}

fn js_of<'js>(ctx: &Ctx<'js>, json: &serde_json::Value) -> rquickjs::Result<JsValue<'js>> {
    host_to_js(ctx, moqui_lib::json::from_json(json))
}

fn operator_limits(ctx: &Ctx<'_>) -> rquickjs::Result<Arc<Limits>> {
    let factory = with_ec(ctx, |ec| Arc::clone(ec.factory()))?;
    Ok(Limits::operator(&factory))
}

fn theme_validate<'js>(
    ctx: Ctx<'js>,
    package: JsValue<'js>,
    options: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let package = Package::from_json(&json_of(&ctx, Some(&package))?);
        let options = ValidateOptions::from_json(&json_of(&ctx, options.0.as_ref())?, &*operator_limits(&ctx)?);
        js_of(&ctx, &theme::validate(&package, &options).to_json())
    })
}

fn theme_render<'js>(ctx: Ctx<'js>, package: JsValue<'js>, request: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let package = Package::from_json(&json_of(&ctx, Some(&package))?);
        let request = RenderRequest::from_json(&json_of(&ctx, Some(&request))?, &*operator_limits(&ctx)?);
        js_of(&ctx, &theme::render(&package, &request).to_json())
    })
}

fn theme_engines(ctx: Ctx<'_>) -> rquickjs::Result<Vec<String>> {
    host_guard(&ctx, || {
        Ok(theme::engine_ids().into_iter().map(str::to_string).collect())
    })
}

fn theme_engine_info(ctx: Ctx<'_>) -> rquickjs::Result<JsValue<'_>> {
    host_guard(&ctx, || js_of(&ctx, &theme::engine_info()))
}

fn theme_limits(ctx: Ctx<'_>) -> rquickjs::Result<JsValue<'_>> {
    host_guard(&ctx, || {
        let limits = operator_limits(&ctx)?;
        js_of(&ctx, &limits.to_json())
    })
}

fn theme_sanitize_svg(ctx: Ctx<'_>, text: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(theme::sanitize_svg(&text)))
}

fn theme_sanitize_rich_text(ctx: Ctx<'_>, html: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(theme::sanitize_rich_text(&html)))
}
