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
//! `new EdiHandler(ec)`, a global like `System`. The `ec` argument is accepted and not used. The constructor is a
//! shim over a hidden `make`, whose object is an [`EDI_HANDLER`]; `EdiHandler.makeAk3(segmentError)` stands in for
//! the Java `segmentError.makeAk3()`.

use std::sync::{Arc, Mutex};

use moqui_lib::object::{HostObject, Value};

use super::{lock_state, method, Call, HostClass, HostError, Ret, Shim};
use crate::edi::{EdiHandler, EdiMap, SegmentError};

const EDI_HANDLER_JS: &str = r#"(function (global, hidden) {
    function EdiHandler(ec) { this.handler = hidden.make(); }
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
    EdiHandler.makeAk3 = hidden.makeAk3;
    global.EdiHandler = EdiHandler;
})"#;

pub(super) static SHIM: Shim = Shim {
    js: EDI_HANDLER_JS,
    hidden: &[&[
        method!("EdiHandler", "make", [], make),
        method!("EdiHandler", "makeAk3", [Value], make_ak3),
    ]],
};

/// The handler a constructed `EdiHandler` wraps.
pub(super) static EDI_HANDLER: HostClass = HostClass {
    name: "host:EdiHandler",
    methods: &[
        method!("EdiHandler", "loadEnvelope", [Text], None, leaf, load_envelope),
        method!("EdiHandler", "loadBody", [Text], None, leaf, load_body),
        method!(
            "EdiHandler",
            "setChars",
            [OptValue, OptValue, OptValue, OptValue],
            set_chars
        ),
        method!("EdiHandler", "setX12DefaultChars", [], set_x12_default_chars),
        method!("EdiHandler", "setITradeDefaultChars", [], set_itrade_default_chars),
        method!("EdiHandler", "setEdifactDefaultChars", [], set_edifact_default_chars),
        method!("EdiHandler", "setSegmentSuffix", [OptValue], set_segment_suffix),
        method!("EdiHandler", "setPaddingChar", [Value], set_padding_char),
        method!("EdiHandler", "parseText", [Text], parse_text),
        method!("EdiHandler", "getSegmentErrors", [], get_segment_errors),
        method!("EdiHandler", "generateText", [Value], generate_text),
        method!("EdiHandler", "splitMessage", [Value, Value, Text], split_message),
        method!("EdiHandler", "combineSegments", [Value], combine_segments),
        method!("EdiHandler", "countSegments", [Value], count_segments),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

type State = Mutex<EdiHandler>;

fn make(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let state: State = Mutex::new(EdiHandler::new());
    Ok(Ret::Object(HostObject::new(state), &EDI_HANDLER))
}

fn make_ak3(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let error = SegmentError::from_value(&call.value_or_null(0)).map_err(HostError::Lib)?;
    Ok(Value::Map(error.make_ak3()).into())
}

fn handler(call: &Call<'_>) -> Result<Arc<State>, HostError> {
    call.this::<State>()
}

/// Change the handler; the call returns nothing (the shim returns the `EdiHandler`).
fn changed(call: &Call<'_>, change: impl FnOnce(&mut EdiHandler) -> Result<(), HostError>) -> Result<Ret, HostError> {
    let state = handler(call)?;
    change(&mut *lock_state(&state, "EdiHandler")?)?;
    Ok(Ret::Undefined)
}

/// Run the structure file in this engine. It evaluates to the structure list.
fn structure_at(call: &mut Call<'_>, location: &str) -> Result<Value, HostError> {
    let path = crate::context::locate(location).map_err(|err| HostError::message(err.message()))?;
    let text =
        std::fs::read_to_string(&path).map_err(|_| HostError::message(format!("could not read script {location}")))?;
    let source = crate::script::parse::file_function(&text);
    match call.ops.evaluate(&source, location) {
        Err(HostError::Message(message)) => Err(HostError::message(format!("EDI structure {location}: {message}"))),
        other => other,
    }
}

fn load_envelope(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let structure = structure_at(call, &location)?;
    changed(call, |handler| {
        handler.set_envelope(&structure).map(|_| ()).map_err(HostError::Lib)
    })
}

fn load_body(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let structure = structure_at(call, &location)?;
    changed(call, |handler| {
        handler.set_body(&structure).map(|_| ()).map_err(HostError::Lib)
    })
}

/// Null, undefined, missing or an empty string is no character. Otherwise the first character.
fn char_arg(call: &Call<'_>, index: usize) -> Option<char> {
    call.opt_text(index).and_then(|text| text.chars().next())
}

fn set_chars(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let chars = [
        char_arg(call, 0),
        char_arg(call, 1),
        char_arg(call, 2),
        char_arg(call, 3),
    ];
    changed(call, |handler| {
        handler.set_chars(chars[0], chars[1], chars[2], chars[3]);
        Ok(())
    })
}

fn set_x12_default_chars(call: &mut Call<'_>) -> Result<Ret, HostError> {
    changed(call, |handler| {
        handler.set_x12_default_chars();
        Ok(())
    })
}

fn set_itrade_default_chars(call: &mut Call<'_>) -> Result<Ret, HostError> {
    changed(call, |handler| {
        handler.set_itrade_default_chars();
        Ok(())
    })
}

fn set_edifact_default_chars(call: &mut Call<'_>) -> Result<Ret, HostError> {
    changed(call, |handler| {
        handler.set_edifact_default_chars();
        Ok(())
    })
}

fn set_segment_suffix(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let suffix = char_arg(call, 0);
    changed(call, |handler| {
        handler.segment_suffix = suffix;
        Ok(())
    })
}

fn set_padding_char(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let padding = char_arg(call, 0).ok_or_else(|| HostError::message("EdiHandler.setPaddingChar needs a character"))?;
    changed(call, |handler| {
        handler.padding_char = padding;
        Ok(())
    })
}

fn parse_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text_or_empty(0);
    let state = handler(call)?;
    let parsed = lock_state(&state, "EdiHandler")?.parse_text(&text);
    Ok(Value::Map(parsed.map_err(HostError::Lib)?).into())
}

fn get_segment_errors(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let state = handler(call)?;
    let handler = lock_state(&state, "EdiHandler")?;
    let errors = handler
        .segment_errors()
        .map(|errors| errors.iter().map(SegmentError::to_value).collect::<Vec<_>>());
    Ok(errors.map_or(Value::Null, Value::List).into())
}

fn edi_map(call: &Call<'_>, index: usize) -> Result<EdiMap, HostError> {
    match call.value_or_null(index) {
        Value::Map(map) => Ok(map),
        _ => Err(HostError::message("EDI segment is not a map")),
    }
}

fn generate_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let root = edi_map(call, 0)?;
    let state = handler(call)?;
    let generated = lock_state(&state, "EdiHandler")?.generate_text(&root);
    Ok(generated.map_err(HostError::Lib)?.into())
}

fn split_message(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let header = call.opt_text(0);
    let trailer = call.opt_text(1);
    let text = call.text_or_empty(2);
    let state = handler(call)?;
    let split = lock_state(&state, "EdiHandler")?.split_message(header.as_deref(), trailer.as_deref(), &text);
    let split = split.map_err(HostError::Lib)?;
    Ok(Value::List(split.into_iter().map(Value::String).collect()).into())
}

fn combine_segments(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let segments = super::host_texts(call.value_or_null(0));
    let state = handler(call)?;
    let combined = lock_state(&state, "EdiHandler")?.combine_segments(&segments);
    Ok(combined.into())
}

fn count_segments(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = edi_map(call, 0)?;
    let state = handler(call)?;
    let count = lock_state(&state, "EdiHandler")?.count_segments(&map);
    Ok(Value::I64(i64::try_from(count).unwrap_or(i64::MAX)).into())
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
                var missing = "";
                try {{ new EdiHandler(ec).loadBody("{dir}/missing.js"); }} catch (e) {{ missing = e.message; }}
                return [st.BEG[0].elements[3], edi.countSegments(st), errors.length, errors[0].segmentId,
                    ak3.elements.join("|"), edi.setSegmentSuffix("\n").generateText(map).replace(/ /g, "") === text.replace("ZZZ*1~\n", ""),
                    edi.splitMessage("ISA", "IEA", text + text).length, plain.indexOf("~\n") < 0 && plain.indexOf(" ") < 0,
                    missing.length > 0];
            }})()"#,
            envelope = envelope.display(),
            body = body.display(),
            dir = dir.display()
        );
        let result = eval_value(&script).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let Value::List(items) = result else {
            panic!("not a list: {result:?}");
        };
        let texts: Vec<String> = items.iter().map(moqui_lib::object::to_plain_string).collect();
        assert_eq!(
            texts,
            ["PO*1", "2", "1", "ZZZ", "AK3|ZZZ|3||6", "true", "2", "true", "true"]
        );
    }
}
