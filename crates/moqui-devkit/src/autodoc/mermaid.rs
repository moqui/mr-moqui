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

//! The Mermaid diagrams of the generated docs (plans/Autodoc.md, "Document content"): an artifact's users and uses in the
//! Artifact Browser's shape, an entity package's E-R diagram, a service directory's class diagram, and the flowcharts of a
//! screen directory, a REST API and an ECA rules file. Pure text from plain inputs, so the templates can place them and a
//! test can parse every one with the vendored Mermaid (`WebrootVue.qvt.markdown.page.js`).
//!
//! Ids are made of `[A-Za-z0-9_]` and never a Mermaid keyword; labels are quoted with `#`, `"`, `<` and `>` as Mermaid's
//! entity codes, so no name breaks a diagram.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// The rows a side of the usage diagram shows of each group, as the Artifact Browser's cards show eight.
pub const GROUP_ROWS: usize = 8;

/// The entities an E-R diagram shows with their key fields; a larger package shows the names only.
pub const ER_ATTRIBUTE_LIMIT: usize = 60;

/// One group of a usage diagram's side: a kind of artifact and the names of those that use it, or that it uses.
#[derive(Clone, Debug, Default)]
pub struct UsageGroup {
    pub label: String,
    pub names: Vec<String>,
    /// Every link of the group was only observed at run time: the connector is dashed.
    pub observed_only: bool,
}

/// A text as a quoted Mermaid label: one line, with the characters that end or open a label as entity codes.
#[must_use]
pub fn label(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '#' => out.push_str("#35;"),
            '"' => out.push_str("#quot;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '\r' | '\n' | '\t' => out.push(' '),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// `text` as an id: letters, digits and `_`, starting with a letter.
#[must_use]
pub fn safe_id(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    if !out.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
        out.insert(0, 'n');
    }
    out
}

/// Unique ids for keys, each made from a readable name: the name, else the name and a number.
#[derive(Default)]
struct Ids {
    by_key: HashMap<String, String>,
    used: HashSet<String>,
}

impl Ids {
    fn id(&mut self, key: &str, readable: &str) -> String {
        if let Some(found) = self.by_key.get(key) {
            return found.clone();
        }
        let base = safe_id(readable);
        let mut id = base.clone();
        let mut next = 2;
        while self.used.contains(&id) || is_keyword(&id) {
            id = format!("{base}_{next}");
            next += 1;
        }
        self.used.insert(id.clone());
        self.by_key.insert(key.to_string(), id.clone());
        id
    }
}

/// Names Mermaid reads as keywords in an id position.
fn is_keyword(id: &str) -> bool {
    matches!(
        id.to_ascii_lowercase().as_str(),
        "end"
            | "graph"
            | "subgraph"
            | "flowchart"
            | "class"
            | "classdef"
            | "click"
            | "style"
            | "linkstyle"
            | "direction"
            | "erdiagram"
            | "classdiagram"
            | "default"
            | "namespace"
            | "note"
            | "call"
            | "href"
    )
}

/// An artifact's users to its left and its uses to its right, one subgraph per group with at most [`GROUP_ROWS`]
/// names and "+ n more", and one connector per group, as the Artifact Browser draws them (plans/design-dev-aide.md D9).
#[must_use]
pub fn usage_flowchart(center: &str, used_by: &[UsageGroup], uses: &[UsageGroup]) -> String {
    let mut out = String::from("flowchart LR\n");
    let _ = writeln!(out, "    center[[{}]]", label(center));
    let side = |out: &mut String, prefix: &str, groups: &[UsageGroup], incoming: bool| {
        for (at, group) in groups.iter().enumerate() {
            let id = format!("{prefix}{}", at + 1);
            let _ = writeln!(
                out,
                "    subgraph {id}[{}]",
                label(&format!("{} ({})", group.label, group.names.len()))
            );
            out.push_str("        direction TB\n");
            for (row, name) in group.names.iter().take(GROUP_ROWS).enumerate() {
                let _ = writeln!(out, "        {id}_{}[{}]", row + 1, label(name));
            }
            if group.names.len() > GROUP_ROWS {
                let _ = writeln!(
                    out,
                    "        {id}_more[{}]",
                    label(&format!("+ {} more", group.names.len() - GROUP_ROWS))
                );
            }
            out.push_str("    end\n");
            let arrow = if group.observed_only { "-.->" } else { "-->" };
            if incoming {
                let _ = writeln!(out, "    {id} {arrow} center");
            } else {
                let _ = writeln!(out, "    center {arrow} {id}");
            }
        }
    };
    side(&mut out, "used_by_", used_by, true);
    side(&mut out, "uses_", uses, false);
    out
}

/// An entity of an E-R diagram.
#[derive(Clone, Debug, Default)]
pub struct ErEntity {
    /// The full name, which the links name.
    pub full_name: String,
    pub short_name: String,
    /// Shown without fields: an entity outside the package, or any in a package over [`ER_ATTRIBUTE_LIMIT`].
    pub stub: bool,
    pub attributes: Vec<ErAttribute>,
}

#[derive(Clone, Debug, Default)]
pub struct ErAttribute {
    pub field_type: String,
    pub name: String,
    pub primary: bool,
    pub foreign: bool,
}

/// How many of the related entity one of the entity has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    /// `one`: the entity's key fields name exactly one related record.
    One,
    /// `one-nofk`: as one, with no foreign key; a dotted line.
    OneNoFk,
    /// `many`.
    Many,
}

/// A relationship of an E-R diagram, from the entity that defines it.
#[derive(Clone, Debug)]
pub struct ErLink {
    pub from: String,
    pub to: String,
    pub cardinality: Cardinality,
    pub title: String,
}

/// An E-R diagram of `entities` and the `links` between them; a link to an entity not listed is left out.
#[must_use]
pub fn er_diagram(entities: &[ErEntity], links: &[ErLink]) -> String {
    let mut ids = Ids::default();
    // the short name unless two entities share it, then the full name
    let mut shorts: HashMap<&str, usize> = HashMap::new();
    for entity in entities {
        *shorts.entry(entity.short_name.as_str()).or_default() += 1;
    }
    let mut id_of: HashMap<&str, String> = HashMap::new();
    for entity in entities {
        let readable = if shorts.get(entity.short_name.as_str()).copied().unwrap_or(0) > 1 {
            entity.full_name.as_str()
        } else {
            entity.short_name.as_str()
        };
        id_of.insert(entity.full_name.as_str(), ids.id(&entity.full_name, readable));
    }
    let mut out = String::from("erDiagram\n");
    for entity in entities {
        let id = &id_of[entity.full_name.as_str()];
        if entity.stub || entity.attributes.is_empty() {
            let _ = writeln!(out, "    {id} {{\n    }}");
            continue;
        }
        let _ = writeln!(out, "    {id} {{");
        for attribute in &entity.attributes {
            let keys = match (attribute.primary, attribute.foreign) {
                (true, true) => " PK, FK",
                (true, false) => " PK",
                (false, true) => " FK",
                (false, false) => "",
            };
            let _ = writeln!(
                out,
                "        {} {}{keys}",
                er_type(&attribute.field_type),
                safe_id(&attribute.name)
            );
        }
        out.push_str("    }\n");
    }
    for link in links {
        let (Some(from), Some(to)) = (id_of.get(link.from.as_str()), id_of.get(link.to.as_str())) else {
            continue;
        };
        let line = match link.cardinality {
            Cardinality::One => "}o--||",
            Cardinality::OneNoFk => "}o..||",
            Cardinality::Many => "||--o{",
        };
        let _ = writeln!(out, "    {from} {line} {to} : {}", er_label(&link.title));
    }
    out
}

/// A field type as an E-R attribute type: letters, digits, `-` and `_`.
fn er_type(field_type: &str) -> String {
    let cleaned: String = field_type
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect();
    if cleaned.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
        cleaned
    } else {
        format!("t{cleaned}")
    }
}

/// A relationship's label: its title, quoted, with what would end the quotes left out.
fn er_label(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.'))
        .collect();
    format!("\"{cleaned}\"")
}

/// A class of a class diagram: a service file with its services as methods.
#[derive(Clone, Debug, Default)]
pub struct ClassBox {
    /// What the links name: a file's path or an interface's service name.
    pub key: String,
    pub name: String,
    pub members: Vec<String>,
    /// An interface service, outside the files, drawn as `<<interface>>`.
    pub interface: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClassLinkKind {
    /// A service of the file implements the interface: `<|..`.
    Realizes,
    /// A service of the file calls a service of the other: `..>`.
    Depends,
}

#[derive(Clone, Debug)]
pub struct ClassLink {
    pub from: String,
    pub to: String,
    pub kind: ClassLinkKind,
}

/// A class diagram of `classes` and the `links` between them, each link once.
#[must_use]
pub fn class_diagram(classes: &[ClassBox], links: &[ClassLink]) -> String {
    let mut ids = Ids::default();
    let mut out = String::from("classDiagram\n");
    let mut id_of: HashMap<&str, String> = HashMap::new();
    for class in classes {
        let id = ids.id(&class.key, &class.name);
        let _ = writeln!(out, "    class {id}[{}] {{", label(&class.name));
        if class.interface {
            out.push_str("        <<interface>>\n");
        }
        for member in &class.members {
            let _ = writeln!(out, "        +{}()", member_text(member));
        }
        out.push_str("    }\n");
        id_of.insert(class.key.as_str(), id);
    }
    let mut seen = HashSet::new();
    for link in links {
        let (Some(from), Some(to)) = (id_of.get(link.from.as_str()), id_of.get(link.to.as_str())) else {
            continue;
        };
        if from == to || !seen.insert((from.clone(), to.clone(), link.kind)) {
            continue;
        }
        match link.kind {
            ClassLinkKind::Realizes => {
                let _ = writeln!(out, "    {to} <|.. {from}");
            }
            ClassLinkKind::Depends => {
                let _ = writeln!(out, "    {from} ..> {to}");
            }
        }
    }
    out
}

/// A service's `verb#noun` as a class member: the `#` as `_`, since a class member takes no entity codes and a `#`
/// would be read as a visibility, and the rest of the punctuation that would open a type or a generic dropped.
fn member_text(name: &str) -> String {
    name.chars()
        .map(|ch| if ch == '#' { '_' } else { ch })
        .filter(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '-'))
        .collect()
}

/// The shape of a flowchart node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Box,
    /// The subject of the diagram.
    Subject,
    /// Rounded: a rule, a method.
    Round,
    /// Outside the set the diagram is about.
    Stub,
}

#[derive(Clone, Debug)]
pub struct FlowNode {
    pub key: String,
    pub label: String,
    pub shape: Shape,
}

#[derive(Clone, Debug)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub label: String,
    pub dashed: bool,
}

/// A flowchart of `nodes` (in order) and `edges` (to nodes listed), `TD` or `LR`; each edge once.
#[must_use]
pub fn flowchart(direction: &str, nodes: &[FlowNode], edges: &[FlowEdge]) -> String {
    let mut ids = Ids::default();
    let mut out = format!("flowchart {direction}\n");
    let mut id_of: BTreeMap<&str, String> = BTreeMap::new();
    for node in nodes {
        if id_of.contains_key(node.key.as_str()) {
            continue;
        }
        let id = ids.id(&node.key, &node.label);
        let text = label(&node.label);
        let _ = match node.shape {
            Shape::Box => writeln!(out, "    {id}[{text}]"),
            Shape::Subject => writeln!(out, "    {id}[[{text}]]"),
            Shape::Round => writeln!(out, "    {id}({text})"),
            Shape::Stub => writeln!(out, "    {id}>{text}]"),
        };
        id_of.insert(node.key.as_str(), id);
    }
    let mut seen = HashSet::new();
    for edge in edges {
        let (Some(from), Some(to)) = (id_of.get(edge.from.as_str()), id_of.get(edge.to.as_str())) else {
            continue;
        };
        if !seen.insert((from.clone(), to.clone(), edge.label.clone())) {
            continue;
        }
        let arrow = if edge.dashed { "-.->" } else { "-->" };
        if edge.label.is_empty() {
            let _ = writeln!(out, "    {from} {arrow} {to}");
        } else {
            let _ = writeln!(out, "    {from} {arrow}|{}| {to}", label(&edge.label));
        }
    }
    out
}

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
