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
        ("UserSecurityChecks.x", "the QuickJS global UserSecurityChecks"),
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
    // a parameter may not shadow ec
    assert_eq!(super::tests::refused("list.some(ec => ec.x)"), "a declaration of ec");
}

/// `let` and `const` are the block's; a declaration may not shadow `ec`, a refused name or a QuickJS global; a method
/// called by a key the check cannot read, and a Set's or Map's or typed array's `set`, `add`, `delete` and `clear`, write.
#[test]
fn block_scopes_shadowing_and_more_writes() {
    for (text, why) in [
        (
            "(function() { { let ec = 0 } ec.service.sync() })()",
            "a declaration of ec",
        ),
        ("(function() { { let eval = 0 } eval('1') })()", "a declaration of eval"),
        ("(function() { let ec = 0; })()", "a declaration of ec"),
        ("(function() { var eval = 1; })()", "a declaration of eval"),
        (
            "(function() { for (let sri = 0; ; ) {} })()",
            "a declaration of sri",
        ),
        ("(function() { { const sri = 1 } })()", "a declaration of sri"),
    ] {
        let analysis = analyze(text, Profile::Actions);
        let reason = analysis.refused().map(str::to_string).unwrap_or_else(|| {
            // an unbound ec path is the allow test's: the walk records it
            analysis.ec_paths().join(",")
        });
        assert!(reason.starts_with(why), "{text}: {reason}");
    }
    // a block's own let is a local inside it
    assert_eq!(
        analyze("(function() { { let total = 1; x = total; } })()", Profile::Actions).refused(),
        None
    );
    for text in ["list[op](x)", "seen.add(1)", "bytes.set(other)", "cache.delete(k)"] {
        let analysis = analyze(text, Profile::Expression);
        assert_eq!(analysis.refused(), None, "{text}");
        assert!(analysis.writes(), "{text}");
    }
    assert!(!analyze("list[0].x", Profile::Expression).writes());
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

/// A method that changes an object in place marks the text as writing (the oxide run writes its tracked values back at
/// its end), and is refused on a value reached from `ec`, which the oxide engine does not track.
#[test]
fn mutating_methods_mark_the_text_as_writing() {
    for text in [
        "items.sort((a, b) => a - b)[0]",
        "items.push(1)",
        "items.pop()",
        "items.shift()",
        "items.unshift(0)",
        "items.splice(0, 1)",
        "items.reverse()",
        "items.fill(0)",
        "items.copyWithin(0, 1)",
        "Object.assign(map, {a: 1})",
        "Object.defineProperty(map, 'a', {value: 1})",
        "Object.freeze(map)",
        "items['sort']()",
        "items?.sort()",
        "order.items.slice().sort()",
    ] {
        let analysis = analyze(text, Profile::Expression);
        assert_eq!(analysis.refused(), None, "{text}");
        assert!(analysis.writes(), "{text}");
    }
    for text in [
        "items.length",
        "price.multiply(2).eq(total)",
        "new Set(items).has(x)",
        "items.map(x => x.a)",
    ] {
        let analysis = analyze(text, Profile::Expression);
        assert_eq!(analysis.refused(), None, "{text}");
        assert!(!analysis.writes(), "{text}");
    }
    assert_eq!(
        refused("ec.user.getPreferences().sort()"),
        "the mutating method sort on a value reached from ec"
    );
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

/// The Actions profile: a generated `(function() { ... })()` with set, if and iterate passes and writes; what it does
/// not know is refused.
#[test]
fn the_actions_profile_takes_generated_set_if_and_iterate() {
    let generated = crate::script::generated_source(
        r#"<actions><set field="x" from="a + 1"/><if condition="x &gt; 1"><set field="y" from="[x]"/></if>
            <iterate list="items" entry="item"><set field="seen" from="item.name"/></iterate></actions>"#,
        false,
    )
    .unwrap();
    let analysis = analyze(&generated, Profile::Actions);
    assert_eq!(analysis.refused(), None, "{generated}");
    assert!(analysis.writes());
    // an entry is read from the list by a key the check cannot read, which may name a prototype's function: an
    // assignment to its member goes to QuickJS
    let entry = crate::script::generated_source(
        r#"<actions><iterate list="items" entry="item"><set field="item.seen" from="true"/></iterate></actions>"#,
        false,
    )
    .unwrap();
    let reason = analyze(&entry, Profile::Actions).refused().unwrap_or_default().to_string();
    assert!(reason.starts_with("an assignment to a member of item"), "{reason}");
    for (text, why) in [
        ("(function() { if (a) { function f() {} } })()", "a function declared in a block"),
        ("(function() { function f() {} x = f; })()", "the function f used as a value"),
        ("(function() { function f() {} list.map(f) })()", "the function f used as a value"),
        ("(function() { function f() {} return f })()", "the function f used as a value"),
        ("(function() { function* g() {} })()", "an async or generator function"),
        ("(function() { function f(...rest) {} })()", "a rest parameter"),
        ("(function() { function f() {} var f = 1 })()", "a second declaration of the function f"),
        ("(function() { for (const ec of list) x = 1; })()", "a declaration of ec"),
        ("(function() { try {} catch (CollectionUtilities) {} })()", "a declaration of CollectionUtilities"),
        ("(function() { for (const {constructor} of list) x = 1; })()", "the property constructor"),
        ("(function() { a: for (;;) { break a } })()", "a statement outside the actions profile"),
        ("(function() { switch (a) {} })()", "a statement outside the actions profile"),
        (
            "(function() { ec.user.x = 1; })()",
            "an assignment to a value reached from ec",
        ),
        ("(function() { ({a} = b); })()", "a destructuring assignment"),
        ("(function(p) { x = p; })(1)", "an actions function with arguments"),
        ("x = 1", "not one actions function"),
        ("(function() { sri.x = 1; })()", "the QuickJS global sri"),
    ] {
        let reason = analyze(text, Profile::Actions)
            .refused()
            .unwrap_or_default()
            .to_string();
        assert!(reason.starts_with(why), "{text}: {reason}");
    }
    // the function's own variables are not context names, wherever declared
    assert_eq!(
        analyze(
            "(function() { if (a) { var total = 1; } x = total; })()",
            Profile::Actions
        )
        .refused(),
        None
    );
}

#[test]
fn the_later_profiles_are_stubs_that_refuse() {
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
            )
        })
        .collect();
    assert_eq!(only, listed, "QuickJS-only globals");
}

/// The context names a text reads, for the router's look at their values: not its own variables, not a name it only
/// assigns, and `ec` never.
#[test]
fn the_names_a_text_reads_are_noted() {
    let analysis = analyze(
        "(function() { var own = a; x = own + b.c; y += 1; if (ec.user.userId) z = b; })()",
        Profile::Actions,
    );
    assert_eq!(analysis.refused(), None);
    assert_eq!(analysis.reads(), ["a", "b", "y"]);
    let analysis = analyze("row.amount + toNumber(limit)", Profile::Expression);
    assert_eq!(analysis.reads(), ["row", "toNumber", "limit"]);
}

/// What reads the clock or a random number, which the difference check does not run twice: by the syntax, not by
/// words in the text (a name such as `knownDate` or a string 'random' moves nothing).
#[test]
fn reads_of_the_clock_and_random_numbers_are_known() {
    for text in [
        "Date.now() - start",
        "new Date(stamp).getTime()",
        "Math.random() > 0.5",
        "ec.user.nowTimestamp",
        "ec.l10n.format(ec.user.nowTimestamp, 'yyyy')",
        "ec.user.randomString(8)",
    ] {
        assert!(analyze(text, Profile::Expression).moving(), "{text}");
    }
    for text in ["knownDate + 1", "'random now'", "item.nowhere", "ec.user.userId"] {
        assert!(!analyze(text, Profile::Expression).moving(), "{text}");
    }
}

/// The wider Actions profile: `for...of` and `for...in` over values, `try` with `catch` and `finally`, functions that
/// are only called, block-bodied arrows passed to an array method, destructuring declarations; their names are the
/// script's own, in their scopes, and a context name read inside is still noted.
#[test]
fn the_actions_profile_takes_loops_over_values_try_and_functions_that_stay() {
    let text = "(function() {
        function label(row) { const {name, qty = 0} = row; return name + ':' + qty }
        for (const row of rows) { out.push(label(row)) }
        for (const key in totals) { if (!key) continue; seen = key }
        for (k of list) { if (k > 2) break }
        do { n = n - 1 } while (n > 0)
        try { risky = ec.user.userId } catch (e) { failure = String(e) } finally { done = true }
        const sums = rows.map((row) => { let total = 0; for (const [, v] of Object.entries(row)) total += v; return total })
    })()";
    let analysis = analyze(text, Profile::Actions);
    assert_eq!(analysis.refused(), None, "{text}");
    assert!(analysis.writes());
    // the loop's, the function's, the catch's and the arrow's names are local; the context names are read
    let reads: Vec<&str> = analysis.reads().iter().map(String::as_str).collect();
    assert_eq!(reads, ["rows", "out", "totals", "list", "k", "n", "String", "Object"]);
    // a `let` of a loop or a block is not seen after it
    let after = analyze(
        "(function() { for (const k of list) {} x = k; try { let t = 1 } catch (e) {} y = t; z = e })()",
        Profile::Actions,
    );
    let reads: Vec<&str> = after.reads().iter().map(String::as_str).collect();
    assert_eq!(reads, ["list", "k", "t", "e"]);
    // the expression profile is unchanged: no block-bodied arrows
    assert!(analyze("rows.map((row) => { return row })", Profile::Expression)
        .refused()
        .is_some());
}

/// `context` (the stack) on oxide: `context.name` is a read of `name`, and the stack handed to a host call reads no
/// value; any other use may read any name, so the router looks at every visible value.
#[test]
fn reads_through_context_are_noted() {
    let analysis = analyze(
        "(function() { x = context.base; ec.service.sync().parameters(context).call(); context.putAll(m) })()",
        Profile::Actions,
    );
    assert_eq!(analysis.refused(), None);
    assert!(!analysis.context_values());
    assert!(analysis.reads().iter().any(|name| name == "base"));
    for text in [
        "(function() { x = context[name] })()",
        "(function() { x = Object.keys(context) })()",
        "(function() { x = context.get('a') })()",
        "(function() { other(context) })()",
    ] {
        let analysis = analyze(text, Profile::Actions);
        assert_eq!(analysis.refused(), None, "{text}");
        assert!(analysis.context_values(), "{text}");
    }
}

/// Two generated shapes the Actions profile takes: XmlActions' copy of a service's results (exactly
/// `Object.prototype.hasOwnProperty.call(globalThis, name)` and `globalThis[name] = value`), and a `const` arrow that
/// is only called. `prototype` and `globalThis` stay refused anywhere else, and such an arrow as a value too.
#[test]
fn generated_result_copies_and_const_arrows_pass() {
    let copy = crate::script::generated_source(
        r#"<actions><service-call name="a.b.c#D" in-map="{x: x}" out-map="context"/></actions>"#,
        false,
    )
    .unwrap();
    assert!(copy.contains("hasOwnProperty.call(globalThis"), "{copy}");
    let analysis = analyze(&copy, Profile::Actions);
    assert_eq!(analysis.refused(), None, "{copy}");
    let arrows = "(function() { const L = (text) => ec.l10n.localize(text); const plain = (v) => v == null || v === '';
        ok = fields.some((field) => plain(row[field])) ? L('no') : L('yes') })()";
    assert_eq!(analyze(arrows, Profile::Actions).refused(), None);
    for (text, why) in [
        ("(function() { x = Object.prototype.toString.call(globalThis) })()", "the property prototype"),
        ("(function() { x = Object.prototype.hasOwnProperty.call(other, k) })()", "the property prototype"),
        ("(function() { x = globalThis[k] })()", "the name globalThis"),
        ("(function() { globalThis.x = 1 })()", "the name globalThis"),
        ("(function() { const L = (t) => t; keep = L })()", "the function L used as a value"),
        ("(function() { const L = (t) => t; list.map(L) })()", "the function L used as a value"),
        // an arrow named before the walk reaches it is still a function, so it does not escape the run
        ("(function() { function g() { return f } const f = () => 1; keep = g() })()", "the function f used as a value"),
        ("(function() { if (a) { const f = () => 1; const g = () => f; keep = g() } })()", "the function f used as a value"),
        // a member of a builtin would stay on the thread's runtime of one engine
        ("(function() { Math.keep = 1 })()", "an assignment to a member of the global Math"),
        ("(function() { JSON.parse.x = 1 })()", "an assignment to a member of the global JSON"),
        ("(function() { Object['x'] = 1 })()", "an assignment to a member of the global Object"),
        // `globalThis[k] =` only as XmlActions' guarded copy
        ("(function() { for (const k in row) globalThis[k] = row[k] })()", "the name globalThis"),
        (
            "(function() { if (!Object.prototype.hasOwnProperty.call(globalThis, k)) globalThis[j] = 1 })()",
            "the name globalThis",
        ),
        (
            "(function() { if (!Object.prototype.hasOwnProperty.call(globalThis, k)) globalThis[k] = 1; else x = 1 })()",
            "the name globalThis",
        ),
    ] {
        let reason = analyze(text, Profile::Actions).refused().unwrap_or_default().to_string();
        assert!(reason.starts_with(why), "{text}: {reason}");
    }
}

/// `context.put`, `remove` and `putAll` write the stack: the text writes, as a mutating method makes it.
#[test]
fn the_context_methods_that_write_mark_the_text_as_writing() {
    for text in ["context.putAll({a: 1})", "context.put('a', 1)", "context.remove('a')"] {
        let analysis = analyze(text, Profile::Expression);
        assert_eq!(analysis.refused(), None, "{text}");
        assert!(analysis.writes(), "{text}");
    }
    assert!(!analyze("context.containsKey('a') && context.keySet().length > 0", Profile::Expression).writes());
}

/// The builtins whose members an assignment may not change are the global objects and functions of both engines: the
/// oxide runtime's, and QuickJS's but for the names refused as QuickJS's own.
#[test]
fn the_builtin_globals_are_both_engines() {
    const OBJECTS: &str = "Object.getOwnPropertyNames(globalThis).filter((name) => { const value = globalThis[name]; \
         return value !== globalThis \
         && (typeof value === 'object' && value !== null || typeof value === 'function') }).join(',')";
    let runtime = quickjs_oxide::engine::api::core::Runtime::new();
    let context = runtime.context();
    let names = context.eval(OBJECTS).unwrap();
    let names = context.to_js_string(&names).unwrap().to_utf8_lossy();
    let oxide: std::collections::BTreeSet<&str> = names.split(',').collect();
    let listed: std::collections::BTreeSet<&str> = super::BUILTIN_GLOBALS
        .iter()
        .copied()
        .filter(|name| !super::OXIDE_HOST_GLOBALS.contains(name))
        .collect();
    assert_eq!(oxide, listed, "the oxide runtime's builtin globals");
    let quickjs: String =
        crate::script::with_ctx(|ctx| ctx.eval::<String, _>(OBJECTS).map_err(crate::script::js_error)).unwrap();
    let quickjs: std::collections::BTreeSet<&str> = quickjs
        .split(',')
        .filter(|name| *name != "ec" && !QUICKJS_ONLY_GLOBALS.contains(name))
        .collect();
    let listed: std::collections::BTreeSet<&str> = super::BUILTIN_GLOBALS.iter().copied().collect();
    assert_eq!(quickjs, listed, "QuickJS's builtin globals");
}

/// A builtin, or a function or object a member read may give (a prototype's method), is shared by every later text on
/// the thread's runtime of one engine: it is not bound to a name or kept as a value, and nothing changes it in place.
/// An object the run itself holds may be changed.
#[test]
fn nothing_changes_an_object_that_outlives_the_run() {
    for (text, profile, why) in [
        ("(function() { const m = Math; m.x = 1 })()", Profile::Actions, "the global Math as a value"),
        ("(function() { const {parse} = JSON; parse.x = 1 })()", Profile::Actions, "the global JSON as a value"),
        ("(function() { [].map.x = 1 })()", Profile::Actions, "an assignment to a member of a value the text may not"),
        (
            "(function() { (1).toFixed.x = 1 })()",
            Profile::Actions,
            "an assignment to a member of a value the text may not",
        ),
        ("(function() { (0, Math).x = 1 })()", Profile::Actions, "an assignment to a member of the global Math"),
        (
            "(function() { [Math][0].x = 1 })()",
            Profile::Actions,
            "an assignment to a member of a value the text may not",
        ),
        ("(function() { const f = row.toString; f.x = 1 })()", Profile::Actions, "an assignment to a member of f,"),
        (
            "(function() { for (const f of [row.toString]) f.x = 1 })()",
            Profile::Actions,
            "an assignment to a member of f,",
        ),
        ("(function() { rows.forEach((r) => { r.x = 1 }) })()", Profile::Actions, "an assignment to a member of r,"),
        ("(function() { var o = {}; o = row.toString; o.x++ })()", Profile::Actions, "an assignment to a member of o,"),
        ("(function() { var Math = {} })()", Profile::Actions, "a declaration of Math"),
        ("Object.assign(Math, {x: 1})", Profile::Expression, "Object.assign on the global Math"),
        ("Object['assign'](JSON, {x: 1})", Profile::Expression, "Object.assign on the global JSON"),
        (
            "Object.defineProperty(JSON, 'x', {value: 1})",
            Profile::Expression,
            "Object.defineProperty on the global JSON",
        ),
        ("Object.setPrototypeOf(row.toString, null)", Profile::Expression, "Object.setPrototypeOf on a value"),
        ("Object.assign(context, {x: 1})", Profile::Expression, "Object.assign on the context stack"),
        ("[].push.call(row.toString, 1)", Profile::Expression, "a call with this as a value the text may not"),
        ("[].push.bind(Math)", Profile::Expression, "a call with this as the global Math"),
        ("({f: Object.assign}).f(Math, {})", Profile::Expression, "a member of the global Object as a value"),
        ("rows.map(Object.assign)", Profile::Expression, "a member of the global Object as a value"),
        ("Object.getPrototypeOf(row)", Profile::Expression, "the property getPrototypeOf"),
        // `||=` may keep the old value, which must be the run's own too
        ("(function() { ([].map ||= {}).x = 1 })()", Profile::Actions, "an assignment to a member of a value"),
        (
            "(function() { var f = row.toString; var o = (f ||= {}); o.x = 1 })()",
            Profile::Actions,
            "an assignment to a member of o,",
        ),
        // the receiver of a method that changes it, which a computed key may make a prototype (`__proto__`)
        ("[][k].push(7)", Profile::Expression, "the method push on a value the text may not"),
        ("(function() { var k = '__proto__'; [][k].sort() })()", Profile::Actions, "the method sort on a value"),
        ("[][k][j](1)", Profile::Expression, "the method named by a computed key on a value"),
        // a member read may be a prototype's object: a mutator on one goes to QuickJS
        ("order.items.sort()", Profile::Expression, "the method sort on a value the text may not"),
    ] {
        let reason = analyze(text, profile).refused().unwrap_or_default().to_string();
        assert!(reason.starts_with(why), "{text}: {reason}");
    }
    for (text, profile) in [
        ("Math.max(a, b) + Math.PI + Number.MAX_SAFE_INTEGER", Profile::Expression),
        ("x instanceof Date && typeof JSON === 'object'", Profile::Expression),
        ("rows.map(String).filter(Boolean)", Profile::Expression),
        ("Object.assign({}, a, b)", Profile::Expression),
        ("Object.assign(row, {x: 1})", Profile::Expression),
        ("statusLogs.slice().sort((a, b) => a - b).concat(Object.keys(row)).reverse()", Profile::Expression),
        // a context name the text does not bind is the run's copy: a function cannot be put there (oxide's tests)
        (
            "(function() { context.f = ''.toString; f.x = 1; o ||= {}; o.y = 1; total = total.add(1) })()",
            Profile::Actions,
        ),
        (
            "(function() { var o = {}; o.x = 1; product = ec.entity.makeValue('X'); product.name = 'a'; \
             order.total = 1; context.seen = true; let n = 0; n++; t = n > 1 ? [] : {}; t.a = 1 })()",
            Profile::Actions,
        ),
    ] {
        let analysis = analyze(text, profile);
        assert_eq!(analysis.refused(), None, "{text}");
    }
}
