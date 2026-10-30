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

//! The power-assert rewrite of a spec file, the part of Spock's AST transform the harness needs.
//!
//! In each top-level `feature(name, { ... })` call, every expression statement directly in the body of the `then` or
//! `expect` member is a condition, as a statement in a Spock `then:` block is. The rewrite wraps each condition so a
//! false one fails the feature with a diagram of the values of its parts, and gives `==`, `!=`, `<` and the rest, `!`,
//! `&&`, `||` and the condition itself Groovy semantics: decimals compare by value and an empty list is false.
//!
//! The rewrite inserts text at node boundaries and adds no line, so a line in an error is the spec file's line. The
//! `then` and `expect` keys it rewrote are renamed `then$pa` and `expect$pa`; a `then` the rewrite could not see, such
//! as one in a feature object a helper function builds, keeps its name and the harness refuses it, so no block of
//! conditions runs unchecked.

use oxc_ast::ast::{
    Argument, BinaryOperator, ChainElement, Expression, LogicalOperator, ObjectExpression, ObjectPropertyKind,
    SourceType, Statement, UnaryOperator,
};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::GetSpan;

use crate::context::Error;

/// The name a rewritten `then` or `expect` member gets.
pub const REWRITTEN_SUFFIX: &str = "$pa";

/// One condition: its source text, where it is, and the parts whose values a failure shows.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Condition {
    pub text: String,
    /// 1-based line of the condition in the spec file.
    pub line: u32,
    /// Whether the condition is a call: one that returns `undefined` passes, as a void method call in Spock.
    pub call: bool,
    /// For each value slot, the column in `text` (in characters, as the diagram draws it) where its value is shown,
    /// and the part's own text.
    pub slots: Vec<(u32, String)>,
}

/// A rewritten spec: the source to run, and the conditions it refers to by index.
#[derive(Clone, Debug)]
pub struct Rewritten {
    pub source: String,
    pub conditions: Vec<Condition>,
}

/// Rewrite a spec file. A file that does not parse is an error naming the first problem, with its line.
pub fn rewrite_spec(source: &str) -> Result<Rewritten, Error> {
    // Each line of a `then` is a statement, as in Spock, so a line that starts with a regex is not a division.
    let ended = super::lines::end_condition_lines(source);
    let source: &str = &ended;
    let allocator = std::default::Default::default();
    let parsed = Parser::new(&allocator, source, SourceType::script())
        .with_options(ParseOptions::default())
        .parse();
    if parsed.panicked || parsed.diagnostics.has_errors() {
        let detail = parsed
            .diagnostics
            .errors()
            .next()
            .map_or_else(|| "syntax error".to_string(), |diagnostic| diagnostic.to_string());
        let line = parsed
            .diagnostics
            .errors()
            .next()
            .and_then(|diagnostic| diagnostic.labels.as_ref().first().map(|label| label.offset()))
            .map(|offset| line_of(source, offset));
        return Err(Error::new(match line {
            Some(line) => format!("line {line}: {detail}"),
            None => detail,
        }));
    }
    let mut rewriter = Rewriter {
        source,
        edits: Vec::new(),
        conditions: Vec::new(),
        seq: 0,
    };
    for statement in &parsed.program.body {
        if let Statement::ExpressionStatement(statement) = statement {
            if let Expression::CallExpression(call) = &statement.expression {
                let is_feature = matches!(&call.callee, Expression::Identifier(name) if name.name == "feature");
                if !is_feature {
                    continue;
                }
                for argument in &call.arguments {
                    if let Some(Expression::ObjectExpression(object)) = argument.as_expression() {
                        rewriter.feature_object(object);
                    }
                }
            }
        }
    }
    Ok(Rewritten {
        source: rewriter.apply(),
        conditions: rewriter.conditions,
    })
}

/// The `dependsOn` names of a spec file, from the string literals of `dependsOn` in its top-level `spec({ ... })`
/// call, read without running the file. A file that does not parse has none.
pub fn spec_depends_on(source: &str) -> Vec<String> {
    spec_strings(source, "dependsOn")
}

/// The build features a spec file `requires` (`spec({ requires: ['git'] })`), read without running the file.
pub fn spec_requires(source: &str) -> Vec<String> {
    spec_strings(source, "requires")
}

/// `spec({ server: false })`, read without running the file. Anything else, including a file that does not parse, is
/// false: the spec is treated as one that needs a server.
pub fn spec_offline(source: &str) -> bool {
    spec_flag(source, "server") == Some(false)
}

/// `spec({ slow: true })`, read without running the file: a spec that runs over 30 seconds, which a run leaves out
/// unless it is asked for slow tests (`--slow`). Anything else, including a file that does not parse, is false.
pub fn spec_slow(source: &str) -> bool {
    spec_flag(source, "slow") == Some(true)
}

/// `spec({ database: false })`, read without running the file. Anything else, including a file that does not parse,
/// is false: the spec is treated as one that needs a database.
pub fn spec_database_free(source: &str) -> bool {
    spec_flag(source, "database") == Some(false)
}

/// The string literals of the array `key` in the top-level `spec({ ... })` call. A file that does not parse has none.
/// The source is read as [`rewrite_spec`] reads it, with each condition line ended, so a `then` line that starts with
/// a regular expression does not make the file unreadable here while it runs.
fn spec_strings(source: &str, key: &str) -> Vec<String> {
    let ended = super::lines::end_condition_lines(source);
    let source: &str = &ended;
    let allocator = std::default::Default::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    let mut names = Vec::new();
    if parsed.panicked || parsed.diagnostics.has_errors() {
        return names;
    }
    for statement in &parsed.program.body {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let Expression::CallExpression(call) = &statement.expression else {
            continue;
        };
        if !matches!(&call.callee, Expression::Identifier(name) if name.name == "spec") {
            continue;
        }
        let Some(Expression::ObjectExpression(object)) = call.arguments.first().and_then(Argument::as_expression)
        else {
            continue;
        };
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if property.key.static_name().as_deref() != Some(key) {
                continue;
            }
            if let Expression::ArrayExpression(array) = &property.value {
                for element in &array.elements {
                    if let Some(Expression::StringLiteral(text)) = element.as_expression() {
                        names.push(text.value.to_string());
                    }
                }
            }
        }
    }
    names
}

/// The boolean `key` of the top-level `spec({ ... })` call, when it is a literal. None when it is absent or not a
/// boolean, and when the file does not parse.
fn spec_flag(source: &str, key: &str) -> Option<bool> {
    let ended = super::lines::end_condition_lines(source);
    let source: &str = &ended;
    let allocator = std::default::Default::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked || parsed.diagnostics.has_errors() {
        return None;
    }
    for statement in &parsed.program.body {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let Expression::CallExpression(call) = &statement.expression else {
            continue;
        };
        if !matches!(&call.callee, Expression::Identifier(name) if name.name == "spec") {
            continue;
        }
        let Some(Expression::ObjectExpression(object)) = call.arguments.first().and_then(Argument::as_expression)
        else {
            continue;
        };
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if property.key.static_name().as_deref() != Some(key) {
                continue;
            }
            if let Expression::BooleanLiteral(literal) = &property.value {
                return Some(literal.value);
            }
        }
    }
    None
}

/// The spec names of a component's `test/suite.js`, `suite(["SpecA", "SpecB"])`, in order: the string literals of the
/// array in its top-level `suite(...)` call. A file that does not parse is an error.
pub fn suite_order(source: &str) -> Result<Vec<String>, Error> {
    let allocator = std::default::Default::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked || parsed.diagnostics.has_errors() {
        return Err(Error::new("suite.js is not JavaScript"));
    }
    for statement in &parsed.program.body {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let Expression::CallExpression(call) = &statement.expression else {
            continue;
        };
        if !matches!(&call.callee, Expression::Identifier(name) if name.name == "suite") {
            continue;
        }
        let Some(Expression::ArrayExpression(array)) = call.arguments.first().and_then(Argument::as_expression) else {
            return Err(Error::new("suite() takes an array of spec names"));
        };
        let mut names = Vec::new();
        for element in &array.elements {
            match element.as_expression() {
                Some(Expression::StringLiteral(text)) => names.push(text.value.to_string()),
                _ => return Err(Error::new("suite() takes an array of spec names as strings")),
            }
        }
        return Ok(names);
    }
    Err(Error::new("suite.js has no suite([...]) call"))
}

/// The server a client suite needs (plans/archive/ClientTestHarness.md 6): `suite([...], { server: { components, specs, conf } })`
/// in `test/client/suite.js`, which `moqui test-client --serve` starts as a child `moqui test ... --serve`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuiteServer {
    /// The components whose server specs run, in order; empty means the suite's own component.
    pub components: Vec<String>,
    /// The server specs that make the fixtures (with what they depend on); empty runs every spec of the components.
    pub specs: Vec<String>,
    /// The conf the server runs under (`MOQUI_CONF`), relative to the runtime directory.
    pub conf: Option<String>,
}

/// The server declaration of a `suite.js`, read without running it as [`suite_order`] reads the order: the literals of
/// the `server` object in the second argument of its top-level `suite(...)` call. None when it declares no server; an
/// error when the file does not parse or the declaration is not literals of the right kind.
pub fn suite_server(source: &str) -> Result<Option<SuiteServer>, Error> {
    let allocator = std::default::Default::default();
    let parsed = Parser::new(&allocator, source, SourceType::script()).parse();
    if parsed.panicked || parsed.diagnostics.has_errors() {
        return Err(Error::new("suite.js is not JavaScript"));
    }
    for statement in &parsed.program.body {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let Expression::CallExpression(call) = &statement.expression else {
            continue;
        };
        if !matches!(&call.callee, Expression::Identifier(name) if name.name == "suite") {
            continue;
        }
        let Some(options) = call.arguments.get(1) else {
            return Ok(None);
        };
        let Some(Expression::ObjectExpression(options)) = options.as_expression() else {
            return Err(Error::new("suite() takes an object of options after the spec names"));
        };
        let Some(server) = object_property(options, "server") else {
            return Ok(None);
        };
        let Expression::ObjectExpression(server) = server else {
            return Err(Error::new("suite() server is an object: { components, specs, conf }"));
        };
        let mut declared = SuiteServer::default();
        for property in &server.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                return Err(Error::new(
                    "suite() server is an object of literals: { components, specs, conf }",
                ));
            };
            match property.key.static_name().as_deref() {
                Some("components") => declared.components = string_array(&property.value, "components")?,
                Some("specs") => declared.specs = string_array(&property.value, "specs")?,
                Some("conf") => match &property.value {
                    Expression::StringLiteral(text) => declared.conf = Some(text.value.to_string()),
                    _ => {
                        return Err(Error::new(
                            "suite() server conf is a string, such as \"conf/MoquiProductionConf.xml\"",
                        ))
                    }
                },
                other => {
                    return Err(Error::new(format!(
                        "suite() server takes components, specs and conf, not {}",
                        other.unwrap_or("a computed key")
                    )))
                }
            }
        }
        return Ok(Some(declared));
    }
    Ok(None)
}

/// The value of the property `key` of an object literal.
fn object_property<'a, 'b>(object: &'b ObjectExpression<'a>, key: &str) -> Option<&'b Expression<'a>> {
    object.properties.iter().find_map(|property| match property {
        ObjectPropertyKind::ObjectProperty(property) if property.key.static_name().as_deref() == Some(key) => {
            Some(&property.value)
        }
        _ => None,
    })
}

/// An array of string literals.
fn string_array(value: &Expression<'_>, name: &str) -> Result<Vec<String>, Error> {
    let not_strings = || Error::new(format!("suite() server {name} is an array of names as strings"));
    let Expression::ArrayExpression(array) = value else {
        return Err(not_strings());
    };
    array
        .elements
        .iter()
        .map(|element| match element.as_expression() {
            Some(Expression::StringLiteral(text)) => Ok(text.value.to_string()),
            _ => Err(not_strings()),
        })
        .collect()
}

/// 1-based line of a byte offset.
fn line_of(source: &str, offset: u32) -> u32 {
    let end = (offset as usize).min(source.len());
    let newlines = source.as_bytes()[..end].iter().filter(|byte| **byte == b'\n').count();
    u32::try_from(newlines).unwrap_or(u32::MAX).saturating_add(1)
}

/// Text inserted at one offset. Closing text goes first, innermost first; then opening and replacement text, outermost
/// first, and in the order made when two have the same span.
struct Edit {
    at: u32,
    close: bool,
    span_len: u32,
    seq: u32,
    text: String,
    /// Source bytes after `at` that this edit replaces.
    delete: u32,
}

struct Rewriter<'s> {
    source: &'s str,
    edits: Vec<Edit>,
    conditions: Vec<Condition>,
    seq: u32,
}

impl Rewriter<'_> {
    fn push(&mut self, at: u32, close: bool, span_len: u32, text: impl Into<String>, delete: u32) {
        self.seq += 1;
        self.edits.push(Edit {
            at,
            close,
            span_len,
            seq: self.seq,
            text: text.into(),
            delete,
        });
    }

    fn feature_object(&mut self, object: &ObjectExpression<'_>) {
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if property.computed || property.shorthand {
                continue;
            }
            let Some(name) = property.key.static_name() else {
                continue;
            };
            if name != "then" && name != "expect" {
                continue;
            }
            let body = match &property.value {
                Expression::FunctionExpression(function) => function.body.as_deref(),
                Expression::ArrowFunctionExpression(arrow) => Some(&*arrow.body),
                _ => None,
            };
            let Some(body) = body else {
                continue;
            };
            let key = property.key.span();
            // The key as written may be quoted; it becomes a plain name.
            self.push(
                key.start,
                false,
                u32::MAX,
                format!("{name}{REWRITTEN_SUFFIX}"),
                key.size(),
            );
            for statement in &body.statements {
                if let Statement::ExpressionStatement(statement) = statement {
                    self.condition(&statement.expression);
                }
            }
        }
    }

    fn condition(&mut self, expression: &Expression<'_>) {
        // An assignment in a `then` block is not a condition, as in Spock.
        if matches!(expression, Expression::AssignmentExpression(_)) {
            return;
        }
        let span = expression.span();
        let index = self.conditions.len();
        let call = matches!(expression, Expression::CallExpression(_))
            || matches!(expression, Expression::ChainExpression(chain) if matches!(chain.expression, ChainElement::CallExpression(_)));
        self.conditions.push(Condition {
            text: self.text(span.start, span.end).to_string(),
            line: line_of(self.source, span.start),
            call,
            slots: Vec::new(),
        });
        self.push(span.start, false, u32::MAX, format!("__pa({index}, ($pa_v) => ("), 0);
        self.instrument(expression, index, span.start);
        self.push(span.end, true, u32::MAX, "))", 0);
    }

    fn text(&self, start: u32, end: u32) -> &str {
        self.source.get(start as usize..end as usize).unwrap_or("")
    }

    /// Record the value of `expression` in the next slot of condition `index`, shown at `show_at`.
    fn slot(&mut self, expression: &Expression<'_>, index: usize, base: u32, show_at: u32) {
        let span = expression.span();
        let part = self.text(span.start, span.end).to_string();
        // Characters, not UTF-8 bytes: the diagram puts the bar under the character (`·` is two bytes).
        let column = u32::try_from(self.text(base, show_at.max(base)).chars().count()).unwrap_or(u32::MAX);
        let condition = &mut self.conditions[index];
        let slot = condition.slots.len();
        condition.slots.push((column, part));
        self.push(span.start, false, span.size(), format!("$pa_v({slot}, "), 0);
        self.push(span.end, true, span.size(), ")", 0);
    }

    /// The offset of `operator` between two operands, or `from` when it is not found.
    fn operator_at(&self, from: u32, to: u32, operator: &str) -> u32 {
        self.text(from, to)
            .find(operator)
            .map_or(from, |found| from + u32::try_from(found).unwrap_or(0))
    }

    fn instrument(&mut self, expression: &Expression<'_>, index: usize, base: u32) {
        match expression {
            Expression::Identifier(identifier) => {
                if identifier.name != "undefined" {
                    self.slot(expression, index, base, identifier.span.start);
                }
            }
            Expression::StaticMemberExpression(member) => {
                self.slot(expression, index, base, member.property.span.start);
                self.instrument(&member.object, index, base);
            }
            Expression::ComputedMemberExpression(member) => {
                let bracket = self.operator_at(member.object.span().end, member.expression.span().start, "[");
                self.slot(expression, index, base, bracket);
                self.instrument(&member.object, index, base);
                self.instrument(&member.expression, index, base);
            }
            Expression::CallExpression(call) => {
                let show_at = match &call.callee {
                    Expression::StaticMemberExpression(member) => member.property.span.start,
                    other => other.span().start,
                };
                self.slot(expression, index, base, show_at);
                // The callee itself is not wrapped, so a method keeps its `this`; its object is.
                match &call.callee {
                    Expression::StaticMemberExpression(member) => self.instrument(&member.object, index, base),
                    Expression::ComputedMemberExpression(member) => {
                        self.instrument(&member.object, index, base);
                        self.instrument(&member.expression, index, base);
                    }
                    _ => {}
                }
                for argument in &call.arguments {
                    if let Argument::SpreadElement(_) = argument {
                        continue;
                    }
                    if let Some(argument) = argument.as_expression() {
                        self.instrument(argument, index, base);
                    }
                }
            }
            Expression::BinaryExpression(binary) => {
                let operator = binary.operator.as_str();
                let left = binary.left.span();
                let right = binary.right.span();
                let at = self.operator_at(left.end, right.start, operator);
                self.slot(expression, index, base, at);
                let groovy = matches!(
                    binary.operator,
                    BinaryOperator::Equality
                        | BinaryOperator::Inequality
                        | BinaryOperator::LessThan
                        | BinaryOperator::LessEqualThan
                        | BinaryOperator::GreaterThan
                        | BinaryOperator::GreaterEqualThan
                );
                if groovy {
                    let span = binary.span;
                    self.push(span.start, false, span.size(), format!("__cmp(\"{operator}\", "), 0);
                    self.push(left.end, false, 0, ", ", right.start - left.end);
                    self.push(span.end, true, span.size(), ")", 0);
                }
                self.instrument(&binary.left, index, base);
                self.instrument(&binary.right, index, base);
            }
            Expression::LogicalExpression(logical) => {
                let operator = logical.operator.as_str();
                let left = logical.left.span();
                let right = logical.right.span();
                let at = self.operator_at(left.end, right.start, operator);
                self.slot(expression, index, base, at);
                if logical.operator != LogicalOperator::Coalesce {
                    let span = logical.span;
                    self.push(span.start, false, span.size(), "(__truthy(", 0);
                    self.push(
                        left.end,
                        false,
                        0,
                        format!(") {operator} __truthy("),
                        right.start - left.end,
                    );
                    self.push(span.end, true, span.size(), "))", 0);
                }
                self.instrument(&logical.left, index, base);
                self.instrument(&logical.right, index, base);
            }
            Expression::UnaryExpression(unary) => {
                self.slot(expression, index, base, unary.span.start);
                if unary.operator == UnaryOperator::LogicalNot {
                    let span = unary.span;
                    self.push(span.start, false, span.size(), "!__truthy(", 1);
                    self.push(span.end, true, span.size(), ")", 0);
                }
                if !matches!(unary.operator, UnaryOperator::Typeof | UnaryOperator::Delete) {
                    self.instrument(&unary.argument, index, base);
                }
            }
            Expression::ParenthesizedExpression(parenthesized) => {
                self.instrument(&parenthesized.expression, index, base);
            }
            // The value of an optional chain, a conditional and a `new` is shown; their parts are not instrumented,
            // which would change when they run.
            Expression::ChainExpression(_) | Expression::ConditionalExpression(_) | Expression::NewExpression(_) => {
                let start = expression.span().start;
                self.slot(expression, index, base, start);
            }
            _ => {}
        }
    }

    fn apply(&mut self) -> String {
        let mut edits = std::mem::take(&mut self.edits);
        edits.sort_by(|a, b| {
            a.at.cmp(&b.at).then_with(|| b.close.cmp(&a.close)).then_with(|| {
                if a.close {
                    a.span_len.cmp(&b.span_len).then(a.seq.cmp(&b.seq))
                } else {
                    b.span_len.cmp(&a.span_len).then(a.seq.cmp(&b.seq))
                }
            })
        });
        let mut out = String::with_capacity(self.source.len() + edits.len() * 12);
        let mut at = 0_usize;
        for edit in edits {
            let position = edit.at as usize;
            if position > at {
                out.push_str(&self.source[at..position]);
                at = position;
            }
            out.push_str(&edit.text);
            at = at.max(position + edit.delete as usize);
        }
        out.push_str(&self.source[at.min(self.source.len())..]);
        out
    }
}

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
