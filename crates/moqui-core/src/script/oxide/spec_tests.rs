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

use moqui_lib::object::Value;

use super::{eval_named_value, install_global};
use crate::script::host::{Call, HostClass, HostError, Ret};
use crate::script::{clear_context_stack, set_context_name};

fn spec(source: &str) -> Value {
    eval_named_value(source, "Probe.spec.js").unwrap_or_else(|err| panic!("{source}: {err}"))
}

#[test]
fn whole_scripts_run_and_globals_last_for_the_thread() {
    clear_context_stack();
    // no small-script lockdown: function constructors, promises, proxies and symbols are QuickJS's main context's
    let made = spec(
        "new Function('a', 'return a * 2')(21) + (typeof Promise) + (typeof Proxy) + String(Symbol('s').description)",
    );
    assert_eq!(made, Value::String("42functionfunctions".to_string()));
    spec("void Object.defineProperty(globalThis, 'probeHelper', { value: (x) => x + 1, writable: true, configurable: true })");
    assert_eq!(spec("probeHelper(1)"), Value::I32(2));
    assert_eq!(
        spec("typeof System.getProperty + typeof StringUtilities"),
        Value::String("functionobject".to_string())
    );
}

#[test]
fn bare_names_are_the_context_stack() {
    clear_context_stack();
    set_context_name("probeName", Value::String("held".to_string())).expect("set");
    assert_eq!(spec("probeName + '!'"), Value::String("held!".to_string()));
    spec("probeWritten = 7");
    assert_eq!(crate::script::context_value("probeWritten"), Some(Value::I32(7)));
}

#[test]
fn an_error_names_the_file_and_line() {
    clear_context_stack();
    let err = eval_named_value("1;\nthrow new Error('probe failed')", "Probe.spec.js").expect_err("throws");
    let text = err.to_string();
    assert!(text.contains("probe failed"), "{text}");
    assert!(text.contains("Probe.spec.js:2"), "{text}");
}

static JOBS: HostClass = HostClass {
    name: "host:ProbeJobs",
    methods: &[],
    getters: &[],
    hidden: &[crate::script::host::method!("Probe", "runJob", [], None, leaf, run_job)],
    shim_js: Some(
        r#"(function (proto, hidden) {
            proto.drain = function (promise) {
                let out = null;
                promise.then((value) => { out = "ok " + value; }, (error) => { out = "no " + error.message; });
                while (out === null && hidden.runJob()) {}
                return out;
            };
        })"#,
    ),
    shim_members: &["drain"],
};

fn run_job(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(call.ops.run_pending_job()?.into())
}

#[test]
fn a_host_method_runs_the_pending_jobs() {
    clear_context_stack();
    install_global("probeJobs", &JOBS).expect("install");
    assert_eq!(
        spec("probeJobs.drain(Promise.resolve(1).then((x) => x + 1))"),
        Value::String("ok 2".to_string())
    );
    assert_eq!(
        spec("probeJobs.drain((async () => { await null; throw new Error('late'); })())"),
        Value::String("no late".to_string())
    );
    // a promise nothing settles: the queue empties and the shim stops
    assert_eq!(spec("probeJobs.drain(new Promise(() => {}))"), Value::Null);
}

static NESTED: HostClass = HostClass {
    name: "host:ProbeNested",
    // not a leaf: the spec runtime is lent, and the script runs on the thread's main runtime
    methods: &[crate::script::host::method!("Probe", "main", [Text], None, main_value)],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

fn main_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = call.text(0).unwrap_or_default();
    let hash = crate::script::text::hash_text(source);
    match super::super::eval_value(source, hash, true) {
        Ok(value) => Ok(Ret::Value(value)),
        Err(super::super::Outcome::Error(err)) => Err(HostError::Context(err)),
        Err(super::super::Outcome::Fallback(reason)) => Err(HostError::message(reason)),
    }
}

#[test]
fn a_host_call_runs_a_script_on_the_main_runtime() {
    clear_context_stack();
    install_global("probeNested", &NESTED).expect("install");
    set_context_name("probeShared", Value::I32(40)).expect("set");
    // the spec's own global is not the main runtime's: there the name is the context stack's, which has none
    spec("void Object.defineProperty(globalThis, 'probeOwn', { value: 1, writable: true, configurable: true })");
    assert_eq!(
        spec("[probeNested.main('probeShared + 2'), probeNested.main('typeof probeOwn'), probeOwn]"),
        Value::List(vec![Value::I32(42), Value::String("object".to_string()), Value::I32(1)])
    );
}

static MARKS: HostClass = HostClass {
    name: "host:ProbeMarks",
    // not a leaf: the main runtime's run is under the lend
    methods: &[crate::script::host::method!(
        "Probe",
        "markThenMain",
        [Text],
        None,
        mark_then_main
    )],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// The spec's run meets what the bindings do not have, then the main runtime runs a script.
fn mark_then_main(call: &mut Call<'_>) -> Result<Ret, HostError> {
    super::super::engine::set_fallback_for_tests("a probe's missing binding");
    main_value(call)
}

/// A lent main run that fails, and the spec run it ends, leave no runtime entered.
#[test]
fn a_failing_main_run_under_a_spec_leaves_the_runtimes_entered_balanced() {
    clear_context_stack();
    install_global("probeNested", &NESTED).expect("install");
    assert_eq!(super::super::engine::active_depth(), 0);
    let err = eval_named_value("probeNested.main('null.x')", "Probe.spec.js").expect_err("throws");
    assert!(err.to_string().contains("Probe.spec.js"), "{err}");
    assert_eq!(super::super::engine::active_depth(), 0);
    assert_eq!(
        spec("try { probeNested.main('null.x') } catch (e) { 'caught' }"),
        Value::String("caught".to_string())
    );
    assert_eq!(super::super::engine::active_depth(), 0);
}

/// The spec run's mark is kept through a main run under it, whether that run starts the main runtime (its shims'
/// host calls are not refused for the mark) or finds it started.
#[test]
fn a_main_run_under_a_spec_keeps_the_spec_runs_fallback() {
    clear_context_stack();
    install_global("probeMarks", &MARKS).expect("install");
    install_global("probeNested", &NESTED).expect("install");
    for _ in 0..2 {
        let err = eval_named_value("probeMarks.markThenMain('1 + 1')", "Probe.spec.js").expect_err("falls back");
        assert!(err.to_string().contains("a probe's missing binding"), "{err}");
        // the mark goes with the run that had it, and the main runtime started
        assert_eq!(spec("probeNested.main('2')"), Value::I32(2));
    }
}

/// The thread's memory report counts the spec runtime beside the main one.
#[test]
fn the_memory_report_counts_the_spec_runtime() {
    clear_context_stack();
    assert_eq!(super::super::engine_memory(false).engines, 0);
    spec("1");
    let spec_only = super::super::engine_memory(true);
    assert_eq!(spec_only.engines, 1);
    assert!(spec_only.reserved_bytes > 0);
    install_global("probeNested", &NESTED).expect("install");
    assert_eq!(spec("probeNested.main('1')"), Value::I32(1));
    let both = super::super::engine_memory(false);
    assert_eq!(both.engines, 2);
    assert!(both.reserved_bytes > spec_only.reserved_bytes);
}
