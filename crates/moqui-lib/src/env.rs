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

//! Property and environment lookup, and `${name}` expansion.
//!
//! Expansion is string replacement. It is not a script engine. Context expansion
//! (`ResourceFacade.expand`) stays in core.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::Error;
use crate::sync::lock;

static PROPERTIES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static DOTENV: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn with_properties<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut guard = lock(&PROPERTIES);
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Set a process property, the stand-in for `System.setProperty`.
///
/// An empty value is stored and then treated as missing by [`get_prop_or_env`].
/// Expansion cannot assign a name. This function is the explicit property table.
pub fn set_property(name: &str, value: impl Into<String>) {
    let value = value.into();
    with_properties(|map| {
        map.insert(name.to_string(), value);
    });
}

/// Remove a process property so a later lookup can see the environment.
pub fn clear_property(name: &str) {
    with_properties(|map| {
        map.remove(name);
    });
}

/// The process property table only. An empty value is missing.
///
/// This does not read the environment or dotenv. [`get_prop_or_env`] does, and that is why the runtime
/// conf uses this for `MOQUI_CONF`: `runtime/.env` is loaded later and must not select the conf file.
#[must_use]
pub fn get_property(name: &str) -> Option<String> {
    with_properties(|map| map.get(name).cloned()).filter(|value| !value.is_empty())
}

fn lookup_one(name: &str) -> Option<String> {
    let property = with_properties(|map| map.get(name).cloned());
    if let Some(value) = property {
        if !value.is_empty() {
            return Some(value);
        }
    }
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => return Some(value),
        _ => {}
    }
    let dotenv = with_dotenv(|map| map.get(name).cloned());
    if let Some(value) = dotenv {
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn with_dotenv<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut guard = lock(&DOTENV);
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Parse `KEY=value` lines into the dotenv table.
///
/// A process property and a real environment variable both win over these
/// values. Blank lines and `#` comments are ignored. A matching pair of quotes
/// is stripped. This does not expand `$` inside a value. Startup loads
/// `runtime/.env` through [`load_dotenv_file`]; that call is not made here.
pub fn load_dotenv(text: &str) {
    for (key, value) in parse_dotenv(text) {
        with_dotenv(|map| {
            map.insert(key, value);
        });
    }
}

/// The `KEY=value` pairs of dotenv text, in order, read as [`load_dotenv`] reads them, without loading them.
#[must_use]
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        let value = ['"', '\'']
            .into_iter()
            .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
            .unwrap_or(value);
        pairs.push((key.to_string(), value.to_string()));
    }
    pairs
}

/// Drop parsed dotenv values so the next [`load_dotenv`] or [`load_dotenv_file`] is the only source.
pub fn clear_dotenv() {
    let mut guard = lock(&DOTENV);
    if let Some(map) = guard.as_mut() {
        map.clear();
    }
}

pub fn load_dotenv_file(path: impl AsRef<std::path::Path>) -> Result<(), Error> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .map_err(|err| Error::new(format!("could not read dotenv {}", path.display())).with_source(err))?;
    load_dotenv(&text);
    Ok(())
}

/// Process property, then environment, then dotenv, then the same lookup with `_` replaced by `.`.
///
/// A missing name is `""`. An empty property or variable is missing.
pub fn get_prop_or_env(name: &str) -> String {
    if let Some(value) = lookup_one(name) {
        return value;
    }
    if let Some(dot) = name.find('_') {
        if dot < name.len() {
            let dotted = name.replace('_', ".");
            if dotted != name {
                if let Some(value) = lookup_one(&dotted) {
                    return value;
                }
            }
        }
    }
    String::new()
}

/// Replace `${name}` from [`get_prop_or_env`].
///
/// `name` is an identifier or a dotted property name. The Groovy Elvis form that
/// `MoquiDefaultConf.xml` uses also works: `${a ?: b}` and `${a ?: 'text'}` give the
/// first term that is not empty, and terms can chain. Anything else inside `${}`
/// is left unchanged, since Java evaluates it as Groovy. A missing name expands to
/// empty. Text with no `${` is returned as it is.
pub fn expand(value: &str) -> Result<String, Error> {
    if value.is_empty() || !value.contains("${") {
        return Ok(value.to_string());
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some((head, tail)) = rest.split_once("${") {
        out.push_str(head);
        let expanded = tail
            .split_once('}')
            .and_then(|(body, after)| Some((expand_body(body)?, after)));
        if let Some((expanded, after)) = expanded {
            out.push_str(&expanded);
            rest = after;
        } else {
            // Not a body it expands: the text stays as it is.
            out.push_str("${");
            rest = tail;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The value of a `${}` body, or `None` when it is not a name or an Elvis chain.
fn expand_body(body: &str) -> Option<String> {
    let mut last = String::new();
    for term in body.split("?:") {
        let term = term.trim();
        last = if is_expand_name(term) {
            get_prop_or_env(term)
        } else {
            quoted_literal(term)?.to_string()
        };
        if !last.is_empty() {
            break;
        }
    }
    Some(last)
}

/// The text of `'text'` or `"text"`. No escapes.
fn quoted_literal(term: &str) -> Option<&str> {
    ['\'', '"'].into_iter().find_map(|quote| {
        term.strip_prefix(quote)?
            .strip_suffix(quote)
            .filter(|inner| !inner.contains(quote))
    })
}

fn is_expand_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_beats_environment_and_underscore_falls_through_to_dots() {
        let key = "moqui_lib_env_probe_alpha";
        let dotted = "moqui.lib.env.probe.beta";
        clear_property(key);
        set_property(dotted, "from-dotted");
        assert_eq!(get_prop_or_env(key), "");
        assert_eq!(get_prop_or_env("moqui_lib_env_probe_beta"), "from-dotted");
        set_property(key, "from-prop");
        assert_eq!(get_prop_or_env(key), "from-prop");
        set_property(key, "");
        assert_eq!(get_prop_or_env(key), "");
        clear_property(key);
        clear_property(dotted);
    }

    #[test]
    fn environment_is_visible_when_no_property_is_set() {
        let path = get_prop_or_env("PATH");
        assert!(!path.is_empty());
    }

    #[test]
    fn dotenv_strips_one_pair_of_matching_quotes() {
        let pairs = parse_dotenv("A=\"x y\"\nB='q'\nC=\"\"\nD=\"\nE='mixed\"\nF= plain \nG=\"'in'\"");
        let pairs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("A", "x y"),
                ("B", "q"),
                ("C", ""),
                ("D", "\""),
                ("E", "'mixed\""),
                ("F", "plain"),
                ("G", "'in'")
            ]
        );
    }

    #[test]
    fn expand_replaces_names_only() {
        set_property("moqui_version", "3");
        set_property("nested.name", "inner");
        assert_eq!(expand("plain").unwrap(), "plain");
        assert_eq!(expand("v${moqui_version}").unwrap(), "v3");
        assert_eq!(expand("${missing_name_zz}").unwrap(), "");
        assert_eq!(expand("${nested.name}").unwrap(), "inner");
        assert_eq!(expand("${1+2}").unwrap(), "${1+2}");
        assert_eq!(expand("$${moqui_version}").unwrap(), "$3");
        assert_eq!(expand("${moqui_version").unwrap(), "${moqui_version");
        clear_property("moqui_version");
        clear_property("nested.name");
    }

    #[test]
    fn expand_takes_the_first_term_that_is_not_empty() {
        set_property("moqui_lib_env_port", "5433");
        set_property("moqui_lib_env_zone", "America/Denver");
        assert_eq!(expand("${moqui_lib_env_port?:'3306'}").unwrap(), "5433");
        assert_eq!(expand("${moqui_lib_env_none_zz?:'3306'}").unwrap(), "3306");
        assert_eq!(
            expand("${moqui_lib_env_none_zz ?: moqui_lib_env_zone}").unwrap(),
            "America/Denver"
        );
        assert_eq!(
            expand("${moqui_lib_env_none_zz ?: moqui_lib_env_none_yy ?: \"x\"}").unwrap(),
            "x"
        );
        assert_eq!(expand("${moqui_lib_env_none_zz ?: moqui_lib_env_none_yy}").unwrap(), "");
        assert_eq!(expand("${a ?: 1+2}").unwrap(), "${a ?: 1+2}");
        assert_eq!(expand("${a ?: 'x}").unwrap(), "${a ?: 'x}");
        clear_property("moqui_lib_env_port");
        clear_property("moqui_lib_env_zone");
    }

    #[test]
    fn dotenv_loses_to_a_property_and_to_the_environment() {
        load_dotenv("MOQUI_LIB_DOTENV_PROBE=from-file\nexport MOQUI_LIB_DOTENV_QUOTED=\"quoted value\"\n# comment\n");
        assert_eq!(get_prop_or_env("MOQUI_LIB_DOTENV_PROBE"), "from-file");
        assert_eq!(get_prop_or_env("MOQUI_LIB_DOTENV_QUOTED"), "quoted value");
        set_property("MOQUI_LIB_DOTENV_PROBE", "from-prop");
        assert_eq!(get_prop_or_env("MOQUI_LIB_DOTENV_PROBE"), "from-prop");
        clear_property("MOQUI_LIB_DOTENV_PROBE");
        let path = std::env::var("PATH").unwrap_or_default();
        load_dotenv(&format!("PATH={path}-not-this"));
        assert_eq!(get_prop_or_env("PATH"), path);
    }
}
