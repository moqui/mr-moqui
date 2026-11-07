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

//! The locales of a Constitution's text (plans/LLM-A2A-Assist.md 7.10, "Document and localization"): a tag normalized
//! as `language[_Script][_TERRITORY]`, and the text of a locale map resolved field by field, exact locale, then base
//! language, then the document's default locale.

use serde_json::Value;

/// `tag` normalized: `-` as `_`, the language in lower case, a four-letter script in title case, a territory (two
/// letters or three digits) in upper case. `None` for a tag that is not of that shape.
pub fn normalize(tag: &str) -> Option<String> {
    let tag = tag.trim().replace('-', "_");
    let mut parts = tag.split('_');
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = language.to_ascii_lowercase();
    let mut seen_script = false;
    let mut seen_territory = false;
    for part in parts {
        if part.len() == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) && !seen_script && !seen_territory {
            let mut chars = part.chars();
            out.push('_');
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
            }
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
            seen_script = true;
        } else if !seen_territory
            && ((part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()))
                || (part.len() == 3 && part.chars().all(|c| c.is_ascii_digit())))
        {
            out.push('_');
            out.push_str(&part.to_ascii_uppercase());
            seen_territory = true;
        } else {
            return None;
        }
    }
    Some(out)
}

/// The base language of a normalized locale (`es` of `es_MX`).
pub fn language(locale: &str) -> &str {
    locale.split('_').next().unwrap_or(locale)
}

/// The text of `map` (a locale map) for `requested`: the exact locale, then its base language, then `default_locale`.
/// Out: the text and the locale it came from; `None` when the map has none of them (an invalid document).
pub fn text<'a>(map: &'a Value, requested: &str, default_locale: &str) -> Option<(&'a str, String)> {
    let entries = map.as_object()?;
    let find = |wanted: &str| {
        entries.iter().find_map(|(key, value)| {
            let text = value.as_str().filter(|text| !text.trim().is_empty())?;
            (normalize(key).as_deref() == Some(wanted)).then(|| (text, wanted.to_string()))
        })
    };
    let requested = normalize(requested);
    if let Some(requested) = requested.as_deref() {
        if let Some(found) = find(requested) {
            return Some(found);
        }
        let base = language(requested);
        if base != requested {
            if let Some(found) = find(base) {
                return Some(found);
            }
        }
    }
    find(&normalize(default_locale)?)
}
