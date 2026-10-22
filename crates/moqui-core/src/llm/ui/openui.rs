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

//! OpenUI Lang on the server (plans/LLM-A2A-Assist.md 4.10, 7.4, D6, J28): the one parser is the vendored
//! `@openuidev/lang-core` bundle (`webroot/libs/openui/lang-core.min.js`, built by `libs/refresh.mjs` with its reviewed
//! patches), run in a QuickJS runtime of its own with the component library the renderer declares
//! (`webroot/js/openui-library.spec.json`, generated from `js/WebrootAssistCanvas.js`). A `write_ui` program is parsed
//! once here, before anything is shown; a patch is merged here once with lang-core's own `mergeStatements` (the
//! history keeps only the patch, the canvas the merged program). There is no repair layer: what does not parse or does
//! not check is an error the model reads, each naming the `openui-lang` skill section to read and quoting its rule
//! ([`Rule`], [`RULES`]), so it fixes the program in the next step.
//!
//! The checks over lang-core's parse ([`check`]): unbalanced statements (a Query whose options object is not closed
//! with `}}` is the `query` rule), lines that are no statement, named arguments, a `$` that holds a Query or a computed
//! value, quoted expressions and single quotes, invented functions and JavaScript calls, unknown components and
//! arguments that do not fit (the component's signature, `components` or `forms`), the Query and Mutation shapes,
//! undefined and unreachable names, a Query nothing shows, a filter that no Query reads, a Link to something that is
//! not a screen path, and for a patch a renamed statement (`edit`). [`evaluate_mutation`] evaluates a canvas
//! Mutation's arguments from the stored program with the state the user's canvas posted, so what runs is the program's
//! own request, never one the page made up.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{CatchResultExt, Context, Runtime};
use serde_json::{json, Value as Json};

use crate::context::{Error, ExecutionContextFactory};

/// The skill whose sections the errors name.
pub const SKILL: &str = "openui-lang";
/// Where the bundle and the library spec are read from.
pub const BUNDLE_LOCATION: &str = "component://webroot/screen/webroot/libs/openui/lang-core.min.js";
pub const SPEC_LOCATION: &str = "component://webroot/screen/webroot/js/openui-library.spec.json";
/// The longest program (or patch) taken, in characters.
pub const MAX_PROGRAM_CHARS: usize = 64 * 1024;
/// The most problems one error lists.
const MAX_PROBLEMS: usize = 5;

const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const STACK_LIMIT: usize = 2 * 1024 * 1024;
/// How long one parse, merge or evaluation may run.
const TIME_LIMIT: Duration = Duration::from_secs(5);

/// A rule of the skill: its section and its text, quoted verbatim in the section (a test checks it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The skill of the section: `openui-lang`, `vue-sfc`.
    pub skill: &'static str,
    pub section: &'static str,
    pub text: &'static str,
}

macro_rules! rule {
    ($name:ident, $section:expr, $text:expr) => {
        pub const $name: Rule = Rule { skill: SKILL, section: $section, text: $text };
    };
}

rule!(STATEMENT, "syntax", "Each statement is one line, name = expression, and root = Stack([...]) is the first line.");
rule!(BALANCED, "syntax", "Brackets, braces and parentheses balance in every statement, and strings use double quotes.");
rule!(
    POSITIONAL,
    "syntax",
    "Arguments are positional: Stack([a, b], \"row\"), never Stack([a, b], direction: \"row\"). A colon is only for object literals and the ternary."
);
rule!(
    DOLLAR,
    "syntax",
    "A $name holds a plain value, such as $q = \"\". A Query, a Mutation or a computed value has a plain name: orders = Query(...), placed = @Count(orders.rows)."
);
rule!(
    QUOTED,
    "syntax",
    "Never put an expression in quotes and never use single quotes: Stat(\"Total\", @Sum(orders.rows.total)), not \"@Sum(orders.rows.total)\"."
);
rule!(COMPONENTS, "components", "Use only the components listed here, with their arguments in the order shown; ? marks an optional one.");
rule!(
    FORM_VALUE,
    "forms",
    "The value of a control is its second argument, $ and its name: Input(\"q\", $q), Select(\"status\", $status, [SelectItem(\"A\", \"Active\")])."
);
rule!(
    FUNCTIONS,
    "functions",
    "Use only the @ functions listed here. There is no @Map and no @If: write rows.field for a column and cond ? a : b for a choice."
);
rule!(JS_CALLS, "functions", "There are no JavaScript calls: no Number(), .toFixed() or .toString(); round with @Round(x, 2).");
rule!(
    QUERY_SHAPE,
    "query",
    "Query is Query(\"request\", {path, query: {...}}, {rows: []}): close the options object with }} before the default data."
);
rule!(
    QUERY_PATH,
    "query",
    "A Query reads a list of a screen: path is an /apps screen path with a list (one data reads), and list names it when the screen has several."
);
rule!(
    MUTATION_SHAPE,
    "query",
    "A Mutation is Mutation(\"request\", {method: \"POST\", path, body: {...}}) on a transition of an /apps screen, run by a Button's Action([@Run(name)])."
);
rule!(
    MUTATION_BODY,
    "query",
    "A Mutation's body uses $variables and literals only; to send a row's value, @Set it into a $variable in the same Action first."
);
rule!(DEFINED, "verify", "Every referenced name is defined.");
rule!(REACHABLE, "verify", "Every defined name other than root is reachable from root, and every Query is shown by a component.");
rule!(WIRED, "verify", "Every visible filter $variable appears in the arguments of a Query.");
rule!(
    LINKS,
    "links",
    "Link(label, path, params) opens a screen the user can view: path is an /apps or /qapps screen path, never a transition, a file or another site."
);
rule!(RENAME, "edit", "Reuse the existing statement names exactly; a patch with a new name for an existing part renames it, and the old one is dropped.");

/// Every rule, for the test that each is quoted in its section.
pub const RULES: &[Rule] = &[
    STATEMENT,
    BALANCED,
    POSITIONAL,
    DOLLAR,
    QUOTED,
    COMPONENTS,
    FORM_VALUE,
    FUNCTIONS,
    JS_CALLS,
    QUERY_SHAPE,
    QUERY_PATH,
    MUTATION_SHAPE,
    MUTATION_BODY,
    DEFINED,
    REACHABLE,
    WIRED,
    LINKS,
    RENAME,
];

/// The form controls and their parts: an arity problem of one of these is the `forms` section's.
const FORM_COMPONENTS: &[&str] = &[
    "Form",
    "FormControl",
    "Input",
    "TextArea",
    "Select",
    "SelectItem",
    "CheckBox",
    "RadioGroup",
    "RadioItem",
    "Switch",
    "Slider",
    "DateTime",
    "DatePeriod",
    "Lookup",
    "Display",
];
/// The controls whose second argument is the `$` value.
const VALUE_CONTROLS: &[&str] = &[
    "Input", "TextArea", "Select", "CheckBox", "RadioGroup", "Switch", "Slider", "DateTime", "Lookup",
];

/// One problem of a program: its rule, what is wrong, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub rule: Rule,
    pub message: String,
    /// 1-based line of the program, 0 for none.
    pub line: usize,
    pub statement: Option<String>,
    /// Problems that keep the program from parsing at all (`openui_parse`) rather than from checking.
    pub parse: bool,
    pub did_you_mean: Vec<String>,
    /// The page's own source and code of a reported problem (`canvasProblems`), for the revalidation gate; empty for a
    /// problem the server's own checks found.
    pub source: String,
    pub code: String,
}

impl Problem {
    pub(crate) fn new(rule: Rule, message: impl Into<String>) -> Self {
        Self {
            rule,
            message: message.into(),
            line: 0,
            statement: None,
            parse: false,
            did_you_mean: Vec::new(),
            source: String::new(),
            code: String::new(),
        }
    }

    fn at(mut self, statement: &Statement) -> Self {
        self.line = statement.line;
        self.statement = statement.id.clone();
        self
    }

    pub(crate) fn parse_error(mut self) -> Self {
        self.parse = true;
        self
    }

    /// `openui-lang#query`.
    pub fn skill(&self) -> String {
        format!("{}#{}", self.rule.skill, self.rule.section)
    }

    /// The hash a reported problem is deduplicated by: its rule, its statement and its message.
    pub fn hash(&self) -> String {
        moqui_lib::json::canonical_sha256(&json!({
            "section": self.rule.section, "rule": self.rule.text, "statement": self.statement, "message": self.message,
        }))
    }
}

/// The error the model reads for `problems` of `program` (plan 7.4): the first problem's section to load and its rule
/// quoted, the line and its text, and the others after it. `patch` says to send the changed statements again rather
/// than the whole program.
pub fn error_json(problems: &[Problem], program: &str, patch: bool) -> String {
    let Some(first) = problems.first() else {
        return String::new();
    };
    let parse = problems.iter().any(|problem| problem.parse);
    let lines: Vec<&str> = program.lines().collect();
    let resend = if patch {
        "send the changed statements again"
    } else {
        "send the whole program again"
    };
    let place = if first.line > 0 {
        format!("Fix line {}", first.line)
    } else {
        "Fix it".to_string()
    };
    let mut error = crate::llm::tools::format::ToolError::new(
        if parse { "openui_parse" } else { "openui_invalid" },
        format!(
            "{} so nothing was shown: {} The rule is in skill {SKILL}, section {}; load it with skill name \"{}\" if it is not above. {place} and {resend}.",
            if parse { "The program did not parse," } else { "The program has a problem," },
            first.message,
            first.rule.section,
            first.skill()
        ),
    )
    .skill(first.skill())
    .rule(first.rule.text)
    .did_you_mean(first.did_you_mean.clone());
    if first.line > 0 {
        error = error.with("line", json!(first.line));
        if let Some(text) = lines.get(first.line - 1) {
            error = error.with(
                "snippet",
                json!(format!("{} | {}", first.line, crate::llm::tools::format::cut(text.trim_end(), 240))),
            );
        }
    }
    let also: Vec<Json> = problems
        .iter()
        .skip(1)
        .take(MAX_PROBLEMS - 1)
        .map(|problem| {
            let mut item = json!({"skill": problem.skill(), "rule": problem.rule.text, "problem": problem.message});
            if problem.line > 0 {
                item["line"] = json!(problem.line);
            }
            item
        })
        .collect();
    if !also.is_empty() {
        error = error.with("also", Json::Array(also));
    }
    error.to_text()
}

// ---- the engine: lang-core in QuickJS ----

/// The glue the bundle is driven by: the parser over the library, each statement's own tree (for the checks), the
/// statement merge and the evaluation of a Mutation's arguments against a posted state. Every answer is JSON text.
const GLUE: &str = r#"
globalThis.__openui = (function (O) {
  "use strict";
  var parser = null, components = [];
  // the components in the spec's order (JSON keeps it; the server's JSON does not): name, parameters, required ones
  function setSpec(text) {
    var spec = JSON.parse(text), defs = spec.$defs || {};
    parser = O.createParser(spec, "Stack");
    components = Object.keys(defs).map(function (name) {
      return { name: name, params: Object.keys(defs[name].properties || {}), required: defs[name].required || [] };
    });
  }
  // The statements as lang-core's merge splits them (a newline at depth 0 outside strings ends one), with the line each
  // starts on and whether its brackets close.
  function split(input) {
    var out = [], depth = 0, inStr = false, esc = false, start = 0, line = 1, startLine = 1, i, c;
    for (i = 0; i < input.length; i++) {
      c = input[i];
      if (c === "\n") line++;
      if (esc) { esc = false; continue; }
      if (c === "\\" && inStr) { esc = true; continue; }
      if (inStr) { if (c === inStr) inStr = false; continue; }
      if (c === '"' || c === "'") { inStr = c; continue; }
      if (c === "(" || c === "[" || c === "{") depth++;
      else if (c === ")" || c === "]" || c === "}") depth = Math.max(0, depth - 1);
      else if (c === "\n" && depth <= 0) { push(input.slice(start, i), startLine, 0, false); start = i + 1; startLine = line; }
    }
    push(input.slice(start), startLine, depth, inStr);
    function push(text, at, open, str) {
      var lead = text.length - text.replace(/^\s+/, "").length;
      at += (text.slice(0, lead).match(/\n/g) || []).length;
      var raw = text.trim();
      if (raw) out.push({ raw: raw, line: at, open: open > 0 || !!str });
    }
    return out;
  }
  function statements(src) {
    return split(src).map(function (s) {
      var id = null, ast = null, error = null;
      try {
        var st = O.split(O.tokenize(s.raw))[0];
        if (st) { id = st.id; ast = O.parseExpression(st.tokens); }
      } catch (e) { error = String((e && e.message) || e); }
      return { raw: s.raw, line: s.line, open: s.open, id: id, ast: ast, error: error };
    });
  }
  function parse(src) {
    var r;
    try { r = parser.parse(src); } catch (e) { return JSON.stringify({ exception: String((e && e.message) || e), statements: statements(src), components: components }); }
    return JSON.stringify({
      root: r.root ? r.root.typeName : null, meta: r.meta, state: r.stateDeclarations,
      queries: r.queryStatements.map(function (q) { return q.statementId; }),
      mutations: r.mutationStatements.map(function (m) { return m.statementId; }),
      statements: statements(src), builtins: Array.from(O.BUILTIN_NAMES), actions: Array.from(O.ACTION_NAMES),
      components: components
    });
  }
  function merge(existing, patch) { return O.mergeStatements(existing, patch, "root"); }
  function evalMutation(src, id, stateText) {
    var state = JSON.parse(stateText || "{}"), r = parser.parse(src);
    var m = r.mutationStatements.filter(function (x) { return x.statementId === id; })[0];
    if (!m) return JSON.stringify({ missing: true });
    var ctx = {
      getState: function (n) {
        var v = state[n];
        if (v === undefined) v = state[String(n).replace(/^\$/, "")];
        if (v && typeof v === "object" && !Array.isArray(v) && Object.prototype.hasOwnProperty.call(v, "value")) v = v.value;
        return v === undefined ? null : v;
      },
      resolveRef: function () { return null; }
    };
    return JSON.stringify({ tool: m.toolAST ? O.evaluate(m.toolAST, ctx) : null, args: m.argsAST ? O.evaluate(m.argsAST, ctx) : null });
  }
  return { setSpec: setSpec, parse: parse, merge: merge, evalMutation: evalMutation };
})(OpenUILang);
"#;

/// The texts the engine runs: the bundle and the library spec.
#[derive(Clone)]
pub struct Sources {
    pub bundle: Arc<str>,
    pub spec: Arc<str>,
}

struct Engine {
    key: u64,
    context: Context,
    // The context borrows the runtime; drop order frees the context first.
    #[allow(dead_code)]
    runtime: Runtime,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn key_of(sources: &Sources) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sources.bundle.hash(&mut hasher);
    sources.spec.hash(&mut hasher);
    hasher.finish()
}

fn engine_for(sources: &Sources) -> Result<Engine, Error> {
    let runtime = Runtime::new().map_err(|err| Error::new(err.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    runtime.set_interrupt_handler(Some(Box::new(|| {
        DEADLINE.with(Cell::get).is_some_and(|at| Instant::now() >= at)
    })));
    let context = Context::full(&runtime).map_err(|err| Error::new(err.to_string()))?;
    let spec = serde_json::to_string(&*sources.spec).map_err(|err| Error::new(err.to_string()))?;
    context.with(|ctx| {
        ctx.eval::<(), _>(&*sources.bundle)
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the OpenUI Lang bundle did not load: {err}")))?;
        ctx.eval::<(), _>(GLUE)
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the OpenUI Lang glue did not load: {err}")))?;
        ctx.eval::<(), _>(format!("__openui.setSpec({spec})"))
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the OpenUI library spec did not load: {err}")))
    })?;
    Ok(Engine {
        key: key_of(sources),
        context,
        runtime,
    })
}

/// Run `script` (an expression answering JSON text or text) in the thread's engine for `sources`.
fn run_script(sources: &Sources, script: &str) -> Result<String, Error> {
    let key = key_of(sources);
    ENGINE.with(|slot| {
        if slot.borrow().as_ref().is_none_or(|engine| engine.key != key) {
            *slot.borrow_mut() = Some(engine_for(sources)?);
        }
        let guard = slot.borrow();
        let engine = guard.as_ref().ok_or_else(|| Error::new("the OpenUI Lang engine did not start"))?;
        DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + TIME_LIMIT)));
        let out: Result<String, Error> = engine.context.with(|ctx| {
            ctx.eval(script)
                .catch(&ctx)
                .map_err(|err| Error::new(format!("OpenUI Lang failed: {err}")))
        });
        DEADLINE.with(|deadline| deadline.set(None));
        out
    })
}

/// The bundle and the spec as the server has them, read once per factory.
///
/// # Errors
///
/// When either cannot be read.
pub fn sources(factory: &ExecutionContextFactory) -> Result<Sources, Error> {
    struct Kept(std::sync::Mutex<Option<Sources>>);
    let kept = factory.extension(|_| Kept(std::sync::Mutex::new(None)));
    if let Some(found) = kept.0.lock().unwrap_or_else(|err| err.into_inner()).as_ref() {
        return Ok(found.clone());
    }
    let read = |location: &str, file: &str| -> Result<Arc<str>, Error> {
        let found = factory.resource().get(location).and_then(|resource| resource.read_text());
        #[cfg(test)]
        if found.is_err() {
            return Ok(std::fs::read_to_string(source_path(file)).map_err(|err| Error::new(err.to_string()))?.into());
        }
        #[cfg(not(test))]
        let _ = file;
        found
            .map(Into::into)
            .map_err(|err| Error::new(format!("{location} cannot be read: {err}")))
    };
    let made = Sources {
        bundle: read(BUNDLE_LOCATION, "libs/openui/lang-core.min.js")?,
        spec: read(SPEC_LOCATION, "js/openui-library.spec.json")?,
    };
    *kept.0.lock().unwrap_or_else(|err| err.into_inner()) = Some(made.clone());
    Ok(made)
}

/// A webroot file of the repository, for the tests.
#[cfg(test)]
pub(crate) fn source_path(file: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot")
        .join(file)
}

/// The sources of the repository, for the tests.
#[cfg(test)]
pub(crate) fn test_sources() -> Sources {
    let read = |file: &str| -> Arc<str> {
        std::fs::read_to_string(source_path(file))
            .unwrap_or_else(|err| panic!("{file}: {err}"))
            .into()
    };
    Sources {
        bundle: read("libs/openui/lang-core.min.js"),
        spec: read("js/openui-library.spec.json"),
    }
}

// ---- parsing ----

/// One statement as the checks read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Statement {
    pub raw: String,
    pub line: usize,
    /// Its brackets or a string do not close.
    pub open: bool,
    pub id: Option<String>,
    pub ast: Json,
    pub error: Option<String>,
}

/// What lang-core made of a program.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub exception: Option<String>,
    /// The root component's name, when there is a root.
    pub root: Option<String>,
    pub incomplete: bool,
    pub unresolved: Vec<String>,
    pub orphaned: Vec<String>,
    /// lang-core's validation errors: `{code, component, path, message, statementId}`.
    pub errors: Vec<Json>,
    /// `$name` → its default.
    pub state: BTreeMap<String, Json>,
    pub queries: Vec<String>,
    pub mutations: Vec<String>,
    pub statements: Vec<Statement>,
    pub builtins: BTreeSet<String>,
    pub actions: BTreeSet<String>,
    /// The library's components: name → (parameter names in order, required names).
    pub components: Components,
}

/// The components of the library spec: name → (parameter names in argument order, required names).
pub type Components = BTreeMap<String, (Vec<String>, BTreeSet<String>)>;

fn strings(value: &Json) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_string)
        .collect()
}

/// Parse `program` with lang-core.
///
/// # Errors
///
/// When the engine cannot run.
pub fn parse(sources: &Sources, program: &str) -> Result<Parsed, Error> {
    let input = serde_json::to_string(program).map_err(|err| Error::new(err.to_string()))?;
    let text = run_script(sources, &format!("__openui.parse({input})"))?;
    let value: Json = serde_json::from_str(&text).map_err(|err| Error::new(err.to_string()))?;
    let meta = &value["meta"];
    Ok(Parsed {
        exception: value["exception"].as_str().map(str::to_string),
        root: value["root"].as_str().map(str::to_string),
        incomplete: meta["incomplete"].as_bool().unwrap_or(false),
        unresolved: strings(&meta["unresolved"]),
        orphaned: strings(&meta["orphaned"]),
        errors: meta["errors"].as_array().cloned().unwrap_or_default(),
        state: value["state"]
            .as_object()
            .map(|map| map.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
            .unwrap_or_default(),
        queries: strings(&value["queries"]),
        mutations: strings(&value["mutations"]),
        statements: value["statements"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| Statement {
                raw: item["raw"].as_str().unwrap_or_default().to_string(),
                line: item["line"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0),
                open: item["open"].as_bool().unwrap_or(false),
                id: item["id"].as_str().map(str::to_string),
                ast: item["ast"].clone(),
                error: item["error"].as_str().map(str::to_string),
            })
            .collect(),
        builtins: strings(&value["builtins"]).into_iter().collect(),
        actions: strings(&value["actions"]).into_iter().collect(),
        components: value["components"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some((
                    item["name"].as_str()?.to_string(),
                    (strings(&item["params"]), strings(&item["required"]).into_iter().collect()),
                ))
            })
            .collect(),
    })
}

/// `patch` merged onto `existing` by statement name with lang-core (a `name = null` removes one; what root no longer
/// reaches is dropped).
///
/// # Errors
///
/// When the engine cannot run.
pub fn merge(sources: &Sources, existing: &str, patch: &str) -> Result<String, Error> {
    let existing = serde_json::to_string(existing).map_err(|err| Error::new(err.to_string()))?;
    let patch = serde_json::to_string(patch).map_err(|err| Error::new(err.to_string()))?;
    run_script(sources, &format!("__openui.merge({existing}, {patch})"))
}

/// The tool and the arguments of the Mutation `statement` of `program`, evaluated with `state` (the canvas's
/// `$variables` as the user's page has them); `None` when the program has no such Mutation.
///
/// # Errors
///
/// When the engine cannot run.
pub fn evaluate_mutation(
    sources: &Sources,
    program: &str,
    statement: &str,
    state: &Json,
) -> Result<Option<(String, Json)>, Error> {
    let input = serde_json::to_string(program).map_err(|err| Error::new(err.to_string()))?;
    let id = serde_json::to_string(statement).map_err(|err| Error::new(err.to_string()))?;
    let state = serde_json::to_string(&state.to_string()).map_err(|err| Error::new(err.to_string()))?;
    let text = run_script(sources, &format!("__openui.evalMutation({input}, {id}, {state})"))?;
    let value: Json = serde_json::from_str(&text).map_err(|err| Error::new(err.to_string()))?;
    if value["missing"].as_bool() == Some(true) {
        return Ok(None);
    }
    Ok(Some((
        value["tool"].as_str().unwrap_or_default().to_string(),
        value["args"].clone(),
    )))
}

/// A program as the model sent it, a code fence around it taken off.
pub fn unfenced(program: &str) -> &str {
    let trimmed = program.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let body = rest.split_once('\n').map_or("", |(_, body)| body);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
}

// ---- the checks ----

/// What the checks may ask of the server: whether a path is a screen the user may open and, for a Query, whether it
/// has a list; `None` when it cannot tell (no factory).
pub trait Paths {
    /// `Err(why)` when `path` is not a screen.
    fn screen(&self, path: &str) -> Option<Result<(), String>>;
}

/// No server checks: only the program itself is checked.
pub struct NoPaths;

impl Paths for NoPaths {
    fn screen(&self, _path: &str) -> Option<Result<(), String>> {
        None
    }
}

/// The signature of a component as the skill writes it: `Select(name, value?, items, label?)`.
pub fn signature(components: &Components, component: &str) -> Option<String> {
    let (params, required) = components.get(component)?;
    let list: Vec<String> = params
        .iter()
        .map(|param| {
            if required.contains(param) {
                param.clone()
            } else {
                format!("{param}?")
            }
        })
        .collect();
    Some(format!("{component}({})", list.join(", ")))
}

/// Visit every node of `ast`.
fn walk<'a>(ast: &'a Json, visit: &mut dyn FnMut(&'a Json)) {
    if !ast.is_object() {
        return;
    }
    visit(ast);
    for key in ["args", "els"] {
        for child in ast[key].as_array().into_iter().flatten() {
            walk(child, visit);
        }
    }
    for entry in ast["entries"].as_array().into_iter().flatten() {
        if let Some(value) = entry.get(1) {
            walk(value, visit);
        }
    }
    for key in ["left", "right", "operand", "cond", "then", "else", "obj", "index", "value"] {
        if let Some(child) = ast.get(key) {
            walk(child, visit);
        }
    }
    if let Some(props) = ast["mappedProps"].as_object() {
        for child in props.values() {
            walk(child, visit);
        }
    }
}

fn kind(ast: &Json) -> &str {
    ast["k"].as_str().unwrap_or_default()
}

fn comp_name(ast: &Json) -> &str {
    if kind(ast) == "Comp" {
        ast["name"].as_str().unwrap_or_default()
    } else {
        ""
    }
}

/// The value of key `name` of an object node.
fn entry<'a>(ast: &'a Json, name: &str) -> Option<&'a Json> {
    ast["entries"]
        .as_array()?
        .iter()
        .find(|pair| pair.get(0).and_then(Json::as_str) == Some(name))
        .and_then(|pair| pair.get(1))
}

/// The names `ast` refers to: statement names (`Ref`, `RuntimeRef`) and `$` names (`StateRef`).
fn refs(ast: &Json) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut names = BTreeSet::new();
    let mut states = BTreeSet::new();
    walk(ast, &mut |node| match kind(node) {
        "Ref" | "RuntimeRef" => {
            if let Some(name) = node["n"].as_str() {
                names.insert(name.to_string());
            }
        }
        "StateRef" => {
            if let Some(name) = node["n"].as_str() {
                states.insert(name.to_string());
            }
        }
        _ => {}
    });
    (names, states)
}

/// The tokens of a statement's text outside its strings: what a scan for `@Name`, named arguments and JavaScript
/// calls reads. Strings are kept as one token each.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    At(String),
    Str(String),
    Single,
    Punct(char),
}

fn tokens(raw: &str) -> Vec<Token> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let ch = chars[at];
        if ch == '"' {
            let mut text = String::new();
            at += 1;
            while at < chars.len() && chars[at] != '"' {
                if chars[at] == '\\' && at + 1 < chars.len() {
                    text.push(chars[at + 1]);
                    at += 2;
                    continue;
                }
                text.push(chars[at]);
                at += 1;
            }
            at += 1;
            out.push(Token::Str(text));
        } else if ch == '\'' {
            out.push(Token::Single);
            at += 1;
            while at < chars.len() && chars[at] != '\'' {
                at += 1;
            }
            at += 1;
        } else if ch == '@' || ch == '$' || ch.is_alphabetic() || ch == '_' {
            let start = at;
            at += 1;
            while at < chars.len() && (chars[at].is_alphanumeric() || chars[at] == '_') {
                at += 1;
            }
            let word: String = chars[start..at].iter().collect();
            if let Some(name) = word.strip_prefix('@') {
                out.push(Token::At(name.to_string()));
            } else {
                out.push(Token::Ident(word));
            }
        } else if ch.is_whitespace() {
            at += 1;
        } else {
            out.push(Token::Punct(ch));
            at += 1;
        }
    }
    out
}

/// The argument names written as `name:` directly in a call's parentheses (not in an object, not a ternary's).
fn named_arguments(raw: &str) -> Vec<String> {
    let list = tokens(raw);
    // the open brackets, each with whether a `?` of a ternary waits for its `:`
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut ternaries = 0_usize;
    let mut out = Vec::new();
    for (index, token) in list.iter().enumerate() {
        match token {
            Token::Punct(open @ ('(' | '[' | '{')) => stack.push((*open, ternaries)),
            Token::Punct(')' | ']' | '}') => {
                if let Some((_, before)) = stack.pop() {
                    ternaries = before;
                }
            }
            Token::Punct('?') => ternaries += 1,
            Token::Punct(':') => {
                if ternaries > stack.last().map_or(0, |(_, before)| *before) {
                    ternaries -= 1;
                } else if stack.last().is_some_and(|(open, _)| *open == '(') {
                    if let Some(Token::Ident(name)) = index.checked_sub(1).and_then(|at| list.get(at)) {
                        out.push(name.clone());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A path a Query, a Mutation or a Link names, when it is written as a literal.
fn literal_path(options: &Json) -> Option<String> {
    let path = entry(options, "path")?;
    (kind(path) == "Str").then(|| path["v"].as_str().unwrap_or_default().to_string())
}

/// Whether `path` is a plain path from the root of the apps: `/apps/…` or `/qapps/…` with no scheme, host, `..`,
/// `//`, backslash or control character.
fn plain_app_path(path: &str) -> bool {
    let bare = path.split(['?', '#']).next().unwrap_or_default();
    (bare == "/apps" || bare.starts_with("/apps/") || bare == "/qapps" || bare.starts_with("/qapps/"))
        && !bare.contains("//")
        && !bare.split('/').any(|part| part == ".." || part == ".")
        && !bare.contains('\\')
        && !bare.chars().any(char::is_control)
}

/// The problems of a parsed program, in the order the model should fix them. `previous` is the program a patch was
/// merged onto (renames are checked against it), `paths` the server's screen checks.
pub fn check(parsed: &Parsed, previous: Option<&str>, patch: Option<&str>, paths: &dyn Paths) -> Vec<Problem> {
    let mut problems: Vec<Problem> = Vec::new();
    let components = &parsed.components;
    if let Some(exception) = &parsed.exception {
        problems.push(Problem::new(STATEMENT, format!("lang-core could not read it ({exception}).")).parse_error());
    }
    let by_id: BTreeMap<&str, &Statement> = parsed
        .statements
        .iter()
        .filter_map(|statement| statement.id.as_deref().map(|id| (id, statement)))
        .collect();
    // statements that do not read
    for statement in &parsed.statements {
        if statement.open {
            let query = statement.raw.contains("Query(") || statement.raw.contains("Mutation(");
            let mut problem = if query {
                Problem::new(QUERY_SHAPE, "a bracket of this Query or Mutation is not closed, so it swallows the lines after it.")
            } else {
                Problem::new(BALANCED, "a bracket or a string of this statement is not closed, so it swallows the lines after it.")
            };
            problem = problem.at(statement).parse_error();
            problems.push(problem);
        } else if statement.id.is_none() || statement.error.is_some() {
            let first = statement.raw.lines().next().unwrap_or_default();
            problems.push(
                Problem::new(
                    STATEMENT,
                    match &statement.error {
                        Some(error) => format!("\"{}\" did not read: {error}.", crate::llm::tools::format::cut(first, 60)),
                        None => format!("\"{}\" is not a statement name = expression.", crate::llm::tools::format::cut(first, 60)),
                    },
                )
                .at(statement)
                .parse_error(),
            );
        }
    }
    if parsed.root.is_none() && !by_id.contains_key("root") && problems.is_empty() {
        problems.push(Problem::new(STATEMENT, "there is no root = Stack([...]) statement.").parse_error());
    }
    let mut named: BTreeSet<String> = BTreeSet::new();
    for statement in &parsed.statements {
        let list = tokens(&statement.raw);
        // named arguments
        for name in named_arguments(&statement.raw) {
            named.insert(name.clone());
            problems.push(
                Problem::new(POSITIONAL, format!("{name}: is a named argument, which lang-core reads as a name that is not defined."))
                    .at(statement)
                    .parse_error(),
            );
        }
        // single quotes and quoted expressions
        if list.contains(&Token::Single) {
            problems.push(Problem::new(QUOTED, "a string is in single quotes.").at(statement).parse_error());
        }
        for token in &list {
            if let Token::Str(text) = token {
                let trimmed = text.trim_start();
                if trimmed.starts_with('@') && trimmed.contains('(') {
                    problems.push(
                        Problem::new(QUOTED, format!("\"{}\" is an expression in quotes, shown as text.", crate::llm::tools::format::cut(text, 40)))
                            .at(statement),
                    );
                }
            }
        }
        // invented functions and JavaScript calls
        for (index, token) in list.iter().enumerate() {
            match token {
                Token::At(name) if !parsed.builtins.contains(name) && !parsed.actions.contains(name) => {
                    let known: Vec<&str> = parsed.builtins.iter().map(String::as_str).collect();
                    let mut problem = Problem::new(FUNCTIONS, format!("@{name} is not a function of OpenUI Lang.")).at(statement);
                    problem.did_you_mean = crate::llm::tools::suggest::nearest(name, known.iter().copied(), 3)
                        .into_iter()
                        .map(|near| format!("@{near}"))
                        .collect();
                    problems.push(problem);
                }
                Token::Ident(name)
                    if (name == "Number" || name == "String" || name == "parseInt" || name == "parseFloat")
                        && list.get(index + 1) == Some(&Token::Punct('('))
                        && !components.contains_key(name) =>
                {
                    problems.push(Problem::new(JS_CALLS, format!("{name}() is JavaScript, which a canvas does not run.")).at(statement));
                }
                Token::Punct('.')
                    if matches!(list.get(index + 1), Some(Token::Ident(method)) if ["toFixed", "toString", "toLowerCase", "toUpperCase", "map", "filter", "join"].contains(&method.as_str()))
                        && list.get(index + 2) == Some(&Token::Punct('(')) =>
                {
                    if let Some(Token::Ident(method)) = list.get(index + 1) {
                        problems.push(Problem::new(JS_CALLS, format!(".{method}() is JavaScript, which a canvas does not run.")).at(statement));
                    }
                }
                _ => {}
            }
        }
        // a $ that holds a Query, a Mutation or a computed value
        if let Some(id) = &statement.id {
            if id.starts_with('$') {
                let value = &statement.ast;
                if matches!(kind(value), "Comp" | "BinOp" | "Ternary" | "Member" | "Index" | "Ref") {
                    let what = match comp_name(value) {
                        "Query" => "a Query".to_string(),
                        "Mutation" => "a Mutation".to_string(),
                        "" => "an expression".to_string(),
                        other if parsed.builtins.contains(other) => format!("@{other}(...)"),
                        other => format!("{other}(...)"),
                    };
                    problems.push(
                        Problem::new(DOLLAR, format!("{id} holds {what}; a $name is stored as written and shows nothing.")).at(statement),
                    );
                }
            }
        }
    }
    // lang-core's validation: unknown components and arguments that do not fit
    for error in &parsed.errors {
        let code = error["code"].as_str().unwrap_or_default();
        let component = error["component"].as_str().unwrap_or_default();
        let message = error["message"].as_str().unwrap_or_default();
        let statement = error["statementId"].as_str().and_then(|id| by_id.get(id).copied());
        let mut problem = match code {
            "unknown-component" => {
                let mut problem = Problem::new(COMPONENTS, format!("{component} is not a component of the canvas."));
                problem.did_you_mean =
                    crate::llm::tools::suggest::nearest(component, components.keys().map(String::as_str), 3);
                problem
            }
            "inline-reserved" => Problem::new(QUERY_SHAPE, format!("{component} is written inside another call; give it a statement of its own.")),
            _ => {
                let rule = if FORM_COMPONENTS.contains(&component) { FORM_VALUE } else { COMPONENTS };
                let short = message.split(" — ").next().unwrap_or(message);
                let signature = signature(components, component).unwrap_or_default();
                Problem::new(rule, format!("{short} (the signature is {signature}).").replace(" ().", "."))
            }
        };
        if let Some(statement) = statement {
            problem = problem.at(statement);
        }
        problems.push(problem);
    }
    // Query and Mutation shapes, and their paths
    let mut query_states: BTreeSet<String> = BTreeSet::new();
    for statement in &parsed.statements {
        let value = &statement.ast;
        let name = comp_name(value);
        if name == "Query" || name == "Mutation" {
            let args = value["args"].as_array().cloned().unwrap_or_default();
            let tool_ok = args.first().is_some_and(|tool| kind(tool) == "Str" && tool["v"].as_str() == Some("request"));
            let options = args.get(1).cloned().unwrap_or(Json::Null);
            let rule = if name == "Query" { QUERY_SHAPE } else { MUTATION_SHAPE };
            if !tool_ok {
                problems.push(Problem::new(rule, format!("the first argument of {name} is \"request\", the only tool.")).at(statement));
                continue;
            }
            if kind(&options) != "Obj" {
                problems.push(Problem::new(rule, format!("the second argument of {name} is its options object, {{path, ...}}.")).at(statement));
                continue;
            }
            // only the options the read proxy and the server's Mutation take; no key the page would read as its own
            let allowed: &[&str] = if name == "Query" { &["path", "list", "query", "method"] } else { &["method", "path", "body"] };
            for pair in options["entries"].as_array().into_iter().flatten() {
                let key = pair.get(0).and_then(Json::as_str).unwrap_or_default();
                if !allowed.contains(&key) {
                    problems.push(
                        Problem::new(rule, format!("{key} is not an option of a {name}; it takes {}.", allowed.join(", "))).at(statement),
                    );
                }
            }
            let mut reserved: Vec<String> = Vec::new();
            for arg in &args {
                walk(arg, &mut |node| {
                    for pair in node["entries"].as_array().into_iter().flatten() {
                        if let Some(key) = pair.get(0).and_then(Json::as_str).filter(|key| key.starts_with("__")) {
                            reserved.push(key.to_string());
                        }
                    }
                });
            }
            if let Some(key) = reserved.first() {
                problems.push(Problem::new(rule, format!("{key} is a reserved name; a {name} may not use it.")).at(statement));
                continue;
            }
            if name == "Query" {
                if args.len() < 3 {
                    problems.push(Problem::new(QUERY_SHAPE, "the Query has no default data, {rows: []}, after its options.").at(statement));
                }
                if let Some(query) = entry(&options, "query") {
                    query_states.extend(refs(query).1);
                }
                query_states.extend(refs(&options).1);
                let method = entry(&options, "method").and_then(|method| method["v"].as_str()).unwrap_or("GET");
                if !method.eq_ignore_ascii_case("GET") {
                    problems.push(Problem::new(MUTATION_SHAPE, format!("a Query reads with GET; {method} is a Mutation's.")).at(statement));
                }
            } else {
                let method = entry(&options, "method").and_then(|method| method["v"].as_str()).unwrap_or("POST");
                if method.eq_ignore_ascii_case("GET") {
                    problems.push(Problem::new(MUTATION_SHAPE, "a Mutation posts; a read is a Query.").at(statement));
                }
                if let Some(body) = entry(&options, "body") {
                    let (names, _) = refs(body);
                    let read: Vec<&String> = names.iter().filter(|name| parsed.queries.contains(name)).collect();
                    if !read.is_empty() {
                        problems.push(
                            Problem::new(
                                MUTATION_BODY,
                                format!("the body reads the Query {}, which the canvas does not send.", read[0]),
                            )
                            .at(statement),
                        );
                    }
                }
            }
            match literal_path(&options) {
                None if entry(&options, "path").is_none() => {
                    problems.push(Problem::new(if name == "Query" { QUERY_PATH } else { MUTATION_SHAPE }, format!("the {name} has no path.")).at(statement));
                }
                Some(path) if !plain_app_path(&path) || path.starts_with("/qapps") => {
                    problems.push(
                        Problem::new(
                            if name == "Query" { QUERY_PATH } else { MUTATION_SHAPE },
                            format!("{path} is not an /apps path of this server."),
                        )
                        .at(statement),
                    );
                }
                Some(path) if name == "Query" => {
                    if let Some(Err(why)) = paths.screen(&path) {
                        problems.push(Problem::new(QUERY_PATH, format!("{path}: {why}.")).at(statement));
                    }
                }
                _ => {}
            }
        }
        // links
        walk(value, &mut |node| {
            if comp_name(node) != "Link" {
                return;
            }
            let Some(path) = node["args"].as_array().and_then(|args| args.get(1)) else {
                return;
            };
            if kind(path) != "Str" {
                return;
            }
            let path = path["v"].as_str().unwrap_or_default();
            if !plain_app_path(path) {
                problems.push(Problem::new(LINKS, format!("{path} is not a screen path of the apps.")).at(statement));
            } else if let Some(Err(why)) = paths.screen(path) {
                problems.push(Problem::new(LINKS, format!("{path}: {why}.")).at(statement));
            }
        });
    }
    // undefined and unreachable names
    for name in &parsed.unresolved {
        if named.contains(name) {
            continue;
        }
        let statement = parsed.statements.iter().find(|statement| {
            refs(&statement.ast).0.contains(name)
        });
        let mut problem = Problem::new(DEFINED, format!("{name} is used but not defined."));
        problem.did_you_mean = crate::llm::tools::suggest::nearest(name, by_id.keys().copied(), 3);
        if let Some(statement) = statement {
            problem = problem.at(statement);
        }
        problems.push(problem);
    }
    for name in &parsed.orphaned {
        let mut problem = Problem::new(REACHABLE, format!("{name} is defined but root does not reach it, so it is not shown."));
        if let Some(statement) = by_id.get(name.as_str()) {
            problem = problem.at(statement);
        }
        problems.push(problem);
    }
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut state_used: BTreeMap<String, usize> = BTreeMap::new();
    for statement in &parsed.statements {
        let (names, states) = refs(&statement.ast);
        referenced.extend(names.into_iter().filter(|name| Some(name) != statement.id.as_ref()));
        for state in states {
            *state_used.entry(state).or_insert(0) += 1;
        }
    }
    for query in &parsed.queries {
        if !query.starts_with('$') && !referenced.contains(query) {
            let mut problem = Problem::new(REACHABLE, format!("the Query {query} is shown by no component."));
            if let Some(statement) = by_id.get(query.as_str()) {
                problem = problem.at(statement);
            }
            problems.push(problem);
        }
    }
    // a visible filter no Query reads (a control in a Form posts with the form instead)
    if !parsed.queries.is_empty() {
        let in_forms = statements_in_forms(parsed, &by_id);
        for statement in &parsed.statements {
            if statement.id.as_deref().is_some_and(|id| in_forms.contains(id)) {
                continue;
            }
            walk(&statement.ast, &mut |node| {
                if !VALUE_CONTROLS.contains(&comp_name(node)) {
                    return;
                }
                let Some(value) = node["args"].as_array().and_then(|args| args.get(1)) else {
                    return;
                };
                if kind(value) != "StateRef" {
                    return;
                }
                let name = value["n"].as_str().unwrap_or_default();
                let used_elsewhere = state_used.get(name).copied().unwrap_or(0) > 1;
                if !query_states.contains(name) && !used_elsewhere {
                    problems.push(
                        Problem::new(WIRED, format!("the filter {name} is shown, but no Query reads it, so it changes nothing."))
                            .at(statement),
                    );
                }
            });
        }
    }
    // a patch that renames: a new name in the patch while a statement of the same component was dropped
    if let (Some(previous), Some(patch)) = (previous, patch) {
        let before: BTreeMap<String, String> = statement_components(previous);
        let after: BTreeMap<String, String> =
            parsed.statements.iter().filter_map(|statement| Some((statement.id.clone()?, comp_name(&statement.ast).to_string()))).collect();
        let patched: BTreeMap<String, String> = statement_components(patch);
        for (name, component) in &patched {
            if before.contains_key(name) || component.is_empty() {
                continue;
            }
            let dropped = before
                .iter()
                .find(|(old, old_component)| !after.contains_key(*old) && *old_component == component && !patched.contains_key(*old));
            if let Some((old, _)) = dropped {
                let mut problem = Problem::new(RENAME, format!("{name} is new and {old}, a {component} too, was dropped: keep the name {old}."));
                if let Some(statement) = by_id.get(name.as_str()) {
                    problem = problem.at(statement);
                }
                problems.push(problem);
            }
        }
    }
    dedup(problems)
}

/// The statement names of `program` and the component each makes, read without lang-core's parser (a text that does
/// not parse has none).
fn statement_components(program: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in program.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$') {
            continue;
        }
        let value = value.trim();
        let component: String = value.chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_').collect();
        let component = if value[component.len()..].starts_with('(') { component } else { String::new() };
        out.insert(name.to_string(), component);
    }
    out
}

/// The statements a Form reaches (its controls post with the form).
fn statements_in_forms(parsed: &Parsed, by_id: &BTreeMap<&str, &Statement>) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = Vec::new();
    for statement in &parsed.statements {
        walk(&statement.ast, &mut |node| {
            if comp_name(node) == "Form" {
                queue.extend(refs(node).0);
                if let Some(id) = &statement.id {
                    // the Form's own statement holds its controls inline
                    if kind(&statement.ast) == "Comp" && comp_name(&statement.ast) == "Form" {
                        out.insert(id.clone());
                    }
                }
            }
        });
    }
    while let Some(name) = queue.pop() {
        if !out.insert(name.clone()) {
            continue;
        }
        if let Some(statement) = by_id.get(name.as_str()) {
            queue.extend(refs(&statement.ast).0);
        }
    }
    out
}

fn dedup(problems: Vec<Problem>) -> Vec<Problem> {
    let mut seen: BTreeSet<(String, usize, String)> = BTreeSet::new();
    let mut out: Vec<Problem> = Vec::new();
    for problem in problems {
        if seen.insert((problem.rule.text.to_string(), problem.line, problem.message.clone())) {
            out.push(problem);
        }
    }
    // a program that does not parse is reported for that alone: what follows from it (a name swallowed by an open
    // bracket is not defined, a Query it holds is not shown) would only send the model after the wrong line
    if out.iter().any(|problem| problem.parse) {
        out.retain(|problem| problem.parse);
    }
    out
}

/// A program that parsed and checked: its text, its statement names and the Mutations it has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Checked {
    pub program: String,
    pub statements: Vec<String>,
    pub queries: Vec<String>,
    pub mutations: Vec<String>,
}

/// Parse and check `program` (or `patch` merged onto `previous`): the program to show, or its problems.
///
/// # Errors
///
/// When the engine cannot run.
pub fn prepare(
    sources: &Sources,
    previous: Option<&str>,
    program: Option<&str>,
    patch: Option<&str>,
    paths: &dyn Paths,
) -> Result<Result<Checked, Vec<Problem>>, Error> {
    let (text, patch_text) = match (program, patch, previous) {
        (_, Some(patch), Some(previous)) => {
            let patch = unfenced(patch);
            // the patch alone must read before it is merged
            let alone = parse(sources, patch)?;
            let unreadable: Vec<Problem> = check(&alone, None, None, &NoPaths)
                .into_iter()
                .filter(|problem| problem.parse)
                .collect();
            if !unreadable.is_empty() {
                return Ok(Err(unreadable));
            }
            (merge(sources, previous, patch)?, Some(patch))
        }
        (Some(program), _, _) => (unfenced(program).to_string(), None),
        (None, Some(patch), None) => (unfenced(patch).to_string(), None),
        (None, None, _) => return Ok(Err(vec![Problem::new(STATEMENT, "there is no program.").parse_error()])),
    };
    let parsed = parse(sources, &text)?;
    let problems = check(&parsed, previous.filter(|_| patch_text.is_some()), patch_text, paths);
    if !problems.is_empty() {
        return Ok(Err(problems));
    }
    Ok(Ok(Checked {
        statements: parsed.statements.iter().filter_map(|statement| statement.id.clone()).collect(),
        queries: parsed.queries.clone(),
        mutations: parsed.mutations.clone(),
        program: text,
    }))
}

/// A problem the drawer reported after it drew a canvas (`canvasProblems`), mapped to its section and rule as the
/// server's own checks map theirs: lang-core's codes, a name it could not resolve, an evaluation that failed, a Query or
/// a Mutation that the read proxy or the server refused.
pub fn reported(source: &str, code: &str, message: &str, statement: Option<&str>, component: Option<&str>) -> Problem {
    let rule = match (source, code) {
        (_, "unknown-component") => COMPONENTS,
        (_, "missing-required" | "null-required" | "excess-args" | "type-mismatch") => {
            if component.is_some_and(|component| FORM_COMPONENTS.contains(&component)) {
                FORM_VALUE
            } else {
                COMPONENTS
            }
        }
        (_, "inline-reserved") => QUERY_SHAPE,
        (_, "unresolved") => DEFINED,
        (_, "no-root" | "parse-exception" | "parse-failed") => STATEMENT,
        ("mutation", _) => MUTATION_SHAPE,
        ("query", _) | (_, "tool-error" | "tool-not-found") => QUERY_PATH,
        _ => FUNCTIONS,
    };
    let mut problem = Problem::new(rule, crate::llm::tools::format::cut(message.trim(), 300));
    problem.statement = statement.map(str::to_string);
    problem.source = source.to_string();
    problem.code = code.to_string();
    problem
}

/// The OpenUI Lang dialect: [`prepare`] over the server's bundle and library spec.
#[derive(Default)]
pub struct OpenUi;

impl super::Dialect for OpenUi {
    fn name(&self) -> &'static str {
        "openui"
    }

    fn label(&self) -> &'static str {
        "OpenUI Lang"
    }

    fn skill(&self) -> &'static str {
        SKILL
    }

    fn prepare(
        &self,
        factory: &ExecutionContextFactory,
        previous: Option<&str>,
        input: &super::WriteInput,
        paths: &dyn Paths,
    ) -> Result<super::Prepared, super::Refused> {
        let too_long = [input.program.as_deref(), input.patch.as_deref()]
            .into_iter()
            .flatten()
            .any(|text| text.chars().count() > MAX_PROGRAM_CHARS);
        if too_long {
            let problem = Problem::new(STATEMENT, format!("the program is longer than {MAX_PROGRAM_CHARS} characters.")).parse_error();
            return Err(refused(&[problem], input));
        }
        let outcome = sources(factory).and_then(|sources| {
            prepare(&sources, previous, input.program.as_deref(), input.patch.as_deref(), paths)
        });
        match outcome {
            Ok(Ok(checked)) => Ok(super::Prepared {
                program: checked.program,
                statements: checked.statements,
                mutations: checked.mutations,
                reads: Vec::new(),
            }),
            Ok(Err(problems)) => Err(refused(&problems, input)),
            Err(error) => {
                tracing::error!("OpenUI Lang could not check a program: {error}");
                Err(super::Refused {
                    error: crate::llm::tools::format::ToolError::new(
                        "unavailable",
                        "The canvas cannot be checked on this server now, so nothing was shown. Answer in text.",
                    )
                    .to_text(),
                    problems: Vec::new(),
                })
            }
        }
    }
}

fn refused(problems: &[Problem], input: &super::WriteInput) -> super::Refused {
    let program = if input.patch.is_some() && input.program.is_none() {
        input.patch.as_deref().unwrap_or_default()
    } else {
        input.program.as_deref().unwrap_or_default()
    };
    super::Refused {
        error: error_json(problems, unfenced(program), input.patch.is_some() && input.program.is_none()),
        problems: problems.iter().map(problem_json).collect(),
    }
}

/// A problem as the events and the canvas keep it.
pub fn problem_json(problem: &Problem) -> Json {
    json!({"hash": problem.hash(), "skill": problem.skill(), "rule": problem.rule.text, "problem": problem.message,
        "line": problem.line, "statement": problem.statement})
}

crate::submit_dialect!(OpenUi);

#[cfg(test)]
#[path = "openui_tests.rs"]
mod tests;
