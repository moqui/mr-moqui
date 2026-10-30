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

//! The schema digest (plans/design-dev-aide.md 5.3, F8): the framework's XSDs read into one digest per root element of
//! a Moqui XML file: each element that can appear, the children it takes, and each attribute with its enumerated values,
//! default, whether it is required, and documentation. It feeds the editor's XML completion (`lang-xml`'s `elements`),
//! hovers, the schema checks, and later the composer's palette and the properties panels.
//!
//! The digest is by element name, as `lang-xml` looks elements up: two declarations of one name in a schema (a local
//! `parameter` in two places) merge, their children and attributes joined. An XSD construct the reader does not take
//! is logged once and leaves its element open (any child, any attribute), so a check never fails a file for what the
//! reader does not know.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use moqui_lib::node::MNode;
use serde::Serialize;

/// The root elements of Moqui XML files and the XSD that defines each.
pub const ROOTS: &[(&str, &str)] = &[
    ("screen", "xml-screen-3.xsd"),
    ("screen-extend", "xml-screen-3.xsd"),
    ("widget-templates", "xml-form-3.xsd"),
    ("entities", "entity-definition-3.xsd"),
    ("services", "service-definition-3.xsd"),
    ("secas", "service-eca-3.xsd"),
    ("eecas", "entity-eca-3.xsd"),
    ("emecas", "email-eca-3.xsd"),
    ("gitecas", "git-eca-3.xsd"),
    ("resource", "rest-api-3.xsd"),
    ("moqui-conf", "moqui-conf-3.xsd"),
];

/// The digest of one root element's schema.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Digest {
    pub root: String,
    /// Every element that can appear under the root, the root first, then by name.
    pub elements: Vec<ElementSpec>,
    /// What the reader did not take, `construct in file`, each leaving its element open.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementSpec {
    pub name: String,
    /// The root element (`lang-xml`'s `top`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub top: bool,
    pub children: Vec<String>,
    pub attributes: Vec<AttributeSpec>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub doc: String,
    /// Any child is allowed (`xs:any`, or a construct the reader does not take).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open_children: bool,
    /// Any attribute is allowed (`xs:anyAttribute`, or a construct the reader does not take).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open_attributes: bool,
    /// Text content is allowed (mixed, simple content, or a simple type).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub text: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttributeSpec {
    pub name: String,
    /// The enumerated values, when the type is an enumeration (or a union with one); `true` and `false` for a boolean.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// The values are only suggestions: a union with free text, such as `boolean-expandable`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open_values: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub doc: String,
}

impl Digest {
    /// The element named `name`.
    #[must_use]
    pub fn element(&self, name: &str) -> Option<&ElementSpec> {
        self.elements.iter().find(|element| element.name == name)
    }
}

impl ElementSpec {
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&AttributeSpec> {
        self.attributes.iter().find(|attribute| attribute.name == name)
    }
}

type Cached = HashMap<(PathBuf, String), (u64, Arc<Digest>)>;

/// The digest of `root` from the XSDs in `xsd_dir` (the framework's `xsd` directory), kept until an XSD changes.
/// `None` for a root no XSD defines.
#[must_use]
pub fn digest(xsd_dir: &Path, root: &str) -> Option<Arc<Digest>> {
    static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
    let file = ROOTS.iter().find(|(name, _)| *name == root).map(|(_, file)| *file)?;
    let stamp = xsd_stamp(xsd_dir);
    let key = (xsd_dir.to_path_buf(), root.to_string());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((kept, digest)) = cache.lock().unwrap_or_else(|err| err.into_inner()).get(&key) {
        if *kept == stamp {
            return Some(Arc::clone(digest));
        }
    }
    let made = Arc::new(build(xsd_dir, file, root)?);
    cache
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .insert(key, (stamp, Arc::clone(&made)));
    Some(made)
}

/// The root of a Moqui XML file's text, when it is one [`ROOTS`] names.
#[must_use]
pub fn root_of(text: &str) -> Option<&'static str> {
    let root = MNode::parse_root_only("root", text).ok()?;
    let name = root.name();
    ROOTS.iter().map(|(name, _)| *name).find(|known| *known == name)
}

/// FNV-1a over the XSDs' names, sizes and modified times: a change to any of them builds the digests again.
fn xsd_stamp(dir: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|read| read.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    for path in entries
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "xsd"))
    {
        mix(path.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(path) {
            mix(&meta.len().to_le_bytes());
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_nanos());
            mix(&modified.to_le_bytes());
        }
    }
    hash
}

/// The global declarations of one XSD and those it includes.
#[derive(Default)]
struct Schema {
    elements: HashMap<String, MNode>,
    complex_types: HashMap<String, MNode>,
    simple_types: HashMap<String, MNode>,
    groups: HashMap<String, MNode>,
    attribute_groups: HashMap<String, MNode>,
    /// The members of each substitution group head, in file order.
    substitutes: HashMap<String, Vec<String>>,
    abstracts: BTreeSet<String>,
    unknown: BTreeSet<String>,
}

fn build(dir: &Path, file: &str, root: &str) -> Option<Digest> {
    let mut schema = Schema::default();
    let mut read = BTreeSet::new();
    schema.read(dir, file, &mut read);
    if !schema.elements.contains_key(root) {
        return None;
    }
    let mut specs: BTreeMap<String, ElementSpec> = BTreeMap::new();
    let mut pending: Vec<MNode> = vec![schema.elements[root].clone()];
    let mut walked: BTreeSet<(String, usize)> = BTreeSet::new();
    while let Some(decl) = pending.pop() {
        let Some(name) = decl.attribute("name") else {
            continue;
        };
        // A declaration is walked once; the same name declared elsewhere (a local element) is walked too and merged.
        if !walked.insert((name.clone(), decl_id(&decl))) {
            continue;
        }
        let mut spec = ElementSpec {
            name: name.clone(),
            top: name == root,
            doc: documentation(&decl),
            ..ElementSpec::default()
        };
        let mut children = Vec::new();
        schema.element_content(&decl, &mut spec, &mut children);
        for child in &children {
            spec.children.push(child.attribute("name").unwrap_or_default());
        }
        pending.extend(children);
        match specs.get_mut(&name) {
            Some(existing) => existing.merge(spec),
            None => {
                specs.insert(name, spec);
            }
        }
    }
    let mut elements: Vec<ElementSpec> = specs.into_values().collect();
    for element in &mut elements {
        element.children.sort();
        element.children.dedup();
        element.attributes.sort_by(|left, right| left.name.cmp(&right.name));
    }
    elements.sort_by_key(|element| (!element.top, element.name.clone()));
    if !schema.unknown.is_empty() {
        tracing::warn!(
            "The schema digest of {root} leaves elements open for what it does not read: {}",
            schema.unknown.iter().cloned().collect::<Vec<_>>().join("; ")
        );
    }
    Some(Digest {
        root: root.to_string(),
        elements,
        unknown: schema.unknown.into_iter().collect(),
    })
}

/// A stable id of a declaration node: its line and column, for walking each declaration once.
fn decl_id(node: &MNode) -> usize {
    (node.line() as usize) << 16 | node.column() as usize
}

impl ElementSpec {
    fn merge(&mut self, other: Self) {
        self.top |= other.top;
        self.children.extend(other.children);
        self.open_children |= other.open_children;
        self.open_attributes |= other.open_attributes;
        self.text |= other.text;
        if self.doc.is_empty() {
            self.doc = other.doc;
        }
        // An attribute is required only where every declaration of the name requires it: one the other declaration does
        // not have is not.
        for have in &mut self.attributes {
            if !other.attributes.iter().any(|attribute| attribute.name == have.name) {
                have.required = false;
            }
        }
        let had: Vec<String> = self.attributes.iter().map(|have| have.name.clone()).collect();
        for mut attribute in other.attributes {
            if !had.contains(&attribute.name) {
                attribute.required = false;
            }
            match self.attributes.iter_mut().find(|have| have.name == attribute.name) {
                Some(have) => {
                    for value in attribute.values {
                        if !have.values.contains(&value) {
                            have.values.push(value);
                        }
                    }
                    // Required only where every declaration requires it.
                    have.required &= attribute.required;
                    have.open_values |= attribute.open_values;
                    if have.doc.is_empty() {
                        have.doc = attribute.doc;
                    }
                }
                None => self.attributes.push(attribute),
            }
        }
    }
}

impl Schema {
    fn read(&mut self, dir: &Path, file: &str, read: &mut BTreeSet<String>) {
        if !read.insert(file.to_string()) {
            return;
        }
        let root = match MNode::parse_file(dir.join(file)) {
            Ok(root) => root,
            Err(err) => {
                tracing::error!(error = %err, "Could not read the schema {file} for the schema digest");
                self.unknown.insert(format!("unreadable {file}"));
                return;
            }
        };
        for node in root.child_iter() {
            let name = node.attribute("name").unwrap_or_default();
            match local(&node).as_str() {
                "include" | "import" => {
                    if let Some(location) = node.attribute("schemaLocation") {
                        if !location.contains("://") {
                            self.read(dir, &location, read);
                        }
                    }
                }
                "element" => {
                    if node.attribute_is("abstract", "true") {
                        self.abstracts.insert(name.clone());
                    }
                    if let Some(head) = node.attribute("substitutionGroup") {
                        self.substitutes.entry(strip(&head)).or_default().push(name.clone());
                    }
                    self.elements.insert(name, node.clone());
                }
                "complexType" => {
                    self.complex_types.insert(name, node.clone());
                }
                "simpleType" => {
                    self.simple_types.insert(name, node.clone());
                }
                "group" => {
                    self.groups.insert(name, node.clone());
                }
                "attributeGroup" => {
                    self.attribute_groups.insert(name, node.clone());
                }
                "annotation" | "attribute" => {}
                other => {
                    self.unknown.insert(format!("{other} in {file}"));
                }
            }
        }
    }

    /// The attributes and child element declarations of an element declaration.
    fn element_content(&mut self, decl: &MNode, spec: &mut ElementSpec, children: &mut Vec<MNode>) {
        if let Some(kind) = decl.attribute("type") {
            let kind = strip(&kind);
            if let Some(complex) = self.complex_types.get(&kind).cloned() {
                self.complex_content(&complex, spec, children);
            } else {
                // A simple type: text only.
                spec.text = true;
            }
            return;
        }
        let mut any_type = true;
        for child in decl.child_iter() {
            match local(&child).as_str() {
                "complexType" => {
                    any_type = false;
                    self.complex_content(&child, spec, children);
                }
                "simpleType" => {
                    any_type = false;
                    spec.text = true;
                }
                "annotation" | "unique" | "key" | "keyref" => {}
                other => self.open(spec, other),
            }
        }
        // `<xs:element name="x"/>` with no type is anyType: text, no attributes declared.
        if any_type && decl.attribute("ref").is_none() {
            spec.text = true;
        }
    }

    fn complex_content(&mut self, complex: &MNode, spec: &mut ElementSpec, children: &mut Vec<MNode>) {
        if complex.attribute_is("mixed", "true") {
            spec.text = true;
        }
        for part in complex.child_iter() {
            match local(&part).as_str() {
                "sequence" | "choice" | "all" => self.particles(&part, spec, children),
                "group" => self.group(&part, spec, children),
                "attribute" => self.attribute(&part, spec),
                "attributeGroup" => self.attribute_group(&part, spec),
                "anyAttribute" => spec.open_attributes = true,
                "complexContent" | "simpleContent" => {
                    if local(&part) == "simpleContent" {
                        spec.text = true;
                    }
                    for derived in part.child_iter() {
                        match local(&derived).as_str() {
                            "extension" | "restriction" => {
                                if let Some(base) = derived.attribute("base") {
                                    if let Some(base) = self.complex_types.get(&strip(&base)).cloned() {
                                        self.complex_content(&base, spec, children);
                                    }
                                }
                                self.complex_content(&derived, spec, children);
                            }
                            "annotation" => {}
                            other => self.open(spec, other),
                        }
                    }
                }
                // The facets of a simple content restriction.
                "enumeration" | "pattern" | "minLength" | "maxLength" | "annotation" => {}
                other => self.open(spec, other),
            }
        }
    }

    fn particles(&mut self, particle: &MNode, spec: &mut ElementSpec, children: &mut Vec<MNode>) {
        for part in particle.child_iter() {
            match local(&part).as_str() {
                "element" => self.child_element(&part, children),
                "sequence" | "choice" | "all" => self.particles(&part, spec, children),
                "group" => self.group(&part, spec, children),
                "any" => spec.open_children = true,
                "annotation" => {}
                other => self.open(spec, other),
            }
        }
    }

    fn group(&mut self, reference: &MNode, spec: &mut ElementSpec, children: &mut Vec<MNode>) {
        let Some(name) = reference.attribute("ref").map(|name| strip(&name)) else {
            return self.particles(reference, spec, children);
        };
        match self.groups.get(&name).cloned() {
            Some(group) => self.particles(&group, spec, children),
            None => self.open(spec, &format!("group {name}")),
        }
    }

    /// A child element: a local declaration as it is, a reference resolved to its global declaration, and a reference
    /// to an abstract head to the members of its substitution group.
    fn child_element(&mut self, element: &MNode, children: &mut Vec<MNode>) {
        let Some(reference) = element.attribute("ref").map(|name| strip(&name)) else {
            children.push(element.clone());
            return;
        };
        let mut heads = vec![reference];
        let mut seen = BTreeSet::new();
        while let Some(name) = heads.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(members) = self.substitutes.get(&name) {
                heads.extend(members.iter().cloned());
            }
            if self.abstracts.contains(&name) {
                continue;
            }
            match self.elements.get(&name) {
                Some(decl) => children.push(decl.clone()),
                None => {
                    self.unknown.insert(format!("element ref {name}"));
                }
            }
        }
    }

    fn attribute(&mut self, attribute: &MNode, spec: &mut ElementSpec) {
        let Some(name) = attribute.attribute("name") else {
            if attribute.attribute("ref").is_some() {
                spec.open_attributes = true;
            }
            return;
        };
        let mut found = AttributeSpec {
            name,
            required: attribute.attribute_is("use", "required"),
            default: attribute.attribute("default"),
            doc: documentation(attribute),
            ..AttributeSpec::default()
        };
        if let Some(kind) = attribute.attribute("type") {
            self.values_of_type(&strip(&kind), &mut found, 0);
        }
        if let Some(simple) = attribute.first_where(|child| local(child) == "simpleType") {
            self.values_of_simple(&simple, &mut found, 0);
        }
        spec.attributes.push(found);
    }

    fn attribute_group(&mut self, reference: &MNode, spec: &mut ElementSpec) {
        let Some(name) = reference.attribute("ref").map(|name| strip(&name)) else {
            return;
        };
        let Some(group) = self.attribute_groups.get(&name).cloned() else {
            return self.open(spec, &format!("attributeGroup {name}"));
        };
        for part in group.child_iter() {
            match local(&part).as_str() {
                "attribute" => self.attribute(&part, spec),
                "attributeGroup" => self.attribute_group(&part, spec),
                "anyAttribute" => spec.open_attributes = true,
                "annotation" => {}
                other => self.open(spec, other),
            }
        }
    }

    fn values_of_type(&self, kind: &str, into: &mut AttributeSpec, depth: usize) {
        if depth > 8 {
            return;
        }
        if kind == "boolean" {
            into.values.extend(["true".to_string(), "false".to_string()]);
            return;
        }
        if let Some(simple) = self.simple_types.get(kind) {
            self.values_of_simple(simple, into, depth + 1);
        }
    }

    fn values_of_simple(&self, simple: &MNode, into: &mut AttributeSpec, depth: usize) {
        for part in simple.child_iter() {
            match local(&part).as_str() {
                "restriction" => {
                    let enumerations: Vec<String> = part
                        .child_iter()
                        .filter(|facet| local(facet) == "enumeration")
                        .filter_map(|facet| facet.attribute("value"))
                        .collect();
                    if enumerations.is_empty() {
                        if let Some(base) = part.attribute("base") {
                            self.values_of_type(&strip(&base), into, depth + 1);
                        }
                    } else {
                        into.values.extend(enumerations);
                    }
                }
                "union" => {
                    let mut free = false;
                    for member in part.attribute("memberTypes").unwrap_or_default().split_whitespace() {
                        let member = strip(member);
                        let before = into.values.len();
                        self.values_of_type(&member, into, depth + 1);
                        free |= into.values.len() == before;
                    }
                    for inline in part.child_iter().filter(|child| local(child) == "simpleType") {
                        let before = into.values.len();
                        self.values_of_simple(&inline, into, depth + 1);
                        free |= into.values.len() == before;
                    }
                    into.open_values |= free;
                }
                _ => {}
            }
        }
    }

    fn open(&mut self, spec: &mut ElementSpec, construct: &str) {
        spec.open_children = true;
        spec.open_attributes = true;
        self.unknown.insert(format!("{construct} in {}", spec.name));
    }
}

/// The local name of an `xs:` element.
fn local(node: &MNode) -> String {
    let name = node.name();
    name.rsplit(':').next().unwrap_or(&name).to_string()
}

/// A type or element name without its namespace prefix.
fn strip(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// The `xs:annotation/xs:documentation` text, with its whitespace collapsed.
fn documentation(node: &MNode) -> String {
    let mut text = String::new();
    for annotation in node.child_iter().filter(|child| local(child) == "annotation") {
        for doc in annotation.child_iter().filter(|child| local(child) == "documentation") {
            if let Some(body) = doc.text() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&body.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xsd_dir() -> PathBuf {
        moqui_core::context::framework_dir().join("xsd")
    }

    #[test]
    fn every_root_reads_with_nothing_left_open() {
        for (root, _) in ROOTS {
            let digest = digest(&xsd_dir(), root).unwrap_or_else(|| panic!("no digest for {root}"));
            assert!(digest.unknown.is_empty(), "{root}: {:?}", digest.unknown);
            assert!(digest.elements[0].top && digest.elements[0].name == *root);
            assert!(digest.elements.len() > 2, "{root}: {}", digest.elements.len());
        }
    }

    #[test]
    fn spot_checks() {
        let services = digest(&xsd_dir(), "services").unwrap();
        let service = services.element("service").unwrap();
        for child in ["in-parameters", "out-parameters", "actions"] {
            assert!(
                service.children.iter().any(|name| name == child),
                "{:?}",
                service.children
            );
        }
        assert!(!service.doc.is_empty() || service.attribute("verb").is_some());
        assert!(service.attribute("verb").unwrap().required);
        // An action from a substitution group of xml-actions-3.xsd, with its enumerated values.
        let find = services.element("entity-find").unwrap();
        assert!(find.attribute("entity-name").unwrap().required);
        let actions = services.element("actions").unwrap();
        assert!(
            actions.children.iter().any(|name| name == "entity-find"),
            "{:?}",
            actions.children
        );
        assert!(actions.children.iter().any(|name| name == "service-call"));
        assert!(!actions.children.iter().any(|name| name == "CallOperations"));
        let screen = digest(&xsd_dir(), "screen").unwrap();
        let text_area = screen.element("text-area").unwrap();
        assert!(text_area.attribute("editor-type").is_some());
        let set = screen.element("set").unwrap();
        assert!(!set.attribute("field").unwrap().doc.is_empty());
        let entities = digest(&xsd_dir(), "entities").unwrap();
        let field = entities.element("field").unwrap();
        let encrypt = field.attribute("encrypt").unwrap();
        assert!(encrypt.values.contains(&"true".to_string()), "{encrypt:?}");
    }

    #[test]
    fn the_root_of_a_file() {
        assert_eq!(
            root_of("<?xml version=\"1.0\"?>\n<screen require-authentication=\"false\"/>"),
            Some("screen")
        );
        assert_eq!(root_of("<entity-facade-xml/>"), None);
    }
}
