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
fn with_arena<R>(body: impl FnOnce(&Allocator) -> R) -> R {
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
        if parsed.panicked || parsed.diagnostics.has_errors() {
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
        if parsed.panicked || parsed.diagnostics.has_errors() {
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
        if parsed.panicked || parsed.diagnostics.has_errors() {
            return Err(Error::new("not JavaScript"));
        }
        Ok(())
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
        if parsed.panicked || parsed.diagnostics.has_errors() {
            None
        } else {
            match parsed.program.body.last() {
                Some(Statement::ExpressionStatement(statement)) => usize::try_from(statement.span.start).ok(),
                _ => None,
            }
        }
    });
    let mut out = String::with_capacity(source.len() + 32);
    out.push_str("(function(){");
    match last.filter(|at| source.is_char_boundary(*at)) {
        Some(at) => {
            out.push_str(&source[..at]);
            out.push_str("return ");
            out.push_str(&source[at..]);
        }
        None => out.push_str(source),
    }
    out.push_str("\n})()");
    out
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

pub(super) fn has_groovy_arrow(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .windows(2)
        .enumerate()
        .any(|(index, pair)| pair == b"->" && (index == 0 || bytes[index - 1] != b'='))
}

pub(super) fn has_closure_call(text: &str) -> bool {
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
    let bytes = text.as_bytes();
    for word in WORDS {
        let mut start = 0;
        while let Some(found) = text[start..].find(word) {
            let at = start + found;
            let end = at + word.len();
            let before_ok = at == 0 || !is_ident(bytes[at - 1]);
            let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
            if before_ok && after_ok && text[end..].trim_start().starts_with('{') {
                return true;
            }
            start = end;
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
