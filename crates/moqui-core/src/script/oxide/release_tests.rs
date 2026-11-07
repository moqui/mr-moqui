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

//! What a script held goes when the outermost script on the thread ends, not at a collection
//! (`bind::release_run_objects`, `run::enter_depth`): the objects stay reachable from the runtime's global object, and
//! their resources go all the same.

use std::sync::Arc;

use moqui_lib::object::{HostObject, Value};

use super::{eval_value, Outcome};
use crate::context::Error;
use crate::script::text::hash_text;
use crate::script::{clear_context_stack, push_context_frame, set_context_name};
use crate::web::multipart::UploadedFile;

fn value(source: &str) -> Result<Value, Error> {
    match eval_value(source, hash_text(source), false) {
        Ok(value) => Ok(value),
        Err(Outcome::Error(err)) => Err(err),
        Err(Outcome::Fallback(reason)) => Err(Error::new(format!("fallback: {reason}"))),
    }
}

/// Defines `name` on the global object as its own property, so the runtime holds the object: a plain assignment to a
/// property the global object does not have goes to the context stack through the bare names' prototype, as a copy
/// the host holds.
fn keep(name: &str, value: &str) -> String {
    format!("Object.defineProperty(globalThis, '{name}', {{value: {value}, writable: true, configurable: true}})")
}

/// An upload and its stream kept on the global object: when the script ends, the runtime holds the upload no more (only
/// the context does), and a later script that reaches them gets an error, not the bytes.
#[test]
fn an_upload_and_its_stream_are_let_go_when_the_script_ends() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let upload = HostObject::new(UploadedFile::in_memory("doc", "a.txt", "text/plain", b"abc".to_vec()));
    let held = Arc::downgrade(&upload.0);
    set_context_name("doc", Value::Object(upload))?;
    assert_eq!(
        value(&format!(
            "(() => {{ {}; {}; return globalThis.keptDoc.getName() }})()",
            keep("keptDoc", "doc"),
            keep("keptStream", "doc.getInputStream()")
        ))?,
        Value::string("a.txt")
    );
    assert_eq!(
        super::bind::release_pending_for_tests(),
        0,
        "the list went with the run"
    );
    assert_eq!(held.strong_count(), 1, "only the context holds the upload");
    clear_context_stack();
    assert_eq!(held.strong_count(), 0, "nothing in the runtime holds it");
    assert_eq!(
        value("[typeof globalThis.keptDoc, typeof globalThis.keptStream].join()")?,
        Value::string("object,object"),
        "the objects are still reachable"
    );
    for text in ["globalThis.keptDoc.getName()", "globalThis.keptStream.available()"] {
        let err = value(text).expect_err(text);
        assert!(
            err.message().contains("was let go when the script that made it ended"),
            "{text}: {}",
            err.message()
        );
    }
    let _ = value("(() => { delete globalThis.keptDoc; delete globalThis.keptStream; return 0 })()");
    Ok(())
}

/// An SFTP client the script did not close is closed when the script ends (its connection, were it open, with an error
/// in the log, as Java's `finalize`), though the global object still holds it.
#[test]
fn an_sftp_client_is_let_go_when_the_script_ends() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    assert_eq!(
        value(&format!(
            "(() => {{ {}; return globalThis.keptClient.isConnected() }})()",
            keep("keptClient", "new SftpClient('localhost', 'nobody')")
        ))?,
        Value::Bool(false)
    );
    let err = value("globalThis.keptClient.isConnected()").expect_err("a client let go");
    assert!(err.message().contains("SftpClient was let go"), "{}", err.message());
    let _ = value("(() => { delete globalThis.keptClient; return 0 })()");
    Ok(())
}

/// A response body a script opened with `send()` and did not read goes when the outermost oxide script ends, as it does
/// on QuickJS.
#[test]
fn an_unread_response_body_goes_when_the_script_ends() -> Result<(), Error> {
    struct Body(Arc<std::sync::atomic::AtomicBool>);
    impl std::io::Read for Body {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }
    impl Drop for Body {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    clear_context_stack();
    push_context_frame();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    crate::web::upstream::register(Box::new(Body(Arc::clone(&dropped))), None);
    assert_eq!(value("1 + 1")?, Value::I32(2));
    assert!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        "the body was dropped"
    );
    Ok(())
}

/// While a release scope is open (a spec's whole run), an object outlives the outermost script that made it; the
/// scope's end lets it go.
#[test]
fn a_release_scope_keeps_objects_across_scripts_until_it_ends() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name(
        "doc",
        Value::Object(HostObject::new(UploadedFile::in_memory(
            "doc",
            "a.txt",
            "text/plain",
            b"abc".to_vec(),
        ))),
    )?;
    let scope = crate::script::ReleaseScope::enter();
    value(&format!(
        "(() => {{ {}; return 0 }})()",
        keep("scopedStream", "doc.getInputStream()")
    ))?;
    assert_eq!(value("globalThis.scopedStream.available()")?, Value::I32(3));
    assert!(super::bind::release_pending_for_tests() > 0, "held for the scope");
    drop(scope);
    assert_eq!(super::bind::release_pending_for_tests(), 0);
    let err = value("globalThis.scopedStream.available()").expect_err("let go at the scope's end");
    assert!(err.message().contains("was let go"), "{}", err.message());
    let _ = value("(() => { delete globalThis.scopedStream; return 0 })()");
    clear_context_stack();
    Ok(())
}

/// An object let go in a value a later script writes back converts to null: the write-back at the script's end does
/// not fail. A call of its methods still throws.
#[test]
fn a_let_go_object_in_a_written_back_value_is_null() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name(
        "doc",
        Value::Object(HostObject::new(UploadedFile::in_memory(
            "doc",
            "a.txt",
            "text/plain",
            b"abc".to_vec(),
        ))),
    )?;
    set_context_name("items", Value::List(Vec::new()))?;
    value(&format!("(() => {{ {}; return 0 }})()", keep("goneDoc", "doc")))?;
    let source = "(() => { items.push(globalThis.goneDoc); return items.length })()";
    match eval_value(source, hash_text(source), true) {
        Ok(value) => assert_eq!(value, Value::I32(1)),
        Err(Outcome::Error(err)) => panic!("{}", err.message()),
        Err(Outcome::Fallback(reason)) => panic!("fallback: {reason}"),
    }
    assert_eq!(
        crate::script::context_value("items"),
        Some(Value::List(vec![Value::Null]))
    );
    let _ = value("(() => { delete globalThis.goneDoc; return 0 })()");
    clear_context_stack();
    Ok(())
}

/// The list of objects to let go drops the entries of those already collected as it grows, so a long script that
/// makes one per record keeps no entry for each.
#[test]
fn the_list_of_objects_to_let_go_drops_collected_ones_as_it_grows() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name(
        "doc",
        Value::Object(HostObject::new(UploadedFile::in_memory(
            "doc",
            "a.txt",
            "text/plain",
            b"abc".to_vec(),
        ))),
    )?;
    // the runtime collects as the loop allocates; each stream is garbage once read
    let source = "(() => { let total = 0; for (let i = 0; i < 20000; i++) total += doc.getInputStream().available(); \
                  return total })()";
    let _scope = crate::script::ReleaseScope::enter();
    assert_eq!(value(source)?, Value::I32(60_000));
    let pending = super::bind::release_pending_for_tests();
    assert!(pending < 10_000, "{pending} entries kept");
    clear_context_stack();
    Ok(())
}
