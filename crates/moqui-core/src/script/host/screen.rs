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

//! `ec.screen` and `ec.resource.xslFoTransform`, for services that render a screen to text: the scheduled screens and
//! email bodies. `ec.screen.render(options)` is the host call and `makeRender` (the class's shim) is a chain of setters
//! over it; `makeTest` is Java's `ScreenTest` over `testRender`. A render mode named `xsl-fo` is `html-print`.

use std::sync::Arc;

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostClass, HostError, Method, Ret};

pub(super) static SCREEN: HostClass = HostClass {
    name: "host:Screen",
    methods: &[
        method!("ec.screen", "isRenderModeValid", [Text], is_render_mode_valid),
        method!("ec.screen", "render", [Value], None, render),
        method!("ec.screen", "testRender", [Value], None, test_render),
        method!(
            "ec.screen",
            "testNoRequiredParameterPaths",
            [Value],
            None,
            test_no_required_parameter_paths
        ),
        method!("ec.screen", "staticReport", [Value], None, static_report),
        method!("ec.screen", "getMimeTypeByMode", [Text], mime_type_by_mode),
        method!("ec.screen", "isRenderModeText", [Text], is_render_mode_text),
        method!("ec.screen", "isRenderModeBinary", [Text], is_render_mode_binary),
    ],
    getters: &[],
    hidden: &[],
    shim_js: Some(SCREEN_JS),
    shim_members: &["makeRender", "makeTest"],
};

pub(super) static XSL_FO_TRANSFORM: Method = method!(
    "ec.resource",
    "xslFoTransform",
    [Value, OptValue, OptValue, OptValue],
    xsl_fo_transform
);

const SCREEN_JS: &str = r#"(function (proto) {
    proto.makeRender = () => {
        const o = {};
        const b = {
            webappName: (v) => { o.webappName = v; return b; },
            rootScreen: (v) => { o.rootScreen = v; return b; },
            rootScreenFromHost: (v) => { o.rootScreenFromHost = v; return b; },
            // a path, or Java's list of path names
            screenPath: (v) => { o.screenPath = Array.isArray(v) ? v.join("/") : v; return b; },
            renderMode: (v) => { o.renderMode = v; return b; },
            baseLinkUrl: (v) => { o.baseLinkUrl = v; return b; },
            lastStandalone: (v) => { o.lastStandalone = String(v); return b; },
            parameters: (v) => { o.parameters = v; return b; },
            render: () => ec.screen.render(o).text,
            // The bytes of the render: a binary mode's document such as xlsx, or a text mode's UTF-8.
            renderBytes: () => ec.screen.render(o).bytes
        };
        return b;
    };
    // Java's ScreenTest: render paths under a base screen path as the current user, with the errors of each render
    // collected on the result instead of thrown, and counts over the renders.
    proto.makeTest = () => {
        const o = {};
        const t = {
            rootScreen: (v) => { o.rootScreen = v; return t; },
            baseScreenPath: (v) => { o.baseScreenPath = v; return t; },
            renderMode: (v) => { o.renderMode = v; return t; },
            baseLinkUrl: (v) => { o.baseLinkUrl = v; return t; },
            webappName: (v) => { o.webappName = v; return t; },
            // a request header of every render; clientModes("qvue,qjs") is X-Moqui-Client-Modes, as moqui.loadComponent sends it
            header: (name, value) => { o.headers = Object.assign({}, o.headers, { [String(name)]: String(value) }); return t; },
            clientModes: (modes) => t.header("X-Moqui-Client-Modes", Array.isArray(modes) ? modes.join(",") : modes),
            // every render as an agent's screen session renders it (its origin, fenced, writes held): what reaches a model
            agent: () => { o.agent = true; return t; },
            // a qvt render of a server-static screen answers its data-free template: with this, the output also gets the text
            // of the screen's data (its /actions package and each form-list's /actions/<form>), so a spec can look for both
            withStaticData: () => { o.withStaticData = true; return t; },
            renderCount: 0, errorCount: 0, renderTotalChars: 0, startTime: Date.now(),
            render: (screenPath, parameters, requestMethod) => {
                const r = ec.screen.testRender(Object.assign({}, o,
                    { screenPath: String(screenPath), parameters: parameters || {}, method: requestMethod || "" }));
                t.renderCount++;
                t.errorCount += r.errorMessages.length;
                t.renderTotalChars += r.output ? r.output.length : 0;
                const headers = r.headers || {};
                const qvtHeader = Object.keys(headers).find((k) => k.toLowerCase() === "x-moqui-qvt");
                if (o.withStaticData && r.status == 200 && qvtHeader && headers[qvtHeader] === "static-template"
                        && !/\/actions(\/|$)/.test(String(screenPath).split("?")[0])) {
                    const texts = [];
                    const collect = (value) => {
                        if (value == null) return;
                        if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") texts.push(String(value));
                        else if (Array.isArray(value)) value.forEach(collect);
                        else if (typeof value === "object") Object.values(value).forEach(collect);
                    };
                    const base = String(screenPath).split("?")[0].replace(/\/$/, "");
                    // the form-lists of the template (an m-static-form-list vnode with the form's name), which the client asks
                    // /actions/<name> for
                    const forms = [];
                    const formRe = /_component_m_static_form_list,\s*\{[^}]{0,200}?\bname:\s*"([A-Za-z0-9_]+)"/g;
                    for (let m = formRe.exec(String(r.output || "")); m; m = formRe.exec(String(r.output || ""))) if (!forms.includes(m[1])) forms.push(m[1]);
                    for (const part of ["actions"].concat(forms.map((form) => "actions/" + form))) {
                        const d = ec.screen.testRender(Object.assign({}, o, { screenPath: base + "/" + part, parameters: parameters || {}, method: "" }));
                        r.errorMessages = r.errorMessages.concat(d.errorMessages);
                        try { collect(JSON.parse(d.output)); } catch (e) { texts.push(String(d.output || "")); }
                    }
                    r.output = String(r.output || "") + "\n" + texts.join("\n");
                }
                const str = {
                    screenPath: String(screenPath), output: r.output, errorMessages: r.errorMessages,
                    renderTime: r.renderTime, status: r.status, headers: headers,
                    // a response header by name in any case, such as X-Moqui-Qvt (the mode a .qvt request got)
                    header: (name) => {
                        const key = Object.keys(headers).find((k) => k.toLowerCase() === String(name).toLowerCase());
                        return key === undefined ? null : headers[key];
                    },
                    assertContains: (text) => !!r.output && r.output.includes(text),
                    assertNotContains: (text) => !r.output || !r.output.includes(text),
                    assertNoErrors: () => r.errorMessages.length === 0,
                    // Java's assertRegex: whether the output has a match of the pattern (text or a RegExp)
                    assertRegex: (regex) => !!r.output && (regex instanceof RegExp ? regex : new RegExp(String(regex))).test(r.output),
                    getJsonObject: () => str.jsonObject,
                    getOutput: () => r.output, getErrorMessages: () => r.errorMessages, getRenderTime: () => r.renderTime
                };
                // the response parsed as JSON, or null when it is not JSON
                Object.defineProperty(str, "jsonObject", { get: () => { try { return JSON.parse(r.output); } catch (e) { return null; } } });
                return str;
            },
            // the paths below the base screen of the screens with no required parameters, skipping the names given
            getNoRequiredParameterPaths: (screensToSkip) => ec.screen.testNoRequiredParameterPaths(Object.assign({}, o,
                { skip: Array.from(screensToSkip || [], String) })),
            renderAll: (screenPaths, parameters, requestMethod) => {
                for (const screenPath of screenPaths) t.render(screenPath, parameters, requestMethod);
            },
            getRenderCount: () => t.renderCount, getErrorCount: () => t.errorCount,
            getRenderTotalChars: () => t.renderTotalChars, getStartTime: () => t.startTime
        };
        return t;
    };
})"#;

/// The options map of the first argument, or the error `message`.
fn options_of(call: &Call<'_>, message: &str) -> Result<moqui_lib::object::ValueMap, HostError> {
    match call.value_or_null(0) {
        Value::Map(options) => Ok(options),
        _ => Err(HostError::message(message)),
    }
}

fn is_render_mode_valid(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mode = call.text(0).unwrap_or_default();
    Ok(with_ec(|ec| crate::screen::ScreenFacade::of(ec.factory()).is_render_mode_valid(mode))?.into())
}

/// `ec.screen.render(options)`: `{screenPath, renderMode, rootScreen, baseLinkUrl, lastStandalone,
/// parameters}` to `{status, contentType, text}`. The render runs in this execution context.
fn render(call: &mut Call<'_>) -> Result<Ret, HostError> {
    {
        let options = options_of(call, "ec.screen.render needs an options map")?;
        let text = |name: &str| {
            options
                .get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let mut parameters = std::collections::BTreeMap::new();
        if let Some(Value::Map(given)) = options.get("parameters") {
            parameters.extend(given.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
        if !text("lastStandalone").is_empty() {
            parameters.insert("lastStandalone".to_string(), Value::String(text("lastStandalone")));
        }
        let factory = with_ec(|ec| Arc::clone(ec.factory()))?;
        let mut render = crate::screen::make_render(&factory)
            .screen_path(text("screenPath"))
            .parameters(parameters)
            .base_url(text("baseLinkUrl"));
        if !text("rootScreen").is_empty() {
            render = render.root_screen(text("rootScreen"));
        } else if !text("rootScreenFromHost").is_empty() {
            let webapp_name = text("webappName");
            let webapp = if webapp_name.is_empty() {
                "webroot"
            } else {
                webapp_name.as_str()
            };
            // the host's root, or the default root for a path it leaves to it (framework-routes, a shared file)
            let location =
                crate::screen::root_for_request(&factory, &text("rootScreenFromHost"), webapp, &text("screenPath"))
                    .map_err(|err| HostError::message(err.message()))?;
            render = render.root_screen(location);
        }
        if !text("renderMode").is_empty() {
            render = render.render_mode(text("renderMode"));
        }
        let output = render.render().map_err(|err| HostError::message(err.message()))?;
        let mut result = indexmap::IndexMap::default();
        result.insert("status".to_string(), Value::I32(i32::from(output.status)));
        result.insert(
            "contentType".to_string(),
            Value::string(output.content_type().unwrap_or_default()),
        );
        result.insert("text".to_string(), Value::String(output.body_text()));
        result.insert("bytes".to_string(), Value::Bytes(output.body));
        Ok(Ret::Value(Value::Map(result)))
    }
}

fn mime_type_by_mode(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mode = call.text(0).unwrap_or_default();
    Ok(with_ec(|ec| crate::screen::ScreenFacade::of(ec.factory()).mime_type(mode))?.into())
}

/// `ec.screen.isRenderModeBinary(mode)`: a `screen-output` mode such as `xlsx`, whose render is bytes.
fn is_render_mode_binary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mode = call.text(0).unwrap_or_default();
    Ok(with_ec(|ec| crate::screen::ScreenFacade::of(ec.factory()).is_render_mode_binary(mode))?.into())
}

/// A mode that writes text is every configured mode: the print mode is HTML until PDF exists.
fn is_render_mode_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mode = call.text(0).unwrap_or_default();
    let text = with_ec(|ec| {
        crate::screen::ScreenFacade::of(ec.factory())
            .mime_type(mode)
            .is_some_and(|mime| mime.starts_with("text/") || mime == "application/javascript")
    })?;
    Ok(text.into())
}

/// `ec.resource.xslFoTransform(document, xslt, out, contentType)`: the PDF bytes of an `html-print` document (the
/// old name `xsl-fo`), as a `Uint8Array`. Java writes to the `out` stream; a script has none, so the bytes are the
/// return value and `out` is ignored. Java's XSLT step was an identity transform every caller skipped with null, so a
/// stylesheet is an error, as is any content type but PDF.
fn xsl_fo_transform(call: &mut Call<'_>) -> Result<Ret, HostError> {
    {
        if call.value(1).is_some_and(|value| !matches!(value, Value::Null)) {
            return Err(HostError::message(
                "xslFoTransform does not apply an XSLT stylesheet; pass null",
            ));
        }
        let content_type = match call.value(3) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        if !content_type.is_empty() && content_type != "application/pdf" {
            return Err(HostError::message(format!(
                "xslFoTransform makes application/pdf, not {content_type}"
            )));
        }
        let Some(Value::String(text)) = call.value(0) else {
            return Err(HostError::message("xslFoTransform needs the html-print document text"));
        };
        let bytes = crate::screen::PdfRenderer::render(text).map_err(|err| HostError::message(err.message()))?;
        Ok(Ret::Value(Value::Bytes(bytes)))
    }
}

/// `ec.screen.testRender({screenPath, parameters, method, rootScreen, baseScreenPath, renderMode, baseLinkUrl, headers,
/// agent})`,
/// the host side of `makeTest().render()`: `{output, errorMessages, renderTime, status, headers}` (the request headers in,
/// the response's out). It does not throw for a failed
/// render; the failure is in `errorMessages`.
fn test_render(call: &mut Call<'_>) -> Result<Ret, HostError> {
    {
        let options = options_of(call, "ec.screen.testRender needs an options map")?;
        let text = |name: &str| {
            options
                .get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let mut parameters = std::collections::BTreeMap::new();
        if let Some(Value::Map(given)) = options.get("parameters") {
            parameters.extend(given.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
        let factory = with_ec(|ec| Arc::clone(ec.factory()))?;
        let mut test = crate::screen::ScreenTest::new(&factory)
            .base_screen_path(text("baseScreenPath"))
            .base_url(text("baseLinkUrl"));
        if !text("rootScreen").is_empty() {
            test = test.root_screen(text("rootScreen"));
        }
        if !text("renderMode").is_empty() {
            test = test.render_mode(text("renderMode"));
        }
        if let Some(Value::Map(headers)) = options.get("headers") {
            for (name, value) in headers {
                test = test.header(name.clone(), moqui_lib::object::to_plain_string(value));
            }
        }
        if matches!(options.get("agent"), Some(Value::Bool(true))) {
            test = test.agent();
        }
        let method = text("method");
        let rendered = test.test_render(&text("screenPath"), parameters, Some(method.as_str()));
        let mut out = indexmap::IndexMap::default();
        out.insert(
            "output".to_string(),
            if rendered.output.is_some() {
                Value::String(rendered.text())
            } else {
                Value::Null
            },
        );
        out.insert(
            "errorMessages".to_string(),
            Value::List(rendered.error_messages.into_iter().map(Value::String).collect()),
        );
        out.insert(
            "renderTime".to_string(),
            Value::I64(i64::try_from(rendered.render_time_ms).unwrap_or(i64::MAX)),
        );
        out.insert(
            "status".to_string(),
            rendered
                .output
                .as_ref()
                .map_or(Value::Null, |output| Value::I64(i64::from(output.status))),
        );
        // the response headers, the last of a repeated name
        let mut headers = indexmap::IndexMap::default();
        for (name, value) in rendered.output.iter().flat_map(|output| output.headers.iter()) {
            headers.insert(name.clone(), Value::string(value));
        }
        out.insert("headers".to_string(), Value::Map(headers));
        Ok(Ret::Value(Value::Map(out)))
    }
}

/// `ec.screen.staticReport({rootScreen, screenPath, parameters})`: the server-static qvt report of the screen with the
/// full check ([`crate::screen::statics::static_report`] with `full`): the static half, then an html render with the real
/// actions and the wrapped `ec`, as the current user. `{location, path, verdict, marked, eligible, fullCheck, reasons,
/// violations: [{kind, element, detail}]}`, where verdict is `clean`, `static half only`, `violations` or
/// `stays a server merge`, as `moqui lint --screens --qvt-static` writes it.
fn static_report(call: &mut Call<'_>) -> Result<Ret, HostError> {
    {
        let options = options_of(call, "ec.screen.staticReport needs an options map")?;
        let text = |name: &str| {
            options
                .get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let root = text("rootScreen");
        if root.is_empty() {
            return Err(HostError::message("ec.screen.staticReport needs a rootScreen"));
        }
        let parameters: std::collections::BTreeMap<String, Value> = match options.get("parameters") {
            Some(Value::Map(map)) => map.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
            _ => std::collections::BTreeMap::new(),
        };
        let factory = with_ec(|ec| Arc::clone(ec.factory()))?;
        let report = crate::screen::statics::static_report(&factory, &root, &text("screenPath"), &parameters, true)
            .map_err(|err| HostError::message(err.to_string()))?;
        let verdict = if report.is_clean() {
            "clean"
        } else if report.eligible && report.violations.is_empty() {
            "static half only"
        } else if report.eligible {
            "violations"
        } else {
            "stays a server merge"
        };
        let mut out = indexmap::IndexMap::default();
        out.insert("location".to_string(), Value::string(&report.location));
        out.insert("path".to_string(), Value::string(&report.path));
        out.insert("verdict".to_string(), Value::string(verdict));
        out.insert("marked".to_string(), Value::Bool(report.marked));
        out.insert("eligible".to_string(), Value::Bool(report.eligible));
        out.insert("fullCheck".to_string(), Value::Bool(report.full_check));
        out.insert(
            "reasons".to_string(),
            Value::List(report.reasons.iter().map(Value::string).collect()),
        );
        let violations = report
            .violations
            .iter()
            .map(|violation| {
                let mut row = indexmap::IndexMap::default();
                row.insert("kind".to_string(), Value::string(&violation.kind));
                row.insert("element".to_string(), Value::string(&violation.element));
                row.insert("detail".to_string(), Value::string(&violation.detail));
                Value::Map(row)
            })
            .collect();
        out.insert("violations".to_string(), Value::List(violations));
        Ok(Ret::Value(Value::Map(out)))
    }
}

/// `ec.screen.testNoRequiredParameterPaths({rootScreen, baseScreenPath, skip})`, the paths of
/// [`crate::screen::ScreenTest::no_required_parameter_paths`].
fn test_no_required_parameter_paths(call: &mut Call<'_>) -> Result<Ret, HostError> {
    {
        let options = options_of(call, "ec.screen.testNoRequiredParameterPaths needs an options map")?;
        let text = |name: &str| {
            options
                .get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let skip = match options.get("skip") {
            Some(Value::List(names)) => names.iter().map(moqui_lib::object::to_plain_string).collect(),
            _ => std::collections::HashSet::new(),
        };
        let factory = with_ec(|ec| Arc::clone(ec.factory()))?;
        let mut test = crate::screen::ScreenTest::new(&factory).base_screen_path(text("baseScreenPath"));
        if !text("rootScreen").is_empty() {
            test = test.root_screen(text("rootScreen"));
        }
        let paths = test
            .no_required_parameter_paths(&skip)
            .map_err(|err| HostError::message(err.to_string()))?;
        Ok(Ret::Value(Value::List(paths.into_iter().map(Value::String).collect())))
    }
}
