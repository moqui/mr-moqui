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

//! `ec.resource` expansion, `ec.l10n`, `ec.context`, `ec.screen`, and the static helpers the screen
//! macros call (`WebUtilities`, `StringUtilities`, `CollectionUtilities`).

use rquickjs::object::Accessor;

use super::*;
use crate::l10n;

const LOCALIZE_LIMIT: usize = 256;

/// Run `body` with the entries of an additional-context object in a frame of their own.
fn with_additional<'js, T>(
    ctx: &Ctx<'js>,
    map: &Opt<JsValue<'js>>,
    body: impl FnOnce() -> rquickjs::Result<T>,
) -> rquickjs::Result<T> {
    // A map, or an entity value as the additional context, gives its fields, as Java's `EntityValueBase` Map does.
    let entries = match map.0.as_ref().filter(|value| value.is_object()) {
        Some(value) => value_map(&js_to_host(ctx, value)?),
        None => None,
    };
    let Some(entries) = entries else {
        return body();
    };
    flush_current(ctx, true)?;
    let frame = ContextFrame::push();
    put_context_all(&entries);
    let result = body();
    // Before the pop, so a list the body read under an additional name is not written over the caller's.
    let flushed = flush_current(ctx, true);
    drop(frame);
    flushed?;
    result
}

fn localized(template: &str) -> Result<String, Error> {
    if template.chars().count() < LOCALIZE_LIMIT {
        l10n::localize(template).map_err(|err| Error::new(err.message()))
    } else {
        Ok(template.to_string())
    }
}

/// `expand` with the localization Java does first. Nothing to expand is the localized text.
pub fn expand_localized(template: &str) -> Result<String, Error> {
    if template.is_empty() {
        return Ok(String::new());
    }
    expand(&localized(template)?)
}

fn expand_with<'js>(
    ctx: &Ctx<'js>,
    template: &str,
    map: &Opt<JsValue<'js>>,
    localize: bool,
) -> rquickjs::Result<String> {
    if template.is_empty() {
        return Ok(String::new());
    }
    let text = if localize {
        localized(template).map_err(|err| throw_error(ctx, &err))?
    } else {
        template.to_string()
    };
    let expanded = with_additional(ctx, map, || {
        expand_here(ctx, &text).map_err(|err| Exception::throw_message(ctx, err.message()))
    });
    // A widget check goes on after a violation, with nothing for the text.
    match expanded {
        Err(_) if super::WIDGET_WRAP.with(Cell::get) => Ok(String::new()),
        other => other,
    }
}

fn throw_error(ctx: &Ctx<'_>, err: &Error) -> JsError {
    throw_context(ctx, err)
}

fn resource_expand_l10n<'js>(
    ctx: Ctx<'js>,
    template: Option<String>,
    _location: Opt<JsValue<'js>>,
    map: Opt<JsValue<'js>>,
) -> rquickjs::Result<String> {
    // a null template expands to nothing, as in Java
    let template = template.unwrap_or_default();
    host_guard(&ctx, || expand_with(&ctx, &template, &map, true))
}

fn resource_expand_no_l10n<'js>(
    ctx: Ctx<'js>,
    template: Option<String>,
    _location: Opt<JsValue<'js>>,
    map: Opt<JsValue<'js>>,
) -> rquickjs::Result<String> {
    // a null template expands to nothing, as in Java
    let template = template.unwrap_or_default();
    host_guard(&ctx, || expand_with(&ctx, &template, &map, false))
}

fn resource_expression<'js>(
    ctx: Ctx<'js>,
    expression: String,
    _location: Opt<JsValue<'js>>,
    map: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        if expression.trim().is_empty() {
            return Null.into_js(&ctx);
        }
        flush_current(&ctx, false)?;
        let value = with_additional(&ctx, &map, || {
            with_widget_ec(&ctx, &expression, || {
                eval_compiled::<JsValue>(&ctx, &as_expression(&expression), true)
            })
            .map_err(|err| Exception::throw_message(&ctx, err.message()))
        });
        match value {
            Err(_) if super::WIDGET_WRAP.with(Cell::get) => Null.into_js(&ctx),
            other => other,
        }
    })
}

fn resource_condition<'js>(
    ctx: Ctx<'js>,
    expression: String,
    _location: Opt<JsValue<'js>>,
    map: Opt<JsValue<'js>>,
) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        if expression.trim().is_empty() {
            return Ok(false);
        }
        flush_current(&ctx, false)?;
        let holds = with_additional(&ctx, &map, || {
            let value = with_widget_ec(&ctx, &expression, || {
                eval_compiled::<JsValue>(&ctx, &as_expression(&expression), true)
            })
            .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
            Ok(js_truthy(&value))
        });
        match holds {
            Err(_) if super::WIDGET_WRAP.with(Cell::get) => Ok(false),
            other => other,
        }
    })
}

pub(super) fn add_resource_functions<'js>(ctx: &Ctx<'js>, resource: &Object<'js>) -> Result<(), Error> {
    let expand = caught(ctx, "resource.expand", Function::new(ctx.clone(), resource_expand_l10n))?;
    caught(ctx, "resource.expand", resource.set("expand", expand))?;
    let plain = caught(
        ctx,
        "resource.expandNoL10n",
        Function::new(ctx.clone(), resource_expand_no_l10n),
    )?;
    caught(ctx, "resource.expandNoL10n", resource.set("expandNoL10n", plain))?;
    let expression = caught(
        ctx,
        "resource.expression",
        Function::new(ctx.clone(), resource_expression),
    )?;
    caught(ctx, "resource.expression", resource.set("expression", expression))?;
    let condition = caught(
        ctx,
        "resource.condition",
        Function::new(ctx.clone(), resource_condition),
    )?;
    caught(ctx, "resource.condition", resource.set("condition", condition))?;
    let xsl_fo = caught(
        ctx,
        "resource.xslFoTransform",
        Function::new(ctx.clone(), super::ec_screen::resource_xsl_fo_transform),
    )?;
    caught(ctx, "resource.xslFoTransform", resource.set("xslFoTransform", xsl_fo))?;
    let template = caught(ctx, "resource.template", Function::new(ctx.clone(), resource_template))?;
    caught(ctx, "resource.template", resource.set("template", template))
}

/// `ec.resource.template(location, data)`: the file rendered by the renderer for its extension, as
/// text, or the file's text when no renderer has its extension. A trusted renderer reads the live context and adds
/// `data` in a frame of its own. A locked-down renderer sees only `data`. A string second argument is Java's
/// `defaultExtension`: the renderer to use when the location's extension has none.
fn resource_template(ctx: Ctx<'_>, location: String, data: Opt<JsValue<'_>>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let mut default_extension = String::new();
        let data = match data.0 {
            Some(value) if value.is_string() => {
                default_extension = js_string(&ctx, &value)?;
                None
            }
            Some(value) if !value.is_undefined() && !value.is_null() => Some(js_to_host(&ctx, &value)?),
            _ => None,
        };
        let factory = with_ec(&ctx, |ec| Arc::clone(ec.factory()))?;
        crate::script::template::TemplateRegistry::of(&factory)
            .render_location_default(&factory, &location, data.as_ref(), &default_extension)
            .map_err(|err| throw_error(&ctx, &err))
    })
}

// ---- ec.l10n ----

/// `localize(original, locale)`: in the given locale, else the session's.
fn l10n_localize(ctx: Ctx<'_>, original: JsValue<'_>, locale: Opt<JsValue<'_>>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let text = moqui_lib::object::to_plain_string(&js_to_host(&ctx, &original)?);
        let localized = match optional_text(&ctx, locale.0.as_ref())?.filter(|locale| !locale.is_empty()) {
            Some(locale) => l10n::localize_in(&text, &locale.replace('-', "_")),
            None => l10n::localize(&text),
        };
        localized.map_err(|err| throw_error(&ctx, &Error::new(err.message())))
    })
}

fn l10n_format(ctx: Ctx<'_>, value: JsValue<'_>, pattern: Opt<JsValue<'_>>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let host = js_to_host(&ctx, &value)?;
        let pattern = match pattern.0.as_ref() {
            Some(found) if !found.is_null() && !found.is_undefined() => {
                Some(moqui_lib::object::to_plain_string(&js_to_host(&ctx, found)?))
            }
            _ => None,
        };
        let host = match pattern.as_deref() {
            Some(pattern) if is_date_pattern(pattern) => epoch_millis_timestamp(&host).unwrap_or(host),
            _ => host,
        };
        l10n::format_value(&host, pattern.as_deref()).map_err(|err| throw_error(&ctx, &Error::new(err.message())))
    })
}

/// A date pattern such as `dd MMM yyyy` or `HH:mm`, not a number pattern such as `#,##0.00`.
fn is_date_pattern(pattern: &str) -> bool {
    pattern.contains(['y', 'd', 'H', 'E']) && !pattern.contains(['#', '0'])
}

/// A date or timestamp reaches a script as epoch millis, so a whole number formatted with a date pattern is a
/// timestamp.
fn epoch_millis_timestamp(value: &Value) -> Option<Value> {
    let millis = match value {
        Value::I64(millis) => *millis,
        Value::I32(millis) => i64::from(*millis),
        Value::F64(millis) if millis.fract() == 0.0 => *millis as i64,
        Value::String(text) => match text.trim().parse::<i64>() {
            Ok(millis) => millis,
            // A script holds a date as text such as `2026-10-16` or `2026-10-16 08:30:00`, which the date pattern formats.
            Err(_) if is_date_text(text) => {
                // Date only text is a Date, which has no time zone to shift it.
                let java_type = if text.trim().len() == 10 {
                    "java.sql.Date"
                } else {
                    "java.sql.Timestamp"
                };
                let converted = moqui_lib::object::basic_convert(value, java_type);
                return (!matches!(converted.value, Value::Null | Value::String(_))).then_some(converted.value);
            }
            Err(_) => return None,
        },
        _ => return None,
    };
    Some(moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value)
}

/// Text that begins `yyyy-MM-dd`.
fn is_date_text(text: &str) -> bool {
    let bytes = text.trim().as_bytes();
    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn currency(
    ctx: &Ctx<'_>,
    amount: &JsValue<'_>,
    uom_id: &Opt<JsValue<'_>>,
    digits: &Opt<JsValue<'_>>,
    hide_symbol: bool,
) -> rquickjs::Result<String> {
    let amount = js_to_host(ctx, amount)?;
    let uom = match uom_id.0.as_ref() {
        Some(found) if !found.is_null() && !found.is_undefined() => {
            Some(moqui_lib::object::to_plain_string(&js_to_host(ctx, found)?))
        }
        _ => None,
    };
    let digits = match digits.0.as_ref() {
        Some(found) if !found.is_null() && !found.is_undefined() => {
            moqui_lib::object::to_plain_string(&js_to_host(ctx, found)?)
                .parse::<usize>()
                .ok()
        }
        _ => None,
    };
    l10n::format_currency(&amount, uom.as_deref(), digits, hide_symbol)
        .map_err(|err| throw_error(ctx, &Error::new(err.message())))
}

fn l10n_format_currency(
    ctx: Ctx<'_>,
    amount: JsValue<'_>,
    uom_id: Opt<JsValue<'_>>,
    digits: Opt<JsValue<'_>>,
) -> rquickjs::Result<String> {
    host_guard(&ctx, || currency(&ctx, &amount, &uom_id, &digits, false))
}

fn l10n_format_currency_no_symbol(
    ctx: Ctx<'_>,
    amount: JsValue<'_>,
    uom_id: Opt<JsValue<'_>>,
    digits: Opt<JsValue<'_>>,
) -> rquickjs::Result<String> {
    host_guard(&ctx, || currency(&ctx, &amount, &uom_id, &digits, true))
}

fn parsed_or_null<'js>(ctx: &Ctx<'js>, value: Option<Value>) -> rquickjs::Result<JsValue<'js>> {
    match value {
        Some(value) => host_to_js(ctx, value),
        None => Null.into_js(ctx),
    }
}

fn l10n_parse_timestamp<'js>(
    ctx: Ctx<'js>,
    input: JsValue<'js>,
    format: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let input = moqui_lib::object::to_plain_string(&js_to_host(&ctx, &input)?);
        let format = optional_text(&ctx, format.0.as_ref())?;
        parsed_or_null(&ctx, l10n::parse_timestamp_format(&input, format.as_deref()))
    })
}

fn l10n_parse_date<'js>(
    ctx: Ctx<'js>,
    input: JsValue<'js>,
    format: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let input = moqui_lib::object::to_plain_string(&js_to_host(&ctx, &input)?);
        let format = optional_text(&ctx, format.0.as_ref())?;
        parsed_or_null(&ctx, l10n::parse_date_format(&input, format.as_deref()))
    })
}

fn l10n_parse_time<'js>(
    ctx: Ctx<'js>,
    input: JsValue<'js>,
    format: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let input = moqui_lib::object::to_plain_string(&js_to_host(&ctx, &input)?);
        let format = optional_text(&ctx, format.0.as_ref())?;
        parsed_or_null(&ctx, l10n::parse_time_format(&input, format.as_deref()))
    })
}

fn l10n_parse_number<'js>(
    ctx: Ctx<'js>,
    input: JsValue<'js>,
    _format: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let input = moqui_lib::object::to_plain_string(&js_to_host(&ctx, &input)?);
        parsed_or_null(&ctx, l10n::parse_number(&input))
    })
}

fn l10n_round_currency<'js>(
    ctx: Ctx<'js>,
    amount: JsValue<'js>,
    uom_id: Opt<JsValue<'js>>,
    precise: Opt<JsValue<'js>>,
    mode: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let amount = js_to_host(&ctx, &amount)?;
        let uom = optional_text(&ctx, uom_id.0.as_ref())?.unwrap_or_else(|| "USD".to_string());
        let precise = precise.0.as_ref().and_then(JsValue::as_bool).unwrap_or(false);
        // The fourth argument is a `RoundingMode` name or an old `ROUND_*` number; none is HALF_UP, as Java.
        let strategy = match mode.0.as_ref().filter(|mode| !mode.is_null() && !mode.is_undefined()) {
            Some(mode) => super::decimal::rounding_strategy(&ctx, Some(mode))?
                .ok_or_else(|| Exception::throw_message(&ctx, "rounding necessary"))?,
            None => moqui_lib::format::RoundingStrategy::MidpointAwayFromZero,
        };
        let rounded =
            l10n::round_currency_with(&amount, &uom, precise, strategy).map_err(|err| throw_error(&ctx, &err))?;
        host_to_js(&ctx, rounded)
    })
}

fn l10n_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let l10n = caught(ctx, "l10n", Object::new(ctx.clone()))?;
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, l10n.set($name, function))?;
        };
    }
    method!("localize", l10n_localize);
    method!("format", l10n_format);
    method!("formatCurrency", l10n_format_currency);
    method!("formatCurrencyNoSymbol", l10n_format_currency_no_symbol);
    method!("parseTimestamp", l10n_parse_timestamp);
    method!("parseDate", l10n_parse_date);
    method!("parseTime", l10n_parse_time);
    method!("parseNumber", l10n_parse_number);
    method!("roundCurrency", l10n_round_currency);
    // `formatNumber(n, format)` and `formatValue` are `format` for a number, as in Java.
    method!("formatNumber", l10n_format);
    method!("formatValue", l10n_format);
    Ok(l10n)
}

// ---- ec.context and ec.screen ----

/// `ec.context.get(name)`: the same list or map a bare `name` reads, so a write to it reaches the stack.
fn context_get<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        readopt_name(&name);
        flush_current(&ctx, false)?;
        let engine = current_engine();
        if let Some(saved) = engine
            .as_ref()
            .and_then(|engine| engine.tracked.borrow().get(&name).cloned())
        {
            return saved.restore(&ctx);
        }
        match stack_get(&name) {
            Lookup::Value(value @ (Value::List(_) | Value::Map(_))) => {
                let js = host_to_js(&ctx, value)?;
                if let Some(engine) = engine {
                    engine
                        .tracked
                        .borrow_mut()
                        .insert(name, Persistent::save(&ctx, js.clone()));
                }
                Ok(js)
            }
            Lookup::Value(value) => host_to_js(&ctx, value),
            _ => Null.into_js(&ctx),
        }
    })
}

fn context_name<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<JsValue<'js>> {
    readopt_name(name);
    flush_current(ctx, false)?;
    match stack_get(name) {
        Lookup::Value(value) => host_to_js(ctx, value),
        _ => Null.into_js(ctx),
    }
}

fn context_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let context = caught(ctx, "context", Object::new(ctx.clone()))?;
    let get = caught(ctx, "context.get", Function::new(ctx.clone(), context_get))?;
    caught(ctx, "context.get", context.set("get", get))?;
    let form_list =
        Accessor::from(|ctx: Ctx<'js>| host_guard(&ctx, || context_name(&ctx, "formListFindId"))).enumerable();
    caught(ctx, "context.formListFindId", context.prop("formListFindId", form_list))?;
    let order_by = Accessor::from(|ctx: Ctx<'js>| host_guard(&ctx, || context_name(&ctx, "orderByField"))).enumerable();
    caught(ctx, "context.orderByField", context.prop("orderByField", order_by))?;
    Ok(context)
}

fn screen_is_render_mode_valid(ctx: Ctx<'_>, mode: String) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        with_ec(&ctx, |ec| {
            crate::screen::ScreenFacade::of(ec.factory()).is_render_mode_valid(&mode)
        })
    })
}

fn screen_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let screen = caught(ctx, "screen", Object::new(ctx.clone()))?;
    let valid = caught(
        ctx,
        "screen.isRenderModeValid",
        Function::new(ctx.clone(), screen_is_render_mode_valid),
    )?;
    caught(ctx, "screen.isRenderModeValid", screen.set("isRenderModeValid", valid))?;
    super::ec_screen::install(ctx, &screen)?;
    Ok(screen)
}

// ---- static helpers ----

fn url_safe_chars(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains("..")
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(|ch| (ch as u32) < 32 || ch as u32 == 127)
}

/// `WebUtilities.encodeHtmlJsSafe`: HTML escapes plus the escapes a single-quoted JavaScript string needs. Unlike
/// Java it also escapes `\`, which would otherwise undo the quote escape, and U+2028 and U+2029, which end a
/// JavaScript string literal in older engines.
pub(crate) fn encode_html_js_safe(original: &str) -> String {
    let mut out = String::with_capacity(original.len());
    for ch in original.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '\u{5}' => out.push_str("..."),
            '\u{12}' => out.push_str("&apos;"),
            '\u{13}' | '\u{14}' => out.push_str("&quot;"),
            '\u{16}' | '\u{17}' => out.push('-'),
            '\u{19}' => out.push_str("tm"),
            other => out.push(other),
        }
    }
    out
}

fn js_safe_object(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::I32(_) | Value::I64(_) | Value::F64(_) | Value::Decimal(_) => moqui_lib::object::to_plain_string(value),
        Value::List(items) => {
            let parts: Vec<String> = items.iter().map(js_safe_object).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Map(entries) => js_safe_map(entries),
        other => format!("'{}'", encode_html_js_safe(&moqui_lib::object::to_plain_string(other))),
    }
}

fn js_safe_map(entries: &indexmap::IndexMap<String, Value>) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("'{}':{}", encode_html_js_safe(key), js_safe_object(value)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

fn web_encode_html(ctx: Ctx<'_>, text: Opt<String>) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(crate::screen::encode_html(text.0.as_deref().unwrap_or(""))))
}

fn web_encode_html_js_safe(ctx: Ctx<'_>, text: Opt<String>) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(encode_html_js_safe(text.0.as_deref().unwrap_or(""))))
}

fn web_field_values_encode_html_js_safe(ctx: Ctx<'_>, map: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || match js_to_host(&ctx, &map)? {
        Value::Map(entries) => Ok(js_safe_map(&entries)),
        _ => Ok("null".to_string()),
    })
}

fn web_ip_matches(ctx: Ctx<'_>, patterns: Opt<String>, address: Opt<String>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let patterns = patterns.0.unwrap_or_default();
        let address = address.0.unwrap_or_default();
        Ok(patterns.is_empty() || crate::web::ip_matches(&patterns, &address))
    })
}

fn web_is_safe_single_path_segment(ctx: Ctx<'_>, name: Opt<String>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(name.0.as_deref().is_some_and(url_safe_chars)))
}

/// The second argument is the configured host name. Java passes the request there, which a script
/// cannot hold, so anything else means the host of the web app's own URL.
fn web_is_same_origin_redirect(
    ctx: Ctx<'_>,
    url: Opt<String>,
    configured_host: Opt<JsValue<'_>>,
) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let configured = configured_host
            .0
            .as_ref()
            .and_then(|value| value.as_string().and_then(|text| text.to_string().ok()));
        Ok(crate::web::is_same_origin_redirect(
            &url.0.unwrap_or_default(),
            configured.as_deref(),
        ))
    })
}

fn web_last_path_segment(ctx: Ctx<'_>, name: Opt<String>) -> rquickjs::Result<Option<String>> {
    host_guard(&ctx, || {
        Ok(name.0.map(|name| {
            let name = name.replace('\\', "/");
            name.rsplit('/').next().unwrap_or("").to_string()
        }))
    })
}

fn string_encode_for_xml_attribute(
    ctx: Ctx<'_>,
    original: Opt<JsValue<'_>>,
    zero_width: Opt<bool>,
) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let text = original
            .0
            .as_ref()
            .and_then(|value| value.as_string().and_then(|text| text.to_string().ok()))
            .unwrap_or_default();
        Ok(moqui_lib::string::encode_for_xml_attribute(
            &text,
            zero_width.0.unwrap_or(false),
        ))
    })
}

fn string_base64_url_encode(ctx: Ctx<'_>, text: Opt<String>) -> rquickjs::Result<String> {
    use base64::Engine;
    host_guard(&ctx, || {
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.0.unwrap_or_default().as_bytes()))
    })
}

/// Standard base64 with padding of the text's UTF-8 bytes, as `Base64.getEncoder().encodeToString(text.getBytes())`.
fn string_base64_encode(text: Opt<String>) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(text.0.unwrap_or_default().as_bytes())
}

/// The text as it is when it is safe in a URL path, else URL encoded.
fn string_url_encode_if_needed(text: Opt<String>) -> String {
    moqui_lib::string::url_encode_if_needed(&text.0.unwrap_or_default())
}

/// HTML cleaned to the safe allow-list of `allow-html="safe"`.
fn string_clean_html(text: Opt<String>) -> String {
    crate::service::clean_safe_html(&text.0.unwrap_or_default())
}

fn string_base64_url_decode(ctx: Ctx<'_>, text: Opt<String>) -> rquickjs::Result<String> {
    use base64::Engine;
    host_guard(&ctx, || {
        let text = text.0.unwrap_or_default();
        let trimmed = text.trim_end_matches('=');
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(trimmed)
            .map_err(|err| Exception::throw_message(&ctx, &format!("not base64url text: {err}")))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
}

fn collection_paginate_parameters<'js>(
    ctx: Ctx<'js>,
    size: i32,
    name: String,
    context: JsValue<'js>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let mut entries = match js_to_host(&ctx, &context)? {
            Value::Map(entries) => entries,
            _ => indexmap::IndexMap::new(),
        };
        moqui_lib::collection::paginate_parameters(size, &name, &mut entries).map_err(|err| throw_lib(&ctx, &err))?;
        if let Some(target) = context.as_object() {
            for suffix in [
                "Count",
                "PageIndex",
                "PageSize",
                "PageMaxIndex",
                "PageRangeLow",
                "PageRangeHigh",
            ] {
                let key = format!("{name}{suffix}");
                if let Some(value) = entries.get(&key) {
                    target.set(key.as_str(), host_to_js(&ctx, value.clone())?)?;
                }
            }
        }
        collection_to_js(&ctx, Value::Map(entries))
    })
}

fn collection_get_map_array_list_values<'js>(
    ctx: Ctx<'js>,
    list: JsValue<'js>,
    key: String,
    exclude_null: Opt<bool>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let Value::List(items) = js_to_host(&ctx, &list)? else {
            return Null.into_js(&ctx);
        };
        let maps: Vec<indexmap::IndexMap<String, Value>> = items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                _ => None,
            })
            .collect();
        let values = moqui_lib::collection::get_map_list_values(&maps, &key, exclude_null.0.unwrap_or(false));
        collection_to_js(&ctx, Value::List(values))
    })
}

/// Fill `UserSecurityChecks`, the checks `Security.eecas.xml` runs before a write of a group membership or a
/// permission grant. A denied check throws, so the write fails.
pub(super) fn fill_security_checks<'js>(ctx: &Ctx<'js>, checks: &Object<'js>) -> Result<(), Error> {
    let function = caught(
        ctx,
        "assertPrivilegedGroupMembership",
        Function::new(ctx.clone(), security_assert_privileged_group_membership),
    )?;
    caught(
        ctx,
        "assertPrivilegedGroupMembership",
        checks.set("assertPrivilegedGroupMembership", function),
    )?;
    let function = caught(
        ctx,
        "assertSealedPermissionGrant",
        Function::new(ctx.clone(), security_assert_sealed_permission_grant),
    )?;
    caught(
        ctx,
        "assertSealedPermissionGrant",
        checks.set("assertSealedPermissionGrant", function),
    )?;
    Ok(())
}

fn security_assert_privileged_group_membership<'js>(
    ctx: Ctx<'js>,
    user_group_id: Opt<JsValue<'js>>,
) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let facade = facade_of(&ctx)?;
        crate::service::assert_privileged_group_membership(&facade, &opt_id_text(&ctx, &user_group_id)?)
            .map_err(|err| throw_lib(&ctx, &err))
    })
}

fn security_assert_sealed_permission_grant<'js>(
    ctx: Ctx<'js>,
    user_permission_id: Opt<JsValue<'js>>,
) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let facade = facade_of(&ctx)?;
        crate::service::assert_sealed_permission_grant(&facade, &opt_id_text(&ctx, &user_permission_id)?)
            .map_err(|err| throw_lib(&ctx, &err))
    })
}

/// An id argument as text: null, undefined or missing is empty.
fn opt_id_text<'js>(ctx: &Ctx<'js>, value: &Opt<JsValue<'js>>) -> rquickjs::Result<String> {
    match &value.0 {
        None => Ok(String::new()),
        Some(value) if value.is_null() || value.is_undefined() => Ok(String::new()),
        Some(value) => Ok(moqui_lib::object::to_plain_string(&js_to_host(ctx, value)?)),
    }
}

/// Fill the helper objects the bare names `WebUtilities`, `StringUtilities` and `CollectionUtilities` resolve to.
pub(super) fn fill_helpers<'js>(
    ctx: &Ctx<'js>,
    string_utilities: &Object<'js>,
    collection_utilities: &Object<'js>,
    web_utilities: &Object<'js>,
) -> Result<(), Error> {
    macro_rules! method {
        ($target:expr, $name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, $target.set($name, function))?;
        };
    }
    method!(
        string_utilities,
        "encodeForXmlAttribute",
        string_encode_for_xml_attribute
    );
    method!(string_utilities, "base64UrlEncode", string_base64_url_encode);
    method!(string_utilities, "base64UrlDecode", string_base64_url_decode);
    method!(string_utilities, "base64Encode", string_base64_encode);
    method!(string_utilities, "cleanHtml", string_clean_html);
    method!(string_utilities, "urlEncodeIfNeeded", string_url_encode_if_needed);
    method!(
        collection_utilities,
        "paginateParameters",
        collection_paginate_parameters
    );
    method!(
        collection_utilities,
        "getMapArrayListValues",
        collection_get_map_array_list_values
    );
    method!(web_utilities, "encodeHtml", web_encode_html);
    method!(web_utilities, "encodeHtmlJsSafe", web_encode_html_js_safe);
    method!(
        web_utilities,
        "fieldValuesEncodeHtmlJsSafe",
        web_field_values_encode_html_js_safe
    );
    method!(web_utilities, "ipMatches", web_ip_matches);
    method!(
        web_utilities,
        "isSafeSinglePathSegment",
        web_is_safe_single_path_segment
    );
    method!(web_utilities, "isSameOriginRedirect", web_is_same_origin_redirect);
    method!(web_utilities, "lastPathSegment", web_last_path_segment);
    Ok(())
}

pub(super) fn install_ec_extras<'js>(ctx: &Ctx<'js>, ec: &Object<'js>) -> Result<(), Error> {
    caught(ctx, "ec.l10n", ec.set("l10n", l10n_object(ctx)?))?;
    caught(ctx, "ec.context", ec.set("context", context_object(ctx)?))?;
    // `ec.factory.moquiVersion`, which the footer items of the default theme show.
    let factory = caught(ctx, "ec.factory", Object::new(ctx.clone()))?;
    caught(
        ctx,
        "ec.factory.moquiVersion",
        factory.set("moquiVersion", env!("CARGO_PKG_VERSION")),
    )?;
    super::native_tools::install(ctx, ec, &factory)?;
    caught(ctx, "ec.factory", ec.set("factory", factory))?;
    caught(ctx, "ec.screen", ec.set("screen", screen_object(ctx)?))
}

#[cfg(test)]
mod tests {
    use moqui_lib::object::Value;

    use crate::context::ExecutionContextFactory;
    use crate::script::{clear_context_stack, eval_condition, set_context_name};

    fn holds(expression: &str) {
        assert!(eval_condition(expression).unwrap(), "{expression}");
    }

    #[test]
    fn js_safe_escapes_backslash_first_and_the_line_separators() {
        assert_eq!(super::encode_html_js_safe(r"a\'b"), r"a\\\'b");
        assert_eq!(super::encode_html_js_safe("\\"), "\\\\");
        assert_eq!(super::encode_html_js_safe("x\u{2028}y\u{2029}z"), r"x\u2028y\u2029z");
        // The escaped text, put in a single-quoted literal, reads back as the original.
        clear_context_stack();
        holds(r#"eval("'" + WebUtilities.encodeHtmlJsSafe("a\\'b\u2028c\\") + "'") === "a\\'b\u2028c\\""#);
        holds(r#"WebUtilities.fieldValuesEncodeHtmlJsSafe({"k\\": "v\\'\u2029"}) === "{'k\\\\':'v\\\\\\'\\u2029'}""#);
    }

    #[test]
    fn web_string_and_collection_helpers() {
        clear_context_stack();
        holds(r#"WebUtilities.encodeHtmlJsSafe("a'b\"<\n") === "a\\'b&quot;&lt;\\n""#);
        holds(
            r#"WebUtilities.fieldValuesEncodeHtmlJsSafe({a: "x'y", b: [1, true, null]}) === "{'a':'x\\'y','b':[1,true,null]}""#,
        );
        holds(r#"WebUtilities.encodeHtml("<&>") === "&lt;&amp;&gt;""#);
        holds(
            r#"ec.l10n.format("1791979200000", "yyyy") === "2026" && ec.l10n.format(1791979200000 - 1, "yyyy") === "2026""#,
        );
        holds(r##"ec.l10n.format(1234.5, "#,##0.00") === "1,234.50""##);
        holds(
            r#"toDecimal("2.345").setScale(2, "HALF_UP").toString() === "2.35" && toDecimal("2.5").setScale(0, 6).toString() === "2""#,
        );
        holds(r#"toDecimal("2").setScale(2).toString() === "2.00""#);
        holds(
            r#"toNumber(toDecimal("2.5")) * 2 === 5 && toNumber("3") === 3 && toNumber(null) === null && toNumber(4) === 4"#,
        );
        holds(r#"elasticQueryAutoWildcard(" john  smith ", true) === "*:john* *:smith*""#);
        holds(r#"elasticQueryAutoWildcard("pump AND name:x 10.5 a-b", false) === "pump* AND name:x 10.5 a-b""#);
        holds(r#"elasticQueryAutoWildcard("  ", true) === "*" && elasticQueryAutoWildcard(null, false) === "*""#);
        holds(r#"WebUtilities.ipMatches("10.1.*.*,192.168.0.1-5", "10.1.2.3")"#);
        holds(r#"WebUtilities.ipMatches("10.1.*.*,192.168.0.1-5", "192.168.0.4")"#);
        holds(r#"!WebUtilities.ipMatches("10.1.*.*,192.168.0.1-5", "192.168.0.9")"#);
        holds(r#"WebUtilities.ipMatches("10.0.0.1", "::ffff:10.0.0.1") && WebUtilities.ipMatches("::1", "::1")"#);
        holds(r#"WebUtilities.ipMatches("", "1.2.3.4") && !WebUtilities.ipMatches("1.2.3.4", "")"#);
        holds(r#"WebUtilities.isSafeSinglePathSegment("a.zip") && !WebUtilities.isSafeSinglePathSegment("../x")"#);
        holds(r#"!WebUtilities.isSafeSinglePathSegment("a/b") && !WebUtilities.isSafeSinglePathSegment("")"#);
        holds(r#"WebUtilities.isSameOriginRedirect("/x") && !WebUtilities.isSameOriginRedirect("//evil/x")"#);
        holds(
            r#"!WebUtilities.isSameOriginRedirect("javascript:alert(1)") && !WebUtilities.isSameOriginRedirect("http://other/x")"#,
        );
        holds(r#"WebUtilities.isSameOriginRedirect("https://app.example/x", "app.example")"#);
        holds(r#"!WebUtilities.isSameOriginRedirect("https://user@app.example/x", "app.example")"#);
        holds(r#"WebUtilities.lastPathSegment("c:\\dir\\file.zip") === "file.zip""#);
        holds(r#"StringUtilities.encodeForXmlAttribute("a<b\"\u00e9") === "a&lt;b&quot;&#233;""#);
        holds(
            r#"StringUtilities.base64UrlDecode(StringUtilities.base64UrlEncode("hello/world?>")) === "hello/world?>""#,
        );
        holds(r#"StringUtilities.base64UrlEncode("??>>") === "Pz8-Pg""#);
        holds(r#"CollectionUtilities.getMapArrayListValues([{a: 1}, {a: null}, {b: 2}], "a", true).length === 1"#);
        holds(r#"CollectionUtilities.getMapArrayListValues([{a: 1}, {a: null}, {b: 2}], "a", false).length === 3"#);
        holds(
            r#"(function () {
                const page = {pageIndex: 1, pageSize: 10};
                CollectionUtilities.paginateParameters(25, "rows", page);
                return page.rowsCount === 25 && page.rowsPageRangeLow === 11 && page.rowsPageRangeHigh === 20 && page.rowsPageMaxIndex === 2;
            })()"#,
        );
    }

    #[test]
    fn resource_l10n_context_and_user_helpers() {
        clear_context_stack();
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        set_context_name("who", Value::string("Ada")).unwrap();
        set_context_name("shared", Value::string("outer")).unwrap();
        holds(r#"ec.resource.expand("Hi ${who}") === "Hi Ada""#);
        holds(r#"ec.resource.expand("Hi ${who} ${other}", "", {other: "Bob", who: "Eve"}) === "Hi Eve Bob""#);
        holds(r#"ec.resource.expand("Hi ${who}") === "Hi Ada""#);
        holds(
            r#"ec.resource.expandNoL10n("${shared}", "", {shared: "inner"}) === "inner" && ec.resource.expandNoL10n("${shared}") === "outer""#,
        );
        holds(
            r#"ec.resource.expression("who.length + 1") === 4 && ec.resource.expression("n * 2", "", {n: 21}) === 42"#,
        );
        holds(r#"ec.resource.expression("{a: 1}").a === 1"#);
        holds(
            r#"ec.resource.condition("who == 'Ada'") && !ec.resource.condition("who == 'Bob'") && !ec.resource.condition("")"#,
        );
        holds(r#"ec.l10n.localize("Plain text") === "Plain text""#);
        holds(
            r##"ec.l10n.format(1234.5, "#,##0.00") === "1,234.50" && ec.l10n.format("text", "0") === "text" && ec.l10n.format(null) === """##,
        );
        holds(
            r#"ec.l10n.formatCurrency(1234.5, "USD") === "$1,234.50" && ec.l10n.formatCurrency(5, "JPY") === "\u00a55""#,
        );
        holds(
            r#"ec.l10n.formatCurrencyNoSymbol(1234.5, "USD") === "1,234.50" && ec.l10n.formatCurrency("", "USD") === """#,
        );
        holds(r#"ec.l10n.formatCurrency(2, "USD", 3) === "$2.000""#);
        holds(
            r#"ec.l10n.parseTimestamp("2024-03-05 14:07") !== null && ec.l10n.parseTimestamp("not a date") === null"#,
        );
        holds(r#"ec.user.locale === "en_US" && ec.user.getLocale() === "en_US" && ec.user.nowTimestamp.length > 8"#);
        holds(
            r#"ec.context.get("who") === "Ada" && ec.context.get("nothing") === null && ec.context.formListFindId === null"#,
        );
        holds(
            r#"ec.screen.isRenderModeValid("csv") && ec.screen.isRenderModeValid("xsl-fo") && !ec.screen.isRenderModeValid("vuet")"#,
        );
        holds(r#"ec.web.getWebappRootUrl("webroot", "", true, true) === """#);
        clear_context_stack();
    }

    #[test]
    fn an_order_by_with_commas_orders_by_each_field() {
        clear_context_stack();
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(crate::testing::open());
        let _entered = context.enter();
        holds(
            r#"ec.entity.find("moqui.basic.EnumerationType").orderBy("-enumTypeId,^description").list().length === 0"#,
        );
    }
}
