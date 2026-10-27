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

//! Highlight fragments for a hit, the Elastic shape `{field: ["... <em>term</em> ..."]}`.
//!
//! Matching words are wrapped in `<em>` and `</em>`, which `FindParty.xml` rewrites to
//! `<strong>`. Only scored text clauses highlight, and only on the fields they name (or every
//! field for an all-fields clause). The stored text is HTML-escaped around the tags, as Elastic's
//! `encoder: html` does, so a fragment is safe to render as HTML and a renderer must not escape it
//! again.

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use super::query::{Query, TextFields, TextMatch};
use crate::entity::document::Document;
use crate::screen::sri::encode_html;

const MAX_FRAGMENTS: usize = 5;

struct Term {
    fields: TextFields,
    kind: TextMatch,
    words: Vec<String>,
}

/// Fragments for each requested field that has a match. Fields with no match are left out.
pub fn highlights(query: &Query, fields: &[String], body: &Document) -> IndexMap<String, Vec<String>> {
    let mut terms = Vec::new();
    collect(query, false, &mut terms);
    let mut out = IndexMap::new();
    if terms.is_empty() {
        return out;
    }
    for field in fields {
        let name = field.strip_suffix(".keyword").unwrap_or(field);
        let applicable: Vec<&Term> = terms
            .iter()
            .filter(|term| match &term.fields {
                TextFields::All => true,
                TextFields::Only(names) => names
                    .iter()
                    .any(|item| item.strip_suffix(".keyword").unwrap_or(item) == name),
            })
            .collect();
        if applicable.is_empty() {
            continue;
        }
        let mut values = Vec::new();
        values_at(body, &name.split('.').collect::<Vec<_>>(), &mut values);
        let mut fragments = Vec::new();
        for value in values {
            if let Some(marked) = mark(&value, &applicable) {
                if !fragments.contains(&marked) {
                    fragments.push(marked);
                }
            }
            if fragments.len() >= MAX_FRAGMENTS {
                break;
            }
        }
        if !fragments.is_empty() {
            out.insert(field.clone(), fragments);
        }
    }
    out
}

fn collect(query: &Query, negated: bool, out: &mut Vec<Term>) {
    match query {
        Query::Text { terms, fields, kind } if !negated => out.push(Term {
            fields: fields.clone(),
            kind: *kind,
            words: words(terms).into_iter().map(|(_, word)| word.to_lowercase()).collect(),
        }),
        // Near marks its words wherever they are; the window is not re-checked here.
        Query::Near { terms, fields, .. } if !negated => out.push(Term {
            fields: fields.clone(),
            kind: TextMatch::Term,
            words: words(terms).into_iter().map(|(_, word)| word.to_lowercase()).collect(),
        }),
        Query::Bool { all, any, none, .. } => {
            for item in all.iter().chain(any) {
                collect(item, negated, out);
            }
            for item in none {
                collect(item, !negated, out);
            }
        }
        Query::Child { query, .. } | Query::Boost { query, .. } => collect(query, negated, out),
        _ => {}
    }
}

fn values_at(map: &Document, path: &[&str], out: &mut Vec<String>) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    match map.get(*first) {
        Some(Value::List(items)) if !rest.is_empty() => {
            for item in items {
                if let Value::Map(child) = item {
                    values_at(child, rest, out);
                }
            }
        }
        Some(Value::Map(child)) if !rest.is_empty() => values_at(child, rest, out),
        Some(Value::List(items)) => out.extend(items.iter().filter(|item| !item.is_null()).map(to_plain_string)),
        Some(Value::Null) | None => {}
        Some(value) if rest.is_empty() => out.push(to_plain_string(value)),
        Some(_) => {}
    }
}

/// Word spans: runs of alphanumeric characters, with their byte start.
fn words(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            if start.is_none() {
                start = Some(index);
            }
        } else if let Some(begin) = start.take() {
            out.push((begin, text[begin..index].to_string()));
        }
    }
    if let Some(begin) = start {
        out.push((begin, text[begin..].to_string()));
    }
    out
}

fn mark(value: &str, terms: &[&Term]) -> Option<String> {
    let spans = words(value);
    let lower: Vec<String> = spans.iter().map(|(_, word)| word.to_lowercase()).collect();
    let mut marked = vec![false; spans.len()];
    for term in terms {
        if term.kind == TextMatch::Phrase && term.words.len() > 1 {
            // Only where the phrase's words are next to each other, in order.
            let len = term.words.len();
            for start in 0..lower.len().saturating_sub(len - 1) {
                if lower[start..start + len] == term.words[..] {
                    marked[start..start + len].fill(true);
                }
            }
        } else {
            for (index, word) in lower.iter().enumerate() {
                if word_matches(term, word) {
                    marked[index] = true;
                }
            }
        }
    }
    if !marked.contains(&true) {
        return None;
    }
    let mut out = String::with_capacity(value.len() + 16);
    let mut last = 0;
    for ((start, word), _) in spans.iter().zip(&marked).filter(|(_, marked)| **marked) {
        out.push_str(&encode_html(&value[last..*start]));
        out.push_str("<em>");
        out.push_str(&encode_html(word));
        out.push_str("</em>");
        last = start + word.len();
    }
    out.push_str(&encode_html(&value[last..]));
    Some(out)
}

fn word_matches(term: &Term, word: &str) -> bool {
    term.words.iter().any(|wanted| match term.kind {
        TextMatch::Term | TextMatch::Phrase => word == wanted,
        TextMatch::Prefix => word.starts_with(wanted.as_str()),
        TextMatch::Contains => word.contains(wanted.as_str()),
        TextMatch::Wildcard => word.contains(wanted.as_str()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_matching_words_in_requested_fields() {
        let mut body = Document::new();
        body.insert("combinedName".into(), Value::string("John Smith"));
        let mut mech = Document::new();
        mech.insert("city".into(), Value::string("Johnstown"));
        body.insert("contactMechs".into(), Value::List(vec![Value::Map(mech)]));
        let query = Query::all(vec![
            Query::text("joh", TextMatch::Prefix),
            Query::exclude(Query::text("smith", TextMatch::Term)),
        ]);
        let fields = vec![
            "combinedName".to_string(),
            "contactMechs.city".to_string(),
            "pseudoId".to_string(),
        ];
        let out = highlights(&query, &fields, &body);
        assert_eq!(out["combinedName"], vec!["<em>John</em> Smith".to_string()]);
        assert_eq!(out["contactMechs.city"], vec!["<em>Johnstown</em>".to_string()]);
        assert!(!out.contains_key("pseudoId"));
    }

    #[test]
    fn marks_a_phrase_only_where_its_words_are_adjacent() {
        let mut body = Document::new();
        body.insert("noteText".into(), Value::string("Pump water, then the water pump"));
        body.insert("title".into(), Value::string("Water and pump"));
        let query = Query::text("water pump", TextMatch::Phrase);
        let fields = vec!["noteText".to_string(), "title".to_string()];
        let out = highlights(&query, &fields, &body);
        assert_eq!(
            out["noteText"],
            vec!["Pump water, then the <em>water</em> <em>pump</em>".to_string()]
        );
        assert!(!out.contains_key("title"));
    }

    #[test]
    fn stored_text_is_escaped_around_the_marks() {
        let mut body = Document::new();
        body.insert(
            "combinedName".into(),
            Value::string("<img src=x onerror=alert(1)> Smith & \"Jones\""),
        );
        let query = Query::text("smith", TextMatch::Term);
        let out = highlights(&query, &["combinedName".to_string()], &body);
        assert_eq!(
            out["combinedName"],
            vec!["&lt;img src=x onerror=alert(1)&gt; <em>Smith</em> &amp; &quot;Jones&quot;".to_string()]
        );
    }
}
