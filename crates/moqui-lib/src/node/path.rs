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

//! Paths to an attribute of an element tree, in two forms (plans/archive/Conf-Paths.md §1): the selector
//! `webapp-list/webapp[webroot]@http-port`, and the environment name
//! `moqui_conf__webapp_list__webapp__WEBROOT__http_port`. Names are matched without case and without any character but
//! letters and digits, so `http-port`, `HTTP_PORT` and `httpPort` are one name. Which children are told apart by key
//! attributes is the caller's knowledge, given as a [`Keying`] for each element path; this module knows nothing of the
//! conf.

use std::fmt;

use super::MNode;

/// The first segment of an environment name for a path under the root element `moqui-conf`.
pub const ENV_PREFIX: &str = "moqui_conf";

/// How the children of one name are told apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keying {
    /// The first element of the name.
    Single,
    /// Each element by the values of these attributes, in order.
    By(&'static [&'static str]),
    /// A list with no key: an element of it cannot be named.
    Sequence,
}

/// One element of a path: its name and, for a keyed element, its key values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub element: String,
    pub keys: Vec<String>,
}

/// An attribute of an element below the root, by the elements from the root's child down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrPath {
    pub steps: Vec<Step>,
    pub attribute: String,
}

/// Why a path or name does not name an attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    /// The text is not a selector: the reason.
    Syntax(String),
    /// The name has an empty segment.
    EmptySegment,
    /// The name ends at an element, or within a keyed element's key values: there is no attribute.
    NoAttribute,
    /// The element is a list with no key, so one of it cannot be named.
    Sequence(String),
    /// More than one child matches: the element path and what matched.
    Ambiguous(String),
    /// No such element, and it may not be created: the element path.
    Missing(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(reason) => write!(f, "not a conf selector: {reason}"),
            Self::EmptySegment => f.write_str("the name has an empty segment"),
            Self::NoAttribute => f.write_str("the name ends before an attribute"),
            Self::Sequence(path) => write!(f, "{path} is a list with no key, so no one element of it can be named"),
            Self::Ambiguous(what) => write!(f, "more than one element matches: {what}"),
            Self::Missing(path) => write!(f, "no element {path}"),
        }
    }
}

impl std::error::Error for PathError {}

/// `text` without case and without any character but letters and digits: how names and keys are compared.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

/// Two names or keys are the same when they normalize alike.
#[must_use]
pub fn same_name(left: &str, right: &str) -> bool {
    let mut a = left.chars().filter(char::is_ascii_alphanumeric);
    let mut b = right.chars().filter(char::is_ascii_alphanumeric);
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x.eq_ignore_ascii_case(&y) => {}
            _ => return false,
        }
    }
}

/// `text` as one segment of a canonical environment name: each run of characters other than letters and digits as one
/// `_`, none at either end, in lower case (an element or attribute) or capitals (a key value). Empty when `text` has
/// no letter or digit.
fn env_segment(text: &str, capitals: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(if capitals {
                ch.to_ascii_uppercase()
            } else {
                ch.to_ascii_lowercase()
            });
        } else {
            gap = true;
        }
    }
    out
}

/// An element or attribute name as a name segment spells it: lower case, `_` as `-`. For one the tree does not have.
fn xml_name(segment: &str) -> String {
    segment
        .trim_matches('_')
        .chars()
        .map(|ch| if ch == '_' { '-' } else { ch.to_ascii_lowercase() })
        .collect()
}

impl AttrPath {
    /// Parse `a/b[k1,k2]/c@attr`. A key value runs to its `,` or `]`, so it may hold `/` and `@`
    /// (`screen[component://webroot/screen/webroot.xml]`).
    ///
    /// # Errors
    ///
    /// When there is no `@attribute`, an element name is empty, or a bracket is not closed.
    pub fn parse(text: &str) -> Result<Self, PathError> {
        let text = text.trim();
        let mut steps = Vec::new();
        let mut chars = text.char_indices().peekable();
        let mut name = String::new();
        let mut attribute = None;
        while let Some((index, ch)) = chars.next() {
            match ch {
                '[' => {
                    if name.is_empty() {
                        return Err(PathError::Syntax(format!("a key with no element at {index}")));
                    }
                    let mut keys = Vec::new();
                    let mut key = String::new();
                    let mut closed = false;
                    for (_, inner) in chars.by_ref() {
                        match inner {
                            ',' => keys.push(std::mem::take(&mut key)),
                            ']' => {
                                keys.push(std::mem::take(&mut key));
                                closed = true;
                                break;
                            }
                            other => key.push(other),
                        }
                    }
                    if !closed {
                        return Err(PathError::Syntax("a [ with no ]".to_string()));
                    }
                    steps.push(Step {
                        element: std::mem::take(&mut name),
                        keys: keys.into_iter().map(|key| key.trim().to_string()).collect(),
                    });
                    match chars.peek() {
                        Some((_, '/')) => {
                            chars.next();
                        }
                        Some((_, '@')) | None => {}
                        Some((at, other)) => {
                            return Err(PathError::Syntax(format!("{other} after ] at {at}")));
                        }
                    }
                }
                '/' => {
                    if name.is_empty() {
                        return Err(PathError::Syntax(format!("an empty element at {index}")));
                    }
                    steps.push(Step {
                        element: std::mem::take(&mut name),
                        keys: Vec::new(),
                    });
                }
                '@' => {
                    if !name.is_empty() {
                        steps.push(Step {
                            element: std::mem::take(&mut name),
                            keys: Vec::new(),
                        });
                    }
                    attribute = Some(text[index + 1..].trim().to_string());
                    break;
                }
                other => name.push(other),
            }
        }
        let attribute = attribute
            .filter(|attribute| !attribute.is_empty())
            .ok_or_else(|| PathError::Syntax("no @attribute".to_string()))?;
        if steps.is_empty() {
            return Err(PathError::Syntax("no element".to_string()));
        }
        Ok(Self { steps, attribute })
    }

    /// The selector: `a/b[k1,k2]/c@attr`.
    #[must_use]
    pub fn selector(&self) -> String {
        let mut out = String::new();
        for (index, step) in self.steps.iter().enumerate() {
            if index > 0 {
                out.push('/');
            }
            out.push_str(&step.element);
            if !step.keys.is_empty() {
                out.push('[');
                out.push_str(&step.keys.join(","));
                out.push(']');
            }
        }
        out.push('@');
        out.push_str(&self.attribute);
        out
    }

    /// The canonical environment name (`moqui_conf__webapp_list__webapp__WEBROOT__http_port`), or `None` when a key
    /// value has no letter or digit, so no name can hold it.
    #[must_use]
    pub fn env_name(&self) -> Option<String> {
        let mut out = String::from(ENV_PREFIX);
        for step in &self.steps {
            out.push_str("__");
            out.push_str(&env_segment(&step.element, false));
            for key in &step.keys {
                let segment = env_segment(key, true);
                if segment.is_empty() {
                    return None;
                }
                out.push_str("__");
                out.push_str(&segment);
            }
        }
        out.push_str("__");
        out.push_str(&env_segment(&self.attribute, false));
        Some(out)
    }

    /// The path of `attribute` of `node`, from below the root, or `None` when `node` is the root or an element on the
    /// way is in a [`Keying::Sequence`]. `keying` answers for an element path from below the root.
    #[must_use]
    pub fn of(node: &MNode, attribute: &str, keying: &dyn Fn(&[&str]) -> Keying) -> Option<Self> {
        let mut chain = Vec::new();
        let mut current = node.clone();
        while let Some(parent) = current.parent() {
            chain.push(current);
            current = parent;
        }
        if chain.is_empty() {
            return None;
        }
        chain.reverse();
        let names: Vec<String> = chain.iter().map(MNode::name).collect();
        let mut steps = Vec::with_capacity(chain.len());
        for (depth, element) in chain.iter().enumerate() {
            let path: Vec<&str> = names[..=depth].iter().map(String::as_str).collect();
            let keys = match keying(&path) {
                Keying::Single => Vec::new(),
                Keying::Sequence => return None,
                Keying::By(attrs) => attrs
                    .iter()
                    .map(|attr| element.attribute(attr).unwrap_or_default())
                    .collect(),
            };
            steps.push(Step {
                element: names[depth].clone(),
                keys,
            });
        }
        Some(Self {
            steps,
            attribute: attribute.to_string(),
        })
    }
}

impl fmt::Display for AttrPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.selector())
    }
}

/// The segments after the prefix when `name` is an environment name of the root (`moqui_conf__...`, in any case), so
/// at least an attribute; `None` for any other name, `MOQUI_CONF` itself among them.
#[must_use]
pub fn env_segments(name: &str) -> Option<Vec<&str>> {
    let mut parts = name.split("__");
    let first = parts.next()?;
    if !same_name(first, ENV_PREFIX) {
        return None;
    }
    let rest: Vec<&str> = parts.collect();
    (!rest.is_empty()).then_some(rest)
}

/// Where a name or selector points: the element (made when it was missing and making was allowed) and the attribute's
/// spelling there.
pub struct Target {
    pub node: MNode,
    pub attribute: String,
    /// The path with the spellings the tree has (or was given).
    pub path: AttrPath,
    /// The attribute was on the element before.
    pub attribute_existed: bool,
    /// An element on the way was made for this target.
    pub created: bool,
}

/// One step of a walk: a name, the key values that follow it, as given.
struct Want<'a> {
    element: &'a str,
    keys: Vec<&'a str>,
}

/// Find, or with `create` make, the target of an environment name's `segments` ([`env_segments`]) under `root`.
///
/// # Errors
///
/// An empty segment; no attribute at the end; a list element; more than one child matching; or with `create` false, a
/// missing element.
pub fn resolve_env(
    root: &MNode,
    segments: &[&str],
    keying: &dyn Fn(&[&str]) -> Keying,
    create: bool,
) -> Result<Target, PathError> {
    if segments.iter().any(|segment| normalize(segment).is_empty()) {
        return Err(PathError::EmptySegment);
    }
    let Some((attribute, elements)) = segments.split_last() else {
        return Err(PathError::NoAttribute);
    };
    if elements.is_empty() {
        return Err(PathError::NoAttribute);
    }
    // the keys an element takes are known only once its name is: walk the tree to split the segments
    let mut wants = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut index = 0;
    let mut node = Some(root.clone());
    while index < elements.len() {
        let segment = elements[index];
        let spelled = node
            .as_ref()
            .and_then(|parent| parent.child_iter().find(|child| same_name(&child.name(), segment)))
            .map_or_else(|| xml_name(segment), |child| child.name());
        names.push(spelled);
        let path: Vec<&str> = names.iter().map(String::as_str).collect();
        let count = match keying(&path) {
            Keying::Single => 0,
            Keying::By(attrs) => attrs.len(),
            Keying::Sequence => return Err(PathError::Sequence(path.join("/"))),
        };
        if index + count >= elements.len() {
            return Err(PathError::NoAttribute);
        }
        wants.push(Want {
            element: segment,
            keys: elements[index + 1..=index + count].to_vec(),
        });
        index += count + 1;
        node = node.and_then(|parent| parent.child_iter().find(|child| same_name(&child.name(), segment)));
    }
    walk(root, &wants, &names, attribute, keying, create, false)
}

/// Find, or with `create` make, the target of `path` under `root`. Names and keys match as environment names do.
///
/// # Errors
///
/// As [`resolve_env`]; also when a step has more or fewer keys than its element takes.
pub fn resolve(
    root: &MNode,
    path: &AttrPath,
    keying: &dyn Fn(&[&str]) -> Keying,
    create: bool,
) -> Result<Target, PathError> {
    let mut names: Vec<String> = Vec::with_capacity(path.steps.len());
    let mut wants = Vec::with_capacity(path.steps.len());
    for step in &path.steps {
        names.push(step.element.clone());
        let joined: Vec<&str> = names.iter().map(String::as_str).collect();
        let count = match keying(&joined) {
            Keying::Single => 0,
            Keying::By(attrs) => attrs.len(),
            Keying::Sequence => return Err(PathError::Sequence(joined.join("/"))),
        };
        if step.keys.len() != count {
            return Err(PathError::Syntax(format!(
                "{} takes {count} key values, not {}",
                joined.join("/"),
                step.keys.len()
            )));
        }
        wants.push(Want {
            element: &step.element,
            keys: step.keys.iter().map(String::as_str).collect(),
        });
    }
    walk(root, &wants, &names, &path.attribute, keying, create, true)
}

fn walk(
    root: &MNode,
    wants: &[Want<'_>],
    names: &[String],
    attribute: &str,
    keying: &dyn Fn(&[&str]) -> Keying,
    create: bool,
    exact_spelling: bool,
) -> Result<Target, PathError> {
    let mut node = root.clone();
    let mut steps = Vec::with_capacity(wants.len());
    let mut created = false;
    for (depth, want) in wants.iter().enumerate() {
        let path: Vec<&str> = names[..=depth].iter().map(String::as_str).collect();
        let key_attrs: &[&str] = match keying(&path) {
            Keying::By(attrs) => attrs,
            Keying::Single | Keying::Sequence => &[],
        };
        let mut matches = node.child_iter().filter(|child| {
            same_name(&child.name(), want.element)
                && key_attrs
                    .iter()
                    .zip(&want.keys)
                    .all(|(attr, key)| child.with_attribute(attr, |value| same_name(value.unwrap_or(""), key)))
        });
        let found = matches.next();
        if found.is_some() && !key_attrs.is_empty() && matches.next().is_some() {
            return Err(PathError::Ambiguous(format!(
                "{}[{}]",
                path.join("/"),
                want.keys.join(",")
            )));
        }
        drop(matches);
        let child = match found {
            Some(child) => child,
            None if create => {
                created = true;
                let made = node.append(names[depth].clone());
                for (attr, key) in key_attrs.iter().zip(&want.keys) {
                    made.set_attribute(*attr, *key);
                }
                made
            }
            None => return Err(PathError::Missing(path.join("/"))),
        };
        let keys = key_attrs
            .iter()
            .map(|attr| child.attribute(attr).unwrap_or_default())
            .collect();
        steps.push(Step {
            element: child.name(),
            keys,
        });
        node = child;
    }
    let existing = node
        .attributes()
        .into_iter()
        .map(|(name, _)| name)
        .find(|name| same_name(name, attribute));
    let attribute_existed = existing.is_some();
    let spelled = existing.unwrap_or_else(|| {
        if exact_spelling {
            attribute.to_string()
        } else {
            xml_name(attribute)
        }
    });
    Ok(Target {
        node,
        attribute: spelled.clone(),
        path: AttrPath {
            steps,
            attribute: spelled,
        },
        attribute_existed,
        created,
    })
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
