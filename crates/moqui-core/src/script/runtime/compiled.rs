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

//! Global code compiled once per engine. QuickJS compiles a script to a bytecode object
//! (`JS_EVAL_FLAG_COMPILE_ONLY`), and `JS_EvalFunction` runs that object as global code, with the
//! same semantics as evaluating the text: a bare assignment still reaches the context stack. Java
//! compiles XML actions and Groovy expressions to classes once; this is the same saving for the
//! action scripts, conditions, and expressions that run on every request and every form-list row.
//!
//! The bytecode is also kept for the process ([`SHARED_BYTECODE`]): an engine that has not compiled a script (another
//! thread's, a nested script's, one whose own cache let it go, or a new engine installing the host scripts) reads it
//! (`JS_ReadObject`) instead of parsing the text.
//!
//! rquickjs does not wrap these two calls for scripts, so they are made through `rquickjs::qjs`.

use std::ffi::CString;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use rquickjs::{qjs, CatchResultExt, Ctx, FromJs, Persistent, Value as JsValue};

use super::{caught_error, current_engine, panic_message, Error};
use crate::script::text::{hash_of, hash_text, ScriptText};

/// Compiled scripts kept per engine, and the source text they may add up to.
pub(super) const MAX_COMPILED_SCRIPTS: usize = 4096;
pub(super) const MAX_COMPILED_SCRIPT_BYTES: usize = 4 * 1024 * 1024;

/// Compiled scripts by the hash of their text and filename, plus strictness. Hits compare both text and filename.
pub(super) type CompiledScripts =
    crate::script::BoundedMap<(u64, bool), (Arc<str>, Arc<str>, Persistent<JsValue<'static>>)>;

/// Run `source` as global code, compiled on first use and kept by its text and strictness.
pub(super) fn eval_compiled<'js, T>(ctx: &Ctx<'js>, source: &str, strict: bool) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    eval_keyed(
        ctx,
        source,
        None,
        hash_of(hash_text(source), "eval_script"),
        strict,
        "eval_script",
    )
}

/// [`eval_compiled`] for a text hashed already. A hit on the same shared text compares no characters.
pub(super) fn eval_compiled_text<'js, T>(ctx: &Ctx<'js>, script: &ScriptText, strict: bool) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    eval_keyed(
        ctx,
        script.shared_text(),
        Some(script.shared_text()),
        hash_of(script.hash(), script.file_name()),
        strict,
        script.file_name(),
    )
}

fn eval_keyed<'js, T>(
    ctx: &Ctx<'js>,
    source: &str,
    shared: Option<&Arc<str>>,
    hash: u64,
    strict: bool,
    file_name: &str,
) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    let engine = current_engine();
    let saved = engine
        .as_ref()
        .and_then(|engine| engine.scripts.borrow_mut().get(&(hash, strict)))
        .filter(|(text, name, _)| {
            **name == *file_name && (shared.is_some_and(|shared| Arc::ptr_eq(shared, text)) || **text == *source)
        });
    let compiled = match saved {
        Some((_, _, saved)) => saved.restore(ctx).map_err(|err| Error::new(err.to_string()))?,
        None => {
            let compiled = compile_shared(ctx, source, hash, strict, file_name)?;
            if let Some(engine) = &engine {
                let text = shared.map_or_else(|| Arc::from(source), Arc::clone);
                engine.scripts.borrow_mut().insert_sized(
                    (hash, strict),
                    (text, Arc::from(file_name), Persistent::save(ctx, compiled.clone())),
                    source.len() + file_name.len(),
                );
            }
            compiled
        }
    };
    let value = run_compiled(ctx, &compiled)?;
    T::from_js(ctx, value)
        .catch(ctx)
        .map_err(|caught| caught_error(Error::script(caught.to_string()), &caught))
}

/// The bytecode of compiled global scripts, kept for the whole process: an engine that has not compiled a script
/// (another thread's, a nested depth's, or one whose own cache let it go) reads its bytecode (`JS_ReadObject`)
/// instead of parsing the text again. By the same key as an engine's cache; a hit compares the text and file name.
type SharedBytecode = crate::script::BoundedMap<(u64, bool), (Arc<str>, Arc<str>, Arc<[u8]>)>;

/// The bytecode the process keeps: the Mantle and Mr. Boss scripts are about 10 MB of source, their bytecode about
/// as much again.
const MAX_SHARED_BYTECODE_BYTES: usize = 64 * 1024 * 1024;

static SHARED_BYTECODE: std::sync::LazyLock<std::sync::Mutex<SharedBytecode>> = std::sync::LazyLock::new(|| {
    std::sync::Mutex::new(crate::script::BoundedMap::with_bytes(
        MAX_COMPILED_SCRIPTS * 4,
        MAX_SHARED_BYTECODE_BYTES,
    ))
});

/// Run `source` once as strict global code through [`SHARED_BYTECODE`] (not kept in the engine): the host scripts an
/// engine installs when it starts, and other code each engine evaluates the same.
pub(super) fn eval_shared<'js, T>(ctx: &Ctx<'js>, source: &str) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    let compiled = compile_shared(ctx, source, hash_text(source), true, "eval_script")?;
    let value = run_compiled(ctx, &compiled)?;
    T::from_js(ctx, value)
        .catch(ctx)
        .map_err(|caught| caught_error(Error::script(caught.to_string()), &caught))
}

/// Scripts read from [`SHARED_BYTECODE`] instead of compiled, since the process started.
static SHARED_BYTECODE_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Scripts compiled from their text, since the process started.
static COMPILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn compiled_count() -> u64 {
    COMPILED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Scripts an engine read from the process's bytecode instead of compiling them.
pub(crate) fn shared_bytecode_reads() -> u64 {
    SHARED_BYTECODE_READS.load(std::sync::atomic::Ordering::Relaxed)
}

/// [`compile_global`] through [`SHARED_BYTECODE`]: read from the bytecode another engine wrote for the same text, or
/// compiled here and its bytecode kept for the others. A bytecode that does not read is compiled again.
fn compile_shared<'js>(
    ctx: &Ctx<'js>,
    source: &str,
    hash: u64,
    strict: bool,
    file_name: &str,
) -> Result<JsValue<'js>, Error> {
    let kept = moqui_lib::sync::lock(&SHARED_BYTECODE)
        .get(&(hash, strict))
        .filter(|(text, name, _)| **text == *source && **name == *file_name)
        .map(|(_, _, bytes)| bytes);
    if let Some(bytes) = kept {
        let flags = i32::try_from(qjs::JS_READ_OBJ_BYTECODE).unwrap_or(1);
        // SAFETY: the context pointer is the live context of `ctx`; `bytes` is bytecode this process's QuickJS wrote
        // and stays alive for the call.
        let raw = unsafe { qjs::JS_ReadObject(ctx.as_raw().as_ptr(), bytes.as_ptr(), bytes.len() as _, flags) };
        if let Ok(read) = owned_or_exception(ctx, raw) {
            SHARED_BYTECODE_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(read);
        }
    }
    let compiled = compile_global(ctx, source, strict, file_name)?;
    let mut size: qjs::size_t = 0;
    let flags = i32::try_from(qjs::JS_WRITE_OBJ_BYTECODE).unwrap_or(1);
    // SAFETY: `compiled` is a live function bytecode object of this context; the buffer QuickJS returns is its own,
    // copied here and then freed with the context's allocator.
    let written = unsafe {
        let ptr = qjs::JS_WriteObject(ctx.as_raw().as_ptr(), &mut size, compiled.as_raw(), flags);
        if ptr.is_null() {
            None
        } else {
            let bytes: Arc<[u8]> = Arc::from(std::slice::from_raw_parts(ptr, size as usize));
            qjs::js_free(ctx.as_raw().as_ptr(), ptr.cast());
            Some(bytes)
        }
    };
    match written {
        Some(bytes) => {
            let weight = bytes.len() + source.len() + file_name.len();
            moqui_lib::sync::lock(&SHARED_BYTECODE).insert_sized(
                (hash, strict),
                (Arc::from(source), Arc::from(file_name), bytes),
                weight,
            );
        }
        // nothing kept: an exception the write left pending is cleared
        None => drop(ctx.catch()),
    }
    Ok(compiled)
}

fn compile_global<'js>(ctx: &Ctx<'js>, source: &str, strict: bool, file_name: &str) -> Result<JsValue<'js>, Error> {
    COMPILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let text = CString::new(source).map_err(|_| Error::new("script source contains a null byte"))?;
    let name = CString::new(file_name).map_err(|_| Error::new("file name contains a null byte"))?;
    let mut flags = qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY;
    if strict {
        flags |= qjs::JS_EVAL_FLAG_STRICT;
    }
    let flags = i32::try_from(flags).map_err(|_| Error::new("bad eval flags"))?;
    // SAFETY: the context pointer is the live context of `ctx`, the source is a NUL-terminated
    // copy whose length excludes the NUL, and the file name outlives the call.
    let raw = unsafe {
        qjs::JS_Eval(
            ctx.as_raw().as_ptr(),
            text.as_ptr(),
            text.as_bytes().len() as _,
            name.as_ptr(),
            flags,
        )
    };
    owned_or_exception(ctx, raw)
}

/// Run `source` once as sloppy global code, not kept, with `file_name` as its name in stack traces: a test spec file,
/// so an error names the file and line.
pub(super) fn eval_named<'js>(ctx: &Ctx<'js>, source: &str, file_name: &str) -> Result<JsValue<'js>, Error> {
    let text = CString::new(source).map_err(|_| Error::new("script source contains a null byte"))?;
    let name = CString::new(file_name).map_err(|_| Error::new("file name contains a null byte"))?;
    let flags = i32::try_from(qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY)
        .map_err(|_| Error::new("bad eval flags"))?;
    // SAFETY: as in `compile_global`; the file name is a NUL-terminated copy that outlives the call.
    let raw = unsafe {
        qjs::JS_Eval(
            ctx.as_raw().as_ptr(),
            text.as_ptr(),
            text.as_bytes().len() as _,
            name.as_ptr(),
            flags,
        )
    };
    let compiled = owned_or_exception(ctx, raw)?;
    run_compiled(ctx, &compiled)
}

fn run_compiled<'js>(ctx: &Ctx<'js>, compiled: &JsValue<'js>) -> Result<JsValue<'js>, Error> {
    // SAFETY: `JS_EvalFunction` takes ownership of the object it is given, so it gets a new
    // reference and the cached bytecode keeps its own.
    let raw = unsafe {
        let function = qjs::JS_DupValue(ctx.as_raw().as_ptr(), compiled.as_raw());
        qjs::JS_EvalFunction(ctx.as_raw().as_ptr(), function)
    };
    owned_or_exception(ctx, raw)
}

/// Wrap an owned value from the C API, or turn its pending exception into an error.
///
/// A callback panic rquickjs caught is kept in the runtime until one of its own wrappers sees an exception, and these
/// calls are not one: left there, it would be resumed by the next exception of some later, unrelated script. So it is
/// taken here, through the one public conversion that looks for it, and becomes this script's error.
fn owned_or_exception<'js>(ctx: &Ctx<'js>, raw: qjs::JSValue) -> Result<JsValue<'js>, Error> {
    // SAFETY: `raw` is an owned value just returned by the C API for this context.
    if unsafe { qjs::JS_IsException(raw) } {
        // SAFETY: the exception marker is not reference counted, so this value owns nothing.
        let marker = unsafe { JsValue::from_raw(ctx.clone(), raw) };
        let parked = std::panic::catch_unwind(AssertUnwindSafe(|| {
            <rquickjs::Result<JsValue> as FromJs>::from_js(ctx, marker).map(|_| ())
        }));
        if let Err(payload) = parked {
            drop(ctx.catch());
            return Err(Error::new(panic_message(payload)));
        }
        return Err(Err::<(), _>(rquickjs::Error::Exception).catch(ctx).map_or_else(
            |caught| caught_error(Error::script(caught.to_string()), &caught),
            |()| Error::new("script failed"),
        ));
    }
    // SAFETY: as above; the new value owns the reference.
    Ok(unsafe { JsValue::from_raw(ctx.clone(), raw) })
}
