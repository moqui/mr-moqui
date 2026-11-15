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

//! Light lexical highlighting for the `code` component: one line at a time, with a small state carried from the line
//! before for what runs over lines (a comment, CDATA, an XML tag and its attribute strings, a JavaScript template
//! string, a Markdown fence). There is no parser: a line gets a theme role for each of its characters.

use crate::theme::Role;

/// The languages the component knows; any other name is plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    Xml,
    Js,
    Json,
    Sql,
    Md,
    Plain,
}

impl Language {
    pub(crate) fn of(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "xml" | "xsd" | "html" | "xhtml" | "svg" => Self::Xml,
            "js" | "javascript" | "mjs" | "ts" | "typescript" => Self::Js,
            "json" => Self::Json,
            "sql" => Self::Sql,
            "md" | "markdown" => Self::Md,
            _ => Self::Plain,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Xml => "xml",
            Self::Js => "js",
            Self::Json => "json",
            Self::Sql => "sql",
            Self::Md => "md",
            Self::Plain => "plain",
        }
    }
}

/// What a line leaves open for the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Carry {
    #[default]
    Normal,
    /// `<!-- -->` in XML, `/* */` in JavaScript and SQL.
    Comment,
    Cdata,
    /// Inside an XML tag, among its attributes.
    Tag,
    /// Inside an XML attribute's value, quoted with the char.
    TagString(char),
    /// A JavaScript template string.
    Template,
    /// A Markdown code fence.
    Fence,
}

/// The look of one character: its role and whether it is bold.
pub(crate) type Paint = (Role, bool);

const TEXT: Paint = (Role::Text, false);
const MUTED: Paint = (Role::Muted, false);
const TAG: Paint = (Role::Primary, false);
const NAME: Paint = (Role::Info, false);
const STRING: Paint = (Role::Positive, false);
const EXPR: Paint = (Role::Warning, false);
const KEYWORD: Paint = (Role::Secondary, true);
const NUMBER: Paint = (Role::Info, false);
const HEADING: Paint = (Role::Primary, true);

pub(super) const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

pub(super) const SQL_KEYWORDS: &[&str] = &[
    "all",
    "alter",
    "and",
    "as",
    "asc",
    "avg",
    "between",
    "by",
    "case",
    "count",
    "create",
    "default",
    "delete",
    "desc",
    "distinct",
    "drop",
    "else",
    "end",
    "exists",
    "foreign",
    "from",
    "group",
    "having",
    "in",
    "index",
    "inner",
    "insert",
    "into",
    "is",
    "join",
    "key",
    "left",
    "like",
    "limit",
    "max",
    "min",
    "not",
    "null",
    "offset",
    "on",
    "or",
    "order",
    "outer",
    "primary",
    "references",
    "right",
    "select",
    "set",
    "sum",
    "table",
    "then",
    "union",
    "update",
    "values",
    "when",
    "where",
    "with",
];

/// The look of each character of `text`, and what it leaves open for the next line.
pub(crate) fn line(language: Language, text: &str, carry: Carry) -> (Vec<Paint>, Carry) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![TEXT; chars.len()];
    let next = match language {
        Language::Xml => xml(&chars, carry, &mut out),
        Language::Js => js(&chars, carry, &mut out),
        Language::Json => json(&chars, &mut out),
        Language::Sql => sql(&chars, carry, &mut out),
        Language::Md => md(&chars, carry, &mut out),
        Language::Plain => Carry::Normal,
    };
    (out, next)
}

/// Whether `with` is at `at` of `chars`.
pub(crate) fn starts(chars: &[char], at: usize, with: &str) -> bool {
    with.chars()
        .enumerate()
        .all(|(index, ch)| chars.get(at + index) == Some(&ch))
}

fn paint(out: &mut [Paint], from: usize, to: usize, look: Paint) {
    let to = to.min(out.len());
    for cell in out.iter_mut().take(to).skip(from) {
        *cell = look;
    }
}

pub(crate) fn is_name_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')
}

fn is_ident(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// A `${...}` from `at`, to its closing brace or the end of the line: the end.
fn expression(chars: &[char], at: usize) -> usize {
    let mut depth = 0;
    let mut index = at + 1;
    while index < chars.len() {
        match chars[index] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    chars.len()
}

/// A quoted string from `at` (the opening quote) to its closing quote, a backslash escaping the next char: the end.
fn quoted(chars: &[char], at: usize, escapes: bool) -> usize {
    let quote = chars[at];
    let mut index = at + 1;
    while index < chars.len() {
        if escapes && chars[index] == '\\' {
            index += 2;
            continue;
        }
        if chars[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    chars.len()
}

fn xml(chars: &[char], mut carry: Carry, out: &mut [Paint]) -> Carry {
    let len = chars.len();
    let mut at = 0;
    while at < len {
        let ch = chars[at];
        match carry {
            Carry::Comment => {
                if starts(chars, at, "-->") {
                    paint(out, at, at + 3, MUTED);
                    at += 3;
                    carry = Carry::Normal;
                } else {
                    out[at] = MUTED;
                    at += 1;
                }
            }
            Carry::Cdata => {
                if starts(chars, at, "]]>") {
                    paint(out, at, at + 3, MUTED);
                    at += 3;
                    carry = Carry::Normal;
                } else if starts(chars, at, "${") {
                    let end = expression(chars, at);
                    paint(out, at, end, EXPR);
                    at = end;
                } else {
                    at += 1;
                }
            }
            Carry::Tag => {
                if ch == '>' {
                    out[at] = TAG;
                    at += 1;
                    carry = Carry::Normal;
                } else if starts(chars, at, "/>") || starts(chars, at, "?>") {
                    paint(out, at, at + 2, TAG);
                    at += 2;
                    carry = Carry::Normal;
                } else if ch == '"' || ch == '\'' {
                    out[at] = STRING;
                    at += 1;
                    carry = Carry::TagString(ch);
                } else if ch == '=' {
                    out[at] = MUTED;
                    at += 1;
                } else if is_name_char(ch) {
                    let start = at;
                    while at < len && is_name_char(chars[at]) {
                        at += 1;
                    }
                    paint(out, start, at, NAME);
                } else {
                    at += 1;
                }
            }
            Carry::TagString(quote) => {
                if ch == quote {
                    out[at] = STRING;
                    at += 1;
                    carry = Carry::Tag;
                } else if starts(chars, at, "${") {
                    let end = expression(chars, at);
                    paint(out, at, end, EXPR);
                    at = end;
                } else {
                    out[at] = STRING;
                    at += 1;
                }
            }
            Carry::Normal | Carry::Template | Carry::Fence => {
                carry = Carry::Normal;
                if starts(chars, at, "<!--") {
                    paint(out, at, at + 4, MUTED);
                    at += 4;
                    carry = Carry::Comment;
                } else if starts(chars, at, "<![CDATA[") {
                    paint(out, at, at + 9, MUTED);
                    at += 9;
                    carry = Carry::Cdata;
                } else if ch == '<'
                    && chars
                        .get(at + 1)
                        .is_some_and(|next| next.is_alphabetic() || matches!(next, '_' | '/' | '?' | '!'))
                {
                    let start = at;
                    at += 1;
                    if matches!(chars[at], '/' | '?' | '!') {
                        at += 1;
                    }
                    while at < len && is_name_char(chars[at]) {
                        at += 1;
                    }
                    paint(out, start, at, TAG);
                    carry = Carry::Tag;
                } else if ch == '&' {
                    let end = chars[at..]
                        .iter()
                        .take(12)
                        .position(|ch| *ch == ';')
                        .map_or(at + 1, |found| at + found + 1);
                    paint(out, at, end, NAME);
                    at = end;
                } else if starts(chars, at, "${") {
                    let end = expression(chars, at);
                    paint(out, at, end, EXPR);
                    at = end;
                } else {
                    at += 1;
                }
            }
        }
    }
    carry
}

/// Words, numbers and the keywords of a C-like language, from `at`: the end of what was painted.
fn word(chars: &[char], at: usize, out: &mut [Paint], keyword: &dyn Fn(&str) -> bool) -> usize {
    let len = chars.len();
    let ch = chars[at];
    let after_ident = at > 0 && is_ident(chars[at - 1]);
    if ch.is_ascii_digit() && !after_ident {
        let mut end = at;
        while end < len && (chars[end].is_ascii_alphanumeric() || chars[end] == '.' || chars[end] == '_') {
            end += 1;
        }
        paint(out, at, end, NUMBER);
        return end;
    }
    if is_ident(ch) {
        let mut end = at;
        while end < len && is_ident(chars[end]) {
            end += 1;
        }
        let text: String = chars[at..end].iter().collect();
        if keyword(&text) {
            paint(out, at, end, KEYWORD);
        }
        return end;
    }
    at + 1
}

fn js(chars: &[char], mut carry: Carry, out: &mut [Paint]) -> Carry {
    let len = chars.len();
    let mut at = 0;
    let keyword = |word: &str| JS_KEYWORDS.binary_search(&word).is_ok();
    while at < len {
        let ch = chars[at];
        match carry {
            Carry::Comment => {
                if starts(chars, at, "*/") {
                    paint(out, at, at + 2, MUTED);
                    at += 2;
                    carry = Carry::Normal;
                } else {
                    out[at] = MUTED;
                    at += 1;
                }
            }
            Carry::Template => {
                if ch == '`' {
                    out[at] = STRING;
                    at += 1;
                    carry = Carry::Normal;
                } else if ch == '\\' {
                    paint(out, at, at + 2, STRING);
                    at += 2;
                } else if starts(chars, at, "${") {
                    let end = expression(chars, at);
                    paint(out, at, end, EXPR);
                    at = end;
                } else {
                    out[at] = STRING;
                    at += 1;
                }
            }
            _ => {
                carry = Carry::Normal;
                if starts(chars, at, "//") {
                    paint(out, at, len, MUTED);
                    at = len;
                } else if starts(chars, at, "/*") {
                    paint(out, at, at + 2, MUTED);
                    at += 2;
                    carry = Carry::Comment;
                } else if ch == '"' || ch == '\'' {
                    let end = quoted(chars, at, true);
                    paint(out, at, end, STRING);
                    at = end;
                } else if ch == '`' {
                    out[at] = STRING;
                    at += 1;
                    carry = Carry::Template;
                } else {
                    at = word(chars, at, out, &keyword);
                }
            }
        }
    }
    carry
}

fn json(chars: &[char], out: &mut [Paint]) -> Carry {
    let len = chars.len();
    let mut at = 0;
    let keyword = |word: &str| matches!(word, "true" | "false" | "null");
    while at < len {
        if chars[at] == '"' {
            let end = quoted(chars, at, true);
            let key = chars[end..].iter().find(|ch| !ch.is_whitespace()) == Some(&':');
            paint(out, at, end, if key { NAME } else { STRING });
            at = end;
        } else {
            at = word(chars, at, out, &keyword);
        }
    }
    Carry::Normal
}

fn sql(chars: &[char], mut carry: Carry, out: &mut [Paint]) -> Carry {
    let len = chars.len();
    let mut at = 0;
    let keyword = |word: &str| SQL_KEYWORDS.binary_search(&word.to_ascii_lowercase().as_str()).is_ok();
    while at < len {
        let ch = chars[at];
        if carry == Carry::Comment {
            if starts(chars, at, "*/") {
                paint(out, at, at + 2, MUTED);
                at += 2;
                carry = Carry::Normal;
            } else {
                out[at] = MUTED;
                at += 1;
            }
            continue;
        }
        carry = Carry::Normal;
        if starts(chars, at, "--") {
            paint(out, at, len, MUTED);
            at = len;
        } else if starts(chars, at, "/*") {
            paint(out, at, at + 2, MUTED);
            at += 2;
            carry = Carry::Comment;
        } else if ch == '\'' {
            // '' inside is a quote, which two strings side by side paint the same
            let end = quoted(chars, at, false);
            paint(out, at, end, STRING);
            at = end;
        } else if ch == '"' {
            let end = quoted(chars, at, false);
            paint(out, at, end, NAME);
            at = end;
        } else {
            at = word(chars, at, out, &keyword);
        }
    }
    carry
}

fn md(chars: &[char], carry: Carry, out: &mut [Paint]) -> Carry {
    let len = chars.len();
    let fence = starts(chars, 0, "```") || starts(chars, 0, "~~~");
    if carry == Carry::Fence {
        paint(out, 0, len, NUMBER);
        return if fence { Carry::Normal } else { Carry::Fence };
    }
    if fence {
        paint(out, 0, len, NUMBER);
        return Carry::Fence;
    }
    let first = chars.iter().position(|ch| !ch.is_whitespace()).unwrap_or(len);
    match chars.get(first) {
        Some('#') => {
            paint(out, 0, len, HEADING);
            return Carry::Normal;
        }
        Some('>') => {
            paint(out, first, first + 1, MUTED);
        }
        Some('-' | '*' | '+') if chars.get(first + 1) == Some(&' ') => {
            paint(out, first, first + 1, MUTED);
        }
        _ => {}
    }
    let mut at = first;
    while at < len {
        if chars[at] == '`' {
            let end = chars[at + 1..]
                .iter()
                .position(|ch| *ch == '`')
                .map_or(len, |found| at + found + 2);
            paint(out, at, end, NUMBER);
            at = end;
        } else {
            at += 1;
        }
    }
    Carry::Normal
}
