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

//! `EdiHandler` for scripts. Mantle writes `new org.moqui.impl.util.EdiHandler(ec)`; the script is
//! `new EdiHandler(ec)`, a global like `System`. The `ec` argument is accepted and not used: the
//! handler runs on the thread's context. The setters and loaders return the handler, so calls chain.
//!
//! `setSegmentSuffix` (the one character after each terminator, a newline by default, so CRLF is not possible; null
//! for none) and `setPaddingChar` (the ISA padding, Java's no-break space by default; X12 senders want a space) are MR
//! Moqui additions for generating text.
//!
//! Parse results are plain objects and arrays. A segment error is a plain map, because mantle
//! passes the list through a service parameter; `EdiHandler.makeAk3(segmentError)` stands in for
//! the Java `segmentError.makeAk3()`.

use crate::edi::{EdiHandler, EdiMap, SegmentError};

use super::*;

const EDI_HANDLER_JS: &str = r#"(function (make, makeAk3) {
    function EdiHandler(ec) { this.handler = make(); }
    ["loadEnvelope", "loadBody", "setChars", "setX12DefaultChars", "setITradeDefaultChars", "setEdifactDefaultChars",
        "setSegmentSuffix", "setPaddingChar"]
        .forEach(function (name) {
            EdiHandler.prototype[name] = function () { this.handler[name].apply(this.handler, arguments); return this; };
        });
    ["parseText", "getSegmentErrors", "generateText", "splitMessage", "combineSegments", "countSegments"]
        .forEach(function (name) {
            EdiHandler.prototype[name] = function () { return this.handler[name].apply(this.handler, arguments); };
        });
    Object.defineProperty(EdiHandler.prototype, "segmentErrors", { get: function () { return this.handler.getSegmentErrors(); } });
    EdiHandler.makeAk3 = makeAk3;
    return EdiHandler;
})"#;

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostEdiHandler {
    #[qjs(skip_trace)]
    inner: RefCell<EdiHandler>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostEdiHandler {
    fn load_envelope(&self, ctx: Ctx<'_>, location: String) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let structure = structure_at(&ctx, &location)?;
            let mut handler = self.inner.borrow_mut();
            handler.set_envelope(&structure).map_err(|err| throw_lib(&ctx, &err))?;
            Ok(())
        })
    }

    fn load_body(&self, ctx: Ctx<'_>, location: String) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let structure = structure_at(&ctx, &location)?;
            let mut handler = self.inner.borrow_mut();
            handler.set_body(&structure).map_err(|err| throw_lib(&ctx, &err))?;
            Ok(())
        })
    }

    fn set_chars<'js>(
        &self,
        ctx: Ctx<'js>,
        segment_terminator: Opt<JsValue<'js>>,
        element_separator: Opt<JsValue<'js>>,
        component_delimiter: Opt<JsValue<'js>>,
        escape_character: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let segment_terminator = char_arg(&ctx, segment_terminator.0)?;
            let element_separator = char_arg(&ctx, element_separator.0)?;
            let component_delimiter = char_arg(&ctx, component_delimiter.0)?;
            let escape_character = char_arg(&ctx, escape_character.0)?;
            self.inner.borrow_mut().set_chars(
                segment_terminator,
                element_separator,
                component_delimiter,
                escape_character,
            );
            Ok(())
        })
    }

    #[qjs(rename = "setX12DefaultChars")]
    fn set_x12_default_chars(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            self.inner.borrow_mut().set_x12_default_chars();
            Ok(())
        })
    }

    #[qjs(rename = "setITradeDefaultChars")]
    fn set_itrade_default_chars(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            self.inner.borrow_mut().set_itrade_default_chars();
            Ok(())
        })
    }

    fn set_segment_suffix<'js>(&self, ctx: Ctx<'js>, suffix: Opt<JsValue<'js>>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            self.inner.borrow_mut().segment_suffix = char_arg(&ctx, suffix.0)?;
            Ok(())
        })
    }

    fn set_padding_char<'js>(&self, ctx: Ctx<'js>, padding: JsValue<'js>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let padding = char_arg(&ctx, Some(padding))?
                .ok_or_else(|| Exception::throw_message(&ctx, "EdiHandler.setPaddingChar needs a character"))?;
            self.inner.borrow_mut().padding_char = padding;
            Ok(())
        })
    }

    fn set_edifact_default_chars(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            self.inner.borrow_mut().set_edifact_default_chars();
            Ok(())
        })
    }

    fn parse_text<'js>(&self, ctx: Ctx<'js>, text: String) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let parsed = self.inner.borrow_mut().parse_text(&text);
            let map = parsed.map_err(|err| throw_lib(&ctx, &err))?;
            collection_to_js(&ctx, Value::Map(map))
        })
    }

    fn get_segment_errors<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let errors = self
                .inner
                .borrow()
                .segment_errors()
                .map(|errors| errors.iter().map(SegmentError::to_value).collect::<Vec<_>>());
            match errors {
                Some(errors) => collection_to_js(&ctx, Value::List(errors)),
                None => Null.into_js(&ctx),
            }
        })
    }

    fn generate_text<'js>(&self, ctx: Ctx<'js>, root: JsValue<'js>) -> rquickjs::Result<String> {
        host_guard(&ctx, || {
            let root = edi_map(&ctx, &root)?;
            let generated = self.inner.borrow().generate_text(&root);
            generated.map_err(|err| throw_lib(&ctx, &err))
        })
    }

    fn split_message<'js>(
        &self,
        ctx: Ctx<'js>,
        root_header_id: JsValue<'js>,
        root_trailer_id: JsValue<'js>,
        text: String,
    ) -> rquickjs::Result<Vec<String>> {
        host_guard(&ctx, || {
            let header = string_arg(&ctx, &root_header_id)?;
            let trailer = string_arg(&ctx, &root_trailer_id)?;
            let split = self
                .inner
                .borrow_mut()
                .split_message(header.as_deref(), trailer.as_deref(), &text);
            split.map_err(|err| throw_lib(&ctx, &err))
        })
    }

    fn combine_segments(&self, ctx: Ctx<'_>, segments: Vec<String>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.inner.borrow().combine_segments(&segments)))
    }

    fn count_segments<'js>(&self, ctx: Ctx<'js>, map: JsValue<'js>) -> rquickjs::Result<i64> {
        host_guard(&ctx, || {
            let map = edi_map(&ctx, &map)?;
            Ok(i64::try_from(self.inner.borrow().count_segments(&map)).unwrap_or(i64::MAX))
        })
    }
}

fn edi_new(ctx: Ctx<'_>) -> rquickjs::Result<Class<'_, HostEdiHandler>> {
    host_guard(&ctx, || {
        Class::instance(
            ctx.clone(),
            HostEdiHandler {
                inner: RefCell::new(EdiHandler::new()),
            },
        )
    })
}

fn edi_make_ak3<'js>(ctx: Ctx<'js>, segment_error: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let value = js_to_host(&ctx, &segment_error)?;
        let error = SegmentError::from_value(&value).map_err(|err| throw_lib(&ctx, &err))?;
        collection_to_js(&ctx, Value::Map(error.make_ak3()))
    })
}

/// Run the structure file in this context. It evaluates to the structure list.
fn structure_at(ctx: &Ctx<'_>, location: &str) -> rquickjs::Result<Value> {
    let source = read_script(location).map_err(|err| Exception::throw_message(ctx, err.message()))?;
    let evaluated: JsValue = eval_source(ctx, &source)
        .map_err(|err| Exception::throw_message(ctx, &format!("EDI structure {location}: {}", err.message())))?;
    js_to_host(ctx, &evaluated)
}

fn edi_map(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<EdiMap> {
    match js_to_host(ctx, value)? {
        Value::Map(map) => Ok(map),
        _ => Err(Exception::throw_message(ctx, "EDI segment is not a map")),
    }
}

/// Null, undefined, or an empty string is no character. Otherwise the first character.
fn char_arg(ctx: &Ctx<'_>, value: Option<JsValue<'_>>) -> rquickjs::Result<Option<char>> {
    let Some(value) = value else {
        return Ok(None);
    };
    Ok(string_arg(ctx, &value)?.and_then(|text| text.chars().next()))
}

fn string_arg(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<Option<String>> {
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    Ok(Some(moqui_lib::object::to_plain_string(&js_to_host(ctx, value)?)))
}

/// Define the global `EdiHandler`.
pub(super) fn install(ctx: &Ctx<'_>) -> Result<(), Error> {
    let make = caught(ctx, "EdiHandler", Function::new(ctx.clone(), edi_new))?;
    let make_ak3 = caught(ctx, "EdiHandler.makeAk3", Function::new(ctx.clone(), edi_make_ak3))?;
    let wrap: Function = eval_js(ctx, EDI_HANDLER_JS)?;
    let class: JsValue = caught(ctx, "EdiHandler", wrap.call((make, make_ak3)))?;
    caught(ctx, "globals", ctx.globals().set("EdiHandler", class))
}

#[cfg(test)]
mod tests {
    use crate::script::eval_value;
    use moqui_lib::object::Value;

    const ENVELOPE: &str = "[{ID:'ISA', LEVEL:[{ID:'GS', LEVEL:[{ID:'ST'}, {ID:'GE'}]}, {ID:'IEA'}]}]";
    const BODY: &str = "[{ID:'ST', LEVEL:[{ID:'BEG'}, {ID:'SE'}]}]";

    #[test]
    fn script_parses_generates_and_makes_ak3() {
        let dir = std::env::temp_dir().join(format!("moqui-edi-js-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let envelope = dir.join("envelope.js");
        let body = dir.join("body.js");
        std::fs::write(&envelope, ENVELOPE).unwrap();
        std::fs::write(&body, BODY).unwrap();
        let script = format!(
            r#"(function () {{
                var edi = new EdiHandler(ec).loadEnvelope("{envelope}").loadBody("{body}")
                    .setChars("~", "*", null, null);
                var text = "ISA*00*X~\nGS*PO*1~\nST*850*0001~\nBEG*00*SA*PO?*1~\nZZZ*1~\nSE*3*0001~\nGE*1*1~\nIEA*1*1~\n";
                // The ISA elements are padded to their fixed widths when generated.
                var map = edi.parseText(text.trim());
                var st = map.ISA[0].GS[0].ST[0];
                var errors = edi.getSegmentErrors();
                var ak3 = EdiHandler.makeAk3(errors[0]);
                var plain = edi.setSegmentSuffix(null).setPaddingChar(" ").generateText(map);
                return [st.BEG[0].elements[3], edi.countSegments(st), errors.length, errors[0].segmentId,
                    ak3.elements.join("|"), edi.setSegmentSuffix("\n").generateText(map).replace(/ /g, "") === text.replace("ZZZ*1~\n", ""),
                    edi.splitMessage("ISA", "IEA", text + text).length, plain.indexOf("~\n") < 0 && plain.indexOf("\u00a0") < 0];
            }})()"#,
            envelope = envelope.display(),
            body = body.display()
        );
        let result = eval_value(&script).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let Value::List(items) = result else {
            panic!("not a list: {result:?}");
        };
        let texts: Vec<String> = items.iter().map(moqui_lib::object::to_plain_string).collect();
        assert_eq!(texts, ["PO*1", "2", "1", "ZZZ", "AK3|ZZZ|3||6", "true", "2", "true"]);
    }
}
