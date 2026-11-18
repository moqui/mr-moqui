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

use moqui_lib::node::path::AttrPath;
use moqui_lib::node::MNode;

use crate::context::conf_schema::{self, Class, Key, Unit, CATALOG, COMPONENT};

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
    /// A top-level element no unit names (a component's own) replaced by `from`, in order.
    Named {
        parent: MNode,
        name: String,
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
            Self::Named { parent, name, from } => {
                parent.remove_where(|child| child.has_name(name));
                for child in from {
                    parent.append_node(child.deep_copy());
                }
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
        let (old_nodes, new_nodes) = (running.children_named(&name), disk.children_named(&name));
        let (old, new) = (text_of(&old_nodes), text_of(&new_nodes));
        if old != new {
            out.push(UnitChange {
                path: name.clone(),
                attr: None,
                kind: kind_of(&old, &new),
                class: COMPONENT,
                old: non_empty(shown_all(&old_nodes)),
                new: non_empty(shown_all(&new_nodes)),
                edit: Edit::Named {
                    parent: running.clone(),
                    name,
                    from: new_nodes,
                },
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
                let shown_new = new_nodes.clone();
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
                    old: non_empty(shown_all(&old_nodes)),
                    new: non_empty(shown_all(&shown_new)),
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
                new: Some(clip(shown(&new))),
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
                old: Some(clip(shown(&old))),
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
                    old: shown_value(&old, &name, before),
                    new: shown_value(&new, &name, after),
                    attr: Some(name),
                    edit,
                    within,
                });
            }
            let covered: Vec<&'static str> = unit.children.iter().map(|child| child.name).collect();
            let (before, after) = (content(&old, &covered), content(&new, &covered));
            if before != after {
                let (shown_before, shown_after) = (shown_content(&old, &covered), shown_content(&new, &covered));
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
                    old: non_empty(shown_before),
                    new: non_empty(shown_after),
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
        let value = shown_value(node, &name, Some(value));
        let (old, new) = match kind {
            Kind::Removed => (value, None),
            _ => (None, value),
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

/// Whether the attribute `attr` of `node` holds a secret: the conf schema marks it, or its name says so.
fn secret_attr(node: &MNode, attr: &str) -> bool {
    AttrPath::of(node, attr, &conf_schema::keying).map_or_else(
        || conf_schema::secret_name(attr),
        |path| crate::context::conf_env::attribute_secret(&path),
    )
}

/// `value` of `node`'s attribute `attr` as a report shows it: masked when it holds a secret.
fn shown_value(node: &MNode, attr: &str, value: Option<String>) -> Option<String> {
    value.map(|value| {
        if !value.is_empty() && secret_attr(node, attr) {
            super::view::MASKED.to_string()
        } else {
            value
        }
    })
}

/// `node` as text, each secret attribute in it masked: a report never shows a secret, though overrides and generated
/// secrets are in the running conf.
fn shown(node: &MNode) -> String {
    fn mask(original: &MNode, copy: &MNode) {
        for (name, value) in original.attributes() {
            if !value.is_empty() && secret_attr(original, &name) {
                copy.set_attribute(name, super::view::MASKED);
            }
        }
        for (original, copy) in original.children().iter().zip(copy.children().iter()) {
            mask(original, copy);
        }
    }
    let copy = node.deep_copy();
    mask(node, &copy);
    copy.write_string()
}

fn shown_all(nodes: &[MNode]) -> String {
    nodes.iter().map(shown).collect()
}

/// [`content`] as a report shows it, secrets masked.
fn shown_content(node: &MNode, covered: &[&str]) -> String {
    let mut out = node.text().unwrap_or_default().trim().to_string();
    for child in node.children() {
        if !child.with_name(|name| covered.contains(&name)) {
            out.push_str(&shown(&child));
        }
    }
    out
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
