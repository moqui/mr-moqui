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

//! A host class for the binders' tests, and the scripts both engines must answer alike ([`EXPECTED`]).

use std::sync::Mutex;

use moqui_lib::object::{HostObject, Value};

use super::{method, Call, HostClass, HostError, Method, Path, Ret};
use crate::context::{Error, ErrorKind};

pub(crate) static TEST: HostClass = HostClass {
    name: "host:Test",
    methods: &[
        method!("ec.test", "echo", [Value], None, leaf, echo),
        method!("ec.test", "nested", [Value], None, nested),
        method!("ec.test", "join", [Text, OptText], None, leaf, join),
        method!("ec.test", "half", [Int], None, leaf, half),
        method!("ec.test", "flag", [OptBool], None, leaf, flag),
        method!("ec.test", "fail", [], None, leaf, fail),
        method!("ec.test", "denied", [], None, leaf, denied),
        method!("ec.test", "boom", [], None, leaf, boom),
        method!("ec.test", "boomLent", [], None, boom),
        method!("ec.test", "twice", [Callback], None, leaf, twice),
        method!("ec.test", "builder", [], None, leaf, builder),
        method!("ec.test", "log", [Text, Thrown], None, leaf, log),
    ],
    getters: &[method!("ec.test", "answer", [], None, leaf, answer)],
    hidden: &[method!("ec.test", "secret", [], None, leaf, answer)],
    shim_js: Some("(function (proto, hidden) { proto.shimmed = function () { return 'shim ' + hidden.secret(); }; })"),
    shim_members: &["shimmed"],
};

static BUILDER: HostClass = HostClass {
    name: "host:TestBuilder",
    methods: &[
        method!("ec.test.builder()", "add", [Text], None, leaf, add),
        method!("ec.test.builder()", "done", [], None, leaf, done),
        method!("ec.test.builder()", "hold", [Callback], None, leaf, hold),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

static TEST_FUNCTION: Method = method!("ec", "testJoin", [Text, OptText], None, leaf, join);

pub(crate) static TEST_PATHS: &[Path] = &[
    Path::object("ec.test", &TEST),
    Path::function("ec.testJoin", &TEST_FUNCTION),
];

/// Each script and what it gives on both engines, as text.
pub(crate) const EXPECTED: &[(&str, &str)] = &[
    ("ec.test.echo({a: [1, 'b', null], c: true}).a[1]", "b"),
    ("ec.test.join('a', 'b') + ec.test.join('c')", "abc"),
    ("ec.test.nested('lent') + ec.test.nested.length", "lent1"),
    ("ec.testJoin('x', 'y') + ec.testJoin.length", "xy1"),
    ("ec.test.join.name + ec.test.join.length + ec.test.echo.length + ec.test.flag.length", "join110"),
    ("ec.test.half(7.9)", "3"),
    ("String(ec.test.flag()) + ec.test.flag(true)", "nulltrue"),
    ("ec.test.answer + 1", "43"),
    ("ec.test.shimmed()", "shim 42"),
    ("typeof ec.test.secret", "undefined"),
    ("ec.test.builder().add('a').add('b').done()", "a,b"),
    ("ec.test.twice((x) => x * 10)", "100"),
    (
        "(() => { const b = ec.test.builder(); try { b.hold(() => b.add('x')) } catch (e) { return e.message } })()",
        "re-entrant call on test builder",
    ),
    ("ec.test.log('m', new TypeError('t')) + '|' + ec.test.log('m', 'x') + '|' + ec.test.log('m')", "m: TypeError: t|m: x|m"),
    (
        "(() => { try { ec.test.fail() } catch (e) { return e.name + ': ' + e.message + ' ' + (e instanceof Error) } })()",
        "Error: it failed true",
    ),
    (
        "(() => { try { ec.test.denied() } catch (e) { return e.message + ' ' + e.moquiErrorKind + ' ' + e.moquiRetryAfter } })()",
        "not yours Unauthorized 30",
    ),
    ("(() => { try { ec.test.boom() } catch (e) { return e.name + ': ' + e.message } })()", "Error: boom"),
    // in a body the engine is lent to on oxide (not a leaf): caught there too, not poisoning the runtime
    ("(() => { try { ec.test.boomLent() } catch (e) { return e.name + ': ' + e.message } })()", "Error: boom"),
    (
        "(() => { try { ec.test.join() } catch (e) { return e.name + ': ' + e.message } })()",
        "TypeError: Error calling function with 0 argument(s) while 1 where expected",
    ),
    (
        "(() => { try { ec.test.join(1) } catch (e) { return e.name + ': ' + e.message } })()",
        "TypeError: Error converting from js 'int' into type 'string'",
    ),
    (
        "(() => { try { ec.test.flag('x') } catch (e) { return e.name + ': ' + e.message } })()",
        "TypeError: Error converting from js 'string' into type 'bool'",
    ),
    (
        "(() => { try { ec.test.twice(() => { throw new RangeError('inner') }) } catch (e) { return e.name + ': ' + e.message } })()",
        "RangeError: inner",
    ),
    (
        "(() => { try { ec.test.builder().done.call({}) } catch (e) { return e.name } })()",
        "TypeError",
    ),
];

fn echo(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(call.value_or_null(0)))
}

/// A body that may run a nested script: on oxide it runs with the engine lent.
fn nested(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(call.value_or_null(0)))
}

fn join(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(format!(
        "{}{}",
        call.text(0).unwrap_or_default(),
        call.text(1).unwrap_or_default()
    )
    .into())
}

fn half(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::I32(call.int(0).unwrap_or_default() / 2)))
}

fn flag(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(call.bool(0).map_or(Value::Null, Value::Bool)))
}

fn fail(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Err(HostError::message("it failed"))
}

fn denied(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let err = Error::new("not yours")
        .with_kind(ErrorKind::Unauthorized)
        .with_retry_after(30);
    Err(HostError::Context(err))
}

fn boom(_: &mut Call<'_>) -> Result<Ret, HostError> {
    panic!("boom")
}

fn twice(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let once = call.ops.call_back(0, vec![Value::I32(1)])?;
    Ok(Ret::Value(call.ops.call_back(0, vec![once])?))
}

fn builder(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Object(HostObject::new(Mutex::new(Vec::<String>::new())), &BUILDER))
}

fn add(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let parts = call.this::<Mutex<Vec<String>>>()?;
    super::lock_state(&parts, "test builder")?.push(call.text(0).unwrap_or_default().to_string());
    Ok(Ret::This)
}

fn done(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let parts = call.this::<Mutex<Vec<String>>>()?;
    let joined = super::lock_state(&parts, "test builder")?.join(",");
    Ok(joined.into())
}

/// Calls back while it holds the state, which the callback reaches again: refused, not a deadlock.
fn hold(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let parts = call.this::<Mutex<Vec<String>>>()?;
    let guard = super::lock_state(&parts, "test builder")?;
    call.ops.call_back(0, Vec::new())?;
    drop(guard);
    Ok(Ret::Undefined)
}

fn log(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let message = call.text(0).unwrap_or_default().to_string();
    Ok(match call.arg(1) {
        super::Arg::Text(thrown) => format!("{message}: {thrown}"),
        _ => message,
    }
    .into())
}

fn answer(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(Value::I32(42)))
}
