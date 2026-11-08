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

//! `runInContext` (server.js) on an engine without `with` and with an indirect `eval` (quickjs-oxide): the source as
//! the body of a function whose parameters are the sandbox's names ([`sandbox_body`]). Its completion value, which an
//! `eval` would give, is kept by assigning the value of each expression statement it runs to [`OUT`]`.value`, and
//! `undefined` before each `if`, loop, `switch` and `try` of a statement list, as those statements complete with
//! `undefined` when their own body gives no value.

use oxc_ast::ast::{SourceType, Statement};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::GetSpan;

/// The function's last parameter: an object whose `value` is the completion value and whose `after` the sandbox's
/// names as the source left them.
pub(super) const OUT: &str = "__moquiOut";

/// The body of `function (...names, globalThis, __moquiOut)` that runs `source` and, however it ends, leaves the names'
/// values in `__moquiOut.after`. The source's directives (`"use strict"`) stay first, so they apply to it. A source that
/// does not parse as a script, or has a `return` outside a function, is the parser's message, as `eval` throws a
/// `SyntaxError` for it.
pub(super) fn sandbox_body(source: &str, names: &[String]) -> Result<String, String> {
    let allocator = oxc_allocator::Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::script())
        .with_options(ParseOptions::default())
        .parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return Err(parsed
            .diagnostics
            .errors()
            .next()
            .map_or_else(|| "syntax error".to_string(), |diagnostic| diagnostic.to_string()));
    }
    let mut edits: Vec<(usize, &str)> = Vec::new();
    list(&parsed.program.body, &mut edits);
    let start = parsed
        .program
        .body
        .first()
        .map_or(source.len(), |statement| statement.span().start as usize);
    let (directives, rest) = if parsed.program.directives.is_empty() {
        ("", source)
    } else {
        source.split_at(start)
    };
    let offset = source.len() - rest.len();
    edits.sort_by_key(|(at, _)| *at);
    let mut body = String::with_capacity(source.len() + 64 * (edits.len() + 1));
    body.push_str(directives);
    body.push_str("try {\n");
    let mut from = offset;
    for (at, text) in edits {
        body.push_str(&source[from..at]);
        body.push_str(text);
        from = at;
    }
    body.push_str(&source[from..]);
    body.push_str(&format!("\n}} finally {{ {OUT}.after = [{}]; }}", names.join(", ")));
    Ok(body)
}

const ASSIGN: &str = "__moquiOut.value = (";
const CLOSE: &str = ")";
const RESET: &str = ";__moquiOut.value = undefined;";

/// A statement list: each statement, and the reset before one that completes with `undefined` when empty.
fn list(statements: &[Statement<'_>], edits: &mut Vec<(usize, &'static str)>) {
    for statement in statements {
        if matches!(
            statement,
            Statement::IfStatement(_)
                | Statement::SwitchStatement(_)
                | Statement::TryStatement(_)
                | Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
                | Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
        ) {
            edits.push((statement.span().start as usize, RESET));
        }
        one(statement, edits);
    }
}

/// One statement: an expression statement's value assigned, and the statements inside it (not inside a function).
fn one(statement: &Statement<'_>, edits: &mut Vec<(usize, &'static str)>) {
    match statement {
        Statement::ExpressionStatement(statement) => {
            let span = statement.expression.span();
            edits.push((span.start as usize, ASSIGN));
            edits.push((span.end as usize, CLOSE));
        }
        Statement::BlockStatement(block) => list(&block.body, edits),
        Statement::IfStatement(statement) => {
            one(&statement.consequent, edits);
            if let Some(alternate) = &statement.alternate {
                one(alternate, edits);
            }
        }
        Statement::TryStatement(statement) => {
            // a finally block's value is not the statement's
            list(&statement.block.body, edits);
            if let Some(handler) = &statement.handler {
                list(&handler.body.body, edits);
            }
        }
        Statement::SwitchStatement(statement) => {
            for case in &statement.cases {
                list(&case.consequent, edits);
            }
        }
        Statement::ForStatement(statement) => one(&statement.body, edits),
        Statement::ForInStatement(statement) => one(&statement.body, edits),
        Statement::ForOfStatement(statement) => one(&statement.body, edits),
        Statement::WhileStatement(statement) => one(&statement.body, edits),
        Statement::DoWhileStatement(statement) => one(&statement.body, edits),
        Statement::LabeledStatement(statement) => one(&statement.body, edits),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::sandbox_body;

    #[test]
    fn expression_statements_assign_and_directives_stay_first() {
        let body = sandbox_body("'use strict';\nx = 1\nif (x) { 'a' } else 'b'", &["x".to_string()]).expect("parses");
        assert_eq!(
            body,
            "'use strict';\ntry {\n__moquiOut.value = (x = 1)\n;__moquiOut.value = undefined;if (x) { \
             __moquiOut.value = ('a') } else __moquiOut.value = ('b')\n} finally { __moquiOut.after = [x]; }"
        );
    }

    #[test]
    fn a_return_outside_a_function_is_a_syntax_error() {
        assert!(sandbox_body("return 1", &[]).is_err());
        assert!(sandbox_body("(function () { return 1 })()", &[]).is_ok());
    }
}
