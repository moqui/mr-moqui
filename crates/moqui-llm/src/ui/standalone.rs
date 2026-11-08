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

//! A JavaScript runtime of its own, with no `ec` and no host functions, for the checks that run a library's code
//! (OpenUI Lang's parser, a Vue SFC script's compile check): QuickJS, or quickjs-oxide under `script_engine=oxide`
//! (plans/Script-Engine-Oxide.md, Phase 3). Each has a heap cap, and an interrupt the caller answers (its deadline).

use moqui_core::script::{script_engine, ScriptEngine};
use quickjs_oxide::engine::api::core::{
    HeapLimits, Host, Js, JsString, PropertyKey, RuntimeError, RuntimeOptions, Value as OxValue,
};
use rquickjs::CatchResultExt;

/// The runtime and its realm.
pub(crate) enum Standalone {
    QuickJs {
        context: rquickjs::Context,
        // the context borrows the runtime; drop order frees the context first
        _runtime: rquickjs::Runtime,
    },
    Oxide {
        context: quickjs_oxide::engine::api::core::Context,
        _runtime: quickjs_oxide::engine::api::core::Runtime,
    },
}

/// The oxide runtime's host: UTC and the system clock (the default's), the caller's interrupt.
struct Interrupt(Box<dyn Fn() -> bool>);

impl Host for Interrupt {
    fn interrupt(&mut self) -> bool {
        (self.0)()
    }
}

impl Standalone {
    /// A runtime of the `script_engine` setting's engine (QuickJS unless it is `oxide`), its heap at most
    /// `memory_limit` bytes, stopped when `interrupt` answers true. `stack_limit` is QuickJS's stack size; quickjs-oxide
    /// keeps its own default.
    pub(crate) fn start(
        memory_limit: usize,
        stack_limit: usize,
        interrupt: impl Fn() -> bool + 'static,
    ) -> Result<Self, String> {
        if script_engine() == ScriptEngine::Oxide {
            let options = RuntimeOptions {
                heap: HeapLimits {
                    max_bytes: memory_limit,
                },
                ..RuntimeOptions::default()
            };
            let runtime = quickjs_oxide::engine::api::core::Runtime::with_host(Interrupt(Box::new(interrupt)), options);
            let context = runtime.context();
            return Ok(Standalone::Oxide {
                context,
                _runtime: runtime,
            });
        }
        let runtime = rquickjs::Runtime::new().map_err(|err| err.to_string())?;
        runtime.set_memory_limit(memory_limit);
        runtime.set_max_stack_size(stack_limit);
        runtime.set_interrupt_handler(Some(Box::new(interrupt)));
        let context = rquickjs::Context::full(&runtime).map_err(|err| err.to_string())?;
        Ok(Standalone::QuickJs {
            context,
            _runtime: runtime,
        })
    }

    /// Run `source` (a script) for what it defines.
    pub(crate) fn run(&self, source: &str) -> Result<(), String> {
        match self {
            Standalone::QuickJs { context, .. } => {
                context.with(|ctx| ctx.eval::<(), _>(source).catch(&ctx).map_err(|err| err.to_string()))
            }
            Standalone::Oxide { context, .. } => {
                let mut context = context.clone();
                context
                    .eval(source)
                    .map(|_| ())
                    .map_err(|err| error_text(&mut context, err))
            }
        }
    }

    /// The value of `source` (a script whose value is a string), as text. A thrown error is `Error: <message>` and its
    /// stack, as QuickJS shows it.
    pub(crate) fn text(&self, source: &str) -> Result<String, String> {
        match self {
            Standalone::QuickJs { context, .. } => {
                context.with(|ctx| ctx.eval::<String, _>(source).catch(&ctx).map_err(|err| err.to_string()))
            }
            Standalone::Oxide { context, .. } => {
                let mut context = context.clone();
                let value = context.eval(source).map_err(|err| error_text(&mut context, err))?;
                match value {
                    OxValue::String(text) => Ok(text.to_utf8_lossy()),
                    other => {
                        let shown = context.print_value(&other).unwrap_or_default();
                        Err(format!("Error converting from js '{shown}' into type 'String'"))
                    }
                }
            }
        }
    }
}

/// An oxide error as QuickJS's caught error shows it.
fn error_text(js: &mut impl Js, err: RuntimeError) -> String {
    let RuntimeError::Exception(thrown) = err else {
        return format!("Error: {err}");
    };
    let object = thrown.as_object().cloned();
    let Some(object) = object.filter(|object| js.is_error_object(object).unwrap_or(false)) else {
        return format!(
            "Exception generated by quickjs: {}",
            js.print_value(&thrown).unwrap_or_default()
        );
    };
    let mut text = String::from("Error:");
    let mut property = |name: &str| -> Option<String> {
        let key = PropertyKey::from_js_string(JsString::try_from_utf8(name).ok()?);
        match js.get_property(&object, &key).ok()? {
            OxValue::String(text) => Some(text.to_utf8_lossy()),
            _ => None,
        }
    };
    if let Some(message) = property("message") {
        text.push(' ');
        text.push_str(&message);
    }
    if let Some(stack) = property("stack") {
        text.push('\n');
        text.push_str(&stack);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::Standalone;
    use moqui_core::script::{with_script_engine, ScriptEngine};

    #[test]
    fn both_engines_run_text_and_show_errors_alike() {
        for engine in [ScriptEngine::Rquickjs, ScriptEngine::Oxide] {
            with_script_engine(engine, || {
                let js = Standalone::start(16 * 1024 * 1024, 1024 * 1024, || false).expect("starts");
                js.run("globalThis.twice = (x) => x + x").expect("defines");
                assert_eq!(js.text("twice('ab')").as_deref(), Ok("abab"), "{engine:?}");
                let thrown = js
                    .text("(function f() { throw new SyntaxError('no') })()")
                    .expect_err("throws");
                assert!(thrown.starts_with("Error: no\n"), "{engine:?}: {thrown}");
                let stopped = Standalone::start(16 * 1024 * 1024, 1024 * 1024, || true).expect("starts");
                assert!(stopped.run("for (;;) {}").is_err(), "{engine:?}");
            });
        }
    }
}
