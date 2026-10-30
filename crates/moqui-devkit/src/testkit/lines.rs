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

//! Each line of a `then` or `expect` block is its own statement, as in a Spock `then:` block.
//!
//! JavaScript joins a line that starts with `/`, `` ` ``, `(` or `[` to the line before it: `a == 1` followed by
//! `/Today · 3/.test(label)` is the division `a == 1 / Today · 3 / .test(label)`, which fails to parse at the `·`
//! (or, all ASCII, parses as a division), and a line starting with a template literal tags the line before. Before the
//! rewrite parses a spec file, a `;` is put after the last token of such a previous line when both lines are directly
//! in the body of a `then` or `expect` member (a method `then() {`, or `then: function () {` or `then: (...) => {`),
//! and the previous line ended an expression. A line inside brackets of its own (a multi-line object, array or call)
//! is not touched, nor is any other block. The `;` is on the same line, so line numbers stay the spec file's.
//!
//! The lexer knows strings, templates (with `${}` nesting), comments, regular expression literals (a `/` where an
//! expression starts) and brackets; anything else is a token it does not need to name. Non-ASCII text is only ever
//! inside a token, so the offsets it inserts at are character boundaries.

use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ident,
    Number,
    Str,
    Template,
    Regex,
    Arrow,
    Punct(u8),
}

#[derive(Clone, Copy)]
struct Tok {
    kind: Kind,
    start: usize,
    end: usize,
    /// For a `)`, the index of its `(` token.
    open: Option<usize>,
}

enum Frame {
    Brace { then_body: bool },
    Paren { open: usize },
    Bracket,
    TemplateExpr,
}

/// Words after which a `/` starts a regular expression, not a division.
const EXPRESSION_KEYWORDS: [&str; 15] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
    "extends",
];

/// The source with a `;` ending each `then` or `expect` line that JavaScript would join to the next one.
pub(super) fn end_condition_lines(source: &str) -> Cow<'_, str> {
    let inserts = Lexer::new(source).run();
    if inserts.is_empty() {
        return Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len() + inserts.len());
    let mut at = 0;
    for offset in inserts {
        out.push_str(&source[at..offset]);
        out.push(';');
        at = offset;
    }
    out.push_str(&source[at..]);
    Cow::Owned(out)
}

struct Lexer<'s> {
    source: &'s str,
    bytes: &'s [u8],
    i: usize,
    tokens: Vec<Tok>,
    frames: Vec<Frame>,
    inserts: Vec<usize>,
}

impl<'s> Lexer<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            i: 0,
            tokens: Vec::new(),
            frames: Vec::new(),
            inserts: Vec::new(),
        }
    }

    fn text(&self, token: &Tok) -> &'s str {
        &self.source[token.start..token.end]
    }

    fn run(mut self) -> Vec<usize> {
        loop {
            let newline = self.skip_space();
            if self.i >= self.bytes.len() {
                break;
            }
            let start = self.i;
            let c = self.bytes[start];
            let mut force_regex = false;
            if newline && matches!(c, b'/' | b'`' | b'(' | b'[') && self.in_then_body() {
                if let Some(previous) = self.tokens.last().copied() {
                    let joins = self.ends_expression(&previous) && (c != b'/' || self.regex_end(start).is_some());
                    if joins {
                        self.inserts.push(previous.end);
                        force_regex = c == b'/';
                    }
                }
            }
            match c {
                b'\'' | b'"' => {
                    let end = self.string_end(start);
                    self.push(Kind::Str, start, end);
                }
                b'`' => self.template(start, start + 1),
                b'/' => {
                    let regex = if force_regex || self.regex_allowed() {
                        self.regex_end(start)
                    } else {
                        None
                    };
                    match regex {
                        Some(end) => self.push(Kind::Regex, start, end),
                        None => self.push(Kind::Punct(b'/'), start, start + 1),
                    }
                }
                b'0'..=b'9' => {
                    let mut end = start + 1;
                    while end < self.bytes.len()
                        && (self.bytes[end].is_ascii_alphanumeric() || matches!(self.bytes[end], b'.' | b'_'))
                    {
                        end += 1;
                    }
                    self.push(Kind::Number, start, end);
                }
                b'{' => {
                    let then_body = self.opens_then_body();
                    self.frames.push(Frame::Brace { then_body });
                    self.push(Kind::Punct(b'{'), start, start + 1);
                }
                b'}' => {
                    if matches!(self.frames.last(), Some(Frame::TemplateExpr)) {
                        self.frames.pop();
                        self.template(start, start + 1);
                    } else {
                        if matches!(self.frames.last(), Some(Frame::Brace { .. })) {
                            self.frames.pop();
                        }
                        self.push(Kind::Punct(b'}'), start, start + 1);
                    }
                }
                b'(' => {
                    self.frames.push(Frame::Paren {
                        open: self.tokens.len(),
                    });
                    self.push(Kind::Punct(b'('), start, start + 1);
                }
                b')' => {
                    let open = match self.frames.last() {
                        Some(Frame::Paren { open }) => Some(*open),
                        _ => None,
                    };
                    if open.is_some() {
                        self.frames.pop();
                    }
                    self.i = start + 1;
                    self.tokens.push(Tok {
                        kind: Kind::Punct(b')'),
                        start,
                        end: start + 1,
                        open,
                    });
                }
                b'[' => {
                    self.frames.push(Frame::Bracket);
                    self.push(Kind::Punct(b'['), start, start + 1);
                }
                b']' => {
                    if matches!(self.frames.last(), Some(Frame::Bracket)) {
                        self.frames.pop();
                    }
                    self.push(Kind::Punct(b']'), start, start + 1);
                }
                b'=' if self.bytes.get(start + 1) == Some(&b'>') => self.push(Kind::Arrow, start, start + 2),
                c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c == b'\\' || c >= 0x80 => {
                    let mut end = start + 1;
                    while end < self.bytes.len() && {
                        let next = self.bytes[end];
                        next.is_ascii_alphanumeric() || next == b'_' || next == b'$' || next == b'\\' || next >= 0x80
                    } {
                        end += 1;
                    }
                    self.push(Kind::Ident, start, end);
                }
                other => self.push(Kind::Punct(other), start, start + 1),
            }
        }
        self.inserts
    }

    fn push(&mut self, kind: Kind, start: usize, end: usize) {
        self.i = end;
        self.tokens.push(Tok {
            kind,
            start,
            end,
            open: None,
        });
    }

    /// Skip white space and comments; whether a line ended in them.
    fn skip_space(&mut self) -> bool {
        let mut newline = false;
        while self.i < self.bytes.len() {
            match self.bytes[self.i] {
                b'\n' => {
                    newline = true;
                    self.i += 1;
                }
                b' ' | b'\t' | b'\r' | 0x0b | 0x0c => self.i += 1,
                b'/' if self.bytes.get(self.i + 1) == Some(&b'/') => {
                    while self.i < self.bytes.len() && self.bytes[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                b'/' if self.bytes.get(self.i + 1) == Some(&b'*') => {
                    let close = self.source[self.i + 2..]
                        .find("*/")
                        .map_or(self.bytes.len(), |at| self.i + 2 + at + 2);
                    newline |= self.source[self.i..close].contains('\n');
                    self.i = close;
                }
                _ => break,
            }
        }
        newline
    }

    /// The end of a string literal from its opening quote; an unclosed one ends at the line.
    fn string_end(&self, start: usize) -> usize {
        let quote = self.bytes[start];
        let mut end = start + 1;
        while end < self.bytes.len() {
            match self.bytes[end] {
                b'\\' => end += 2,
                b'\n' => return end,
                found if found == quote => return end + 1,
                _ => end += 1,
            }
        }
        self.bytes.len()
    }

    /// A template part from `from`: up to its closing backquote (a template token), or up to a `${`, which opens an
    /// expression frame whose `}` comes back here.
    fn template(&mut self, start: usize, from: usize) {
        let mut end = from;
        while end < self.bytes.len() {
            match self.bytes[end] {
                b'\\' => end += 2,
                b'`' => {
                    self.push(Kind::Template, start, end + 1);
                    return;
                }
                b'$' if self.bytes.get(end + 1) == Some(&b'{') => {
                    self.push(Kind::Punct(b'{'), start, end + 2);
                    self.frames.push(Frame::TemplateExpr);
                    return;
                }
                _ => end += 1,
            }
        }
        self.push(Kind::Template, start, self.bytes.len());
    }

    /// The end of a regular expression literal at `start` (its flags included), when one closes on this line.
    fn regex_end(&self, start: usize) -> Option<usize> {
        if matches!(self.bytes.get(start + 1), Some(b'/' | b'*')) {
            return None;
        }
        let mut end = start + 1;
        let mut class = false;
        while end < self.bytes.len() {
            match self.bytes[end] {
                b'\\' => end += 2,
                b'\n' => return None,
                b'[' => {
                    class = true;
                    end += 1;
                }
                b']' => {
                    class = false;
                    end += 1;
                }
                b'/' if !class => {
                    end += 1;
                    while end < self.bytes.len() && self.bytes[end].is_ascii_alphabetic() {
                        end += 1;
                    }
                    return Some(end);
                }
                _ => end += 1,
            }
        }
        None
    }

    /// Whether a `/` here starts a regular expression: after no token, a punctuator other than a closing bracket, or
    /// a keyword that an expression follows.
    fn regex_allowed(&self) -> bool {
        match self.tokens.last() {
            None => true,
            Some(token) => match token.kind {
                Kind::Punct(b')' | b']' | b'}') => false,
                Kind::Punct(_) | Kind::Arrow => true,
                Kind::Ident => EXPRESSION_KEYWORDS.contains(&self.text(token)),
                _ => false,
            },
        }
    }

    fn ends_expression(&self, token: &Tok) -> bool {
        match token.kind {
            Kind::Ident => !EXPRESSION_KEYWORDS.contains(&self.text(token)),
            Kind::Number | Kind::Str | Kind::Template | Kind::Regex => true,
            Kind::Punct(c) => matches!(c, b')' | b']' | b'}'),
            Kind::Arrow => false,
        }
    }

    fn in_then_body(&self) -> bool {
        matches!(self.frames.last(), Some(Frame::Brace { then_body: true }))
    }

    /// `then` or `expect` (quoted or not) as the key of an object member: after `{` or `,`.
    fn is_then_key(&self, index: usize) -> bool {
        let Some(token) = self.tokens.get(index) else {
            return false;
        };
        let name = match token.kind {
            Kind::Ident => self.text(token),
            Kind::Str => self.text(token).trim_matches(['\'', '"']),
            _ => return false,
        };
        if name != "then" && name != "expect" {
            return false;
        }
        let mut before = index.checked_sub(1);
        // `async then() {`
        if let Some(at) = before {
            if self.tokens[at].kind == Kind::Ident && self.text(&self.tokens[at]) == "async" {
                before = at.checked_sub(1);
            }
        }
        before.is_some_and(|at| matches!(self.tokens[at].kind, Kind::Punct(b'{' | b',')))
    }

    /// `key: ` before the token at `index`: whether that key is `then` or `expect`.
    fn after_then_colon(&self, index: usize) -> bool {
        let mut at = index;
        if self
            .tokens
            .get(at)
            .is_some_and(|token| token.kind == Kind::Ident && self.text(token) == "async")
        {
            let Some(previous) = at.checked_sub(1) else {
                return false;
            };
            at = previous;
            return self.colon_key(at);
        }
        self.colon_key(at)
    }

    fn colon_key(&self, colon: usize) -> bool {
        self.tokens
            .get(colon)
            .is_some_and(|token| token.kind == Kind::Punct(b':'))
            && colon.checked_sub(1).is_some_and(|key| self.is_then_key(key))
    }

    /// Whether the `{` about to be read opens the body of a `then` or `expect` member.
    fn opens_then_body(&self) -> bool {
        let Some(last) = self.tokens.len().checked_sub(1) else {
            return false;
        };
        let token = self.tokens[last];
        match token.kind {
            // `then() {`, `then: function () {`, `then: function name() {`
            Kind::Punct(b')') => {
                let Some(open) = token.open else {
                    return false;
                };
                let Some(before) = open.checked_sub(1) else {
                    return false;
                };
                if self.is_then_key(before) {
                    return true;
                }
                let mut function = before;
                let is_function =
                    |at: usize| self.tokens[at].kind == Kind::Ident && self.text(&self.tokens[at]) == "function";
                if !is_function(function) {
                    // a named function expression
                    match function.checked_sub(1) {
                        Some(at) if self.tokens[function].kind == Kind::Ident && is_function(at) => function = at,
                        _ => return false,
                    }
                }
                function
                    .checked_sub(1)
                    .is_some_and(|colon| self.after_then_colon(colon))
            }
            // `then: (...) => {`, `then: name => {`
            Kind::Arrow => {
                let Some(params_end) = last.checked_sub(1) else {
                    return false;
                };
                let params = self.tokens[params_end];
                let params_start = match params.kind {
                    Kind::Punct(b')') => match params.open {
                        Some(open) => open,
                        None => return false,
                    },
                    Kind::Ident => params_end,
                    _ => return false,
                };
                params_start
                    .checked_sub(1)
                    .is_some_and(|colon| self.after_then_colon(colon))
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::end_condition_lines;

    fn ended(source: &str) -> String {
        end_condition_lines(source).into_owned()
    }

    #[test]
    fn a_then_line_starting_a_regex_template_or_bracket_ends_the_line_before() {
        let source = "feature('f', {\n  then() {\n    a == 1\n    /Today · 3/.test(label)\n    `· ${n} ·` == b\n    (x) == 1\n    [1] == y\n  },\n})\n";
        assert_eq!(
            ended(source),
            "feature('f', {\n  then() {\n    a == 1;\n    /Today · 3/.test(label);\n    `· ${n} ·` == b;\n    (x) == 1;\n    [1] == y\n  },\n})\n"
        );
        for opening in [
            "then: function () {",
            "then: function named() {",
            "expect: ({ a }) => {",
            "'then': a => {",
            "async then() {",
        ] {
            let source = format!("feature('f', {{ {opening}\n  a\n  /b/.test(c)\n}} }})");
            assert!(ended(&source).contains("  a;\n"), "{opening}");
        }
    }

    #[test]
    fn other_blocks_brackets_and_continued_lines_are_left() {
        for source in [
            // another block
            "feature('f', { when() {\n  a\n  /b/.test(c)\n} })",
            // a multi-line object or array, a line ending in an operator, a division with spaces
            "feature('f', { then() {\n  x == { a: 1,\n    b: [2,\n    (3)] }\n  y ==\n    [1]\n  z == 4 / 2\n} })",
            // a member call named then
            "p.then(() => {\n  a\n  (b)\n})",
            // a regex that does not close on its line is a division
            "feature('f', { then() {\n  a\n  / 2\n} })",
            // a comment and a string with a slash
            "feature('f', { then() {\n  a // x\n  // /re/\n  '/x/' == b\n} })",
        ] {
            assert_eq!(ended(source), source);
        }
    }

    #[test]
    fn templates_nest_and_regexes_after_operators_are_read() {
        let source =
            "feature('f', { then() {\n  `a ${ {b: 1}.b } c` == d\n  /x}/.test(e) && /[/]/.test(f)\n  (g)\n} })";
        assert_eq!(
            ended(source),
            "feature('f', { then() {\n  `a ${ {b: 1}.b } c` == d;\n  /x}/.test(e) && /[/]/.test(f);\n  (g)\n} })"
        );
    }
}
