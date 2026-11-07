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

//! JavaScript parse through `oxc_parser`. Groovy leftovers are rejected by the service loader.

use std::cell::RefCell;

use oxc_allocator::Allocator;
use oxc_ast::ast::{SourceType, Statement};
use oxc_parser::{ParseOptions, Parser};

use crate::context::Error;

/// Parse JavaScript. An expression or a program is accepted. `1 + 1` is accepted.
///
/// Groovy that is not also JavaScript fails the parse: Elvis `?:`, closures `{ x -> }`, map
/// literals `[a:1]`, and `as` casts. Optional chaining `?.` is JavaScript and is accepted.
pub fn parse_js(expr: &str) -> Result<(), Error> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok(());
    }
    if parse_expression(expr).is_ok() || parse_program(expr).is_ok() {
        return Ok(());
    }
    let shown = clip(expr);
    Err(Error::new(match groovy_hint(expr) {
        Some(kind) => format!("expression `{shown}` is Groovy ({kind})"),
        None => format!("expression `{shown}` is not JavaScript"),
    }))
}

/// An arena past this after a parse is let go rather than kept for the next.
const ARENA_KEEP: usize = 4 << 20;

thread_local! {
    /// The parser's arena for this thread, reset after each parse: a new one for each would allocate its first chunks
    /// every time, and the service loader checks every expression of every service.
    static ARENA: RefCell<Allocator> = RefCell::new(Allocator::default());
}

/// `body` with the thread's arena, empty. A parse inside another parse's body gets an arena of its own.
pub(crate) fn with_arena<R>(body: impl FnOnce(&Allocator) -> R) -> R {
    ARENA.with(|arena| match arena.try_borrow_mut() {
        Ok(mut arena) => {
            let out = body(&arena);
            if arena.capacity() > ARENA_KEEP {
                *arena = Allocator::default();
            } else {
                arena.reset();
            }
            out
        }
        Err(_) => body(&Allocator::default()),
    })
}

/// Parse a whole generated script. The error names the first problem the parser reports.
pub(crate) fn check_program(source: &str) -> Result<(), Error> {
    with_arena(|allocator| {
        let parsed = Parser::new(allocator, source, SourceType::default()).parse();
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            let detail = parsed
                .diagnostics
                .errors()
                .next()
                .map_or_else(|| "syntax error".to_string(), |diagnostic| diagnostic.to_string());
            return Err(Error::new(format!("generated script is not JavaScript: {detail}")));
        }
        Ok(())
    })
}

/// `Parser::parse_expression` stops after the first expression and ignores what follows, so
/// `a b` and `def x = 1` would pass. A parenthesized program must consume every token.
fn parse_expression(source: &str) -> Result<(), Error> {
    let wrapped = format!("({source}\n)");
    with_arena(|allocator| {
        let parsed = Parser::new(allocator, &wrapped, SourceType::default()).parse();
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            return Err(Error::new("not a JavaScript expression"));
        }
        Ok(())
    })
}

/// A `<script>` body runs inside the function the action macros emit, so a top-level `return`
/// ends the script, as it ended a Groovy script.
fn parse_program(source: &str) -> Result<(), Error> {
    let options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    with_arena(|allocator| {
        let parsed = Parser::new(allocator, source, SourceType::default())
            .with_options(options)
            .parse();
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            return Err(Error::new("not JavaScript"));
        }
        Ok(())
    })
}

/// Whether a script file, run as [`file_function`] makes it, gives a module whose value may be kept for the rest of
/// the outermost script run ([`crate::script::runtime::facades`], `ec.resource.script`): what it does at its top level
/// only declares, so running it again would make the same functions over the same constants. Each top-level statement
/// is a function declaration, a `const` of an inert value (a function, a literal, a table of literals, a `Set` or
/// `Map` of them, another module by `ec.resource.script("...")`, or names taken apart from one), or the value given
/// back last (an object of names and functions, a name, or a function). A loop, a call, a `let` or a `var` at the top
/// level (a memo for one run, rows read from the database) keeps the file out.
pub(crate) fn module_is_inert(source: &str) -> bool {
    module_not_inert(source).is_none()
}

/// What keeps a script file out of [`module_is_inert`]: the first statement that does, as text.
pub(crate) fn module_not_inert(source: &str) -> Option<String> {
    use std::collections::HashSet;

    use oxc_ast::ast::{
        ArrayExpressionElement, BindingPattern, Expression, ObjectPropertyKind, Statement, VariableDeclarationKind,
    };
    /// The names the module declares at its top level: its functions and its constants, destructured ones included.
    fn declared<'a>(body: &'a [Statement<'a>]) -> HashSet<&'a str> {
        fn bound<'a>(pattern: &'a BindingPattern<'a>, names: &mut HashSet<&'a str>) {
            match pattern {
                BindingPattern::BindingIdentifier(name) => {
                    names.insert(name.name.as_str());
                }
                BindingPattern::ObjectPattern(object) => {
                    for property in &object.properties {
                        bound(&property.value, names);
                    }
                    if let Some(rest) = &object.rest {
                        bound(&rest.argument, names);
                    }
                }
                BindingPattern::ArrayPattern(array) => {
                    for element in array.elements.iter().flatten() {
                        bound(element, names);
                    }
                    if let Some(rest) = &array.rest {
                        bound(&rest.argument, names);
                    }
                }
                BindingPattern::AssignmentPattern(assignment) => bound(&assignment.left, names),
            }
        }
        let mut names = HashSet::new();
        for statement in body {
            match statement {
                Statement::FunctionDeclaration(function) => {
                    if let Some(id) = &function.id {
                        names.insert(id.name.as_str());
                    }
                }
                Statement::VariableDeclaration(declaration) if declaration.kind == VariableDeclarationKind::Const => {
                    for declarator in &declaration.declarations {
                        bound(&declarator.id, &mut names);
                    }
                }
                _ => {}
            }
        }
        names
    }
    /// A value that is the same each time the module's top level runs: a function, a literal, a table of them, a
    /// `Set` or `Map` of them, another module by `ec.resource.script("...")`, the module's own names and what is read
    /// from them, and operators over all of these.
    fn inert(expression: &Expression<'_>, names: &HashSet<&str>, depth: usize) -> bool {
        if depth > 32 {
            return false;
        }
        let again = |value: &Expression<'_>| inert(value, names, depth + 1);
        match expression {
            Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_) => true,
            Expression::TemplateLiteral(template) => template.expressions.iter().all(again),
            Expression::Identifier(name) => name.name == "undefined" || names.contains(name.name.as_str()),
            Expression::ParenthesizedExpression(inner) => again(&inner.expression),
            Expression::UnaryExpression(unary) => again(&unary.argument),
            Expression::BinaryExpression(binary) => again(&binary.left) && again(&binary.right),
            Expression::LogicalExpression(logical) => again(&logical.left) && again(&logical.right),
            Expression::ConditionalExpression(choice) => {
                again(&choice.test) && again(&choice.consequent) && again(&choice.alternate)
            }
            Expression::StaticMemberExpression(member) => again(&member.object),
            Expression::ComputedMemberExpression(member) => again(&member.object) && again(&member.expression),
            Expression::ArrayExpression(array) => array.elements.iter().all(|element| match element {
                ArrayExpressionElement::SpreadElement(spread) => again(&spread.argument),
                ArrayExpressionElement::Elision(_) => true,
                other => other.as_expression().is_some_and(again),
            }),
            Expression::ObjectExpression(object) => object.properties.iter().all(|property| match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    (!property.computed || property.key.as_expression().is_some_and(again)) && again(&property.value)
                }
                ObjectPropertyKind::SpreadProperty(spread) => again(&spread.argument),
            }),
            Expression::NewExpression(new) => {
                matches!(&new.callee, Expression::Identifier(name) if name.name == "Set" || name.name == "Map")
                    && new
                        .arguments
                        .iter()
                        .all(|argument| argument.as_expression().is_some_and(again))
            }
            Expression::CallExpression(call) => module_load(call),
            _ => false,
        }
    }
    /// `ec.resource.script("component://...")`.
    fn module_load(call: &oxc_ast::ast::CallExpression<'_>) -> bool {
        let Expression::StaticMemberExpression(script) = &call.callee else {
            return false;
        };
        let Expression::StaticMemberExpression(resource) = &script.object else {
            return false;
        };
        script.property.name == "script"
            && resource.property.name == "resource"
            && matches!(&resource.object, Expression::Identifier(ec) if ec.name == "ec")
            && call.arguments.len() == 1
            && matches!(call.arguments[0].as_expression(), Some(Expression::StringLiteral(_)))
    }
    let options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    with_arena(|allocator| {
        let parsed = Parser::new(allocator, source, SourceType::script())
            .with_options(options)
            .parse();
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            return Some("does not parse".to_string());
        }
        let body = &parsed.program.body;
        let Some((last, declarations)) = body.split_last() else {
            return Some("empty".to_string());
        };
        let excerpt = |span: oxc_span::Span| {
            let text = &source[span.start as usize..span.end as usize];
            text.chars().take(90).collect::<String>().replace('\n', " ")
        };
        let names = declared(body);
        let gives_back = match last {
            Statement::ExpressionStatement(statement) => inert(&statement.expression, &names, 0),
            Statement::ReturnStatement(statement) => {
                statement.argument.as_ref().is_some_and(|value| inert(value, &names, 0))
            }
            _ => false,
        };
        if !gives_back {
            return Some(format!("gives back {}", excerpt(oxc_span::GetSpan::span(last))));
        }
        declarations
            .iter()
            .find(|statement| match statement {
                Statement::FunctionDeclaration(_) | Statement::EmptyStatement(_) => false,
                Statement::VariableDeclaration(declaration) => {
                    declaration.kind != VariableDeclarationKind::Const
                        || !declaration
                            .declarations
                            .iter()
                            .all(|declarator| declarator.init.as_ref().is_some_and(|init| inert(init, &names, 0)))
                }
                _ => true,
            })
            .map(|statement| excerpt(oxc_span::GetSpan::span(statement)))
    })
}

/// A script file as the body of a function called at once, the way the action macros run a `<script>` body.
///
/// The thread's engine keeps global code's declarations: a top-level `let` or `const` would fail as a redeclaration at
/// the next run, and a `var` or `function` would become a global that hides the context name for every later script on
/// the thread. In the function they are the file's own. A bare assignment still sets the context, `return` ends the
/// script with its value, and a last expression statement is returned, as Groovy returns the last expression. The
/// wrapper opens on the file's first line, so a line number in an error is the file's.
pub(crate) fn file_function(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 32);
    out.push_str("(function(){");
    out.push_str(&completion_body(source));
    out.push_str("\n})()");
    out
}

/// A script as a function body that returns the script's value: its last statement, when that is an expression
/// statement, gets a `return` (a script that does not parse is left as it is, for the engine to report). The test
/// harness's `runInContext` runs a file this way on an engine without `with` and direct `eval` (quickjs-oxide).
pub fn completion_body(source: &str) -> String {
    let options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    let last = with_arena(|allocator| {
        // Not a module: the file runs as sloppy script code.
        let parsed = Parser::new(allocator, source, SourceType::script())
            .with_options(options)
            .parse();
        // A file that does not parse is wrapped as it is, and QuickJS reports the error at its line.
        if parsed.fatal_error || parsed.diagnostics.has_errors() {
            None
        } else {
            match parsed.program.body.last() {
                Some(Statement::ExpressionStatement(statement)) => usize::try_from(statement.span.start).ok(),
                _ => None,
            }
        }
    });
    match last.and_then(|at| source.split_at_checked(at)) {
        Some((before, last)) => format!("{before}return {last}"),
        None => source.to_string(),
    }
}

/// Why a failed parse looks like Groovy, to make the message useful. It never decides a parse.
fn groovy_hint(text: &str) -> Option<&'static str> {
    if text.contains("?:") {
        return Some("?:");
    }
    if has_groovy_arrow(text) {
        return Some("->");
    }
    if has_closure_call(text) {
        return Some("closure");
    }
    None
}

pub fn has_groovy_arrow(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .windows(2)
        .enumerate()
        .any(|(index, pair)| pair == b"->" && (index == 0 || bytes[index - 1] != b'='))
}

pub fn has_closure_call(text: &str) -> bool {
    const WORDS: &[&str] = &[
        "eachWithIndex",
        "findAll",
        "collect",
        "inject",
        "every",
        "each",
        "find",
        "any",
        "sort",
        "with",
    ];
    for word in WORDS {
        for (at, _) in text.match_indices(word) {
            let Some((before, after)) = text.split_at_checked(at) else {
                continue;
            };
            let after = after.strip_prefix(word).unwrap_or(after);
            let before_ok = !before.bytes().next_back().is_some_and(is_ident);
            let after_ok = !after.bytes().next().is_some_and(is_ident);
            if before_ok && after_ok && after.trim_start().starts_with('{') {
                return true;
            }
        }
    }
    false
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(240).collect();
    if text.chars().nth(240).is_some() {
        out.push_str("...");
    }
    out
}
