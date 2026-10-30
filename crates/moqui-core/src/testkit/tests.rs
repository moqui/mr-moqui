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

//! The harness on the in-memory database: what a spec file can say, and what a run reports.

use std::sync::{Arc, Mutex};

use super::run::{
    run_on_thread, run_spec, CaseExtras, Outcome, RunOptions, SpecFile, SpecHost, SpecResult, SLOW_REASON,
};
use crate::context::{ExecutionContext, ExecutionContextFactory};

fn run(source: &str) -> SpecResult {
    run_with(source, &RunOptions::default())
}

fn run_with(source: &str, options: &RunOptions) -> SpecResult {
    crate::script::clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let _entered = context.enter();
    let spec = SpecFile {
        name: "Sample".to_string(),
        display: "test/Sample.spec.js".to_string(),
        source: source.to_string(),
    };
    run_spec(&spec, options, &mut |_| {})
}

fn outcomes(result: &SpecResult) -> Vec<(String, &'static str)> {
    result
        .cases
        .iter()
        .map(|case| {
            let kind = match &case.outcome {
                Outcome::Passed => "passed",
                Outcome::Failed(_) => "failed",
                Outcome::Ignored(_) => "ignored",
                Outcome::Skipped(_) => "skipped",
            };
            (case.name.clone(), kind)
        })
        .collect()
}

fn failure(result: &SpecResult, index: usize) -> &super::run::Failure {
    match &result.cases[index].outcome {
        Outcome::Failed(failure) => failure,
        other => panic!("case {index} did not fail: {other:?}"),
    }
}

#[test]
fn features_share_file_state_and_pass_values_from_when_to_then() {
    let result = run(r#"
let shared = 0
const order = []
spec({
    setupSpec() { order.push("setupSpec") },
    setup() { order.push("setup") },
    cleanup() { order.push("cleanup") },
    cleanupSpec() { if (order.join() !== "setupSpec,setup,cleanup,setup,cleanup") throw new Error(order.join()) },
})
feature("first", {
    when() { shared = 2; return { total: toDecimal("16.990"), list: [1, toDecimal("2.0")] } },
    then({ total, list }) {
        total == 16.99
        total > 10
        shared == 2
        list == [1, 2]
        list != []
        !""
    },
})
feature("second", {
    given() { return { a: 1 } },
    expect: ({ a }) => a + shared == "3",
})
"#);
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert!(result.cleanup_failure.is_none(), "{:?}", result.cleanup_failure);
    assert_eq!(
        outcomes(&result),
        vec![("first".to_string(), "passed"), ("second".to_string(), "passed")]
    );
}

#[test]
fn a_false_condition_fails_with_a_diagram_and_its_line() {
    let result = run(r#"spec({})
feature("prices", {
    when() { return { priceMap: { price: toDecimal("16.98"), uom: "USD" } } },
    then({ priceMap }) {
        priceMap.uom == "USD"
        priceMap.price == 16.99
    },
})
"#);
    let failure = failure(&result, 0);
    assert_eq!(failure.kind, "assertion");
    assert_eq!(failure.block, "then");
    assert_eq!(failure.location.as_deref(), Some("test/Sample.spec.js:6"));
    assert_eq!(
        failure.message,
        "Condition not satisfied:\n\npriceMap.price == 16.99\n|        |     |\n|        16.98 false\n{\"price\":\"16.98\",\"uom\":\"USD\"}"
    );
}

#[test]
fn groovy_truth_and_void_calls() {
    let result = run(r#"
feature("empty list is false", { expect() { !list }, where: [{ list: [] }] })
feature("void call passes", { expect() { (function () {})() } })
feature("empty list fails", { expect() { [] } })
feature("zero decimal fails", { expect() { toDecimal("0.00") } })
"#);
    assert_eq!(
        outcomes(&result),
        vec![
            ("empty list is false".to_string(), "passed"),
            ("void call passes".to_string(), "passed"),
            ("empty list fails".to_string(), "failed"),
            ("zero decimal fails".to_string(), "failed"),
        ]
    );
}

#[test]
fn assert_checks_a_condition_in_a_loop() {
    let result =
        run("feature('loop', {\n  expect() {\n    for (const n of [1, 2, 3]) assert(n < 3, `n is ${n}`)\n  },\n})\n");
    let failure = failure(&result, 0);
    assert_eq!(failure.kind, "assertion");
    assert_eq!(failure.message, "Assertion failed: n is 3\n\nvalue: false");
    assert_eq!(
        failure.location.as_deref(),
        Some("test/Sample.spec.js:3"),
        "{failure:?}"
    );
}

#[test]
fn an_error_names_the_block_and_the_spec_line() {
    let result = run("feature('throws', {\n  when() {\n    const x = null\n    return { y: x.field }\n  },\n  then({ y }) { y == 1 },\n})\n");
    let failure = failure(&result, 0);
    assert_eq!(failure.kind, "error");
    assert_eq!(failure.block, "when");
    assert_eq!(
        failure.location.as_deref(),
        Some("test/Sample.spec.js:4"),
        "{failure:?}"
    );
    assert!(failure.message.starts_with("TypeError: "), "{}", failure.message);
}

#[test]
fn where_rows_unroll_and_the_filter_picks_features() {
    let source = r#"
feature("row #a plus #b", {
    where: [{ a: 1, b: 2, sum: 3 }, { a: "x", b: 2, sum: 5 }],
    expect: ({ a, b, sum }) => a + b == sum,
})
feature("other", { expect() { true } })
"#;
    let result = run(source);
    assert_eq!(
        outcomes(&result),
        vec![
            ("row 1 plus 2".to_string(), "passed"),
            ("row \"x\" plus 2".to_string(), "failed"),
            ("other".to_string(), "passed"),
        ]
    );
    let filtered = run_with(
        source,
        &RunOptions {
            feature_filter: Some("other".to_string()),
            ..RunOptions::default()
        },
    );
    assert_eq!(outcomes(&filtered), vec![("other".to_string(), "passed")]);
}

#[test]
fn stepwise_skips_after_a_failure_and_ignore_is_reported() {
    let result = run(r#"
spec({ stepwise: true })
feature("a", { ignore: "not yet", expect() { false } })
feature("b", { expect() { 1 == 2 } })
feature("c", { expect() { true } })
"#);
    assert_eq!(
        outcomes(&result),
        vec![
            ("a".to_string(), "ignored"),
            ("b".to_string(), "failed"),
            ("c".to_string(), "skipped")
        ]
    );
}

/// A slow feature or spec is ignored, with the reason, unless the run asks for slow tests; a slow spec's setupSpec does
/// not run either.
#[test]
fn slow_features_and_specs_run_only_with_slow() {
    let source = r#"
feature("quick", { expect() { true } })
feature("long", { slow: true, expect() { 1 == 2 } })
"#;
    let result = run(source);
    assert_eq!(
        outcomes(&result),
        vec![("quick".to_string(), "passed"), ("long".to_string(), "ignored")]
    );
    assert_eq!(result.cases[1].outcome, Outcome::Ignored(SLOW_REASON.to_string()));
    let slow = RunOptions {
        slow: true,
        ..RunOptions::default()
    };
    assert_eq!(
        outcomes(&run_with(source, &slow)),
        vec![("quick".to_string(), "passed"), ("long".to_string(), "failed")]
    );
    let whole = r#"
spec({ slow: true, setupSpec() { throw new Error("setupSpec ran") } })
feature("a", { expect() { 1 == 2 } })
feature("b", { expect() { 1 == 2 } })
"#;
    let result = run(whole);
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert_eq!(
        outcomes(&result),
        vec![("a".to_string(), "ignored"), ("b".to_string(), "ignored")]
    );
    assert!(run_with(whole, &slow).setup_failure.is_some());
}

#[test]
fn message_errors_fail_a_feature_unless_allowed() {
    let result = run(r#"
feature("left an error", { expect() { ec.message.addError("left behind") } })
feature("allowed", { allowMessageErrors: true, expect() { ec.message.addError("fine") } })
feature("next starts clean", { expect() { !ec.message.hasError() } })
feature("service failure throws", { when() { call("no.such.Service.do#Nothing", {}) }, then() { true } })
"#);
    assert_eq!(
        outcomes(&result),
        vec![
            ("left an error".to_string(), "failed"),
            ("allowed".to_string(), "passed"),
            ("next starts clean".to_string(), "passed"),
            ("service failure throws".to_string(), "failed"),
        ]
    );
    assert_eq!(failure(&result, 0).kind, "messages");
    assert_eq!(failure(&result, 0).details, vec!["left behind".to_string()]);
    let thrown = failure(&result, 3);
    assert!(
        thrown
            .message
            .starts_with("Error: service no.such.Service.do#Nothing failed"),
        "{}",
        thrown.message
    );
}

#[test]
fn registration_errors_fail_the_spec() {
    let unrewritten = run("const make = () => ({ then() { 1 == 2 }, when() {} })\nfeature('helper built', make())\n");
    let failure = unrewritten.setup_failure.expect("registration failure");
    assert_eq!(failure.kind, "registration");
    assert!(failure.message.contains("was not rewritten"), "{}", failure.message);
    let misspelled = run("feature('typo', { expcet() { true } })\n");
    assert!(misspelled
        .setup_failure
        .expect("registration failure")
        .message
        .contains("unknown field expcet"));
    let syntax = run("feature('bad', { expect() { 1 == } })\n");
    assert!(syntax.setup_failure.expect("syntax").message.contains("line 1"));
}

#[test]
fn setup_spec_failure_skips_the_features_and_cleanup_still_runs() {
    let result = run(r#"
let cleaned = false
spec({ setupSpec() { throw new Error("no fixture") }, cleanupSpec() { cleaned = true; throw new Error("cleaned") } })
feature("a", { expect() { true } })
"#);
    assert!(result.setup_failure.as_ref().unwrap().message.contains("no fixture"));
    assert_eq!(result.cleanup_failure.as_ref().unwrap().message, "Error: cleaned");
    assert_eq!(outcomes(&result), vec![("a".to_string(), "skipped")]);
}

#[test]
fn temp_sequences_effective_time_and_check_data() {
    let result = run(r#"
const effectiveTime = 1265198400000
spec({
    tempSequences: { start: 55500, banks: { "moqui.basic.Enumeration": 10 } },
    setupSpec() { ec.user.setEffectiveTime(effectiveTime) },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})
feature("pinned", {
    when() {
        const e = ec.entity.makeValue("moqui.basic.Enumeration")
        e.enumTypeId = "T1"; e.description = "one"
        e.setSequencedIdPrimary(); e.create()
        const errs = checkData(`<entity-facade-xml>
            <moqui.basic.Enumeration enumId="55500" enumTypeId="T1" description="one"/></entity-facade-xml>`)
        const bad = checkData(`<entity-facade-xml>
            <moqui.basic.Enumeration enumId="55500" description="two"/></entity-facade-xml>`)
        return { id: e.enumId, errs, bad, now: ec.user.nowTimestamp }
    },
    then({ id, errs, bad, now }) {
        id == "55500"
        errs.length == 0
        bad.length == 1
        now == effectiveTime
    },
})
"#);
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert_eq!(
        outcomes(&result),
        vec![("pinned".to_string(), "passed")],
        "{:?}",
        result.cases
    );
}

#[test]
fn data_check_messages_are_in_a_failure() {
    let result = run(r#"
feature("check", {
    when() { return { errs: checkData(`<entity-facade-xml><moqui.basic.Enumeration enumId="NOPE"/></entity-facade-xml>`) } },
    then({ errs }) { errs.length == 0 },
})
"#);
    let failure = failure(&result, 0);
    assert_eq!(
        failure.details,
        vec!["data check: Entity moqui.basic.Enumeration record not found for primary key {enumId=NOPE}".to_string()]
    );
}

#[test]
fn parallel_runs_a_function_on_threads_with_their_own_contexts() {
    let result = run(r#"
feature("threads", {
    when() {
        return { out: parallel(3, (n, args) => ({ n, base: args.base, user: ec.user.username }), { base: 10 }, { transaction: false }) }
    },
    then({ out }) {
        out.length == 3
        out.map(o => o.n + o.base).join() == "10,11,12"
        out[0].user == null
    },
})
feature("a failing thread fails the feature", {
    expect() { parallel(2, (n) => { if (n === 1) throw new Error("thread one") }, null, { transaction: false }) },
})
"#);
    assert_eq!(
        outcomes(&result),
        vec![
            ("threads".to_string(), "passed"),
            ("a failing thread fails the feature".to_string(), "failed"),
        ],
        "{:?}",
        result.cases
    );
    assert!(
        failure(&result, 1).message.contains("thread 1: Error: thread one"),
        "{}",
        failure(&result, 1).message
    );
}

/// A regex literal, a string or a template with non-ASCII text in a `then` block runs, and a failing one draws its
/// diagram under the right characters.
#[test]
fn unicode_in_conditions_runs_and_draws_its_diagram() {
    let result = run(r#"
feature("unicode passes", {
    when() { return { label: "Today · 3 — é", n: 2 } },
    then({ label, n }) {
        label.length == 13
        /^Today · \d+ — é$/.test(label)
        label.match(/[·•]/u) != null
        `· ${n} ·` == "· 2 ·"
        'é·' + n == "é·2"
    },
})
feature("unicode fails", {
    when() { return { label: "Today · 3" } },
    then({ label }) {
        /^Tomorrow · \d+$/.test(label) || label == "é·"
    },
})
"#);
    assert_eq!(
        outcomes(&result),
        vec![
            ("unicode passes".to_string(), "passed"),
            ("unicode fails".to_string(), "failed")
        ],
        "{:?}",
        result.cases
    );
    let failure = failure(&result, 1);
    assert_eq!(failure.kind, "assertion", "{failure:?}");
    let lines: Vec<&str> = failure.message.lines().collect();
    assert_eq!(lines[2], r#"/^Tomorrow · \d+$/.test(label) || label == "é·""#);
    // The bar under `label ==` is under its `==`, counted in characters, not UTF-8 bytes.
    let bars: Vec<char> = lines[3].chars().collect();
    let column = |needle: &str| lines[2][..lines[2].find(needle).unwrap()].chars().count();
    assert_eq!(bars.get(column("==")), Some(&'|'), "{}", failure.message);
    assert_eq!(bars.get(column("test")), Some(&'|'), "{}", failure.message);
}

#[test]
fn skip_in_a_feature_and_in_setup_spec_reports_skipped() {
    let result = run(r#"
feature("skipped here", { expect() { skip("no fixture user"); false } })
feature("runs", { expect() { true } })
"#);
    assert_eq!(
        outcomes(&result),
        vec![("skipped here".to_string(), "skipped"), ("runs".to_string(), "passed")]
    );
    assert_eq!(result.cases[0].outcome, Outcome::Skipped("no fixture user".to_string()));
    let whole = run(r#"
let cleaned = false
spec({ setupSpec() { skip("the server has no fixtures") }, cleanupSpec() { cleaned = true } })
feature("a", { expect() { false } })
feature("b", { expect() { false } })
"#);
    assert!(whole.setup_failure.is_none(), "{:?}", whole.setup_failure);
    assert!(!whole.failed());
    assert_eq!(
        whole.cases.iter().map(|case| case.outcome.clone()).collect::<Vec<_>>(),
        vec![Outcome::Skipped("the server has no fixtures".to_string()); 2]
    );
}

/// A client host for the tests: `echo` answers its arguments, `problem` records a page problem for the case, and a
/// failed case gets an attachment.
#[derive(Default)]
struct FakeHost {
    calls: Mutex<Vec<String>>,
    problems: Mutex<Vec<String>>,
    specs_closed: Mutex<u32>,
}

const FAKE_PRELUDE: &str = r#"(function (g) {
    const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
    const host = (name, args) => JSON.parse(__moquiTestHost.call(name, JSON.stringify(args)));
    Object.defineProperty(g, "ec", { get() { throw new Error("`ec` is the server's: a client test runs outside it") }, configurable: true });
    define("call", () => { throw new Error("call() is the server's: use http") });
    define("echo", (value) => host("echo", value));
    define("problem", (text) => host("problem", text));
    define("fail", () => host("fail", null));
})(globalThis);"#;

impl SpecHost for FakeHost {
    fn prelude(&self) -> &str {
        FAKE_PRELUDE
    }

    fn call(&self, name: &str, args: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push(format!("{name} {args}"));
        match name {
            "echo" => Ok(format!(
                "{{\"echoed\":{args},\"context\":{}}}",
                ExecutionContext::with_current(|_| ()).is_some()
            )),
            "problem" => {
                self.problems.lock().unwrap().push(args.trim_matches('"').to_string());
                Ok("null".to_string())
            }
            other => Err(format!("{other}: the host refused")),
        }
    }

    fn after_case(&self, _case: &str, failed: bool, _allow_problems: bool) -> CaseExtras {
        CaseExtras {
            problems: std::mem::take(&mut *self.problems.lock().unwrap()),
            attachments: if failed {
                vec!["out/failed-1.png".to_string()]
            } else {
                Vec::new()
            },
            output: String::new(),
        }
    }

    fn after_spec(&self) {
        *self.specs_closed.lock().unwrap() += 1;
    }
}

fn run_client(source: &str) -> (SpecResult, Arc<FakeHost>) {
    crate::script::clear_context_stack();
    assert!(
        ExecutionContext::with_current(|_| ()).is_none(),
        "no context is entered"
    );
    let host = Arc::new(FakeHost::default());
    let spec = SpecFile {
        name: "Client".to_string(),
        display: "test/client/Client.spec.js".to_string(),
        source: source.to_string(),
    };
    let options = RunOptions {
        host: Some(host.clone()),
        ..RunOptions::default()
    };
    let result = run_spec(&spec, &options, &mut |_| {});
    (result, host)
}

#[test]
fn a_client_spec_runs_under_its_host_with_no_execution_context() {
    let (result, host) = run_client(
        r#"
let seen
spec({ setupSpec() { seen = echo({ n: 1 }) } })
feature("host calls answer JSON", {
    when() { return { out: echo(["a", 2]) } },
    then({ out }) {
        out.echoed == ["a", 2]
        out.context == false
        seen.echoed.n == 1
        dec("1.50") == 1.5
        sum([{ v: 1 }, { v: "2.5" }], "v") == 3.5
    },
})
feature("ec names the client equivalent", { expect() { ec.user.username == null } })
feature("call names the client equivalent", { expect() { call("x.y#z", {}) } })
feature("a host error is the script's", { expect() { fail() } })
feature("a page problem fails a passing case", { when() { problem("console error: boom") }, then() { true } })
feature("an allowed page problem passes", { allowPageProblems: true, when() { problem("console error: fine") }, then() { true } })
feature("skip in a client spec", { expect() { skip("not on this server") } })
"#,
    );
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert_eq!(
        outcomes(&result),
        vec![
            ("host calls answer JSON".to_string(), "passed"),
            ("ec names the client equivalent".to_string(), "failed"),
            ("call names the client equivalent".to_string(), "failed"),
            ("a host error is the script's".to_string(), "failed"),
            ("a page problem fails a passing case".to_string(), "failed"),
            ("an allowed page problem passes".to_string(), "passed"),
            ("skip in a client spec".to_string(), "skipped"),
        ],
        "{:?}",
        result.cases
    );
    let ec_failure = failure(&result, 1);
    assert!(
        ec_failure
            .message
            .contains("`ec` is the server's: a client test runs outside it"),
        "{}",
        ec_failure.message
    );
    assert_eq!(ec_failure.location.as_deref(), Some("test/client/Client.spec.js:14"));
    assert!(failure(&result, 2).message.contains("call() is the server's: use http"));
    let refused = failure(&result, 3);
    assert_eq!(refused.message, "Error: fail: the host refused");
    assert_eq!(refused.attachments, vec!["out/failed-1.png".to_string()]);
    let problems = failure(&result, 4);
    assert_eq!(problems.kind, "problems");
    assert_eq!(problems.details, vec!["console error: boom".to_string()]);
    assert_eq!(*host.specs_closed.lock().unwrap(), 1);
    assert!(
        host.calls.lock().unwrap()[0].starts_with("echo {\"n\":1}"),
        "{:?}",
        host.calls
    );
}

#[test]
fn skip_in_a_client_setup_spec_skips_the_spec_and_a_load_failure_still_closes_it() {
    let (result, host) =
        run_client("spec({ setupSpec() { skip('no fixtures') } })\nfeature('a', { expect() { false } })\n");
    assert!(!result.failed());
    assert_eq!(result.cases[0].outcome, Outcome::Skipped("no fixtures".to_string()));
    assert_eq!(*host.specs_closed.lock().unwrap(), 1);
    let (broken, host) = run_client("feature('bad', { expect() { 1 == } })\n");
    assert_eq!(broken.setup_failure.expect("syntax").kind, "registration");
    assert_eq!(*host.specs_closed.lock().unwrap(), 1);
}

#[test]
fn run_on_thread_calls_a_function_in_its_own_engine_with_the_host() {
    let host: Arc<dyn SpecHost> = Arc::new(FakeHost::default());
    let handle = run_on_thread(
        Arc::clone(&host),
        "moqui-test-thread",
        "(user, args) => ({ user, sum: args.a + args.b, echoed: echo(user).echoed, hasEc: (() => { try { return !!ec.user } catch (e) { return e.message } })() })",
        "[3, {\"a\": 1, \"b\": 2}]",
    )
    .unwrap();
    let out: serde_json::Value = serde_json::from_str(&handle.join().unwrap().unwrap()).unwrap();
    assert_eq!(out["user"], 3);
    assert_eq!(out["sum"], 3);
    assert_eq!(out["echoed"], 3);
    assert_eq!(out["hasEc"], "`ec` is the server's: a client test runs outside it");
    let failed = run_on_thread(
        host,
        "moqui-test-thread",
        "() => { throw new Error('in the thread') }",
        "[]",
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(failed.unwrap_err().contains("in the thread"));
}

#[test]
fn assert_methods_pass_and_a_failure_shows_both_values() {
    let passed = run(r#"
feature("methods", {
    expect() {
        assert.equal(1, 1)
        assert.equal(NaN, NaN)
        assert.notEqual(1, 2)
        assert.notEqual(0, -0)
        assert.ok("x")
        assert.deepEqual({ a: [1, { b: "x" }], c: undefined }, { a: [1, { b: "x" }], c: undefined })
        assert.deepEqual(new Date(5), new Date(5))
        assert.deepEqual(/a/i, /a/i)
        const left = {}; left.self = left
        const right = {}; right.self = right
        assert.deepEqual(left, right)
        assert.match("abc", /b/)
        assert.doesNotMatch("abc", /z/)
        assert.throws(() => { throw new Error("nope 12") }, /nope/)
        assert.throws(() => { throw new TypeError("t") }, TypeError)
        assert.rejects(() => Promise.reject(new Error("later")), /later/)
        assert.rejects(async () => { throw new Error("async later") }, /async later/)
    },
})
"#);
    assert!(passed.setup_failure.is_none(), "{:?}", passed.setup_failure);
    assert_eq!(outcomes(&passed), vec![("methods".to_string(), "passed")]);

    let unequal = run("feature('equal', { expect() { assert.equal(1, 2, 'count') } })");
    let unequal_failure = failure(&unequal, 0);
    assert_eq!(unequal_failure.kind, "assertion");
    assert_eq!(
        unequal_failure.message,
        "Assertion failed: assert.equal\n\nactual:   1\nexpected: 2\n\ncount"
    );

    let shallow = run("feature('deep', { expect() { assert.deepEqual({ a: undefined }, {}) } })");
    let shallow_failure = failure(&shallow, 0);
    assert_eq!(shallow_failure.kind, "assertion");
    assert!(
        shallow_failure.message.contains("assert.deepEqual") && shallow_failure.message.contains("undefined"),
        "{}",
        shallow_failure.message
    );

    let pending = run("feature('pending', { expect() { assert.rejects(() => new Promise(() => {})) } })");
    let pending_failure = failure(&pending, 0);
    assert_eq!(pending_failure.kind, "assertion");
    assert!(
        pending_failure.message.contains("still pending"),
        "{}",
        pending_failure.message
    );
}

#[test]
fn run_in_context_calls_back_into_the_sandbox() {
    let result = run(r#"
feature("dialog sandbox", {
    expect() {
        let html = ""
        const context = {
            sri: { write: (text) => { html += text } },
            ec: { resource: { expand: (text) => text } },
            items: new Set(),
            box: { n: 0, add(x) { this.n += x; return this } }
        }
        const source = `(function () { "use strict";
            const w = (text) => sri.write(text);
            w("hi");
            items.add("a");
            box.add(1).add(2);
            const h = { containerDialog(node) { sri.write(node.attrs.id); return h } };
            Object.defineProperty(globalThis, "$mqHtml", { value: h, configurable: true });
            return StringUtilities != null;
        })()`
        const sawHelper = runInContext(source, context)
        context.$mqHtml.containerDialog({ attrs: { id: "D" } })
        assert.equal(html, "hiD")
        assert.equal(context.box.n, 3)
        assert.ok(context.items.has("a"))
        assert.equal(sawHelper, true)
    },
})
"#);
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert_eq!(outcomes(&result), vec![("dialog sandbox".to_string(), "passed")]);
}

#[test]
fn a_database_free_spec_masks_entity_call_and_screen_test() {
    let result = run(r#"
spec({ database: false })
feature("masked", {
    expect() {
        assert.throws(() => ec.entity.find("moqui.test.TestEntity"), /database: false/)
        assert.throws(() => call("no.such.service"), /database: false/)
        assert.throws(() => screenTest("/"), /database: false/)
        let resourceError = ""
        try { ec.resource.getLocationText("component://no-such/nope.txt") }
        catch (e) { resourceError = String(e && e.message ? e.message : e) }
        assert.ok(resourceError)
        assert.doesNotMatch(resourceError, /database: false/)
    },
})
"#);
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert_eq!(outcomes(&result), vec![("masked".to_string(), "passed")], "{result:?}");
}

/// The mask is that spec's. The next spec on this thread sees `ec.entity` again.
#[test]
fn a_database_free_spec_does_not_mask_the_next_spec() {
    let masked = run(r#"
spec({ database: false })
feature("masked", {
    expect() {
        assert.throws(() => ec.entity.find("moqui.test.TestEntity"), /database: false/)
    },
})
"#);
    assert!(masked.setup_failure.is_none(), "{:?}", masked.setup_failure);
    assert_eq!(outcomes(&masked), vec![("masked".to_string(), "passed")], "{masked:?}");
    let result = run(r#"
spec({})
feature("entity is back", {
    expect() {
        assert.equal(typeof ec.entity.find("moqui.test.TestEntity").count(), "number")
    },
})
"#);
    assert!(result.setup_failure.is_none(), "{:?}", result.setup_failure);
    assert_eq!(
        outcomes(&result),
        vec![("entity is back".to_string(), "passed")],
        "{result:?}"
    );
}

/// The screen-template specs read module text through `ec.resource` and never open an entity facade.
#[test]
fn template_specs_run_with_no_entity_facade() {
    let runtime = std::env::temp_dir().join(format!("moqui-template-specs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runtime);
    std::fs::create_dir_all(&runtime).unwrap();
    let framework = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let _lock = crate::user::session::cookie_property_lock();
    let factory = ExecutionContextFactory::boot_with_options(
        &runtime,
        &framework,
        crate::context::BootOptions {
            executor: crate::context::Executor::Inline,
            test_confs: true,
            ..crate::context::BootOptions::default()
        },
    )
    .expect("boot");
    assert!(
        factory.entity().is_none(),
        "the template specs boot with no entity facade"
    );
    let context = factory.new_context();
    let _entered = context.enter();
    for (name, count) in [
        ("ScreenHtml", 13),
        ("ScreenQvt", 16),
        ("DatePeriod", 6),
        ("FieldTitle", 6),
    ] {
        let path = framework.join(format!("test/{name}.spec.js"));
        let source = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"));
        let spec = SpecFile {
            name: name.to_string(),
            display: format!("framework/test/{name}.spec.js"),
            source,
        };
        let result = run_spec(&spec, &RunOptions::default(), &mut |_| {});
        if let Some(failure) = &result.setup_failure {
            panic!("{name} setup: {}\n{}", failure.message, failure.stack);
        }
        assert_eq!(
            result.cases.len(),
            count,
            "{name} registered {}: {:?}",
            result.cases.len(),
            result.cases.iter().map(|case| case.name.as_str()).collect::<Vec<_>>()
        );
        for case in &result.cases {
            if let Outcome::Failed(failure) = &case.outcome {
                panic!(
                    "{name} / {} failed ({})\n{}\n{}",
                    case.name, failure.kind, failure.message, failure.stack
                );
            }
            assert!(
                matches!(case.outcome, Outcome::Passed),
                "{name} / {} is {:?}",
                case.name,
                case.outcome
            );
        }
    }
    drop(_entered);
    drop(context);
    drop(factory);
    drop(_lock);
    let _ = std::fs::remove_dir_all(&runtime);
}
