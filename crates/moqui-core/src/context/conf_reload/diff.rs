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

//! The changes between the running conf and the conf the files make now, unit by unit ([`super::catalog`]), each with
//! the edit that makes the running conf take it. An element added or removed whole is one edit; the changes inside it
//! are reported with it and take its edit.

use moqui_lib::node::MNode;

use super::catalog::{Class, Key, Unit, CATALOG, UNKNOWN};

/// Longest text of an element kept in a change, for the report.
const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Added,
    Removed,
    Changed,
}

/// One change to a unit of the conf.
pub(super) struct UnitChange {
    /// The unit's element path, each keyed element with its key: `webapp-list/webapp[webroot]/token-route[s]`.
    pub path: String,
    /// The attribute that changed; `None` for the element added or removed, or for its other content.
    pub attr: Option<String>,
    pub kind: Kind,
    pub class: Class,
    pub old: Option<String>,
    pub new: Option<String>,
    pub edit: Edit,
    /// The change of the added or removed element this one is inside, whose edit carries it.
    pub within: Option<usize>,
}

/// How the running conf takes a change. The nodes are the running conf's, and the disk conf's copied.
pub(super) enum Edit {
    /// Carried by the change of the element it is inside.
    None,
    Attr {
        node: MNode,
        name: String,
        value: Option<String>,
    },
    /// The children no unit names, and the text, of `node` replaced by those of `from`.
    Content {
        node: MNode,
        covered: Vec<&'static str>,
        from: MNode,
    },
    Append {
        parent: MNode,
        child: MNode,
    },
    Remove {
        parent: MNode,
        child: MNode,
    },
    /// The children named `name` of `parent` replaced by `from`, in order.
    Sequence {
        parent: MNode,
        name: &'static str,
        from: Vec<MNode>,
    },
}

impl Edit {
    /// Make the change in the running conf.
    pub(super) fn apply(&self) {
        match self {
            Self::None => {}
            Self::Attr { node, name, value } => match value {
                Some(value) => node.set_attribute(name.clone(), value.clone()),
                None => {
                    node.remove_attribute(name);
                }
            },
            Self::Content { node, covered, from } => {
                node.remove_where(|child| !child.with_name(|name| covered.contains(&name)));
                for child in from.children() {
                    if !child.with_name(|name| covered.contains(&name)) {
                        node.append_node(child.deep_copy());
                    }
                }
                node.set_text(from.text());
            }
            Self::Append { parent, child } => parent.append_node(child.deep_copy()),
            Self::Remove { parent, child } => {
                let mut removed = false;
                parent.remove_where(|node| {
                    let same = !removed && node.same_node(child);
                    removed |= same;
                    same
                });
            }
            Self::Sequence { parent, name, from } => {
                parent.remove_where(|child| child.has_name(name));
                for child in from {
                    parent.append_node(child.deep_copy());
                }
            }
        }
    }
}

/// The changes from `running` to `disk`, both `moqui-conf` roots, in catalog order and then the elements no unit
/// names.
pub(super) fn diff(running: &MNode, disk: &MNode) -> Vec<UnitChange> {
    let mut out = Vec::new();
    diff_units(Some(running), Some(disk), CATALOG, "", &mut out, None);
    // the top-level elements no unit names, each name a unit of its own
    let mut names: Vec<String> = Vec::new();
    let (old_top, new_top) = (running.children(), disk.children());
    let top = old_top.iter().chain(new_top.iter());
    for node in top {
        let name = node.name();
        if !CATALOG.iter().any(|unit| unit.name == name) && !names.contains(&name) {
            names.push(name);
        }
    }
    for name in names {
        let old = text_of(&running.children_named(&name));
        let new = text_of(&disk.children_named(&name));
        if old != new {
            out.push(UnitChange {
                path: name,
                attr: None,
                kind: kind_of(&old, &new),
                class: UNKNOWN,
                old: non_empty(old),
                new: non_empty(new),
                edit: Edit::None,
                within: None,
            });
        }
    }
    out
}

fn diff_units(
    old: Option<&MNode>,
    new: Option<&MNode>,
    units: &[Unit],
    path: &str,
    out: &mut Vec<UnitChange>,
    within: Option<usize>,
) {
    for unit in units {
        let unit_path = join(path, unit.name);
        match unit.key {
            Key::Single => {
                let old_node = old.and_then(|node| node.first_named(unit.name));
                let new_node = new.and_then(|node| node.first_named(unit.name));
                instance(old, old_node, new_node, unit, &unit_path, out, within);
            }
            Key::By(attrs) => {
                let old_nodes = keyed(old, unit.name, attrs);
                let new_nodes = keyed(new, unit.name, attrs);
                let mut keys: Vec<&String> = old_nodes.iter().map(|(key, _)| key).collect();
                for (key, _) in &new_nodes {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
                for key in keys {
                    let find =
                        |nodes: &[(String, MNode)]| nodes.iter().find(|(held, _)| held == key).map(|(_, n)| n.clone());
                    let path = format!("{unit_path}[{key}]");
                    instance(old, find(&old_nodes), find(&new_nodes), unit, &path, out, within);
                }
            }
            Key::Sequence => {
                let old_nodes = old.map(|node| node.children_named(unit.name)).unwrap_or_default();
                let new_nodes = new.map(|node| node.children_named(unit.name)).unwrap_or_default();
                let (old_text, new_text) = (text_of(&old_nodes), text_of(&new_nodes));
                if old_text == new_text {
                    continue;
                }
                let edit = match (within, old) {
                    (None, Some(parent)) => Edit::Sequence {
                        parent: parent.clone(),
                        name: unit.name,
                        from: new_nodes,
                    },
                    _ => Edit::None,
                };
                out.push(UnitChange {
                    path: unit_path,
                    attr: None,
                    kind: kind_of(&old_text, &new_text),
                    class: unit.class,
                    old: non_empty(old_text),
                    new: non_empty(new_text),
                    edit,
                    within,
                });
            }
        }
    }
}

/// The elements of `name` under `parent` by their key, a repeated key numbered from its second: `name#2`.
pub(super) fn keyed(parent: Option<&MNode>, name: &str, attrs: &[&str]) -> Vec<(String, MNode)> {
    let mut out: Vec<(String, MNode)> = Vec::new();
    let Some(parent) = parent else {
        return out;
    };
    for node in parent.children_named(name) {
        let key = attrs
            .iter()
            .map(|attr| node.attribute(attr).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(",");
        let seen = out
            .iter()
            .filter(|(held, _)| held == &key || held.starts_with(&format!("{key}#")))
            .count();
        let key = if seen == 0 { key } else { format!("{key}#{}", seen + 1) };
        out.push((key, node));
    }
    out
}

/// One element of a unit in the running conf and on disk.
fn instance(
    parent: Option<&MNode>,
    old: Option<MNode>,
    new: Option<MNode>,
    unit: &Unit,
    path: &str,
    out: &mut Vec<UnitChange>,
    within: Option<usize>,
) {
    match (old, new) {
        (None, None) => {}
        (None, Some(new)) => {
            let at = out.len();
            let edit = match (within, parent) {
                (None, Some(parent)) => Edit::Append {
                    parent: parent.clone(),
                    child: new.clone(),
                },
                _ => Edit::None,
            };
            out.push(UnitChange {
                path: path.to_string(),
                attr: None,
                kind: Kind::Added,
                class: unit.class,
                old: None,
                new: Some(clip(new.write_string())),
                edit,
                within,
            });
            let inside = Some(within.unwrap_or(at));
            whole_attrs(&new, unit, path, Kind::Added, out, inside);
            diff_units(None, Some(&new), unit.children, path, out, inside);
        }
        (Some(old), None) => {
            let at = out.len();
            let edit = match (within, parent) {
                (None, Some(parent)) => Edit::Remove {
                    parent: parent.clone(),
                    child: old.clone(),
                },
                _ => Edit::None,
            };
            out.push(UnitChange {
                path: path.to_string(),
                attr: None,
                kind: Kind::Removed,
                class: unit.class,
                old: Some(clip(old.write_string())),
                new: None,
                edit,
                within,
            });
            let inside = Some(within.unwrap_or(at));
            whole_attrs(&old, unit, path, Kind::Removed, out, inside);
            diff_units(Some(&old), None, unit.children, path, out, inside);
        }
        (Some(old), Some(new)) => {
            let mut names: Vec<String> = old.attributes().into_iter().map(|(name, _)| name).collect();
            for (name, _) in new.attributes() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            for name in names {
                let (before, after) = (old.attribute(&name), new.attribute(&name));
                if before == after {
                    continue;
                }
                let edit = if within.is_none() {
                    Edit::Attr {
                        node: old.clone(),
                        name: name.clone(),
                        value: after.clone(),
                    }
                } else {
                    Edit::None
                };
                out.push(UnitChange {
                    path: path.to_string(),
                    kind: match (&before, &after) {
                        (None, _) => Kind::Added,
                        (_, None) => Kind::Removed,
                        _ => Kind::Changed,
                    },
                    class: unit.attr_class(&name),
                    attr: Some(name),
                    old: before,
                    new: after,
                    edit,
                    within,
                });
            }
            let covered: Vec<&'static str> = unit.children.iter().map(|child| child.name).collect();
            let (before, after) = (content(&old, &covered), content(&new, &covered));
            if before != after {
                let edit = if within.is_none() {
                    Edit::Content {
                        node: old.clone(),
                        covered,
                        from: new.clone(),
                    }
                } else {
                    Edit::None
                };
                out.push(UnitChange {
                    path: path.to_string(),
                    attr: None,
                    kind: Kind::Changed,
                    class: unit.class,
                    old: non_empty(before),
                    new: non_empty(after),
                    edit,
                    within,
                });
            }
            diff_units(Some(&old), Some(&new), unit.children, path, out, within);
        }
    }
}

/// For an element added or removed whole, a change for each attribute whose class differs from its unit's, so the
/// element waits for a restart when one of them needs it.
fn whole_attrs(node: &MNode, unit: &Unit, path: &str, kind: Kind, out: &mut Vec<UnitChange>, within: Option<usize>) {
    for (name, value) in node.attributes() {
        let class = unit.attr_class(&name);
        if class == unit.class {
            continue;
        }
        let (old, new) = match kind {
            Kind::Removed => (Some(value), None),
            _ => (None, Some(value)),
        };
        out.push(UnitChange {
            path: path.to_string(),
            attr: Some(name),
            kind,
            class,
            old,
            new,
            edit: Edit::None,
            within,
        });
    }
}

/// The children of `node` no unit names, and its text, as text.
fn content(node: &MNode, covered: &[&str]) -> String {
    let mut out = node.text().unwrap_or_default().trim().to_string();
    for child in node.children() {
        if !child.with_name(|name| covered.contains(&name)) {
            out.push_str(&child.write_string());
        }
    }
    out
}

fn text_of(nodes: &[MNode]) -> String {
    nodes.iter().map(MNode::write_string).collect()
}

fn kind_of(old: &str, new: &str) -> Kind {
    match (old.is_empty(), new.is_empty()) {
        (true, _) => Kind::Added,
        (_, true) => Kind::Removed,
        _ => Kind::Changed,
    }
}

pub(super) fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}/{name}")
    }
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn clip(text: String) -> String {
    if text.len() <= MAX_TEXT {
        return text;
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Each attribute of `root`'s units, with the class of its unit, whose value refers to one of `names` (`${name}` or
/// `${name ?: ...}`): the units a property change reaches. (unit path, attribute, class, property name)
pub(super) fn property_users(root: &MNode, names: &[String]) -> Vec<(String, String, Class, String)> {
    let mut out = Vec::new();
    if names.is_empty() {
        return out;
    }
    users_in(root, CATALOG, "", names, &mut out);
    for node in root.children() {
        let name = node.name();
        if !CATALOG.iter().any(|unit| unit.name == name) {
            scan(&node, &name, UNKNOWN, names, &mut out);
        }
    }
    out
}

fn users_in(
    parent: &MNode,
    units: &[Unit],
    path: &str,
    names: &[String],
    out: &mut Vec<(String, String, Class, String)>,
) {
    for unit in units {
        let unit_path = join(path, unit.name);
        for node in parent.children_named(unit.name) {
            let path = match unit.key {
                Key::By(attrs) => {
                    let key: Vec<String> = attrs
                        .iter()
                        .map(|attr| node.attribute(attr).unwrap_or_default())
                        .collect();
                    format!("{unit_path}[{}]", key.join(","))
                }
                Key::Single | Key::Sequence => unit_path.clone(),
            };
            for (attr, value) in node.attributes() {
                for name in names {
                    if refers(&value, name) {
                        out.push((path.clone(), attr.clone(), unit.attr_class(&attr), name.clone()));
                    }
                }
            }
            for child in node.children() {
                if !unit.children.iter().any(|known| child.has_name(known.name)) {
                    scan(&child, &path, unit.class, names, out);
                }
            }
            users_in(&node, unit.children, &path, names, out);
            if unit.key == Key::Single {
                break;
            }
        }
    }
}

/// Every attribute and text of `node` and below it, as part of the unit at `path` of `class`.
fn scan(node: &MNode, path: &str, class: Class, names: &[String], out: &mut Vec<(String, String, Class, String)>) {
    for (attr, value) in node.attributes() {
        for name in names {
            if refers(&value, name) {
                out.push((path.to_string(), format!("{}@{attr}", node.name()), class, name.clone()));
            }
        }
    }
    if let Some(text) = node.text() {
        for name in names {
            if refers(&text, name) {
                out.push((path.to_string(), node.name(), class, name.clone()));
            }
        }
    }
    for child in node.children() {
        scan(&child, path, class, names, out);
    }
}

/// Whether `text` expands the property `name`: `${name}`, `${name ?: 'x'}`.
pub(super) fn refers(text: &str, name: &str) -> bool {
    let opener = format!("${{{name}");
    text.match_indices(&opener).any(|(at, _)| {
        text[at + opener.len()..]
            .chars()
            .next()
            .is_some_and(|next| !(next.is_ascii_alphanumeric() || next == '_' || next == '.'))
    })
}
