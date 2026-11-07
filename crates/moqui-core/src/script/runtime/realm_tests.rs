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

//! What QuickJS (through rquickjs) allows for one runtime per thread with a context per nesting depth, in place of a
//! runtime per depth: a nested context entered from inside a host call of the outer one, and compiled code shared
//! between contexts. Plain rquickjs, no engine of ours.

use std::ffi::CString;
use std::ptr::NonNull;

use rquickjs::{qjs, Context, Ctx, Function, Runtime, Value as JsValue};

/// Compile `source` as global code in `ctx` without running it: a function bytecode object.
fn compile<'js>(ctx: &Ctx<'js>, source: &str) -> JsValue<'js> {
    let text = CString::new(source).unwrap();
    let name = CString::new("realm_test").unwrap();
    let flags = i32::try_from(qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY).unwrap();
    // SAFETY: a live context, a NUL-terminated source whose length excludes the NUL.
    let raw = unsafe {
        qjs::JS_Eval(
            ctx.as_raw().as_ptr(),
            text.as_ptr(),
            text.as_bytes().len() as _,
            name.as_ptr(),
            flags,
        )
    };
    assert!(!unsafe { qjs::JS_IsException(raw) }, "compiles");
    // SAFETY: an owned value just returned for this context.
    unsafe { JsValue::from_raw(ctx.clone(), raw) }
}

/// Run a compiled global script in `ctx`, its result as text.
fn run<'js>(ctx: &Ctx<'js>, compiled: &JsValue<'js>) -> String {
    // SAFETY: `JS_EvalFunction` takes the reference it is given, so it gets a new one.
    let raw = unsafe {
        let function = qjs::JS_DupValue(ctx.as_raw().as_ptr(), compiled.as_raw());
        qjs::JS_EvalFunction(ctx.as_raw().as_ptr(), function)
    };
    // SAFETY: as in `compile`.
    let value = unsafe { JsValue::from_raw(ctx.clone(), raw) };
    if value.is_exception() {
        return format!("exception: {:?}", ctx.catch());
    }
    value
        .get::<rquickjs::Coerced<String>>()
        .map(|text| text.0)
        .unwrap_or_default()
}

/// The bytecode of a compiled script, and the same read back in another context.
fn write<'js>(ctx: &Ctx<'js>, compiled: &JsValue<'js>) -> Vec<u8> {
    let mut size: qjs::size_t = 0;
    let flags = i32::try_from(qjs::JS_WRITE_OBJ_BYTECODE).unwrap();
    // SAFETY: a live bytecode object of this context; the buffer is copied, then freed with the context's allocator.
    unsafe {
        let ptr = qjs::JS_WriteObject(ctx.as_raw().as_ptr(), &mut size, compiled.as_raw(), flags);
        assert!(!ptr.is_null());
        let bytes = std::slice::from_raw_parts(ptr, size as usize).to_vec();
        qjs::js_free(ctx.as_raw().as_ptr(), ptr.cast());
        bytes
    }
}

fn read<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> JsValue<'js> {
    let flags = i32::try_from(qjs::JS_READ_OBJ_BYTECODE).unwrap();
    // SAFETY: bytecode this process's QuickJS wrote; a live context.
    let raw = unsafe { qjs::JS_ReadObject(ctx.as_raw().as_ptr(), bytes.as_ptr(), bytes.len() as _, flags) };
    assert!(!unsafe { qjs::JS_IsException(raw) }, "reads");
    // SAFETY: as in `compile`.
    unsafe { JsValue::from_raw(ctx.clone(), raw) }
}

/// A host function of context A, called from A's script while A's `with` holds the runtime, enters context B of the
/// same runtime through `Ctx::from_raw` (no second `with`, which the held lock refuses) and runs a script there. Each
/// context keeps its own globals: B's `execute` and `x` do not replace A's.
#[test]
fn a_nested_context_of_the_same_runtime_runs_inside_a_host_call_with_its_own_globals() {
    let runtime = Runtime::new().unwrap();
    let outer = Context::full(&runtime).unwrap();
    let inner = Context::full(&runtime).unwrap();
    let inner_raw: NonNull<qjs::JSContext> = inner.as_raw();
    let result: String = outer.with(|a| {
        let nested = Function::new(a.clone(), move || -> String {
            // SAFETY: the runtime's lock is held by the outer `with` on this thread, and `b` does not outlive the call.
            let b = unsafe { Ctx::from_raw(inner_raw) };
            b.eval::<String, _>("var x = 'b'; function execute() { return 'B' } execute() + x")
                .unwrap()
        })
        .unwrap();
        a.globals().set("nested", nested).unwrap();
        a.eval::<String, _>(
            "var x = 'a'; function execute() { return 'A' } const r = nested(); r + '|' + execute() + x",
        )
        .unwrap()
    });
    assert_eq!(result, "Bb|Aa");
    // B kept its own global after the call
    let again: String = inner.with(|b| b.eval::<String, _>("execute() + x").unwrap());
    assert_eq!(again, "Bb");
}

/// A function bytecode object compiled in context A runs in context B with A's globals: QuickJS keeps the realm it
/// was compiled in (`JSFunctionBytecode.realm`) and a call runs there. So a compiled object cannot be shared between
/// contexts as it is, while bytecode read into each context (`JS_ReadObject`) runs with that context's globals.
#[test]
fn compiled_code_keeps_its_realm_but_bytecode_read_into_a_context_runs_there() {
    let runtime = Runtime::new().unwrap();
    let first = Context::full(&runtime).unwrap();
    let second = Context::full(&runtime).unwrap();
    let source = "typeof marker === 'string' ? marker : 'none'";
    let second_raw = second.as_raw();
    let (direct, read_back) = first.with(|a| {
        a.globals().set("marker", "first").unwrap();
        let compiled = compile(&a, source);
        let bytes = write(&a, &compiled);
        // SAFETY: the runtime's lock is held by this `with`.
        let b = unsafe { Ctx::from_raw(second_raw) };
        b.globals().set("marker", "second").unwrap();
        // the object compiled in A, run from B
        let compiled_in_b =
            unsafe { JsValue::from_raw(b.clone(), qjs::JS_DupValue(b.as_raw().as_ptr(), compiled.as_raw())) };
        let direct = run(&b, &compiled_in_b);
        // the bytecode read into B
        let read_in_b = read(&b, &bytes);
        (direct, run(&b, &read_in_b))
    });
    eprintln!("compiled in the first context, run from the second: {direct}; read into the second: {read_back}");
    assert_eq!(
        read_back, "second",
        "bytecode read into a context runs with its globals"
    );
}

/// Contexts nest three deep inside host calls, and an exception thrown in the innermost one comes back as an error of
/// that context's call without leaving the outer ones broken.
#[test]
fn three_contexts_nest_and_an_inner_exception_comes_back() {
    let runtime = Runtime::new().unwrap();
    let contexts: Vec<Context> = (0..3).map(|_| Context::full(&runtime).unwrap()).collect();
    let raws: Vec<NonNull<qjs::JSContext>> = contexts.iter().map(Context::as_raw).collect();
    let result: String = contexts[0].with(|a| {
        let (r1, r2) = (raws[1], raws[2]);
        let down = Function::new(a.clone(), move || -> String {
            // SAFETY: the lock is held by the outermost `with`.
            let b = unsafe { Ctx::from_raw(r1) };
            let deeper = Function::new(b.clone(), move || -> String {
                // SAFETY: as above.
                let c = unsafe { Ctx::from_raw(r2) };
                match c.eval::<String, _>("throw new Error('inner failed')") {
                    Ok(text) => text,
                    Err(_) => format!("caught: {:?}", c.catch().as_exception().and_then(|e| e.message())),
                }
            })
            .unwrap();
            b.globals().set("deeper", deeper).unwrap();
            b.eval::<String, _>("'b>' + deeper()").unwrap()
        })
        .unwrap();
        a.globals().set("down", down).unwrap();
        a.eval::<String, _>("'a>' + down() + '<a'").unwrap()
    });
    assert_eq!(result, "a>b>caught: Some(\"inner failed\")<a");
    let after: i32 = contexts[0].with(|a| a.eval::<i32, _>("1 + 1").unwrap());
    assert_eq!(after, 2);
}

/// Memory of 20 nesting levels holding the same large script: 20 runtimes (as now) against one runtime with 20
/// contexts, each reading the script's bytecode.
#[test]
fn twenty_contexts_of_one_runtime_against_twenty_runtimes() {
    let mut source = String::new();
    for at in 0..400 {
        source.push_str(&format!(
            "function f{at}(input) {{ const out = {{}}; for (const key of Object.keys(input)) out[key + '{at}'] = input[key]; return out }}\n"
        ));
    }
    source.push_str("'loaded'");
    let bytes = {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| write(&ctx, &compile(&ctx, &source)))
    };
    let runtimes: Vec<(Runtime, Context)> = (0..20)
        .map(|_| {
            let runtime = Runtime::new().unwrap();
            let context = Context::full(&runtime).unwrap();
            (runtime, context)
        })
        .collect();
    for (_, context) in &runtimes {
        context.with(|ctx| assert_eq!(run(&ctx, &read(&ctx, &bytes)), "loaded"));
    }
    let separate: i64 = runtimes
        .iter()
        .map(|(runtime, _)| {
            runtime.run_gc();
            runtime.memory_usage().malloc_size
        })
        .sum();
    let shared = Runtime::new().unwrap();
    let contexts: Vec<Context> = (0..20).map(|_| Context::full(&shared).unwrap()).collect();
    for context in &contexts {
        context.with(|ctx| assert_eq!(run(&ctx, &read(&ctx, &bytes)), "loaded"));
    }
    shared.run_gc();
    let one = shared.memory_usage().malloc_size;
    eprintln!(
        "20 runtimes: {} KiB; one runtime with 20 contexts: {} KiB; bytecode {} KiB",
        separate / 1024,
        one / 1024,
        bytes.len() / 1024
    );
    assert!(one < separate, "one runtime takes less");
}

/// What a runtime, a context and a loaded script each cost: an empty runtime with one full context, each further
/// empty context of it, and one more copy of the script's functions in a context.
#[test]
fn what_a_runtime_a_context_and_a_script_copy_cost() {
    let mut source = String::new();
    for at in 0..400 {
        source.push_str(&format!(
            "function f{at}(input) {{ const out = {{}}; for (const key of Object.keys(input)) out[key + '{at}'] = input[key]; return out }}\n"
        ));
    }
    source.push_str("'loaded'");
    let size = |runtime: &Runtime| {
        runtime.run_gc();
        runtime.memory_usage().malloc_size
    };
    let runtime = Runtime::new().unwrap();
    let bare = size(&runtime);
    let first = Context::full(&runtime).unwrap();
    let with_one = size(&runtime);
    let more: Vec<Context> = (0..10).map(|_| Context::full(&runtime).unwrap()).collect();
    let with_eleven = size(&runtime);
    let bytes = first.with(|ctx| write(&ctx, &compile(&ctx, &source)));
    first.with(|ctx| assert_eq!(run(&ctx, &read(&ctx, &bytes)), "loaded"));
    let one_copy = size(&runtime);
    for context in &more {
        context.with(|ctx| assert_eq!(run(&ctx, &read(&ctx, &bytes)), "loaded"));
    }
    let eleven_copies = size(&runtime);
    eprintln!(
        "runtime {} KiB, first context {} KiB, each further context {} KiB, the script's first copy {} KiB, each further copy {} KiB",
        bare / 1024,
        (with_one - bare) / 1024,
        (with_eleven - with_one) / 10 / 1024,
        (one_copy - with_eleven) / 1024,
        (eleven_copies - one_copy) / 10 / 1024
    );
}
