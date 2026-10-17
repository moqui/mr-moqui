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
//! rquickjs does not wrap these two calls for scripts, so they are made through `rquickjs::qjs`.

use std::ffi::CString;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use rquickjs::{qjs, CatchResultExt, Ctx, FromJs, Persistent, Value as JsValue};

use super::{caught_kind, current_engine, panic_message, Error};

/// Compiled scripts kept per engine, and the source text they may add up to.
pub(super) const MAX_COMPILED_SCRIPTS: usize = 4096;
pub(super) const MAX_COMPILED_SCRIPT_BYTES: usize = 4 * 1024 * 1024;

/// Compiled scripts by the hash of their text and filename, plus strictness. Hits compare both text and filename.
pub(super) type CompiledScripts =
    crate::script::BoundedMap<(u64, bool), (Arc<str>, Arc<str>, Persistent<JsValue<'static>>)>;

/// A script's text with the hash its compiled form is kept by, made once for a text that runs many times.
#[derive(Clone)]
pub(crate) struct ScriptText {
    text: Arc<str>,
    file_name: Arc<str>,
    hash: u64,
}

impl ScriptText {
    pub(crate) fn new(text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        Self::named(text, "eval_script")
    }

    pub(crate) fn named(text: impl Into<Arc<str>>, file_name: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let file_name = file_name.into();
        Self {
            hash: hash_text(&text),
            text,
            file_name,
        }
    }

    pub(crate) fn with_file_name(&self, file_name: &str) -> Self {
        Self {
            text: Arc::clone(&self.text),
            file_name: Arc::from(file_name),
            hash: self.hash,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn hash_of(text_hash: u64, file_name: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text_hash.hash(&mut hasher);
    file_name.hash(&mut hasher);
    hasher.finish()
}

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
        &script.text,
        Some(&script.text),
        hash_of(script.hash, &script.file_name),
        strict,
        &script.file_name,
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
            let compiled = compile_global(ctx, source, strict, file_name)?;
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
        .map_err(|caught| Error::script(caught.to_string()).with_kind(caught_kind(&caught)))
}

fn compile_global<'js>(ctx: &Ctx<'js>, source: &str, strict: bool, file_name: &str) -> Result<JsValue<'js>, Error> {
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
            |caught| Error::script(caught.to_string()).with_kind(caught_kind(&caught)),
            |()| Error::new("script failed"),
        ));
    }
    // SAFETY: as above; the new value owns the reference.
    Ok(unsafe { JsValue::from_raw(ctx.clone(), raw) })
}
