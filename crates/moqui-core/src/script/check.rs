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
//! - the names QuickJS has on its global object that quickjs-oxide does not ([`QUICKJS_ONLY_GLOBALS`]).
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
    "context",
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
        Analysis {
            refused: walk.refused,
            ec_paths: walk.ec_paths,
            writes: walk.writes,
        }
    })
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
            Expression::Identifier(identifier) => self.name(identifier.name.as_str()),
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
                self.expression(&binary.right);
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
                    self.refuse("delete");
                    return;
                }
                self.expression(&unary.argument);
            }
            Expression::SequenceExpression(sequence) => {
                for item in &sequence.expressions {
                    self.expression(item);
                }
            }
            Expression::ParenthesizedExpression(inner) => self.expression(&inner.expression),
            Expression::CallExpression(call) => {
                let arrows = self.callee(&call.callee);
                self.arguments(&call.arguments, arrows);
            }
            Expression::NewExpression(new) => {
                match &new.callee {
                    Expression::Identifier(identifier) => self.name(identifier.name.as_str()),
                    _ => self.refuse("new of anything but a name"),
                }
                self.arguments(&new.arguments, false);
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    let arrows = self.callee(&call.callee);
                    self.arguments(&call.arguments, arrows);
                }
                ChainElement::TSNonNullExpression(_) => self.refuse("TypeScript"),
                other => match other.as_member_expression() {
                    Some(member) => self.member(member),
                    None => self.refuse("a chain"),
                },
            },
            Expression::StaticMemberExpression(member) => {
                self.static_member(&member.object, member.property.name.as_str());
            }
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
                self.expression(&member.object);
                self.expression(&member.expression);
            }
            Expression::PrivateFieldExpression(_) => self.refuse("a private field"),
            Expression::ArrowFunctionExpression(_) => {
                self.refuse("an arrow function that is not a call's argument");
            }
            Expression::AssignmentExpression(assignment) if self.actions => {
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
        // the function's variables, wherever declared in it, are its own and not context names
        self.locals.push(HashSet::new());
        for statement in &body.statements {
            self.declared(statement, true);
        }
        for statement in &body.statements {
            self.statement(statement);
        }
        self.locals.pop();
    }

    /// Note the names `statement` declares into the current scope: `var` at any depth of blocks, ifs and loops (it is
    /// the function's), `let` and `const` only when `top` (they belong to the block that holds them).
    fn declared(&mut self, statement: &'a Statement<'a>, top: bool) {
        let add = |declaration: &'a oxc_ast::ast::VariableDeclaration<'a>, locals: &mut Vec<HashSet<&'a str>>| {
            for declarator in &declaration.declarations {
                if let BindingPattern::BindingIdentifier(name) = &declarator.id {
                    if let Some(scope) = locals.last_mut() {
                        scope.insert(name.name.as_str());
                    }
                }
            }
        };
        let hoisted = |declaration: &oxc_ast::ast::VariableDeclaration<'_>| {
            top || declaration.kind == oxc_ast::ast::VariableDeclarationKind::Var
        };
        match statement {
            Statement::VariableDeclaration(declaration) if hoisted(declaration) => add(declaration, &mut self.locals),
            Statement::BlockStatement(block) => {
                for inner in &block.body {
                    self.declared(inner, false);
                }
            }
            Statement::IfStatement(branch) => {
                self.declared(&branch.consequent, false);
                if let Some(alternate) = &branch.alternate {
                    self.declared(alternate, false);
                }
            }
            Statement::ForStatement(each) => {
                if let Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(declaration)) = &each.init {
                    if declaration.kind == oxc_ast::ast::VariableDeclarationKind::Var {
                        add(declaration, &mut self.locals);
                    }
                }
                self.declared(&each.body, false);
            }
            Statement::WhileStatement(each) => self.declared(&each.body, false),
            _ => {}
        }
    }

    /// The `let` and `const` names `statements` declare directly: a block's own scope.
    fn block_scope(&mut self, statements: &'a [Statement<'a>]) -> HashSet<&'a str> {
        let mut names = HashSet::new();
        for statement in statements {
            if let Statement::VariableDeclaration(declaration) = statement {
                if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Var {
                    for declarator in &declaration.declarations {
                        if let BindingPattern::BindingIdentifier(name) = &declarator.id {
                            names.insert(name.name.as_str());
                        }
                    }
                }
            }
        }
        names
    }

    /// Refuse a declaration (a variable, an arrow's parameter) that would shadow `ec`, a refused name or a QuickJS
    /// global: a later read of it would not be what the check took it for.
    fn binding(&mut self, name: &str) {
        if name == "ec" || REFUSED_NAMES.contains(&name) || QUICKJS_ONLY_GLOBALS.contains(&name) {
            self.refuse(format!("a declaration of {name}"));
        }
    }

    /// A statement of an actions function: declarations, expressions (assignments among them), `if`, `for`, `while`,
    /// blocks, `return` and `throw`. Anything else (a function or class declaration, `try`, `switch`, `for...in` or
    /// `for...of`, labels, `with`) is refused.
    fn statement(&mut self, statement: &'a Statement<'a>) {
        if self.refused.is_some() {
            return;
        }
        match statement {
            Statement::VariableDeclaration(declaration) => self.declaration(declaration),
            Statement::ExpressionStatement(statement) => self.expression(&statement.expression),
            Statement::IfStatement(branch) => {
                self.expression(&branch.test);
                self.statement(&branch.consequent);
                if let Some(alternate) = &branch.alternate {
                    self.statement(alternate);
                }
            }
            Statement::ForStatement(each) => {
                // a `let` or `const` of the loop's start is the loop's own
                let mut scope = HashSet::new();
                if let Some(oxc_ast::ast::ForStatementInit::VariableDeclaration(declaration)) = &each.init {
                    if declaration.kind != oxc_ast::ast::VariableDeclarationKind::Var {
                        for declarator in &declaration.declarations {
                            if let BindingPattern::BindingIdentifier(name) = &declarator.id {
                                scope.insert(name.name.as_str());
                            }
                        }
                    }
                }
                self.locals.push(scope);
                self.for_statement(each);
                self.locals.pop();
            }
            Statement::WhileStatement(each) => {
                self.expression(&each.test);
                self.statement(&each.body);
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

    fn statement_rest(&mut self, statement: &'a Statement<'a>) {
        match statement {
            Statement::BlockStatement(block) => {
                let scope = self.block_scope(&block.body);
                self.locals.push(scope);
                for inner in &block.body {
                    self.statement(inner);
                }
                self.locals.pop();
            }
            Statement::ReturnStatement(statement) => {
                if let Some(argument) = &statement.argument {
                    self.expression(argument);
                }
            }
            Statement::ThrowStatement(statement) => self.expression(&statement.argument),
            Statement::EmptyStatement(_) => {}
            _ => self.refuse("a statement outside the actions profile"),
        }
    }

    fn declaration(&mut self, declaration: &'a oxc_ast::ast::VariableDeclaration<'a>) {
        for declarator in &declaration.declarations {
            let BindingPattern::BindingIdentifier(name) = &declarator.id else {
                self.refuse("a destructuring declaration");
                return;
            };
            self.binding(name.name.as_str());
            if let Some(init) = &declarator.init {
                self.expression(init);
            }
        }
    }

    fn name(&mut self, name: &'a str) {
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
    fn static_member(&mut self, object: &'a Expression<'a>, property: &str) {
        if !self.property_allowed(object, property) {
            return;
        }
        if let Some(path) = self.ec_path(object) {
            self.ec_paths.push(format!("{path}.{property}"));
            return;
        }
        if self.is_ec(object) {
            self.ec_paths.push(format!("ec.{property}"));
            return;
        }
        self.expression(object);
    }

    /// A member of an optional chain.
    fn member(&mut self, member: &'a MemberExpression<'a>) {
        match member {
            MemberExpression::StaticMemberExpression(member) => {
                self.static_member(&member.object, member.property.name.as_str());
            }
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
                self.expression(&member.object);
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

    fn arguments(&mut self, arguments: &'a [Argument<'a>], arrows: bool) {
        for argument in arguments {
            match argument {
                Argument::SpreadElement(spread) => self.expression(&spread.argument),
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

    /// An arrow function passed as an argument: plain parameters, an expression body.
    fn arrow(&mut self, arrow: &'a oxc_ast::ast::ArrowFunctionExpression<'a>) {
        if arrow.r#async {
            self.refuse("an async arrow function");
            return;
        }
        if arrow.params.rest.is_some() {
            self.refuse("a rest parameter");
            return;
        }
        let mut names = HashSet::new();
        for parameter in &arrow.params.items {
            match &parameter.pattern {
                BindingPattern::BindingIdentifier(name) if parameter.initializer.is_none() => {
                    self.binding(name.name.as_str());
                    names.insert(name.name.as_str());
                }
                _ => {
                    self.refuse("an arrow parameter that is not a plain name");
                    return;
                }
            }
        }
        let ArrowFunctionBody::FunctionBody(_) = &arrow.body else {
            self.locals.push(names);
            match arrow.body.as_expression() {
                Some(body) => self.expression(body),
                None => self.refuse("an arrow body"),
            }
            self.locals.pop();
            return;
        };
        self.refuse("an arrow function with a block body");
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
