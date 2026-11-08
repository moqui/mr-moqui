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

//! The static check of a script's text for quickjs-oxide (plans/Script-Engine-Oxide.md section 3): whether the text
//! uses only what the oxide bindings provide, made once per text with `oxc_parser` and kept for the process by the
//! text's hash. A text that fails runs on QuickJS.
//!
//! The check is an allow-list over the syntax tree: what it does not know is refused. The [`Profile::Expression`]
//! profile (Phase 1: conditions and values) allows bare names, literals, the operators, member access, calls on values,
//! `new` of a name, arrow functions with an expression body passed directly to one of the array methods that keep
//! nothing ([`ARROW_METHODS`]), and the `ec` paths the allow test accepts. It refuses:
//!
//! - writes to names and fields: any assignment, update or `delete`; and the methods that change an object in place
//!   ([`MUTATING_METHODS`]) on a value reached from `ec`, which the oxide engine does not track. On any other value
//!   such a method marks the text as writing ([`Verdict::writes`]): the oxide run tracks what it reads by name and
//!   writes it back at its end;
//! - functions that could outlive the run or leave the engine: function and class literals, block-bodied arrows, an
//!   arrow anywhere but as an argument of an [`ARROW_METHODS`] call, `Promise`, `then`, `WeakRef`,
//!   `FinalizationRegistry`, the legacy accessor definers;
//! - ways round the check: `Function`, `eval`, `constructor`, `prototype`, `__proto__`, `Reflect`, `Proxy`, `Symbol`,
//!   `globalThis`, `this`, `arguments`, `with`, computed `ec[...]` (or computed members of any value reached from
//!   `ec`), `ec` itself as a value (an alias would reach paths the check cannot see), every statement but the one
//!   expression ([`REFUSED_NAMES`], [`REFUSED_PROPERTIES`]);
//! - the names QuickJS has on its global object that quickjs-oxide does not ([`QUICKJS_ONLY_GLOBALS`]);
//! - a change to an object that outlives the run on the thread's runtime of one engine: a builtin global or a value
//!   reached from one ([`BUILTIN_GLOBALS`]) bound to a name, a declaration that shadows a builtin, and an assignment to
//!   a member, an `Object.assign` (or `defineProperty`, `setPrototypeOf`, ...) or a `call`'s `this` on anything but an
//!   object the run itself holds ([`Walk::own_object`]): an object or array literal, a `new` object, a context value,
//!   or a name every binding of which is one of those. A member read (`[].map`, `row.toString`) may be a prototype's
//!   function, so a value reached by one is not the run's own.
//!
//! A computed key the check cannot read (`row[name]`) may still name `constructor` at run time, so the oxide runtime
//! also cuts the way from a function to `Function` and drops the legacy accessor definers when it starts
//! (`oxide::engine`).

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, BindingPattern, ChainElement, Expression, MemberExpression,
    ObjectPropertyKind, PropertyKey, PropertyKind, Statement, UnaryOperator,
};
use oxc_parser::Parser;

use crate::script::BoundedMap;

/// What a text is checked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Actions and Full are the stubs of Phases 2 and 3
pub(crate) enum Profile {
    /// One read-only expression: conditions and values (Phase 1).
    Expression,
    /// The subset of generated actions (Phase 2). Not written yet: every text fails.
    Actions,
    /// Any script (Phase 3). Not written yet: every text fails.
    Full,
}

/// What the parse of one text found, independent of which `ec` paths are bound: the first thing the profile refuses,
/// and the `ec` paths the text names, in order.
#[derive(Debug, Default)]
pub(crate) struct Analysis {
    refused: Option<String>,
    ec_paths: Vec<String>,
    /// Whether the text may change a value it read by name (a mutating method), so the oxide run writes its tracked
    /// values back at its end.
    writes: bool,
    /// The context names the text reads (bare names that are not its own variables), each once.
    reads: Vec<String>,
    /// The `name.property` reads of those names, each once: what the router looks up of an entity value.
    members: Vec<(String, String)>,
    /// Whether the text reads a value that moves between two runs: the clock or a random number (`Date`, a `now` or
    /// `random` method, an `ec` getter such as `ec.user.nowTimestamp`).
    moving: bool,
    /// Whether the text may read any context value through `context` (`context[name]`, `context.get(name)`,
    /// `Object.keys(context)`, `context` as a value), not only the names of `context.name` (noted as reads) or the
    /// whole stack handed to a host call.
    context_values: bool,
}

#[cfg(test)]
impl Analysis {
    pub(crate) fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    pub(crate) fn ec_paths(&self) -> &[String] {
        &self.ec_paths
    }

    pub(crate) fn writes(&self) -> bool {
        self.writes
    }

    pub(crate) fn reads(&self) -> &[String] {
        &self.reads
    }

    pub(crate) fn moving(&self) -> bool {
        self.moving
    }

    pub(crate) fn context_values(&self) -> bool {
        self.context_values
    }
}

/// The check's answer for one text under one allow test.
#[derive(Clone, Debug)]
pub(crate) struct Verdict {
    analysis: Arc<Analysis>,
    /// The first `ec` path the allow test refused.
    unbound: Option<String>,
}

impl Verdict {
    pub(crate) fn passes(&self) -> bool {
        self.analysis.refused.is_none() && self.unbound.is_none()
    }

    /// Why the text fails, or `None` when it passes.
    pub(crate) fn reason(&self) -> Option<String> {
        match (&self.analysis.refused, &self.unbound) {
            (Some(refused), _) => Some(refused.clone()),
            (None, Some(path)) => Some(format!("{path} is not bound on quickjs-oxide")),
            (None, None) => None,
        }
    }

    /// The `ec` paths the text names.
    pub(crate) fn ec_paths(&self) -> &[String] {
        &self.analysis.ec_paths
    }

    /// Whether the text may change a value it read by name.
    pub(crate) fn writes(&self) -> bool {
        self.analysis.writes
    }

    /// The context names the text reads, and their `name.property` reads.
    pub(crate) fn reads(&self) -> &[String] {
        &self.analysis.reads
    }

    pub(crate) fn members(&self) -> &[(String, String)] {
        &self.analysis.members
    }

    /// Whether the text may read any context value through `context`, so the router looks at every one.
    pub(crate) fn reads_all_context(&self) -> bool {
        self.analysis.context_values
    }

    /// Whether the text reads the clock or a random number, so two runs of it may differ.
    pub(crate) fn reads_moving_values(&self) -> bool {
        self.analysis.moving
    }

    /// Whether a run of the text may make a side effect (a host call, a write to the stack or an entity value) and
    /// then read a context name: what an oxide run must not then fall back on, so the router looks at those names'
    /// values first.
    pub(crate) fn may_act_before_a_read(&self) -> bool {
        !self.analysis.ec_paths.is_empty() || self.analysis.writes
    }
}

/// The global names QuickJS has (the host's and QuickJS's own) that the oxide engine does not, so a bare one would
/// read the context stack there instead. A test keeps this list in step with both engines' globals.
pub(crate) const QUICKJS_ONLY_GLOBALS: &[&str] = &[
    // QuickJS builtins quickjs-oxide does not have
    "AsyncDisposableStack",
    "DOMException",
    "DisposableStack",
    "SuppressedError",
    "atob",
    "btoa",
    "performance",
    "queueMicrotask",
    // the host's globals (`install_ec`, `install_helpers`, `tools_host`, `edi`, `sftp`)
    "EdiHandler",
    "ResourceReference",
    "SftpClient",
    "System",
    "__compareOrder",
    "__inDateRange",
    "__moquiWrap",
    "compare",
    "elasticQueryAutoWildcard",
    "filterMapList",
    "filterMapListByDate",
    "numberToWords",
    "numberToWordsWithDecimal",
    "orderMapList",
    "sri",
    // names the QuickJS bare-name object answers itself (`read_name`'s helper objects, the stack as `context`)
    "ObjectUtilities",
    "CollectionUtilities",
    "StringUtilities",
    "WebUtilities",
    "UserSecurityChecks",
];

/// The objects and functions both engines' global objects have (ECMAScript's, and the host's `toDecimal`, `toNumber`
/// and `basicConvert`): an assignment to a member of one is refused, since it would outlive the text on the thread's
/// runtime of one engine only. A test keeps it in step with the oxide runtime's globals.
pub(crate) const BUILTIN_GLOBALS: &[&str] = &[
    "AggregateError", "Array", "ArrayBuffer", "Atomics", "BigInt", "BigInt64Array", "BigUint64Array", "Boolean",
    "DataView", "Date", "Error", "EvalError", "FinalizationRegistry", "Float16Array", "Float32Array", "Float64Array",
    "Function", "Int16Array", "Int32Array", "Int8Array", "InternalError", "Iterator", "JSON", "Map", "Math", "Number", "Object",
    "Promise", "Proxy", "RangeError", "ReferenceError", "Reflect", "RegExp", "Set", "SharedArrayBuffer", "String",
    "Symbol", "SyntaxError", "TypeError", "URIError", "Uint16Array", "Uint32Array", "Uint8Array", "Uint8ClampedArray",
    "WeakMap", "WeakRef", "WeakSet", "decodeURI", "decodeURIComponent", "encodeURI", "encodeURIComponent", "escape",
    "eval", "isFinite", "isNaN", "parseFloat", "parseInt", "unescape", "basicConvert", "toDecimal", "toNumber",
];

/// The host globals the oxide runtime has too (`oxide::decimal::install_to_decimal`), which a text may name; the test
/// of [`QUICKJS_ONLY_GLOBALS`] leaves them out.
#[cfg(test)]
pub(crate) const OXIDE_HOST_GLOBALS: &[&str] = &["basicConvert", "toDecimal", "toNumber"];

/// Names a text may not read at all: the ways to code the check cannot see, to the global object itself, to a function
/// that outlives the run (a promise's reaction, a finalizer) or to an object the check cannot follow.
const REFUSED_NAMES: &[&str] = &[
    "eval",
    "Function",
    "arguments",
    "globalThis",
    "Reflect",
    "Proxy",
    "Promise",
    "Symbol",
    "WeakRef",
    "FinalizationRegistry",
];

/// Property names a text may not read, whatever the object: the way to `Function` (`constructor`), to prototypes,
/// to accessors defined on a shared object, and to a promise's reaction (`then`).
const REFUSED_PROPERTIES: &[&str] = &[
    "constructor",
    "__proto__",
    "prototype",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "then",
    // a prototype object, or a descriptor holding a builtin's accessor
    "getPrototypeOf",
    "getOwnPropertyDescriptor",
    "getOwnPropertyDescriptors",
];

/// Methods that change their receiver or another object in place: a text that calls one writes, so its oxide run writes
/// its tracked values back at the end. `add`, `set`, `delete` and `clear` (a Set's or Map's) are not on the list: a
/// decimal's `add` is everywhere, the stack holds no JavaScript Map, and a Set read from the stack falls back to QuickJS
/// (`oxide::convert`), so those methods change only a Set or Map the expression made itself.
const MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
    "assign",
    "defineProperty",
    "defineProperties",
    "setPrototypeOf",
    "freeze",
    "seal",
    "preventExtensions",
];

/// Methods that change their receiver in place but are refused on no receiver: a Set's, a Map's or a typed array's
/// `set`, `add`, `delete` and `clear` (a decimal's `add` changes nothing, but the text is taken as writing anyway).
const WRITE_METHODS: &[&str] = &["set", "add", "delete", "clear"];

/// The `Object` functions that change their first argument in place: only on an object the run holds
/// ([`Walk::own_object`]), since a builtin changed so would stay changed on the thread's runtime.
const OBJECT_WRITERS: &[&str] = &[
    "assign",
    "defineProperty",
    "defineProperties",
    "setPrototypeOf",
    "freeze",
    "seal",
    "preventExtensions",
];

/// The function methods that call or bind it with their first argument as `this`, which a builtin method could change.
const THIS_METHODS: &[&str] = &["call", "apply", "bind"];

/// The members of a builtin that are numbers, which a text may use as values: any other member of a builtin (a
/// function, an object) and a builtin itself are only called, constructed or read from, so none is kept in the run's
/// data where an assignment or `Object.assign` could reach it.
const BUILTIN_CONSTANTS: &[(&str, &str)] = &[
    ("Math", "E"),
    ("Math", "LN10"),
    ("Math", "LN2"),
    ("Math", "LOG10E"),
    ("Math", "LOG2E"),
    ("Math", "PI"),
    ("Math", "SQRT1_2"),
    ("Math", "SQRT2"),
    ("Number", "EPSILON"),
    ("Number", "MAX_SAFE_INTEGER"),
    ("Number", "MAX_VALUE"),
    ("Number", "MIN_SAFE_INTEGER"),
    ("Number", "MIN_VALUE"),
    ("Number", "NaN"),
    ("Number", "NEGATIVE_INFINITY"),
    ("Number", "POSITIVE_INFINITY"),
];

/// The methods whose result is a new array or string, not their receiver or a value it holds.
const COPY_METHODS: &[&str] = &[
    "slice", "concat", "map", "filter", "flat", "flatMap", "toSorted", "toReversed", "toSpliced", "with", "split",
];

/// The builtins' functions whose result is a new object or array.
const FRESH_BUILTINS: &[(&str, &str)] = &[
    ("Array", "from"),
    ("Array", "of"),
    ("Object", "keys"),
    ("Object", "values"),
    ("Object", "entries"),
    ("Object", "fromEntries"),
    ("JSON", "parse"),
];

/// The builtin functions an array method may be handed as its callback (`list.map(String)`, `filter(Boolean)`): each
/// returns a primitive and keeps nothing.
const PURE_CALLBACKS: &[&str] = &[
    "String", "Number", "Boolean", "parseFloat", "parseInt", "isNaN", "isFinite", "toNumber",
];

/// The methods an arrow function may be passed to: the array methods that call it during the call and keep nothing.
const ARROW_METHODS: &[&str] = &[
    "map",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "some",
    "every",
    "reduce",
    "reduceRight",
    "flatMap",
    "forEach",
    "sort",
    "toSorted",
];

/// Texts kept with their analysis, and the source bytes they may add up to.
const MAX_CHECKED: usize = 16 * 1024;
const MAX_CHECKED_BYTES: usize = 8 * 1024 * 1024;

type Checked = BoundedMap<(u64, ProfileKey), (Arc<str>, Arc<Analysis>)>;

/// [`Profile`] as a map key.
type ProfileKey = u8;

static CHECKED: LazyLock<Mutex<Checked>> =
    LazyLock::new(|| Mutex::new(BoundedMap::with_bytes(MAX_CHECKED, MAX_CHECKED_BYTES)));

/// Texts each thread keeps in front of [`CHECKED`], so a text checked again on the thread takes no lock.
const MAX_LOCAL_CHECKED: usize = 2048;

thread_local! {
    static LOCAL: std::cell::RefCell<Checked> = std::cell::RefCell::new(BoundedMap::new(MAX_LOCAL_CHECKED));
}

/// Analyses made since the process started (each a parse), for the census and the stats.
static PARSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Texts the check parsed since the process started.
pub(crate) fn parses() -> u64 {
    PARSES.load(std::sync::atomic::Ordering::Relaxed)
}

fn profile_key(profile: Profile) -> ProfileKey {
    match profile {
        Profile::Expression => 0,
        Profile::Actions => 1,
        Profile::Full => 2,
    }
}

/// The check of `text` (whose [`crate::script::text::hash_text`] is `hash`) under `profile`, with `allow` telling
/// whether an `ec` path is bound on both engines. The parse is made once per text for the process; the allow test runs
/// on each call, over the paths the parse found.
pub(crate) fn verdict(text: &str, hash: u64, profile: Profile, allow: &dyn Fn(&str) -> bool) -> Verdict {
    let key = (hash, profile_key(profile));
    let local = LOCAL.with(|local| {
        local
            .borrow_mut()
            .get(&key)
            .filter(|(kept, _)| **kept == *text)
            .map(|(_, analysis)| analysis)
    });
    let analysis = match local {
        Some(analysis) => analysis,
        None => {
            let kept = moqui_lib::sync::lock(&CHECKED)
                .get(&key)
                .filter(|(kept, _)| **kept == *text);
            let (text, analysis) = match kept {
                Some(kept) => kept,
                None => {
                    let made = (Arc::<str>::from(text), Arc::new(analyze(text, profile)));
                    moqui_lib::sync::lock(&CHECKED).insert_sized(key, made.clone(), text.len() + 64);
                    made
                }
            };
            LOCAL.with(|local| local.borrow_mut().insert(key, (text, Arc::clone(&analysis))));
            analysis
        }
    };
    let unbound = analysis.ec_paths.iter().find(|path| !allow(path)).cloned();
    Verdict { analysis, unbound }
}

/// Parse `text` and walk it under `profile`. Not kept; see [`verdict`].
pub(crate) fn analyze(text: &str, profile: Profile) -> Analysis {
    PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match profile {
        Profile::Expression | Profile::Actions => {}
        Profile::Full => {
            return Analysis {
                refused: Some(format!("the {profile:?} profile is not written yet")),
                ec_paths: Vec::new(),
                writes: true,
                ..Analysis::default()
            }
        }
    }
    crate::script::parse::with_arena(|allocator| {
        let parsed = Parser::new(allocator, text, oxc_ast::ast::SourceType::default()).parse();
        let mut walk = Walk {
            actions: profile == Profile::Actions,
            // generated actions assign to bare names: they always write
            writes: profile == Profile::Actions,
            ..Walk::default()
        };
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            walk.refuse("not JavaScript");
        } else if walk.actions {
            walk.actions_program(&parsed.program);
        } else {
            let program = &parsed.program;
            match (program.body.as_slice(), program.directives.as_slice()) {
                ([Statement::ExpressionStatement(statement)], []) => walk.expression(&statement.expression),
                // a lone string literal parses as a directive; its value is the text's
                ([], [_]) => {}
                ([], []) => walk.refuse("empty"),
                _ => walk.refuse("not one expression"),
            }
        }
        walk.finish_own_names();
        let mut reads: Vec<String> = Vec::new();
        for name in walk.reads {
            if !reads.iter().any(|kept| kept == name) {
                reads.push(name.to_string());
            }
        }
        let mut members: Vec<(String, String)> = Vec::new();
        for (name, property) in walk.members {
            if !members.iter().any(|(kept, prop)| kept == name && prop == property) {
                members.push((name.to_string(), property.to_string()));
            }
        }
        let moving = walk.moving || walk.ec_paths.iter().any(|path| moving_path(path));
        Analysis {
            refused: walk.refused,
            ec_paths: walk.ec_paths,
            writes: walk.writes,
            reads,
            members,
            moving,
            context_values: walk.context_values,
        }
    })
}

/// Whether an `ec` path reads the clock or a random number: a segment named `now...` (`ec.user.nowTimestamp`) or
/// naming `random` (`ec.user.randomString`).
fn moving_path(path: &str) -> bool {
    path.split('.')
        .skip(1)
        .any(|segment| segment.starts_with("now") || segment.to_ascii_lowercase().contains("random"))
}

/// The names a declaration's pattern binds, into `names`.
fn pattern_names<'a>(pattern: &'a BindingPattern<'a>, names: &mut Vec<&'a str>) {
    match pattern {
        BindingPattern::BindingIdentifier(name) => names.push(name.name.as_str()),
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                pattern_names(element, names);
            }
            if let Some(rest) = &array.rest {
                pattern_names(&rest.argument, names);
            }
        }
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                pattern_names(&property.value, names);
            }
            if let Some(rest) = &object.rest {
                pattern_names(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => pattern_names(&assignment.left, names),
    }
}

/// The walk of one text: the first refusal, the `ec` paths, and the names arrow functions bind around the node.
#[derive(Default)]
struct Walk<'a> {
    refused: Option<String>,
    writes: bool,
    /// The [`Profile::Actions`] walk: assignments and the actions statements allowed.
    actions: bool,
    ec_paths: Vec<String>,
    locals: Vec<HashSet<&'a str>>,
    /// The context names read, and their `name.property` reads, in order and repeated.
    reads: Vec<&'a str>,
    members: Vec<(&'a str, &'a str)>,
    /// A read of the clock or a random number outside `ec` (`Date`, `.now()`, `.random()`).
    moving: bool,
    /// The functions the text declares (actions only), wherever: a name of one is only ever called.
    functions: HashSet<&'a str>,
    /// See [`Analysis::context_values`].
    context_values: bool,
    /// Each name the text binds (a variable, a parameter, a bare name assigned), wherever, and whether every binding of
    /// it is an object the run makes ([`fresh_making`]).
    bound: std::collections::HashMap<&'a str, bool>,
    /// The names an assignment to a member (or an `Object.assign`, a `call`'s `this`) changes, with the refusal if one
    /// is not the run's own object: checked against [`Self::bound`] once the whole text is walked.
    own_names: Vec<(&'a str, String)>,
}

/// Whether `expression` makes a new object or a primitive (an object or array literal, `new` of a builtin but `Object`,
/// a literal, an operator's result), so a name it is bound to holds the run's own object.
fn fresh_making(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ObjectExpression(_)
        | Expression::ArrayExpression(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::BinaryExpression(_)
        | Expression::UnaryExpression(_)
        | Expression::UpdateExpression(_) => true,
        Expression::NewExpression(new) => {
            matches!(&new.callee, Expression::Identifier(id)
                if BUILTIN_GLOBALS.contains(&id.name.as_str()) && id.name != "Object")
        }
        Expression::ParenthesizedExpression(inner) => fresh_making(&inner.expression),
        Expression::LogicalExpression(logical) => fresh_making(&logical.left) && fresh_making(&logical.right),
        Expression::ConditionalExpression(conditional) => {
            fresh_making(&conditional.consequent) && fresh_making(&conditional.alternate)
        }
        Expression::SequenceExpression(sequence) => sequence.expressions.last().is_some_and(fresh_making),
        // `name = value` is the value; `name ||= value` may be what the name held, `+=` and the rest make primitives
        Expression::AssignmentExpression(assignment) if assignment.operator.is_assign() => {
            fresh_making(&assignment.right)
        }
        Expression::AssignmentExpression(assignment) => !assignment.operator.is_logical(),
        Expression::CallExpression(call) => fresh_call(&call.callee),
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => fresh_call(&call.callee),
            _ => false,
        },
        _ => false,
    }
}

/// Whether a call of `callee` makes a new object or a primitive: a host call (its result is converted for the run;
/// `ec` is never the text's own name), a method that copies ([`COPY_METHODS`]: on a shared object it is the builtin's,
/// which no text changes, and on the run's own object it is the builtin's too or a data member holding a builtin
/// method, whose result is new or the receiver itself), or a builtin's function that makes one ([`FRESH_BUILTINS`]).
fn fresh_call(callee: &Expression<'_>) -> bool {
    match callee {
        Expression::StaticMemberExpression(member) => {
            let property = member.property.name.as_str();
            ec_rooted(&member.object)
                || COPY_METHODS.contains(&property)
                || matches!(&member.object, Expression::Identifier(id)
                    if FRESH_BUILTINS.contains(&(id.name.as_str(), property)))
        }
        other => ec_rooted(other),
    }
}

/// Whether `expression` is reached from the bare `ec` down members and calls, as [`Walk::rooted_at_ec`] without the
/// text's names: a declaration of `ec` is refused.
fn ec_rooted(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::Identifier(identifier) => identifier.name == "ec",
        Expression::StaticMemberExpression(member) => ec_rooted(&member.object),
        Expression::ComputedMemberExpression(member) => ec_rooted(&member.object),
        Expression::CallExpression(call) => ec_rooted(&call.callee),
        Expression::ParenthesizedExpression(inner) => ec_rooted(&inner.expression),
        _ => false,
    }
}

/// What an [`Walk::own_object`] check is for, which names its refusal.
#[derive(Clone, Copy)]
enum OwnUse<'a> {
    /// `object.name = value`, `object[key]++`.
    Assignment,
    /// `Object.assign(object, ...)` and the other [`OBJECT_WRITERS`]: the builtin and its function.
    Writer(&'a str, &'a str),
    /// `fn.call(object, ...)` and the other [`THIS_METHODS`], or a method named by a computed key.
    This,
    /// `object.push(...)` and the other methods that change their receiver.
    Receiver(&'a str),
}

impl OwnUse<'_> {
    fn refusal(self, of: &str) -> String {
        match self {
            OwnUse::Assignment => format!("an assignment to a member of {of}"),
            OwnUse::Writer(builtin, name) => format!("{builtin}.{name} on {of}"),
            OwnUse::This => format!("a call with this as {of}"),
            OwnUse::Receiver(name) => format!("the method {name} on {of}"),
        }
    }
}

impl<'a> Walk<'a> {
    fn refuse(&mut self, why: impl Into<String>) {
        if self.refused.is_none() {
            self.refused = Some(why.into());
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|scope| scope.contains(name))
    }

    fn expression(&mut self, expression: &'a Expression<'a>) {
        if self.refused.is_some() {
            return;
        }
        match expression {
            Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::RegExpLiteral(_) => {}
            Expression::BigIntLiteral(_) => self.refuse("a BigInt literal"),
            Expression::TemplateLiteral(template) => {
                for part in &template.expressions {
                    self.expression(part);
                }
            }
            Expression::Identifier(identifier) => {
                let name = identifier.name.as_str();
                if BUILTIN_GLOBALS.contains(&name) {
                    self.refuse(format!("the global {name} as a value"));
                    return;
                }
                self.name(name);
            }
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(spread) => self.expression(&spread.argument),
                        ArrayExpressionElement::Elision(_) => {}
                        other => match other.as_expression() {
                            Some(item) => self.expression(item),
                            None => self.refuse("an array element"),
                        },
                    }
                }
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::SpreadProperty(spread) => self.expression(&spread.argument),
                        ObjectPropertyKind::ObjectProperty(property) => {
                            if property.kind != PropertyKind::Init || property.method {
                                self.refuse("a getter, setter or method in an object literal");
                                return;
                            }
                            if property.computed {
                                match &property.key {
                                    PropertyKey::StaticIdentifier(_) | PropertyKey::PrivateIdentifier(_) => {}
                                    key => match key.as_expression() {
                                        Some(key) => self.expression(key),
                                        None => self.refuse("a computed key"),
                                    },
                                }
                            }
                            self.expression(&property.value);
                        }
                    }
                }
            }
            Expression::BinaryExpression(binary) => {
                self.expression(&binary.left);
                // `value instanceof Date` reads the builtin and keeps nothing
                if binary.operator == oxc_ast::ast::BinaryOperator::Instanceof {
                    self.operand(&binary.right);
                } else {
                    self.expression(&binary.right);
                }
            }
            Expression::LogicalExpression(logical) => {
                self.expression(&logical.left);
                self.expression(&logical.right);
            }
            Expression::ConditionalExpression(conditional) => {
                self.expression(&conditional.test);
                self.expression(&conditional.consequent);
                self.expression(&conditional.alternate);
            }
            Expression::UnaryExpression(unary) => {
                if unary.operator == UnaryOperator::Delete {
                    // `delete context.name` in actions: the name goes from the top frame, a write as `name = null` is
                    if let Expression::StaticMemberExpression(member) = &unary.argument {
                        if self.actions
                            && matches!(&member.object, Expression::Identifier(id) if id.name == "context")
                            && !self.is_local("context")
                        {
                            return;
                        }
                    }
                    self.refuse("delete");
                    return;
                }
                if unary.operator == UnaryOperator::Typeof {
                    self.operand(&unary.argument);
                } else {
                    self.expression(&unary.argument);
                }
            }
            Expression::SequenceExpression(sequence) => {
                for item in &sequence.expressions {
                    self.expression(item);
                }
            }
            Expression::ParenthesizedExpression(inner) => self.expression(&inner.expression),
            Expression::CallExpression(call) if self.actions && self.is_own_global_test(call) => {
                // the name, read; `globalThis` itself goes nowhere
                if let Some(name) = call.arguments.get(1).and_then(Argument::as_expression) {
                    self.expression(name);
                }
            }
            Expression::CallExpression(call) => {
                let host = self.rooted_at_ec(&call.callee);
                self.call_target(&call.callee, &call.arguments);
                let arrows = self.callee(&call.callee);
                self.arguments(&call.arguments, arrows, host);
            }
            Expression::NewExpression(new) => {
                match &new.callee {
                    Expression::Identifier(identifier) => self.name(identifier.name.as_str()),
                    _ => self.refuse("new of anything but a name"),
                }
                self.arguments(&new.arguments, false, false);
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    let host = self.rooted_at_ec(&call.callee);
                    self.call_target(&call.callee, &call.arguments);
                    let arrows = self.callee(&call.callee);
                    self.arguments(&call.arguments, arrows, host);
                }
                ChainElement::TSNonNullExpression(_) => self.refuse("TypeScript"),
                other => match other.as_member_expression() {
                    Some(member) => self.member(member),
                    None => self.refuse("a chain"),
                },
            },
            Expression::StaticMemberExpression(member) => {
                if !self.builtin_member_value(&member.object, Some(member.property.name.as_str())) {
                    self.static_member(&member.object, member.property.name.as_str());
                }
            }
            Expression::ComputedMemberExpression(member) if self.builtin_member_value(&member.object, None) => {}
            Expression::ComputedMemberExpression(member) => {
                if self.rooted_at_ec(&member.object) {
                    self.refuse("computed ec[...]");
                    return;
                }
                if let Expression::StringLiteral(key) = &member.expression {
                    let allowed = match key.value.as_str() {
                        Some(name) => self.property_allowed(&member.object, name),
                        None => {
                            self.refuse("a string key that is not text");
                            false
                        }
                    };
                    if !allowed {
                        return;
                    }
                }
                self.operand(&member.object);
                self.expression(&member.expression);
            }
            Expression::PrivateFieldExpression(_) => self.refuse("a private field"),
            Expression::ArrowFunctionExpression(_) => {
                self.refuse("an arrow function that is not a call's argument");
            }
            Expression::AssignmentExpression(assignment) if self.actions => {
                if let oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(identifier) = &assignment.left {
                    // `name = value`, `name ??= value`: what the name holds after; `+=` and the rest make primitives
                    if assignment.operator.is_assign() || assignment.operator.is_logical() {
                        let fresh = fresh_making(&assignment.right);
                        self.bind(identifier.name.as_str(), fresh);
                    }
                }
                if let (
                    oxc_ast::ast::AssignmentOperator::Assign,
                    oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(identifier),
                ) = (assignment.operator, &assignment.left)
                {
                    self.written_name(identifier.name.as_str());
                    self.expression(&assignment.right);
                    return;
                }
                match assignment.left.as_simple_assignment_target() {
                    Some(target) => self.assignment_target(target),
                    None => self.refuse("a destructuring assignment"),
                }
                self.expression(&assignment.right);
            }
            Expression::UpdateExpression(update) if self.actions => self.assignment_target(&update.argument),
            Expression::AssignmentExpression(_) => self.refuse("an assignment"),
            Expression::UpdateExpression(_) => self.refuse("an update (++ or --)"),
            Expression::ThisExpression(_) => self.refuse("this"),
            Expression::FunctionExpression(_) => self.refuse("a function literal"),
            Expression::ClassExpression(_) => self.refuse("a class literal"),
            Expression::TaggedTemplateExpression(_) => self.refuse("a tagged template"),
            Expression::AwaitExpression(_) | Expression::YieldExpression(_) => self.refuse("await or yield"),
            Expression::ImportExpression(_) | Expression::ImportMeta(_) => self.refuse("import"),
            Expression::Super(_) | Expression::NewTarget(_) | Expression::PrivateInExpression(_) => {
                self.refuse("super, new.target or #x in")
            }
            _ => self.refuse("syntax outside the expression profile"),
        }
    }

    /// What an actions script assigns to: a bare name (a context name, or the function's own variable) or a member of
    /// a value not reached from `ec`.
    fn assignment_target(&mut self, target: &'a oxc_ast::ast::SimpleAssignmentTarget<'a>) {
        use oxc_ast::ast::SimpleAssignmentTarget;
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) => self.name(identifier.name.as_str()),
            other => match other.as_member_expression() {
                Some(member) => {
                    if self.rooted_at_ec(member.object()) {
                        self.refuse("an assignment to a value reached from ec");
                        return;
                    }
                    if let Some(name) = self.builtin_root(member.object()) {
                        // it would stay on the thread's runtime for later texts, on one engine and not the other
                        self.refuse(format!("an assignment to a member of the global {name}"));
                        return;
                    }
                    self.own_object(member.object(), OwnUse::Assignment);
                    self.member(member);
                }
                None => self.refuse("an assignment target"),
            },
        }
    }

    /// A generated actions script: `(function() { ... })()`, the macros' root, with no parameters.
    fn actions_program(&mut self, program: &'a oxc_ast::ast::Program<'a>) {
        let [Statement::ExpressionStatement(statement)] = program.body.as_slice() else {
            self.refuse("not one actions function");
            return;
        };
        let Expression::CallExpression(call) = statement.expression.get_inner_expression() else {
            self.refuse("not one actions function");
            return;
        };
        let Expression::FunctionExpression(function) = call.callee.get_inner_expression() else {
            self.refuse("not one actions function");
            return;
        };
        if !call.arguments.is_empty()
            || function.r#async
            || function.generator
            || !function.params.items.is_empty()
            || function.params.rest.is_some()
        {
            self.refuse("an actions function with arguments");
            return;
        }
        let Some(body) = &function.body else {
            self.refuse("an actions function with no body");
            return;
        };
        self.function_body(HashSet::new(), &body.statements);
    }

    /// The body of a function (the actions function, a function declared in it, a block-bodied arrow): its variables,
    /// wherever declared in it, and its functions are its own and not context names. A function is declared only at the
    /// top of a body, never in a block.
    fn function_body(&mut self, scope: HashSet<&'a str>, statements: &'a [Statement<'a>]) {
        self.locals.push(scope);
        for statement in statements {
            self.declared(statement, true);
        }
        for statement in statements {
            if self.refused.is_some() {
                break;
            }
            match statement {
                Statement::FunctionDeclaration(function) => self.function_declaration(function),
                other => self.statement(other),
            }
        }
        self.locals.pop();
    }

    /// A function declaration that never escapes: plain parameters, a body of the actions profile, and its name only ever
    /// called ([`Self::name`] refuses it as a value), so it runs only while the script runs.
    fn function_declaration(&mut self, function: &'a oxc_ast::ast::Function<'a>) {
        if function.r#async || function.generator {
            self.refuse("an async or generator function");
            return;
        }
        let Some(body) = &function.body else {
            self.refuse("a function with no body");
            return;
        };
        let Some(names) = self.parameters(&function.params) else {
            return;
        };
        self.function_body(names, &body.statements);
    }

    /// The names of plain parameters (no default, rest or pattern), checked as declarations; `None` when refused.
    fn parameters(&mut self, params: &'a oxc_ast::ast::FormalParameters<'a>) -> Option<HashSet<&'a str>> {
        if params.rest.is_some() {
            self.refuse("a rest parameter");
            return None;
        }
        let mut names = HashSet::new();
        for parameter in &params.items {
            match &parameter.pattern {
                BindingPattern::BindingIdentifier(name) if parameter.initializer.is_none() => {
                    self.binding(name.name.as_str());
                    self.bind(name.name.as_str(), false);
                    names.insert(name.name.as_str());
                }
                _ => {
                    self.refuse("a parameter that is not a plain name");
                    return None;
                }
            }
        }
        Some(names)
    }

    /// Note the names `statement` declares into the current scope: `var` at any depth of blocks, ifs, loops and `try`
    /// (it is the function's), `let`, `const` and functions only when `top` (they belong to the body or block that holds
    /// them).
    fn declared(&mut self, statement: &'a Statement<'a>, top: bool) {
        let var = |declaration: &oxc_ast::ast::VariableDeclaration<'_>| {
            declaration.kind == oxc_ast::ast::VariableDeclarationKind::Var
        };
        match statement {
            Statement::VariableDeclaration(declaration) if top || var(declaration) => {
                let mut names = Vec::new();
                for declarator in &declaration.declarations {
                    pattern_names(&declarator.id, &mut names);
                }
                if let Some(scope) = self.locals.last_mut() {
                    scope.extend(names);
                }
                self.const_arrows(declaration);
            }
            Statement::FunctionDeclaration(function) if top => {
                if let Some(id) = &function.id {
                    let name = id.name.as_str();
                    self.binding(name);
                    self.bind(name, false);
                    self.functions.insert(name);
                    if let Some(scope) = self.locals.last_mut() {
                        scope.insert(name);
                    }
                }
            }
            Statement::BlockStatement(block) => self.declared_in(&block.body),
            Statement::IfStatement(branch) => {
                self.declared(&branch.consequent, false);
                if let Some(alternate) = &branch.alternate {
                    self.declared(alternate, false);
                }
            }
            Statement::ForStatement(each) => {
                if let Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(declaration)) = &each.init {
                    if var(declaration) {
                        self.declared_var(declaration);
                    }
                }
                self.declared(&each.body, false);
            }
            Statement::ForOfStatement(each) => {
                if let oxc_ast::ast::ForStatementLeft::VariableDeclaration(declaration) = &each.left {
                    if var(declaration) {
                        self.declared_var(declaration);
                    }
                }
                self.declared(&each.body, false);
            }
            Statement::ForInStatement(each) => {
                if let oxc_ast::ast::ForStatementLeft::VariableDeclaration(declaration) = &each.left {
                    if var(declaration) {
                        self.declared_var(declaration);
                    }
                }
                self.declared(&each.body, false);
            }
            Statement::WhileStatement(each) => self.declared(&each.body, false),
            Statement::DoWhileStatement(each) => self.declared(&each.body, false),
            Statement::TryStatement(each) => {
                self.declared_in(&each.block.body);
                if let Some(handler) = &each.handler {
                    self.declared_in(&handler.body.body);
                }
                if let Some(finalizer) = &each.finalizer {
                    self.declared_in(&finalizer.body);
                }
            }
            _ => {}
        }
    }

    fn declared_in(&mut self, statements: &'a [Statement<'a>]) {
        for inner in statements {
            self.declared(inner, false);
        }
    }

    fn declared_var(&mut self, declaration: &'a oxc_ast::ast::VariableDeclaration<'a>) {
        let mut names = Vec::new();
        for declarator in &declaration.declarations {
            pattern_names(&declarator.id, &mut names);
        }
        if let Some(scope) = self.locals.last_mut() {
            scope.extend(names);
        }
    }

    /// The `let` and `const` names `statements` declare directly: a block's own scope.
    fn block_scope(&mut self, statements: &'a [Statement<'a>]) -> HashSet<&'a str> {
        let mut names = Vec::new();
        for statement in statements {
            if let Statement::VariableDeclaration(declaration) = statement {
                if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Var {
                    for declarator in &declaration.declarations {
                        pattern_names(&declarator.id, &mut names);
                    }
                    self.const_arrows(declaration);
                }
            }
        }
        names.into_iter().collect()
    }

    /// The `const name = (params) => body` of a declaration (actions only), noted as the text's functions before the
    /// walk reaches any statement of their scope: a function declared before the arrow that names it then sees it as
    /// a function, so it cannot return the arrow as a value for it to outlive the run.
    fn const_arrows(&mut self, declaration: &'a oxc_ast::ast::VariableDeclaration<'a>) {
        if !self.actions || declaration.kind != oxc_ast::ast::VariableDeclarationKind::Const {
            return;
        }
        for declarator in &declaration.declarations {
            if let (BindingPattern::BindingIdentifier(name), Some(Expression::ArrowFunctionExpression(_))) =
                (&declarator.id, &declarator.init)
            {
                let name = name.name.as_str();
                self.binding(name);
                self.functions.insert(name);
            }
        }
    }

    /// Refuse a declaration (a variable, a parameter, a function) that would shadow `ec`, a refused name, a QuickJS
    /// global or a builtin (so a builtin's name is never the text's own), or that names a function the text declares:
    /// a later read of it would not be what the check took it for.
    fn binding(&mut self, name: &str) {
        if name == "ec"
            || REFUSED_NAMES.contains(&name)
            || QUICKJS_ONLY_GLOBALS.contains(&name)
            || BUILTIN_GLOBALS.contains(&name)
        {
            self.refuse(format!("a declaration of {name}"));
        } else if self.functions.contains(name) {
            self.refuse(format!("a second declaration of the function {name}"));
        }
    }

    /// A statement of an actions function: declarations, expressions (assignments among them), `if`, `for`, `for...of`
    /// and `for...in`, `while` and `do...while`, `try` with `catch` and `finally`, blocks, `break` and `continue`
    /// without a label, `return` and `throw`. Anything else (a function declared in a block, a class, `switch`, labels,
    /// `with`) is refused.
    fn statement(&mut self, statement: &'a Statement<'a>) {
        if self.refused.is_some() {
            return;
        }
        match statement {
            Statement::VariableDeclaration(declaration) => self.declaration(declaration),
            Statement::ExpressionStatement(statement) => self.expression(&statement.expression),
            Statement::IfStatement(branch) if self.own_global_copy(branch) => {}
            Statement::IfStatement(branch) => {
                self.expression(&branch.test);
                self.statement(&branch.consequent);
                if let Some(alternate) = &branch.alternate {
                    self.statement(alternate);
                }
            }
            Statement::ForStatement(each) => {
                // a `let` or `const` of the loop's start is the loop's own
                let mut names = Vec::new();
                if let Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(declaration)) = &each.init {
                    if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Var {
                        for declarator in &declaration.declarations {
                            pattern_names(&declarator.id, &mut names);
                        }
                    }
                }
                self.locals.push(names.into_iter().collect());
                self.for_statement(each);
                self.locals.pop();
            }
            Statement::ForOfStatement(each) => {
                if each.r#await {
                    self.refuse("for await");
                    return;
                }
                self.for_each(&each.left, &each.right, &each.body);
            }
            Statement::ForInStatement(each) => self.for_each(&each.left, &each.right, &each.body),
            Statement::WhileStatement(each) => {
                self.expression(&each.test);
                self.statement(&each.body);
            }
            Statement::DoWhileStatement(each) => {
                self.statement(&each.body);
                self.expression(&each.test);
            }
            _ => self.statement_rest(statement),
        }
    }

    fn for_statement(&mut self, each: &'a oxc_ast::ast::ForStatement<'a>) {
        match &each.init {
            Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(declaration)) => self.declaration(declaration),
            Some(init) => match init.as_expression() {
                Some(init) => self.expression(init),
                None => self.refuse("a for statement's start"),
            },
            None => {}
        }
        if let Some(test) = &each.test {
            self.expression(test);
        }
        if let Some(update) = &each.update {
            self.expression(update);
        }
        self.statement(&each.body);
    }

    /// `for (left of right) body` or `for (left in right) body`: `right` read outside the loop, a `let` or `const` of
    /// `left` the loop's own, a bare name of `left` written.
    fn for_each(
        &mut self,
        left: &'a oxc_ast::ast::ForStatementLeft<'a>,
        right: &'a Expression<'a>,
        body: &'a Statement<'a>,
    ) {
        self.expression(right);
        let mut names = Vec::new();
        match left {
            oxc_ast::ast::ForStatementLeft::VariableDeclaration(declaration) => {
                let [declarator] = declaration.declarations.as_slice() else {
                    self.refuse("a for statement's declaration");
                    return;
                };
                if declarator.init.is_some() {
                    self.refuse("a for statement's declaration");
                    return;
                }
                self.pattern(&declarator.id);
                self.bind_unknown(&declarator.id);
                if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Var {
                    pattern_names(&declarator.id, &mut names);
                }
            }
            oxc_ast::ast::ForStatementLeft::AssignmentTargetIdentifier(identifier) => {
                self.written_name(identifier.name.as_str());
                self.bind(identifier.name.as_str(), false);
            }
            other => match other.as_assignment_target().and_then(|target| target.as_simple_assignment_target()) {
                Some(target) => self.assignment_target(target),
                None => self.refuse("a destructuring assignment"),
            },
        }
        self.locals.push(names.into_iter().collect());
        self.statement(body);
        self.locals.pop();
    }

    fn statement_rest(&mut self, statement: &'a Statement<'a>) {
        match statement {
            Statement::BlockStatement(block) => self.block(&block.body),
            Statement::TryStatement(each) => {
                self.block(&each.block.body);
                if let Some(handler) = &each.handler {
                    let mut names = Vec::new();
                    if let Some(parameter) = &handler.param {
                        self.pattern(&parameter.pattern);
                        self.bind_unknown(&parameter.pattern);
                        pattern_names(&parameter.pattern, &mut names);
                    }
                    self.locals.push(names.into_iter().collect());
                    self.block(&handler.body.body);
                    self.locals.pop();
                }
                if let Some(finalizer) = &each.finalizer {
                    self.block(&finalizer.body);
                }
            }
            Statement::BreakStatement(each) if each.label.is_none() => {}
            Statement::ContinueStatement(each) if each.label.is_none() => {}
            Statement::ReturnStatement(statement) => {
                if let Some(argument) = &statement.argument {
                    self.expression(argument);
                }
            }
            Statement::ThrowStatement(statement) => self.expression(&statement.argument),
            Statement::EmptyStatement(_) => {}
            Statement::FunctionDeclaration(_) => self.refuse("a function declared in a block"),
            _ => self.refuse("a statement outside the actions profile"),
        }
    }

    /// A block: its `let` and `const` names its own.
    fn block(&mut self, statements: &'a [Statement<'a>]) {
        let scope = self.block_scope(statements);
        self.locals.push(scope);
        for inner in statements {
            self.statement(inner);
        }
        self.locals.pop();
    }

    fn declaration(&mut self, declaration: &'a oxc_ast::ast::VariableDeclaration<'a>) {
        for declarator in &declaration.declarations {
            match (&declarator.id, &declarator.init) {
                (BindingPattern::BindingIdentifier(name), init) => {
                    self.bind(name.name.as_str(), init.as_ref().is_none_or(fresh_making));
                }
                // a destructured name holds a member read, which may be a prototype's function
                (pattern, _) => self.bind_unknown(pattern),
            }
            let noted = matches!(
                (&declarator.id, &declarator.init),
                (BindingPattern::BindingIdentifier(name), Some(Expression::ArrowFunctionExpression(_)))
                    if self.actions
                        && declaration.kind == oxc_ast::ast::VariableDeclarationKind::Const
                        && self.functions.contains(name.name.as_str())
            );
            if !noted {
                self.pattern(&declarator.id);
            }
            // `const name = (params) => body` in actions: a function as a declared one is, only ever called
            if let (
                true,
                oxc_ast::ast::VariableDeclarationKind::Const,
                BindingPattern::BindingIdentifier(name),
                Some(Expression::ArrowFunctionExpression(arrow)),
            ) = (self.actions, declaration.kind, &declarator.id, &declarator.init)
            {
                // noted with its scope ([`Self::const_arrows`]), or here for a scope with no pre-pass
                if !self.functions.contains(name.name.as_str()) {
                    self.functions.insert(name.name.as_str());
                }
                self.arrow(arrow);
                continue;
            }
            if let Some(init) = &declarator.init {
                self.expression(init);
            }
        }
    }

    /// XmlActions' copy of a service's result by name, walked when `branch` is exactly it:
    /// `if (!Object.prototype.hasOwnProperty.call(globalThis, k)) globalThis[k] = value` with no `else`, the same bare
    /// `k` in the test and the key, so only a name the global object does not have (a context name) is written.
    /// `globalThis[k] = v` anywhere else is refused, since it could replace a builtin (`JSON`) for the thread.
    fn own_global_copy(&mut self, branch: &'a oxc_ast::ast::IfStatement<'a>) -> bool {
        if !self.actions || branch.alternate.is_some() {
            return false;
        }
        let Expression::UnaryExpression(not) = &branch.test else {
            return false;
        };
        let Expression::CallExpression(test) = &not.argument else {
            return false;
        };
        if not.operator != oxc_ast::ast::UnaryOperator::LogicalNot || !self.is_own_global_test(test) {
            return false;
        }
        let Some(Expression::Identifier(tested)) = test.arguments[1].as_expression() else {
            return false;
        };
        let Statement::ExpressionStatement(statement) = &branch.consequent else {
            return false;
        };
        let Expression::AssignmentExpression(assignment) = &statement.expression else {
            return false;
        };
        let (oxc_ast::ast::AssignmentOperator::Assign, oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(member)) =
            (assignment.operator, &assignment.left)
        else {
            return false;
        };
        let Expression::Identifier(key) = &member.expression else {
            return false;
        };
        if !self.is_global_this(&member.object) || key.name != tested.name {
            return false;
        }
        self.name(key.name.as_str());
        self.expression(&assignment.right);
        true
    }

    /// Whether `expression` is the bare `globalThis`.
    fn is_global_this(&self, expression: &Expression<'_>) -> bool {
        matches!(expression, Expression::Identifier(id) if id.name == "globalThis") && !self.is_local("globalThis")
    }

    /// XmlActions' test before it copies a service's results: exactly
    /// `Object.prototype.hasOwnProperty.call(globalThis, name)`, which reads no value and keeps nothing.
    fn is_own_global_test(&self, call: &oxc_ast::ast::CallExpression<'a>) -> bool {
        let Expression::StaticMemberExpression(call_member) = &call.callee else {
            return false;
        };
        let Expression::StaticMemberExpression(has_own) = &call_member.object else {
            return false;
        };
        let Expression::StaticMemberExpression(prototype) = &has_own.object else {
            return false;
        };
        call_member.property.name == "call"
            && has_own.property.name == "hasOwnProperty"
            && prototype.property.name == "prototype"
            && matches!(&prototype.object, Expression::Identifier(id) if id.name == "Object")
            && !self.is_local("Object")
            && call.arguments.len() == 2
            && call.arguments[0].as_expression().is_some_and(|first| self.is_global_this(first))
            && call.arguments[1].as_expression().is_some()
    }

    /// A declaration's pattern: each name checked as a declaration, defaults and computed keys walked as expressions.
    fn pattern(&mut self, pattern: &'a BindingPattern<'a>) {
        match pattern {
            BindingPattern::BindingIdentifier(name) => self.binding(name.name.as_str()),
            BindingPattern::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.pattern(element);
                }
                if let Some(rest) = &array.rest {
                    self.pattern(&rest.argument);
                }
            }
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    if property.computed {
                        match property.key.as_expression() {
                            Some(key) => self.expression(key),
                            None => self.refuse("a computed key"),
                        }
                    } else {
                        let name = match &property.key {
                            PropertyKey::StaticIdentifier(name) => Some(name.name.as_str()),
                            PropertyKey::StringLiteral(name) => name.value.as_str(),
                            _ => None,
                        };
                        if let Some(name) = name.filter(|name| REFUSED_PROPERTIES.contains(name)) {
                            self.refuse(format!("the property {name}"));
                        }
                    }
                    self.pattern(&property.value);
                }
                if let Some(rest) = &object.rest {
                    self.pattern(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                self.pattern(&assignment.left);
                self.expression(&assignment.right);
            }
        }
    }

    /// A bare name read: refused for what the check refuses, else noted as a context name the text reads.
    fn name(&mut self, name: &'a str) {
        if self.is_local(name) {
            if self.functions.contains(name) {
                // as a value it could be kept past the run
                self.refuse(format!("the function {name} used as a value"));
            }
            return;
        }
        self.written_name(name);
        if name == "Date" {
            self.moving = true;
        }
        if name == "context" {
            self.context_values = true;
        }
        if self.refused.is_none() && name != "ec" {
            self.reads.push(name);
        }
    }

    /// A bare name written and not read (`name = value`): refused for what the check refuses.
    fn written_name(&mut self, name: &'a str) {
        if self.is_local(name) {
            return;
        }
        if name == "ec" {
            self.refuse("ec as a value");
        } else if REFUSED_NAMES.contains(&name) {
            self.refuse(format!("the name {name}"));
        } else if QUICKJS_ONLY_GLOBALS.contains(&name) {
            self.refuse(format!("the QuickJS global {name}"));
        }
    }

    /// Whether `expression` is the bare `ec` (not an arrow's own parameter of that name).
    fn is_ec(&self, expression: &Expression<'_>) -> bool {
        matches!(expression, Expression::Identifier(identifier) if identifier.name == "ec" && !self.is_local("ec"))
    }

    /// The static `ec` path of a member chain rooted at the bare `ec`: `ec.user.userId` is `ec.user.userId`.
    fn ec_path(&self, expression: &Expression<'_>) -> Option<String> {
        match expression {
            Expression::Identifier(_) if self.is_ec(expression) => Some("ec".to_string()),
            Expression::StaticMemberExpression(member) => self
                .ec_path(&member.object)
                .map(|path| format!("{path}.{}", member.property.name)),
            Expression::ParenthesizedExpression(inner) => self.ec_path(&inner.expression),
            _ => None,
        }
    }

    /// Whether `expression` is a value reached from `ec`: a member or call result down a chain from the bare `ec`, a
    /// host object whose methods could keep a function passed to them.
    /// The global object of the engine (`Math`, `JSON`, `Object`, a builtin function) a member chain starts at, when it
    /// is not the text's own name.
    fn builtin_root<'e>(&self, expression: &'e Expression<'_>) -> Option<&'e str> {
        match expression {
            Expression::Identifier(identifier) => {
                let name = identifier.name.as_str();
                (BUILTIN_GLOBALS.contains(&name) && !self.is_local(name)).then_some(name)
            }
            Expression::StaticMemberExpression(member) => self.builtin_root(&member.object),
            Expression::ComputedMemberExpression(member) => self.builtin_root(&member.object),
            Expression::ParenthesizedExpression(inner) => self.builtin_root(&inner.expression),
            _ => None,
        }
    }

    fn rooted_at_ec(&self, expression: &Expression<'_>) -> bool {
        match expression {
            Expression::Identifier(_) => self.is_ec(expression),
            Expression::StaticMemberExpression(member) => self.rooted_at_ec(&member.object),
            Expression::ComputedMemberExpression(member) => self.rooted_at_ec(&member.object),
            Expression::CallExpression(call) => self.rooted_at_ec(&call.callee),
            Expression::ParenthesizedExpression(inner) => self.rooted_at_ec(&inner.expression),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => self.rooted_at_ec(&call.callee),
                other => other
                    .as_member_expression()
                    .is_some_and(|member| self.rooted_at_ec(member.object())),
            },
            _ => false,
        }
    }

    /// `object.property`; an `ec` path is recorded whole, from its longest static chain.
    fn static_member(&mut self, object: &'a Expression<'a>, property: &'a str) {
        if !self.property_allowed(object, property) {
            return;
        }
        if property == "now" || property == "random" {
            self.moving = true;
        }
        if let Some(path) = self.ec_path(object) {
            self.ec_paths.push(format!("{path}.{property}"));
            return;
        }
        if self.is_ec(object) {
            self.ec_paths.push(format!("ec.{property}"));
            return;
        }
        if let Expression::Identifier(identifier) = object {
            if !self.is_local(identifier.name.as_str()) {
                if identifier.name == "context" {
                    match property {
                        "get" | "getCombinedMap" => self.context_values = true,
                        // they read the old value and write the stack
                        "put" | "remove" => {
                            self.context_values = true;
                            self.writes = true;
                        }
                        // writes the stack, reading no value of it
                        "putAll" => self.writes = true,
                        // read only names (`in` and the stack's own keys), which convert no value
                        "containsKey" | "keySet" => {}
                        name => self.reads.push(name),
                    }
                    return;
                }
                self.members.push((identifier.name.as_str(), property));
            }
        }
        self.operand(object);
    }

    /// A member of an optional chain.
    fn member(&mut self, member: &'a MemberExpression<'a>) {
        match member {
            MemberExpression::StaticMemberExpression(member) => {
                if !self.builtin_member_value(&member.object, Some(member.property.name.as_str())) {
                    self.static_member(&member.object, member.property.name.as_str());
                }
            }
            MemberExpression::ComputedMemberExpression(member) if self.builtin_member_value(&member.object, None) => {}
            MemberExpression::ComputedMemberExpression(member) => {
                if self.rooted_at_ec(&member.object) {
                    self.refuse("computed ec[...]");
                    return;
                }
                if let Expression::StringLiteral(key) = &member.expression {
                    let allowed = match key.value.as_str() {
                        Some(name) => self.property_allowed(&member.object, name),
                        None => {
                            self.refuse("a string key that is not text");
                            false
                        }
                    };
                    if !allowed {
                        return;
                    }
                }
                self.operand(&member.object);
                self.expression(&member.expression);
            }
            MemberExpression::PrivateFieldExpression(_) => self.refuse("a private field"),
        }
    }

    /// A property name a text may read of `object`: not one of [`REFUSED_PROPERTIES`]; one of [`MUTATING_METHODS`] marks
    /// the text as writing, and is refused on a value reached from `ec` (a host object the oxide engine does not track).
    fn property_allowed(&mut self, object: &Expression<'_>, property: &str) -> bool {
        if REFUSED_PROPERTIES.contains(&property) {
            self.refuse(format!("the property {property}"));
            false
        } else if WRITE_METHODS.contains(&property) {
            self.writes = true;
            true
        } else if MUTATING_METHODS.contains(&property) {
            if self.rooted_at_ec(object) {
                self.refuse(format!("the mutating method {property} on a value reached from ec"));
                return false;
            }
            self.writes = true;
            true
        } else {
            true
        }
    }

    /// Note a binding of `name`: whether it holds an object the run makes ([`fresh_making`]).
    fn bind(&mut self, name: &'a str, fresh: bool) {
        let all = self.bound.entry(name).or_insert(true);
        *all &= fresh;
    }

    /// Note each name of `pattern` as holding a value the run may not have made.
    fn bind_unknown(&mut self, pattern: &'a BindingPattern<'a>) {
        let mut names = Vec::new();
        pattern_names(pattern, &mut names);
        for name in names {
            self.bind(name, false);
        }
    }

    /// An expression that is read from, called or constructed, not kept: a builtin itself may be it (`Math` of
    /// `Math.max`, `Date` of `x instanceof Date`), where [`Self::expression`] refuses a builtin as a value.
    fn operand(&mut self, expression: &'a Expression<'a>) {
        if self.refused.is_some() {
            return;
        }
        match expression {
            Expression::Identifier(identifier) => self.name(identifier.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                self.static_member(&member.object, member.property.name.as_str());
            }
            Expression::ParenthesizedExpression(inner) => self.operand(&inner.expression),
            other => self.expression(other),
        }
    }

    /// Whether `object.property` (`property` `None` for a computed key) is a member of a builtin used as a value, which
    /// is refused, but for the number constants ([`BUILTIN_CONSTANTS`]).
    fn builtin_member_value(&mut self, object: &'a Expression<'a>, property: Option<&str>) -> bool {
        let Some(root) = self.builtin_root(object) else {
            return false;
        };
        // `Array.prototype` and the like are refused by their property's name
        if property.is_some_and(|property| REFUSED_PROPERTIES.contains(&property)) {
            return false;
        }
        let constant = match (object, property) {
            (Expression::Identifier(identifier), Some(property)) => {
                BUILTIN_CONSTANTS.contains(&(identifier.name.as_str(), property))
            }
            _ => false,
        };
        if constant {
            return false;
        }
        // a refusal further in (`Array.prototype.x`) is named first
        let root = root.to_string();
        self.operand(object);
        self.refuse(format!("a member of the global {root} as a value"));
        true
    }

    /// An object that `used` changes in place must be one the run holds: an object or array literal, a `new` object,
    /// a host call's result, a primitive, or a name every binding of which is one of those (a context value the text
    /// does not bind is the run's own copy). Anything else (a member read, another call's result, a parameter) may be a
    /// builtin or a prototype's function, shared by every later text on the thread's runtime.
    fn own_object(&mut self, object: &'a Expression<'a>, used: OwnUse<'_>) {
        match object {
            Expression::ParenthesizedExpression(inner) => self.own_object(&inner.expression, used),
            Expression::LogicalExpression(logical) => {
                self.own_object(&logical.left, used);
                self.own_object(&logical.right, used);
            }
            Expression::ConditionalExpression(conditional) => {
                self.own_object(&conditional.consequent, used);
                self.own_object(&conditional.alternate, used);
            }
            Expression::SequenceExpression(sequence) => {
                if let Some(last) = sequence.expressions.last() {
                    self.own_object(last, used);
                }
            }
            Expression::AssignmentExpression(assignment) if assignment.operator.is_assign() => {
                self.own_object(&assignment.right, used);
            }
            // `target ||= value` is the target's old value or the value: both must be the run's own
            Expression::AssignmentExpression(assignment) if assignment.operator.is_logical() => {
                match &assignment.left {
                    oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                        self.own_name(identifier.name.as_str(), used);
                    }
                    _ => self.refuse(used.refusal("a value the text may not have made")),
                }
                self.own_object(&assignment.right, used);
            }
            // `+=` and the rest make primitives
            Expression::AssignmentExpression(_) => {}
            Expression::Identifier(identifier) => self.own_name(identifier.name.as_str(), used),
            other if fresh_making(other) => {}
            other => match self.builtin_root(other) {
                Some(root) => self.refuse(used.refusal(&format!("the global {root}"))),
                None => self.refuse(used.refusal("a value the text may not have made")),
            },
        }
    }

    /// The name `name` as an object `used` changes: not a builtin, the stack itself only for a member assignment, and
    /// any other name checked against its bindings once the whole text is walked ([`Self::finish_own_names`]).
    fn own_name(&mut self, name: &'a str, used: OwnUse<'_>) {
        if BUILTIN_GLOBALS.contains(&name) {
            self.refuse(used.refusal(&format!("the global {name}")));
        } else if name == "context" && !self.is_local(name) {
            // `context.name = value` writes the stack; the stack object itself is the runtime's
            if !matches!(used, OwnUse::Assignment) {
                self.refuse(used.refusal("the context stack"));
            }
        } else {
            let why = used.refusal(&format!("{name}, which the text binds to a value it may not have made"));
            self.own_names.push((name, why));
        }
    }

    /// A call that changes its receiver in place ([`MUTATING_METHODS`], or a method named by a key
    /// the check cannot read, which may also reach a prototype by `__proto__`), or its first argument
    /// ([`OBJECT_WRITERS`], `Error.captureStackTrace`), or calls with that argument as `this` ([`THIS_METHODS`], or a
    /// method named by a key the check cannot read): that receiver or argument must be the run's own object
    /// ([`Self::own_object`]).
    fn call_target(&mut self, callee: &'a Expression<'a>, arguments: &'a [Argument<'a>]) {
        let (object, property) = match callee {
            Expression::StaticMemberExpression(member) => (&member.object, Some(member.property.name.as_str())),
            Expression::ComputedMemberExpression(member) => match &member.expression {
                Expression::StringLiteral(key) => (&member.object, key.value.as_str()),
                _ => (&member.object, None),
            },
            _ => return,
        };
        if self.rooted_at_ec(object) {
            return;
        }
        let builtin = match object {
            Expression::Identifier(identifier) => Some(identifier.name.as_str()),
            _ => None,
        };
        // [`WRITE_METHODS`] change only a Map, a Set or a typed array, and no builtin object is one, so on a shared
        // receiver they throw or change nothing; a decimal's `add` changes nothing either
        let writer = matches!((builtin, property), (Some("Object"), Some(name)) if OBJECT_WRITERS.contains(&name));
        match property {
            Some(name) if !writer && MUTATING_METHODS.contains(&name) => {
                self.own_object(object, OwnUse::Receiver(name));
            }
            None => self.own_object(object, OwnUse::Receiver("named by a computed key")),
            Some(_) => {}
        }
        let used = match (builtin, property) {
            (Some("Object"), Some(name)) if OBJECT_WRITERS.contains(&name) => OwnUse::Writer("Object", name),
            (Some("Error"), Some("captureStackTrace")) => OwnUse::Writer("Error", "captureStackTrace"),
            (_, Some(name)) if THIS_METHODS.contains(&name) => OwnUse::This,
            (_, None) => OwnUse::This,
            _ => return,
        };
        match arguments.first() {
            None => {}
            Some(Argument::SpreadElement(_)) => self.refuse(used.refusal("a spread argument")),
            // refused by the arguments' walk, as an arrow where one may not go
            Some(Argument::ArrowFunctionExpression(_)) => {}
            Some(first) => match first.as_expression() {
                Some(first) => self.own_object(first, used),
                None => self.refuse(used.refusal("an argument")),
            },
        }
    }

    /// The names [`Self::own_object`] left for the end of the walk: refused when any binding of one may hold a value
    /// the run did not make.
    fn finish_own_names(&mut self) {
        for (name, why) in std::mem::take(&mut self.own_names) {
            if self.bound.get(name) == Some(&false) {
                self.refuse(why);
            }
        }
    }

    /// A call's callee; whether an arrow function may be its argument (one of [`ARROW_METHODS`] on a value not
    /// reached from `ec`).
    fn callee(&mut self, callee: &'a Expression<'a>) -> bool {
        match callee {
            Expression::StaticMemberExpression(member) => {
                let property = member.property.name.as_str();
                let arrows = !self.rooted_at_ec(&member.object) && ARROW_METHODS.contains(&property);
                self.static_member(&member.object, property);
                arrows
            }
            Expression::Identifier(identifier) if identifier.name == "ec" && !self.is_local("ec") => {
                self.refuse("ec called");
                false
            }
            // a function the text declares, called
            Expression::Identifier(identifier)
                if self.functions.contains(identifier.name.as_str()) && self.is_local(identifier.name.as_str()) =>
            {
                false
            }
            // a builtin function called (`String(x)`) is not kept
            Expression::Identifier(identifier) => {
                self.name(identifier.name.as_str());
                false
            }
            other => {
                // a method named by a key the check cannot read (`list[op](x)`) may be a mutator
                if let Expression::ComputedMemberExpression(member) = other {
                    if !matches!(
                        member.expression,
                        Expression::StringLiteral(_) | Expression::NumericLiteral(_)
                    ) {
                        self.writes = true;
                    }
                }
                self.expression(other);
                false
            }
        }
    }

    /// A call's arguments; `host` when the callee is reached from `ec`: `context` handed to it is the stack as a host
    /// map, which converts none of its values to script values.
    fn arguments(&mut self, arguments: &'a [Argument<'a>], arrows: bool, host: bool) {
        for argument in arguments {
            match argument {
                Argument::Identifier(identifier)
                    if host && identifier.name == "context" && !self.is_local("context") => {}
                Argument::SpreadElement(spread) => self.expression(&spread.argument),
                Argument::Identifier(identifier) if arrows && PURE_CALLBACKS.contains(&identifier.name.as_str()) => {
                    self.name(identifier.name.as_str());
                }
                Argument::ArrowFunctionExpression(arrow) => {
                    if !arrows {
                        self.refuse("an arrow function passed to anything but an array method that keeps nothing");
                        return;
                    }
                    self.arrow(arrow);
                }
                other => match other.as_expression() {
                    Some(expression) => self.expression(expression),
                    None => self.refuse("an argument"),
                },
            }
        }
    }

    /// An arrow function passed as an argument: plain parameters, an expression body (or, in an actions script, a
    /// body of the actions profile: the array method calls it during the call and keeps nothing).
    fn arrow(&mut self, arrow: &'a oxc_ast::ast::ArrowFunctionExpression<'a>) {
        if arrow.r#async {
            self.refuse("an async arrow function");
            return;
        }
        let Some(names) = self.parameters(&arrow.params) else {
            return;
        };
        match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) if self.actions => self.function_body(names, &body.statements),
            ArrowFunctionBody::FunctionBody(_) => self.refuse("an arrow function with a block body"),
            body => {
                self.locals.push(names);
                match body.as_expression() {
                    Some(body) => self.expression(body),
                    None => self.refuse("an arrow body"),
                }
                self.locals.pop();
            }
        }
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
