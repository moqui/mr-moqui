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

//! Full-string matcher for the `matches` operator.
//!
//! Java `String.matches` compiles the pattern with `java.util.regex` and requires
//! the whole input to match. `.` does not cross a line ending. There is no regex crate.
//!
//! Groups do not capture, so `(?:...)` is the same as `(...)` and a reluctant quantifier
//! accepts the same strings as a greedy one. Backreferences, lookaround, inline flags,
//! possessive quantifiers and escapes outside `\d\D\w\W\s\S\n\r\t\f` are errors rather
//! than literals, so an unsupported pattern fails closed.
//!
//! The parsed pattern is compiled to a Thompson NFA and matched by stepping the set of its states over the text, so
//! a match takes time in proportion to the text length times the pattern size, nested quantifiers or not. A counted
//! quantifier is written out once per count, so a pattern whose program would pass [`MAX_PROGRAM`] is an error too.

use crate::error::Error;

#[derive(Clone, Debug)]
enum Re {
    Empty,
    Lit(char),
    Any,
    /// A char hits when it is in `ranges`, or outside any one of the `outside` sets
    /// (`\D`, `\W`, `\S` inside a class).
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
        outside: Vec<Vec<(char, char)>>,
    },
    Seq(Vec<Re>),
    Alt(Vec<Re>),
    Repeat {
        inner: Box<Re>,
        min: u32,
        max: Option<u32>,
    },
    Start,
    End,
}

/// A pattern parsed and compiled once, for a caller that matches it many times.
#[derive(Clone, Debug)]
pub struct CompiledRegex(Option<Program>);

impl CompiledRegex {
    /// Parse `pattern`. An unsupported or bad pattern is an error, as [`is_full_match`] reports it.
    pub fn new(pattern: &str) -> Result<Self, Error> {
        if pattern.is_empty() {
            return Ok(Self(None));
        }
        Program::compile(&parse_regex(pattern)?).map(|program| Self(Some(program)))
    }

    /// Whether the whole of `text` matches, as Java's `String.matches`.
    pub fn is_full_match(&self, text: &str) -> bool {
        match &self.0 {
            None => text.is_empty(),
            Some(program) => program.is_full_match(&text.chars().collect::<Vec<char>>()),
        }
    }
}

pub fn is_full_match(text: &str, pattern: &str) -> Result<bool, Error> {
    Ok(CompiledRegex::new(pattern)?.is_full_match(text))
}

struct Parser {
    pat: Vec<char>,
    i: usize,
    /// Groups open in `parse_group`, bounded by [`MAX_GROUP_DEPTH`].
    depth: usize,
}

/// Deepest group nesting a pattern may have. Parsing and matching recurse on it, and a stack overflow aborts the
/// process, so a deeper pattern is an error, as an unsupported one is.
const MAX_GROUP_DEPTH: usize = 256;

fn parse_regex(pattern: &str) -> Result<Re, Error> {
    let mut parser = Parser {
        pat: pattern.chars().collect(),
        i: 0,
        depth: 0,
    };
    let re = parser.parse_alt()?;
    if parser.i != parser.pat.len() {
        return Err(Error::new("unmatched ')' in regex"));
    }
    Ok(re)
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.pat.get(self.i).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.pat.get(self.i + offset).copied()
    }

    fn parse_alt(&mut self) -> Result<Re, Error> {
        let first = self.parse_seq()?;
        if self.peek() != Some('|') {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.parse_seq()?);
        }
        Ok(Re::Alt(alts))
    }

    fn parse_seq(&mut self) -> Result<Re, Error> {
        let mut items = Vec::new();
        while let Some(ch) = self.peek() {
            if ch == '|' || ch == ')' {
                break;
            }
            items.push(self.parse_repeat()?);
        }
        Ok(collapse(items))
    }

    fn parse_repeat(&mut self) -> Result<Re, Error> {
        let atom = self.parse_atom()?;
        let (min, max) = match self.peek() {
            Some('*') => {
                self.i += 1;
                (0, None)
            }
            Some('+') => {
                self.i += 1;
                (1, None)
            }
            Some('?') => {
                self.i += 1;
                (0, Some(1))
            }
            Some('{') => self.parse_bounds()?,
            _ => return Ok(atom),
        };
        match self.peek() {
            Some('?') => self.i += 1,
            Some('+') => return Err(Error::new("possessive quantifiers are not supported in regex")),
            _ => {}
        }
        Ok(Re::Repeat {
            inner: Box::new(atom),
            min,
            max,
        })
    }

    fn parse_bounds(&mut self) -> Result<(u32, Option<u32>), Error> {
        self.i += 1;
        let min = self.take_u32()?;
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok((min, Some(min)));
        }
        if self.peek() != Some(',') {
            return Err(Error::new("invalid '{n,m}' quantifier"));
        }
        self.i += 1;
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok((min, None));
        }
        let max = self.take_u32()?;
        if self.peek() != Some('}') || max < min {
            return Err(Error::new("invalid '{n,m}' quantifier"));
        }
        self.i += 1;
        Ok((min, Some(max)))
    }

    fn take_u32(&mut self) -> Result<u32, Error> {
        let start = self.i;
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == start {
            return Err(Error::new("invalid '{n,m}' quantifier"));
        }
        self.pat[start..self.i]
            .iter()
            .collect::<String>()
            .parse()
            .map_err(|_| Error::new("invalid '{n,m}' quantifier"))
    }

    fn parse_atom(&mut self) -> Result<Re, Error> {
        let ch = self.peek().ok_or_else(|| Error::new("regex ended before an atom"))?;
        match ch {
            '(' => self.parse_group(),
            '[' => self.parse_class(),
            '.' => {
                self.i += 1;
                Ok(Re::Any)
            }
            '^' => {
                self.i += 1;
                Ok(Re::Start)
            }
            '$' => {
                self.i += 1;
                Ok(Re::End)
            }
            '\\' => {
                self.i += 1;
                let esc = self.peek().ok_or_else(|| Error::new("dangling '\\' in regex"))?;
                self.i += 1;
                escape_atom(esc)
            }
            ')' | '|' | '*' | '+' | '?' | '{' => Err(Error::new(format!("unexpected '{ch}' in regex"))),
            _ => {
                self.i += 1;
                Ok(Re::Lit(ch))
            }
        }
    }

    fn parse_group(&mut self) -> Result<Re, Error> {
        self.i += 1;
        if self.peek() == Some('?') {
            if self.peek_at(1) != Some(':') {
                return Err(Error::new(
                    "lookaround, named groups and inline flags are not supported in regex",
                ));
            }
            self.i += 2;
        }
        if self.depth >= MAX_GROUP_DEPTH {
            return Err(Error::new("groups nested too deeply in regex"));
        }
        self.depth += 1;
        let inner = self.parse_alt();
        self.depth -= 1;
        let inner = inner?;
        if self.peek() != Some(')') {
            return Err(Error::new("unclosed '(' in regex"));
        }
        self.i += 1;
        Ok(inner)
    }

    fn parse_class(&mut self) -> Result<Re, Error> {
        self.i += 1;
        let negated = if self.peek() == Some('^') {
            self.i += 1;
            true
        } else {
            false
        };
        let mut ranges = Vec::new();
        let mut outside = Vec::new();
        if self.peek() == Some(']') {
            ranges.push((']', ']'));
            self.i += 1;
        }
        while let Some(ch) = self.peek() {
            if ch == ']' {
                break;
            }
            if ch == '[' || (ch == '&' && self.peek_at(1) == Some('&')) {
                return Err(Error::new("nested classes and '&&' are not supported in regex"));
            }
            let start = match self.class_item()? {
                ClassItem::Char(start) => start,
                ClassItem::Set {
                    negated: false,
                    ranges: set,
                } => {
                    ranges.extend(set);
                    continue;
                }
                ClassItem::Set {
                    negated: true,
                    ranges: set,
                } => {
                    outside.push(set);
                    continue;
                }
            };
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|next| next != ']') {
                self.i += 1;
                let ClassItem::Char(end) = self.class_item()? else {
                    return Err(Error::new("illegal character range in regex"));
                };
                if end < start {
                    return Err(Error::new("illegal character range in regex"));
                }
                ranges.push((start, end));
            } else {
                ranges.push((start, start));
            }
        }
        if self.peek() != Some(']') {
            return Err(Error::new("unclosed '[' in regex"));
        }
        self.i += 1;
        Ok(Re::Class {
            negated,
            ranges,
            outside,
        })
    }

    fn class_item(&mut self) -> Result<ClassItem, Error> {
        let ch = self.peek().ok_or_else(|| Error::new("unclosed '[' in regex"))?;
        self.i += 1;
        if ch != '\\' {
            return Ok(ClassItem::Char(ch));
        }
        let esc = self
            .peek()
            .ok_or_else(|| Error::new("dangling '\\' in character class"))?;
        self.i += 1;
        Ok(match escape_atom(esc)? {
            Re::Lit(lit) => ClassItem::Char(lit),
            Re::Class { negated, ranges, .. } => ClassItem::Set { negated, ranges },
            _ => return Err(Error::new(format!("unsupported escape '\\{esc}' in character class"))),
        })
    }
}

enum ClassItem {
    Char(char),
    Set { negated: bool, ranges: Vec<(char, char)> },
}

fn collapse(mut items: Vec<Re>) -> Re {
    if items.len() > 1 {
        Re::Seq(items)
    } else {
        items.pop().unwrap_or(Re::Empty)
    }
}

/// Java rejects an escaped letter it does not define and reads an escaped digit as a
/// backreference or octal value. Neither is supported here, so both are errors.
fn escape_atom(ch: char) -> Result<Re, Error> {
    Ok(match ch {
        'd' => shorthand(false, &[('0', '9')]),
        'D' => shorthand(true, &[('0', '9')]),
        'w' => shorthand(false, WORD),
        'W' => shorthand(true, WORD),
        's' => shorthand(false, SPACE),
        'S' => shorthand(true, SPACE),
        'n' => Re::Lit('\n'),
        'r' => Re::Lit('\r'),
        't' => Re::Lit('\t'),
        'f' => Re::Lit('\u{000c}'),
        other if other.is_ascii_alphanumeric() => {
            return Err(Error::new(format!("unsupported escape '\\{other}' in regex")));
        }
        other => Re::Lit(other),
    })
}

const WORD: &[(char, char)] = &[('A', 'Z'), ('a', 'z'), ('0', '9'), ('_', '_')];

const SPACE: &[(char, char)] = &[
    (' ', ' '),
    ('\t', '\t'),
    ('\n', '\n'),
    ('\u{000b}', '\u{000b}'),
    ('\u{000c}', '\u{000c}'),
    ('\r', '\r'),
];

fn shorthand(negated: bool, ranges: &[(char, char)]) -> Re {
    Re::Class {
        negated,
        ranges: ranges.to_vec(),
        outside: Vec::new(),
    }
}

fn class_hit(ch: char, ranges: &[(char, char)]) -> bool {
    ranges.iter().any(|(start, end)| ch >= *start && ch <= *end)
}

fn is_line_end(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

/// Without `MULTILINE`, Java `$` also matches before one line terminator that ends the input.
fn is_final_terminator(rest: &[char]) -> bool {
    match rest {
        [ch] => is_line_end(*ch),
        ['\r', '\n'] => true,
        _ => false,
    }
}

/// Most instructions a compiled pattern may have. Counted quantifiers are written out, so `(a{1000}){1000}` would be a
/// million; such a pattern is an error rather than a program that large.
const MAX_PROGRAM: usize = 100_000;

/// One instruction of the NFA. Execution starts at 0 and accepts on reaching [`Inst::Match`] at the end of the text.
#[derive(Clone, Debug)]
enum Inst {
    /// Consume one char that passes the test, then go on to the next instruction.
    Char(CharTest),
    /// Go on at both.
    Split(usize, usize),
    Jump(usize),
    /// `^`: only at the start of the text.
    Start,
    /// `$`: at the end of the text, or before one line terminator that ends it.
    End,
    Match,
}

#[derive(Clone, Debug)]
enum CharTest {
    Lit(char),
    /// `.`: any char but a line terminator.
    Any,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
        outside: Vec<Vec<(char, char)>>,
    },
}

impl CharTest {
    fn hits(&self, ch: char) -> bool {
        match self {
            Self::Lit(lit) => *lit == ch,
            Self::Any => !is_line_end(ch),
            Self::Class {
                negated,
                ranges,
                outside,
            } => (class_hit(ch, ranges) || outside.iter().any(|set| !class_hit(ch, set))) ^ negated,
        }
    }
}

#[derive(Clone, Debug)]
struct Program(Vec<Inst>);

impl Program {
    fn compile(re: &Re) -> Result<Self, Error> {
        let mut code = Vec::new();
        emit(re, &mut code)?;
        code.push(Inst::Match);
        Ok(Self(code))
    }

    /// Step the set of live states over `text`: a state is kept once per position, so the work is the text length
    /// times the program length.
    fn is_full_match(&self, text: &[char]) -> bool {
        let mut seen = vec![usize::MAX; self.0.len()];
        let mut current = Vec::new();
        let mut stack = Vec::new();
        self.closure(0, 0, text, &mut seen, &mut stack, &mut current);
        for (pos, ch) in text.iter().enumerate() {
            let mut next = Vec::new();
            for pc in std::mem::take(&mut current) {
                if let Inst::Char(test) = &self.0[pc] {
                    if test.hits(*ch) {
                        self.closure(pc + 1, pos + 1, text, &mut seen, &mut stack, &mut next);
                    }
                }
            }
            if next.is_empty() {
                return false;
            }
            current = next;
        }
        current.iter().any(|pc| matches!(self.0[*pc], Inst::Match))
    }

    /// Add to `out` the char and match states reached from `start` at `pos` without consuming a char. `seen` holds the
    /// position each state was last added at, so a state is added once per position and an empty loop ends.
    fn closure(
        &self,
        start: usize,
        pos: usize,
        text: &[char],
        seen: &mut [usize],
        stack: &mut Vec<usize>,
        out: &mut Vec<usize>,
    ) {
        stack.push(start);
        while let Some(pc) = stack.pop() {
            if seen[pc] == pos {
                continue;
            }
            seen[pc] = pos;
            match &self.0[pc] {
                Inst::Char(_) | Inst::Match => out.push(pc),
                Inst::Jump(to) => stack.push(*to),
                Inst::Split(first, second) => {
                    stack.push(*second);
                    stack.push(*first);
                }
                Inst::Start => {
                    if pos == 0 {
                        stack.push(pc + 1);
                    }
                }
                Inst::End => {
                    if pos == text.len() || is_final_terminator(&text[pos..]) {
                        stack.push(pc + 1);
                    }
                }
            }
        }
    }
}

/// Append the instructions of `re` to `code`. They run on into whatever follows.
fn emit(re: &Re, code: &mut Vec<Inst>) -> Result<(), Error> {
    if code.len() > MAX_PROGRAM {
        return Err(Error::new("regular expression is too large to match"));
    }
    match re {
        Re::Empty => {}
        Re::Lit(ch) => code.push(Inst::Char(CharTest::Lit(*ch))),
        Re::Any => code.push(Inst::Char(CharTest::Any)),
        Re::Class {
            negated,
            ranges,
            outside,
        } => code.push(Inst::Char(CharTest::Class {
            negated: *negated,
            ranges: ranges.clone(),
            outside: outside.clone(),
        })),
        Re::Start => code.push(Inst::Start),
        Re::End => code.push(Inst::End),
        Re::Seq(items) => {
            for item in items {
                emit(item, code)?;
            }
        }
        Re::Alt(alts) => {
            // Split to each alternative but the last, which the last split's second branch runs; each ends with a
            // jump past the others.
            let mut exits = Vec::new();
            for (index, alt) in alts.iter().enumerate() {
                if index + 1 < alts.len() {
                    let split = code.len();
                    code.push(Inst::Split(split + 1, 0));
                    emit(alt, code)?;
                    exits.push(code.len());
                    code.push(Inst::Jump(0));
                    let next = code.len();
                    code[split] = Inst::Split(split + 1, next);
                } else {
                    emit(alt, code)?;
                }
            }
            let end = code.len();
            for exit in exits {
                code[exit] = Inst::Jump(end);
            }
        }
        Re::Repeat { inner, min, max } => {
            for _ in 0..*min {
                emit(inner, code)?;
            }
            match max {
                // A loop: split to the body or out, and the body jumps back to the split.
                None => {
                    let split = code.len();
                    code.push(Inst::Split(split + 1, 0));
                    emit(inner, code)?;
                    code.push(Inst::Jump(split));
                    let out = code.len();
                    code[split] = Inst::Split(split + 1, out);
                }
                // Each optional copy may stop, which skips the copies after it.
                Some(max) => {
                    let mut splits = Vec::new();
                    for _ in *min..*max {
                        splits.push(code.len());
                        code.push(Inst::Split(code.len() + 1, 0));
                        emit(inner, code)?;
                    }
                    let out = code.len();
                    for split in splits {
                        code[split] = Inst::Split(split + 1, out);
                    }
                }
            }
        }
    }
    if code.len() > MAX_PROGRAM {
        return Err(Error::new("regular expression is too large to match"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_full_match;

    #[test]
    fn deeply_nested_groups_are_an_error_not_a_stack_overflow() {
        let deep = format!("{}a{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(is_full_match("a", &deep).is_err());
        let fine = format!("{}a{}", "(".repeat(50), ")".repeat(50));
        assert!(is_full_match("a", &fine).unwrap());
    }

    fn hit(text: &str, pattern: &str) -> bool {
        is_full_match(text, pattern).unwrap()
    }

    #[test]
    fn stock_moqui_patterns() {
        let wiki = r"[\w\.\-,':()!\? ]*";
        assert!(hit("Page1", wiki));
        assert!(hit("My Page (v2)!", wiki));
        assert!(hit("it's_a-b.c,d:e?", wiki));
        assert!(!hit("bad/name", wiki));
        let phone = r"^\+?\d[-\. \d]*\d\d$";
        assert!(hit("+1 801-555.1234", phone));
        assert!(!hit("801x5551234", phone));
        assert!(hit("snake_case9", "[a-z][a-z0-9_]*"));
        assert!(!hit("Snake", "[a-z][a-z0-9_]*"));
        assert!(hit("org.moqui2", r"[a-z][a-z0-9\.]*"));
        assert!(hit("camelCase_1", r"[A-Za-z]\w*"));
        assert!(hit("Pascal", r"[A-Z]\w*"));
        assert!(!hit("pascal", r"[A-Z]\w*"));
    }

    #[test]
    fn shorthands_inside_classes() {
        assert!(hit("a1 _", r"[\d\w\s]+"));
        assert!(!hit("-", r"[\d\w\s]"));
        assert!(hit("x", r"[\D]"));
        assert!(!hit("5", r"[\D]"));
        assert!(hit("5", r"[^\D]"));
        assert!(hit("-", r"[\d-]"));
        assert!(hit("\u{000b}", r"\s"));
        assert!(is_full_match("a", r"[\d-z]").is_ok());
        assert!(is_full_match("a", r"[a-\d]").is_err());
    }

    #[test]
    fn groups_quantifiers_and_anchors() {
        assert!(hit("abab", "(?:ab)+"));
        assert!(hit("aaa", "a*?"));
        assert!(hit("ab", "a{1,2}?b"));
        assert!(hit("a\n", "a$\n"));
        assert!(!hit("a\n", "a$"));
    }

    /// Nested quantifiers that a backtracking or position-set matcher takes polynomial or exponential time on finish at
    /// once, and a counted quantifier too large to write out is an error.
    #[test]
    fn nested_quantifiers_match_in_linear_time() {
        let long = "a".repeat(5000);
        let started = std::time::Instant::now();
        assert!(!hit(&format!("{long}b"), "(a*)*"));
        assert!(hit(&long, "(a*)*"));
        assert!(!hit(&format!("{long}!"), "((a+)+)+b"));
        assert!(hit(&format!("{long}b"), "(a|aa)*b"));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(is_full_match("a", "(a{1000}){1000}").is_err());
        assert!(hit(&"ab".repeat(3), "(ab){2,4}"));
        assert!(!hit(&"ab".repeat(5), "(ab){2,4}"));
        assert!(hit("", "(ab){0,2}") && hit("abc", "a(x|y|z|b)c") && !hit("ac", "a(x|y)c"));
        assert!(hit("ac", "a(|x)c") && hit("abc", "^abc$"));
    }

    #[test]
    fn unsupported_features_fail_closed() {
        for pattern in [
            r"(a)\1", r"\bword", r"a*+", "(?=a)a", "(?i)a", r"\p{L}", "[a[b]]", "[a&&b]", r"\x41",
        ] {
            assert!(is_full_match("a1", pattern).is_err(), "{pattern}");
        }
    }
}
