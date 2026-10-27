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

//! The query-string subset existing screens and services send, parsed into a [`Query`].
//!
//! ```text
//! query   = clause*                         juxtaposition is OR, as Lucene's default operator
//! clause  = [conj] [mod] (field ":")? unary
//! conj    = "AND" | "&&" | "OR" | "||"
//! mod     = "+" | "-" | "NOT" | "!"
//! unary   = "(" query ")" | range | quoted | term
//! range   = ("[" | "{") bound "TO" bound ("]" | "}")
//! term    = chars, "\" escapes; "*" and "?" are wildcards
//! ```
//!
//! Clauses combine as Lucene's classic parser does: `AND` makes the clauses on both sides
//! required, `+` requires one, `-` and `NOT` prohibit one, and the rest are optional. A group with
//! only optional clauses needs one of them. `_exists_:field`, `field:*`, `*:*`, and a lone `*` are
//! the existence and match-all forms. `*:term` searches every field, as a bare term does.
//!
//! Bare fuzzy (`term~`) and positive boosts (`clause^2`) are supported. Numbered fuzziness, phrase slop and regex are errors.

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::query::{Query, TextFields, TextMatch};

/// Parse a query string. Empty is match-all.
///
/// # Errors
///
/// Returns an error naming the clause that is not in the supported subset.
pub fn parse_query_string(input: &str) -> Result<Query, Error> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0, input };
    let query = parser.query(None, 0)?;
    if parser.pos < parser.tokens.len() {
        return Err(parser.error_at("unexpected ')'"));
    }
    Ok(query)
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Not,
    Plus,
    Minus,
    Fuzzy,
    Boost(String),
    /// A word and whether it had a quote. Escapes are already removed; `wild` marks unescaped
    /// wildcard characters by position.
    Word {
        text: String,
        quoted: bool,
        wild: Vec<bool>,
    },
    /// `field:` before a clause.
    Field(String),
    Range {
        low: String,
        high: String,
        low_inclusive: bool,
        high_inclusive: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Occur {
    Must,
    Should,
    MustNot,
}

fn is_special(ch: char) -> bool {
    matches!(ch, '(' | ')' | '"' | ':' | '[' | ']' | '{' | '}' | '^' | '~' | '\\')
}

fn tokenize(input: &str) -> Result<Vec<(Token, usize)>, Error> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        match ch {
            '(' => {
                tokens.push((Token::Open, start));
                index += 1;
            }
            ')' => {
                tokens.push((Token::Close, start));
                index += 1;
            }
            '&' if chars.get(index + 1) == Some(&'&') => {
                tokens.push((Token::And, start));
                index += 2;
            }
            '|' if chars.get(index + 1) == Some(&'|') => {
                tokens.push((Token::Or, start));
                index += 2;
            }
            '+' | '-' | '!' if at_clause_start(&tokens) && next_starts_clause(&chars, index + 1) => {
                tokens.push((
                    match ch {
                        '+' => Token::Plus,
                        '-' => Token::Minus,
                        _ => Token::Not,
                    },
                    start,
                ));
                index += 1;
            }
            '"' => {
                let mut text = String::new();
                index += 1;
                loop {
                    match chars.get(index) {
                        None => return Err(clause_error(input, start, "unclosed quote")),
                        Some('\\') => {
                            if let Some(next) = chars.get(index + 1) {
                                text.push(*next);
                            }
                            index += 2;
                        }
                        Some('"') => {
                            index += 1;
                            break;
                        }
                        Some(other) => {
                            text.push(*other);
                            index += 1;
                        }
                    }
                }
                let wild = vec![false; text.chars().count()];
                tokens.push((
                    Token::Word {
                        text,
                        quoted: true,
                        wild,
                    },
                    start,
                ));
            }
            '[' | '{' => {
                let close = chars[index + 1..]
                    .iter()
                    .position(|item| *item == ']' || *item == '}')
                    .map(|offset| index + 1 + offset)
                    .ok_or_else(|| clause_error(input, start, "unclosed range"))?;
                let body: String = chars[index + 1..close].iter().collect();
                let parts: Vec<&str> = body.split_whitespace().collect();
                if parts.len() != 3 || parts[1] != "TO" {
                    return Err(clause_error(input, start, "a range is [low TO high]"));
                }
                tokens.push((
                    Token::Range {
                        low: unquote(parts[0]),
                        high: unquote(parts[2]),
                        low_inclusive: ch == '[',
                        high_inclusive: chars[close] == ']',
                    },
                    start,
                ));
                index = close + 1;
            }
            '~' => {
                index += 1;
                if chars.get(index).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
                    return Err(clause_error(
                        input,
                        start,
                        "numbered fuzziness and phrase slop are not supported",
                    ));
                }
                if chars
                    .get(index)
                    .is_some_and(|c| !c.is_whitespace() && !matches!(c, ')' | '^'))
                {
                    return Err(clause_error(input, start, "bare fuzzy suffix must end the term"));
                }
                tokens.push((Token::Fuzzy, start));
            }
            '^' => {
                index += 1;
                let number_start = index;
                while chars
                    .get(index)
                    .is_some_and(|c| !c.is_whitespace() && !matches!(c, ')' | '(' | '^' | '~'))
                {
                    index += 1;
                }
                let number: String = chars[number_start..index].iter().collect();
                if number
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .is_none()
                {
                    return Err(clause_error(input, start, "boost weight must be positive and finite"));
                }
                tokens.push((Token::Boost(number), start));
            }
            ']' | '}' => return Err(clause_error(input, start, "unexpected range close")),
            '/' => return Err(clause_error(input, start, "regular expressions are not supported")),
            ':' => return Err(clause_error(input, start, "a field name is missing before ':'")),
            _ => {
                let mut text = String::new();
                let mut wild = Vec::new();
                while index < chars.len() {
                    let item = chars[index];
                    if item.is_whitespace() || (is_special(item) && item != '\\') {
                        break;
                    }
                    if item == '\\' {
                        match chars.get(index + 1) {
                            Some(next) => {
                                text.push(*next);
                                wild.push(false);
                                index += 2;
                            }
                            None => return Err(clause_error(input, start, "a trailing '\\' escapes nothing")),
                        }
                        continue;
                    }
                    text.push(item);
                    wild.push(item == '*' || item == '?');
                    index += 1;
                }
                if chars.get(index) == Some(&':') && !text.is_empty() {
                    index += 1;
                    tokens.push((Token::Field(text), start));
                    continue;
                }
                let token = match text.as_str() {
                    "AND" => Token::And,
                    "OR" => Token::Or,
                    "NOT" => Token::Not,
                    _ => Token::Word {
                        text,
                        quoted: false,
                        wild,
                    },
                };
                tokens.push((token, start));
            }
        }
    }
    Ok(tokens)
}

fn at_clause_start(tokens: &[(Token, usize)]) -> bool {
    !matches!(tokens.last(), Some((Token::Field(_), _)))
}

fn next_starts_clause(chars: &[char], index: usize) -> bool {
    chars.get(index).is_some_and(|ch| !ch.is_whitespace())
}

fn unquote(text: &str) -> String {
    text.trim_matches('"').to_string()
}

fn clause_error(input: &str, at: usize, problem: &str) -> Error {
    let clause: String = input.chars().skip(at).take(40).collect();
    Error::new(format!("{problem} at position {at}: {clause}"))
}

struct Parser<'a> {
    tokens: Vec<(Token, usize)>,
    pos: usize,
    input: &'a str,
}

impl Parser<'_> {
    fn error_at(&self, problem: &str) -> Error {
        let at = self
            .tokens
            .get(self.pos)
            .map_or(self.input.chars().count(), |(_, at)| *at);
        clause_error(self.input, at, problem)
    }

    fn query(&mut self, field: Option<&str>, depth: usize) -> Result<Query, Error> {
        if depth > 64 {
            return Err(self.error_at("too many nested groups"));
        }
        let mut clauses: Vec<(Occur, Query)> = Vec::new();
        while let Some((token, _)) = self.tokens.get(self.pos).cloned() {
            if token == Token::Close {
                break;
            }
            let mut conj_and = false;
            match token {
                Token::And => {
                    conj_and = true;
                    self.pos += 1;
                }
                Token::Or => self.pos += 1,
                _ => {}
            }
            if (conj_and || matches!(token, Token::Or)) && clauses.is_empty() {
                return Err(self.error_at("AND or OR needs a clause before it"));
            }
            let mut occur = Occur::Should;
            let mut negated = false;
            loop {
                match self.tokens.get(self.pos).map(|(token, _)| token) {
                    Some(Token::Not) | Some(Token::Minus) => {
                        negated = true;
                        self.pos += 1;
                    }
                    Some(Token::Plus) => {
                        occur = Occur::Must;
                        self.pos += 1;
                    }
                    _ => break,
                }
            }
            if negated {
                occur = Occur::MustNot;
            }
            if conj_and {
                if let Some(last) = clauses.last_mut() {
                    if last.0 != Occur::MustNot {
                        last.0 = Occur::Must;
                    }
                }
                if occur != Occur::MustNot {
                    occur = Occur::Must;
                }
            }
            let clause = self.clause(field, depth)?;
            clauses.push((occur, clause));
        }
        Ok(combine(clauses))
    }

    fn clause(&mut self, field: Option<&str>, depth: usize) -> Result<Query, Error> {
        let fuzzy_word = matches!(self.tokens.get(self.pos), Some((Token::Word { quoted: false, wild, .. }, _)) if !wild.iter().any(|v| *v));
        let mut query = self.atom(field, depth)?;
        if matches!(self.tokens.get(self.pos), Some((Token::Fuzzy, _))) {
            if !fuzzy_word || field.is_some_and(|name| name.starts_with("attributes.")) {
                return Err(self.error_at("fuzzy suffix needs an unquoted term on a mapped textual field"));
            }
            let Query::Text {
                terms,
                fields,
                kind: TextMatch::Term,
            } = query
            else {
                return Err(self.error_at("fuzzy suffix needs a term"));
            };
            query = Query::FuzzyIn { terms, fields };
            self.pos += 1;
        }
        if let Some((Token::Boost(number), _)) = self.tokens.get(self.pos) {
            let weight = number
                .parse::<f64>()
                .map_err(|_| self.error_at("invalid boost weight"))?;
            query = Query::Boost {
                query: Box::new(query),
                weight,
            };
            self.pos += 1;
        }
        if matches!(self.tokens.get(self.pos), Some((Token::Fuzzy | Token::Boost(_), _))) {
            return Err(self.error_at("invalid repeated suffix"));
        }
        Ok(query)
    }

    fn atom(&mut self, field: Option<&str>, depth: usize) -> Result<Query, Error> {
        let Some((token, _)) = self.tokens.get(self.pos).cloned() else {
            return Err(self.error_at("a clause is missing"));
        };
        match token {
            Token::Field(name) => {
                self.pos += 1;
                if field.is_some() {
                    return Err(self.error_at("a field inside a field group is not supported"));
                }
                if name == "_exists_" {
                    return match self.tokens.get(self.pos).cloned() {
                        Some((Token::Word { text, .. }, _)) => {
                            self.pos += 1;
                            Ok(Query::exists(text))
                        }
                        _ => Err(self.error_at("_exists_ needs a field name")),
                    };
                }
                self.clause(Some(&name), depth)
            }
            Token::Open => {
                self.pos += 1;
                let inner = self.query(field, depth + 1)?;
                match self.tokens.get(self.pos) {
                    Some((Token::Close, _)) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => Err(self.error_at("a group is missing ')'")),
                }
            }
            Token::Range {
                low,
                high,
                low_inclusive,
                high_inclusive,
            } => {
                self.pos += 1;
                let name = field
                    .filter(|name| *name != "*")
                    .ok_or_else(|| self.error_at("a range needs a field"))?;
                let bound = |text: &str| -> Option<Value> {
                    if text == "*" || text.is_empty() {
                        None
                    } else {
                        Some(Value::string(text))
                    }
                };
                let (gt, gte) = if low_inclusive {
                    (None, bound(&low))
                } else {
                    (bound(&low), None)
                };
                let (lt, lte) = if high_inclusive {
                    (None, bound(&high))
                } else {
                    (bound(&high), None)
                };
                Ok(Query::range_full(name, gt, gte, lt, lte))
            }
            Token::Word { text, quoted, wild } => {
                self.pos += 1;
                Ok(word_query(field, &text, quoted, &wild))
            }
            Token::Fuzzy | Token::Boost(_) => Err(self.error_at("a suffix needs a clause before it")),
            Token::Close => Err(self.error_at("unexpected ')'")),
            Token::And | Token::Or | Token::Not | Token::Plus | Token::Minus => {
                Err(self.error_at("an operator needs a clause after it"))
            }
        }
    }
}

fn word_query(field: Option<&str>, text: &str, quoted: bool, wild: &[bool]) -> Query {
    let field = field.filter(|name| *name != "*");
    if !quoted && wild.len() == 1 && wild[0] {
        return match field {
            Some(name) => Query::exists(name),
            None => Query::MatchAll,
        };
    }
    let fields = match field {
        Some(name) => TextFields::Only(vec![name.to_string()]),
        None => TextFields::All,
    };
    if quoted {
        return Query::Text {
            terms: text.to_string(),
            fields,
            kind: TextMatch::Phrase,
        };
    }
    let chars: Vec<char> = text.chars().collect();
    let wild_count = wild.iter().filter(|flag| **flag).count();
    let last_star = chars.last() == Some(&'*') && wild.last() == Some(&true);
    let first_star = chars.first() == Some(&'*') && wild.first() == Some(&true);
    let (terms, kind) = if wild_count == 0 {
        (text.to_string(), TextMatch::Term)
    } else if wild_count == 1 && last_star {
        (chars[..chars.len() - 1].iter().collect(), TextMatch::Prefix)
    } else if wild_count == 2 && last_star && first_star && chars.len() > 2 {
        (chars[1..chars.len() - 1].iter().collect(), TextMatch::Contains)
    } else {
        // Keep the wildcard text, with literal * and ? escaped by a backslash.
        let mut out = String::new();
        for (ch, flag) in chars.iter().zip(wild) {
            if !flag && (*ch == '*' || *ch == '?' || *ch == '\\') {
                out.push('\\');
            }
            out.push(*ch);
        }
        (out, TextMatch::Wildcard)
    };
    Query::Text { terms, fields, kind }
}

fn combine(clauses: Vec<(Occur, Query)>) -> Query {
    if clauses.is_empty() {
        return Query::MatchAll;
    }
    if clauses.len() == 1 && clauses[0].0 != Occur::MustNot {
        return clauses
            .into_iter()
            .next()
            .map(|(_, query)| query)
            .unwrap_or(Query::MatchAll);
    }
    let mut all = Vec::new();
    let mut any = Vec::new();
    let mut none = Vec::new();
    for (occur, query) in clauses {
        match occur {
            Occur::Must => all.push(query),
            Occur::Should => any.push(query),
            Occur::MustNot => none.push(query),
        }
    }
    Query::Bool {
        all,
        any,
        none,
        min_any: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Query {
        parse_query_string(text).unwrap_or_else(|err| panic!("{text}: {err}"))
    }

    fn text(field: Option<&str>, terms: &str, kind: TextMatch) -> Query {
        Query::Text {
            terms: terms.into(),
            fields: field.map_or(TextFields::All, |name| TextFields::Only(vec![name.into()])),
            kind,
        }
    }

    #[test]
    fn auto_wildcard_party_search() {
        // elasticQueryAutoWildcard("john smith", true), wrapped and AND-ed as search#Party does.
        let query = parse("(*:john* *:smith*) AND roles.roleTypeId:Customer AND pseudoId:AB*");
        assert_eq!(
            query,
            Query::Bool {
                all: vec![
                    Query::any(vec![
                        text(None, "john", TextMatch::Prefix),
                        text(None, "smith", TextMatch::Prefix)
                    ]),
                    text(Some("roles.roleTypeId"), "Customer", TextMatch::Term),
                    text(Some("pseudoId"), "AB", TextMatch::Prefix),
                ],
                any: vec![],
                none: vec![],
                min_any: None,
            }
        );
    }

    #[test]
    fn field_groups_exists_and_match_all() {
        assert_eq!(
            parse("ownerPartyId:(ORG_A OR _NA_)"),
            Query::any(vec![
                text(Some("ownerPartyId"), "ORG_A", TextMatch::Term),
                text(Some("ownerPartyId"), "_NA_", TextMatch::Term)
            ])
        );
        assert_eq!(parse("_exists_:customerStatusId"), Query::exists("customerStatusId"));
        assert_eq!(parse("*:*"), Query::MatchAll);
        assert_eq!(parse("*"), Query::MatchAll);
        assert_eq!(parse(""), Query::MatchAll);
        assert_eq!(parse("statusId:*"), Query::exists("statusId"));
        assert_eq!(
            parse("combinedName:*mit*"),
            text(Some("combinedName"), "mit", TextMatch::Contains)
        );
    }

    #[test]
    fn modifiers_and_not() {
        assert_eq!(
            parse("shoe -red +size:10"),
            Query::Bool {
                all: vec![text(Some("size"), "10", TextMatch::Term)],
                any: vec![text(None, "shoe", TextMatch::Term)],
                none: vec![text(None, "red", TextMatch::Term)],
                min_any: None,
            }
        );
        assert_eq!(
            parse("a AND NOT b"),
            Query::Bool {
                all: vec![text(None, "a", TextMatch::Term)],
                any: vec![],
                none: vec![text(None, "b", TextMatch::Term)],
                min_any: None,
            }
        );
        // A hyphen inside a word is part of the word.
        assert_eq!(parse("foo-bar"), text(None, "foo-bar", TextMatch::Term));
        assert_eq!(
            parse("jo@example.com*"),
            text(None, "jo@example.com", TextMatch::Prefix)
        );
    }

    #[test]
    fn ranges_phrases_and_wildcards() {
        assert_eq!(
            parse("placedDate:[2026-01-01 TO *}"),
            Query::range_full("placedDate", None, Some(Value::string("2026-01-01")), None, None)
        );
        assert_eq!(
            parse("title:\"blue shoe\""),
            text(Some("title"), "blue shoe", TextMatch::Phrase)
        );
        assert_eq!(parse("te?t"), text(None, "te?t", TextMatch::Wildcard));
        assert_eq!(parse("a\\*b"), text(None, "a*b", TextMatch::Term));
    }

    #[test]
    fn errors_name_the_clause() {
        for bad in [
            "john~test",
            "john~1",
            "name^0",
            "name^NaN",
            "name^",
            "name^^2",
            "\"two words\"~",
            "a*~",
            "attributes.custom:hello~",
            "(a OR b",
            "a)",
            "AND b",
            "/re/",
            "x:[1 TO]",
            "\"open",
        ] {
            let err = parse_query_string(bad).unwrap_err().to_string();
            assert!(err.contains("position"), "{bad}: {err}");
        }
    }
}
