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

use super::{analyze, verdict, Profile, QUICKJS_ONLY_GLOBALS};
use crate::script::text::hash_text;

fn passes(text: &str) -> bool {
    let analysis = analyze(text, Profile::Expression);
    analysis.refused().is_none()
}

fn refused(text: &str) -> String {
    analyze(text, Profile::Expression)
        .refused()
        .unwrap_or_else(|| panic!("{text} passed"))
        .to_string()
}

#[test]
fn bare_names_operators_literals_members_and_calls_on_values_pass() {
    for text in [
        "orderId == \"2000\"",
        "!unsetBareName",
        "(a && b || c) ?? d",
        "x ? y : z",
        "(price > 10) && statusId === 'OrderPlaced'",
        "typeof Array === \"function\"",
        "item?.statusId",
        "item.amount.compare(other) > 0",
        "list.length > 0 && list[0].name",
        "`${firstName} ${lastName}`",
        "new Date(0).getTime()",
        "[1, 2, ...rest]",
        "({a: 1, b: c, [k]: v})",
        "/ab+c/i.test(name)",
        "items.some(item => item.a > 1)",
        "items.map((x, i) => x + i).join(',')",
        "a, b",
        "\"Y\"",
        "Math.max(1, 2)",
        "-x + +y",
        "void 0",
    ] {
        assert!(
            passes(text),
            "{text}: {:?}",
            analyze(text, Profile::Expression).refused()
        );
    }
}

#[test]
fn writes_functions_and_ways_round_the_check_are_refused() {
    for (text, why) in [
        ("assignedName = 1", "an assignment"),
        ("(assignedName = \"x\") === \"x\"", "an assignment"),
        ("a.b = 1", "an assignment"),
        ("x++", "an update"),
        ("delete a.b", "delete"),
        ("function f() {}", "not one expression"),
        ("(function () { return 1 })()", "a function literal"),
        ("(class {})", "a class literal"),
        ("[x => x]", "an arrow function that is not a call's argument"),
        ("({f: () => 1})", "an arrow function that is not a call's argument"),
        ("items.map(x => { return x })", "an arrow function with a block body"),
        (
            "items.map(x => y => 1)",
            "an arrow function that is not a call's argument",
        ),
        ("this.x", "this"),
        ("eval('1')", "the name eval"),
        ("Function('return 1')()", "the name Function"),
        ("globalThis.x", "the name globalThis"),
        ("compare(a, b)", "the QuickJS global compare"),
        ("context.x", "the QuickJS global context"),
        ("StringUtilities.x", "the QuickJS global StringUtilities"),
        ("atob('eA==')", "the QuickJS global atob"),
        ("a; b", "not one expression"),
        ("", "empty"),
        ("a b", "not JavaScript"),
        ("tag`x`", "a tagged template"),
        ("1n", "a BigInt literal"),
        ("ec", "ec as a value"),
        ("typeof ec", "ec as a value"),
        ("ec[name]", "computed ec[...]"),
        ("ec.user[name]", "computed ec[...]"),
        (
            "ec.service.sync().name(x).call(y => 1)",
            "an arrow function passed to anything but",
        ),
        ("ec.list.forEach(x => x)", "an arrow function passed to anything but"),
    ] {
        let reason = refused(text);
        assert!(reason.starts_with(why), "{text}: {reason}");
    }
}

#[test]
fn ec_paths_are_the_longest_static_chain_and_the_allow_test_decides() {
    let analysis = analyze(
        "ec.user.userId && ec.l10n.localize('Hi') + ec.user.getPreference('a').length",
        Profile::Expression,
    );
    assert_eq!(analysis.refused(), None);
    assert_eq!(
        analysis.ec_paths(),
        ["ec.user.userId", "ec.l10n.localize", "ec.user.getPreference"]
    );
    let text = "ec.user.userId == 'john'";
    let allowed = verdict(text, hash_text(text), Profile::Expression, &|path| {
        path.starts_with("ec.user.")
    });
    assert!(allowed.passes());
    let refused = verdict(text, hash_text(text), Profile::Expression, &|_| false);
    assert!(!refused.passes());
    assert_eq!(
        refused.reason().as_deref(),
        Some("ec.user.userId is not bound on quickjs-oxide")
    );
    // an arrow's own parameter named ec is a local, not the host's
    assert!(passes("list.some(ec => ec.x)"));
    assert!(analyze("list.some(ec => ec.x)", Profile::Expression)
        .ec_paths()
        .is_empty());
}

#[test]
fn the_verdict_is_kept_by_text_and_a_hash_collision_is_parsed_again() {
    let before = super::parses();
    let text = "keptVerdictName > 1";
    let hash = hash_text(text);
    assert!(verdict(text, hash, Profile::Expression, &|_| false).passes());
    assert!(verdict(text, hash, Profile::Expression, &|_| false).passes());
    // the same hash with another text is that text's own verdict
    assert!(!verdict("kept = 1", hash, Profile::Expression, &|_| false).passes());
    assert!(super::parses() >= before + 2);
}

/// A method that changes an object in place is refused whatever its receiver: the oxide copy would not be written back.
#[test]
fn mutating_methods_are_refused_on_any_receiver() {
    for (text, method) in [
        ("items.sort((a, b) => a - b)[0]", "sort"),
        ("items.push(1)", "push"),
        ("items.pop()", "pop"),
        ("items.shift()", "shift"),
        ("items.unshift(0)", "unshift"),
        ("items.splice(0, 1)", "splice"),
        ("items.reverse()", "reverse"),
        ("items.fill(0)", "fill"),
        ("items.copyWithin(0, 1)", "copyWithin"),
        ("Object.assign(map, {a: 1})", "assign"),
        ("Object.defineProperty(map, 'a', {value: 1})", "defineProperty"),
        ("Object.setPrototypeOf(map, null)", "setPrototypeOf"),
        ("Object.freeze(map)", "freeze"),
        ("items['sort']()", "sort"),
        ("items?.sort()", "sort"),
    ] {
        assert_eq!(refused(text), format!("the mutating method {method}"), "{text}");
    }
    // a decimal's add, and the add of a Set the expression made itself, are reads of the stack
    assert!(passes("price.add(1).eq(total)"));
    assert!(passes("new Set(items).has(x)"));
}

/// The ways round the check the review found, each refused.
#[test]
fn ways_round_the_check_are_refused() {
    for (text, why) in [
        (
            "''.constructor.constructor('return globalThis')()",
            "the property constructor",
        ),
        ("[].map['constructor']('return 1')()", "the property constructor"),
        ("({}).__proto__", "the property __proto__"),
        ("Array.prototype.x", "the property prototype"),
        (
            "Object.prototype.__defineGetter__('x', () => 1)",
            "the property __defineGetter__",
        ),
        ("({}).__defineGetter__('x', () => 1)", "the property __defineGetter__"),
        ("({}).__lookupGetter__('x')", "the property __lookupGetter__"),
        ("Promise.resolve(1)", "the name Promise"),
        ("Promise.resolve().then(() => 1)", "the property then"),
        ("p.then(() => 1)", "the property then"),
        ("Reflect.get(o, 'x')", "the name Reflect"),
        ("new Proxy({}, {})", "the name Proxy"),
        ("Symbol.iterator", "the name Symbol"),
        ("new WeakRef(o)", "the name WeakRef"),
        ("new FinalizationRegistry(x => 1)", "the name FinalizationRegistry"),
        ("items.call(x => 1)", "an arrow function passed to anything but"),
        ("setTimeout(() => 1)", "an arrow function passed to anything but"),
        (
            "new Array(1).map(x => 1).map(x => y => 2)",
            "an arrow function that is not a call's argument",
        ),
        ("ec.user.userAccount[k]", "computed ec[...]"),
        ("ec.user.getPreference('a')[k]", "computed ec[...]"),
    ] {
        let reason = refused(text);
        assert!(reason.starts_with(why), "{text}: {reason}");
    }
    for text in [
        "items.map(x => x.a).filter(a => a > 1).length",
        "items.find(x => x.id === id)",
        "items.reduce((sum, x) => sum + x.qty, 0)",
        "items.forEach(x => x)",
        "items?.some(x => x)",
    ] {
        assert!(
            passes(text),
            "{text}: {:?}",
            analyze(text, Profile::Expression).refused()
        );
    }
}

#[test]
fn the_later_profiles_are_stubs_that_refuse() {
    assert!(analyze("1", Profile::Actions).refused().is_some());
    assert!(analyze("1", Profile::Full).refused().is_some());
}

/// The names the check refuses as QuickJS-only are exactly what QuickJS's global object has and the oxide runtime's
/// does not (the host's globals and QuickJS's own extras), plus the names the bare-name object answers itself.
#[test]
fn the_quickjs_only_globals_are_what_quickjs_has_and_oxide_does_not() {
    let quickjs: String = crate::script::with_ctx(|ctx| {
        ctx.eval::<String, _>("Object.getOwnPropertyNames(globalThis).join(',')")
            .map_err(crate::script::js_error)
    })
    .unwrap();
    let runtime = quickjs_oxide::engine::api::core::Runtime::new();
    let context = runtime.context();
    let names = context
        .eval("Object.getOwnPropertyNames(globalThis).join(',')")
        .unwrap();
    let oxide = context.to_js_string(&names).unwrap().to_utf8_lossy();
    let oxide: std::collections::BTreeSet<&str> = oxide.split(',').collect();
    let only: std::collections::BTreeSet<&str> = quickjs
        .split(',')
        .filter(|name| !oxide.contains(name) && *name != "ec" && !super::OXIDE_HOST_GLOBALS.contains(name))
        .collect();
    let listed: std::collections::BTreeSet<&str> = QUICKJS_ONLY_GLOBALS
        .iter()
        .copied()
        .filter(|name| {
            !matches!(
                *name,
                "ObjectUtilities"
                    | "CollectionUtilities"
                    | "StringUtilities"
                    | "WebUtilities"
                    | "UserSecurityChecks"
                    | "context"
            )
        })
        .collect();
    assert_eq!(only, listed, "QuickJS-only globals");
}
