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

//! XML tree. Parsing is filled in against the corpus. No XML crate.
//!
//! Direct character data stays on the element that contains it. A node may have
//! both text and children. The old SAX handler attached still-unassigned text to
//! the next child; these files do not depend on that.

mod parse;

pub use parse::{XmlEvent, XmlEvents};

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;

use crate::error::Error;

/// One element. Clones share the same node, the way a Java reference does.
#[derive(Clone)]
pub struct MNode {
    inner: Rc<RefCell<NodeInner>>,
}

pub(crate) struct NodeInner {
    name: String,
    attributes: IndexMap<String, String>,
    text: Option<String>,
    children: Vec<MNode>,
    parent: Option<Weak<RefCell<NodeInner>>>,
    location: Option<String>,
    line: u32,
    column: u32,
}

/// Called with the base node and the override node when a merge visits both.
type NodeMerger<'a> = dyn FnMut(&MNode, &MNode) + 'a;

impl MNode {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        name: String,
        attributes: IndexMap<String, String>,
        text: Option<String>,
        children: Vec<MNode>,
        parent: Option<Weak<RefCell<NodeInner>>>,
        location: Option<String>,
        line: u32,
        column: u32,
    ) -> Self {
        let node = Self {
            inner: Rc::new(RefCell::new(NodeInner {
                name,
                attributes,
                text,
                children: Vec::new(),
                parent,
                location,
                line,
                column,
            })),
        };
        for child in children {
            node.append_node(child);
        }
        node
    }

    /// Build an element. Used by tests and by code that assembles a tree without a file.
    pub fn new(name: impl Into<String>) -> Self {
        Self::from_parts(name.into(), IndexMap::new(), None, Vec::new(), None, None, 0, 0)
    }

    pub fn parse_text(location: &str, text: &str) -> Result<Self, Error> {
        parse::parse_document(Some(location), text, false)
    }

    /// Parse keeping each element's text as written, whitespace included, as SAX `characters` gives it; the entity
    /// data loader reads field text this way, as Java's does.
    pub fn parse_text_raw(location: &str, text: &str) -> Result<Self, Error> {
        parse::parse_document_with(Some(location), text, false, true)
    }

    pub fn parse_root_only(location: &str, text: &str) -> Result<Self, Error> {
        parse::parse_document(Some(location), text, true)
    }

    pub fn parse_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
        Self::parse_text(&path.display().to_string(), &text)
    }

    pub fn name(&self) -> String {
        self.inner.borrow().name.clone()
    }

    pub fn set_name(&self, name: impl Into<String>) {
        self.inner.borrow_mut().name = name.into();
    }

    pub fn file_location(&self) -> Option<String> {
        self.inner.borrow().location.clone()
    }

    pub fn line(&self) -> u32 {
        self.inner.borrow().line
    }

    pub fn column(&self) -> u32 {
        self.inner.borrow().column
    }

    pub fn attribute(&self, name: &str) -> Option<String> {
        self.inner.borrow().attributes.get(name).cloned()
    }

    pub fn attributes(&self) -> Vec<(String, String)> {
        self.inner
            .borrow()
            .attributes
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn set_attribute(&self, name: impl Into<String>, value: impl Into<String>) {
        self.inner.borrow_mut().attributes.insert(name.into(), value.into());
    }

    /// Copy attribute values from `other` onto this node. Existing attributes not in `other` stay.
    pub fn copy_attributes_from(&self, other: &MNode) {
        let pairs = other.attributes();
        let mut inner = self.inner.borrow_mut();
        for (key, value) in pairs {
            inner.attributes.insert(key, value);
        }
    }

    pub fn text(&self) -> Option<String> {
        self.inner.borrow().text.clone()
    }

    pub fn set_text(&self, text: Option<String>) {
        self.inner.borrow_mut().text = text.filter(|value| !value.trim().is_empty());
    }

    pub fn parent(&self) -> Option<MNode> {
        self.inner
            .borrow()
            .parent
            .as_ref()
            .and_then(Weak::upgrade)
            .map(|inner| MNode { inner })
    }

    pub fn children(&self) -> Vec<MNode> {
        self.inner.borrow().children.clone()
    }

    pub fn children_named(&self, name: &str) -> Vec<MNode> {
        self.inner
            .borrow()
            .children
            .iter()
            .filter(|child| child.name() == name)
            .cloned()
            .collect()
    }

    pub fn first_child(&self) -> Option<MNode> {
        self.inner.borrow().children.first().cloned()
    }

    pub fn first_named(&self, name: &str) -> Option<MNode> {
        self.children_named(name).into_iter().next()
    }

    pub fn first_named_attrs(&self, name: &str, attrs: &[(&str, &str)]) -> Option<MNode> {
        self.children_named(name).into_iter().find(|child| {
            attrs
                .iter()
                .all(|(key, value)| child.attribute(key).as_deref() == Some(*value))
        })
    }

    pub fn first_where(&self, mut pred: impl FnMut(&MNode) -> bool) -> Option<MNode> {
        self.children().into_iter().find(|child| pred(child))
    }

    pub fn append(&self, name: impl Into<String>) -> MNode {
        let child = MNode::new(name);
        self.append_node(child.clone());
        child
    }

    pub fn append_node(&self, child: MNode) {
        child.inner.borrow_mut().parent = Some(Rc::downgrade(&self.inner));
        if let Some(location) = self.file_location() {
            if child.file_location().is_none() {
                child.inner.borrow_mut().location = Some(location);
            }
        }
        self.inner.borrow_mut().children.push(child);
    }

    /// Insert `child` before the child at `index`. An index past the end appends.
    pub fn insert_node(&self, index: usize, child: MNode) {
        child.inner.borrow_mut().parent = Some(Rc::downgrade(&self.inner));
        let mut inner = self.inner.borrow_mut();
        let index = index.min(inner.children.len());
        inner.children.insert(index, child);
    }

    /// Position of `child` among this node's children, by identity.
    pub fn index_of(&self, child: &MNode) -> Option<usize> {
        self.inner
            .borrow()
            .children
            .iter()
            .position(|candidate| Rc::ptr_eq(&candidate.inner, &child.inner))
    }

    pub fn replace_child(&self, index: usize, child: MNode) -> Result<(), Error> {
        let inner = self.inner.borrow_mut();
        if index >= inner.children.len() {
            return Err(Error::new(format!("child index {index} is past the end")));
        }
        drop(inner);
        child.inner.borrow_mut().parent = Some(Rc::downgrade(&self.inner));
        self.inner.borrow_mut().children[index] = child;
        Ok(())
    }

    pub fn remove_where(&self, mut pred: impl FnMut(&MNode) -> bool) -> bool {
        let children = self.children();
        let mut kept = Vec::new();
        let mut removed = false;
        for child in children {
            if pred(&child) {
                child.inner.borrow_mut().parent = None;
                removed = true;
            } else {
                kept.push(child);
            }
        }
        if removed {
            self.inner.borrow_mut().children = kept;
        }
        removed
    }

    pub fn clear_children(&self) {
        for child in self.children() {
            child.inner.borrow_mut().parent = None;
        }
        self.inner.borrow_mut().children.clear();
    }

    pub fn replace_children_from(&self, other: &MNode) {
        self.clear_children();
        for child in other.children() {
            self.append_node(child.deep_copy());
        }
    }

    pub fn deep_copy(&self) -> MNode {
        let inner = self.inner.borrow();
        let children = inner.children.clone();
        let name = inner.name.clone();
        let attributes = inner.attributes.clone();
        let text = inner.text.clone();
        let location = inner.location.clone();
        let line = inner.line;
        let column = inner.column;
        drop(inner);
        let copied_children: Vec<MNode> = children.iter().map(MNode::deep_copy).collect();
        Self::from_parts(name, attributes, text, copied_children, None, location, line, column)
    }

    /// Same structure, ignoring file location and shared identity.
    pub fn same_structure(&self, other: &MNode) -> bool {
        if self.name() != other.name() || self.attributes() != other.attributes() || self.text() != other.text() {
            return false;
        }
        let left = self.children();
        let right = other.children();
        left.len() == right.len() && left.iter().zip(right.iter()).all(|(a, b)| a.same_structure(b))
    }

    pub fn merge_single_child(&self, override_node: &MNode, child_name: &str) {
        let Some(override_child) = override_node.first_named(child_name) else {
            return;
        };
        if let Some(base_child) = self.first_named(child_name) {
            base_child.copy_attributes_from(&override_child);
            if !override_child.children().is_empty() {
                base_child.replace_children_from(&override_child);
            }
        } else {
            self.append_node(override_child.deep_copy());
        }
    }

    pub fn merge_child_with_child_key(
        &self,
        override_node: &MNode,
        child_name: &str,
        grandchild_name: &str,
        key_attrs: &[&str],
        merger: Option<&mut NodeMerger<'_>>,
    ) {
        let Some(override_child) = override_node.first_named(child_name) else {
            return;
        };
        if let Some(base_child) = self.first_named(child_name) {
            base_child.merge_node_with_child_key(&override_child, grandchild_name, key_attrs, merger);
        } else {
            self.append_node(override_child.deep_copy());
        }
    }

    pub fn merge_node_with_child_key(
        &self,
        override_node: &MNode,
        child_name: &str,
        key_attrs: &[&str],
        merger: Option<&mut NodeMerger<'_>>,
    ) {
        self.copy_attributes_from(override_node);
        self.merge_children_by_keys(override_node, child_name, key_attrs, merger);
    }

    /// Match children named `child_name`.
    ///
    /// An override child with an empty key attribute is skipped. With no key attributes,
    /// every override child matches the first base child of that name. A missing merger
    /// replaces that child's children with copies of the override child's children.
    pub fn merge_children_by_keys(
        &self,
        override_node: &MNode,
        child_name: &str,
        key_attrs: &[&str],
        mut merger: Option<&mut NodeMerger<'_>>,
    ) {
        for override_child in override_node.children_named(child_name) {
            let mut key_values = Vec::with_capacity(key_attrs.len());
            let mut skip = false;
            for key in key_attrs {
                match non_empty(override_child.attribute(key)) {
                    Some(value) => key_values.push(value),
                    None => skip = true,
                }
            }
            if skip {
                continue;
            }
            let base_child = self.children_named(child_name).into_iter().find(|child| {
                if key_attrs.is_empty() {
                    return true;
                }
                key_attrs
                    .iter()
                    .zip(key_values.iter())
                    .all(|(key, value)| child.attribute(key).as_deref() == Some(value.as_str()))
            });
            if let Some(base_child) = base_child {
                base_child.copy_attributes_from(&override_child);
                if let Some(merger) = merger.as_mut() {
                    merger(&base_child, &override_child);
                } else {
                    base_child.replace_children_from(&override_child);
                }
            } else {
                self.append_node(override_child.deep_copy());
            }
        }
    }

    /// `MNode.toString`: four-space indent, attributes in insertion order, child text as CDATA.
    pub fn write_string(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out, 0);
        out
    }

    fn write_into(&self, out: &mut String, level: usize) {
        for _ in 0..level {
            out.push_str("    ");
        }
        let name = self.name();
        out.push('<');
        out.push_str(&name);
        for (key, value) in self.attributes() {
            out.push(' ');
            out.push_str(&key);
            out.push_str("=\"");
            for ch in value.chars() {
                match ch {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '"' => out.push_str("&quot;"),
                    _ => out.push(ch),
                }
            }
            out.push('"');
        }
        let text = self.text();
        let children = self.children();
        if text.is_some() || !children.is_empty() {
            out.push('>');
            if let Some(text) = text {
                out.push_str("<![CDATA[");
                out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
                out.push_str("]]>");
            }
            if !children.is_empty() {
                for child in &children {
                    out.push('\n');
                    child.write_into(out, level + 1);
                }
                if children.len() > 1 {
                    out.push('\n');
                    for _ in 0..level {
                        out.push_str("    ");
                    }
                }
            }
            out.push_str("</");
            out.push_str(&name);
            out.push('>');
        } else {
            out.push_str("/>");
        }
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn expect_parse_err(result: Result<MNode, crate::error::Error>) -> crate::error::Error {
        match result {
            Err(err) => err,
            Ok(_) => panic!("expected a parse error"),
        }
    }

    #[test]
    fn deeply_nested_elements_are_an_error_not_a_stack_overflow() {
        let deep = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
        let err = expect_parse_err(MNode::parse_text("deep.xml", &deep));
        assert!(err.to_string().contains("nested deeper"), "{err}");
        let fine = format!("{}{}", "<a>".repeat(100), "</a>".repeat(100));
        MNode::parse_text("fine.xml", &fine).unwrap();
    }

    #[test]
    fn declaration_comment_cdata_entities_and_raw_names() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <!-- comment with &not-an-entity; -->
        <root xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" b="1" a="&lt;&gt;&amp;&apos;&quot;&#65;&#x42;">
            <![CDATA[  hello & not-an-entity  ]]>
            <child/>
        </root>
        "#;
        let root = MNode::parse_text("sample.xml", xml).unwrap();
        assert_eq!(root.name(), "root");
        assert_eq!(root.line(), 3);
        let attrs = root.attributes();
        assert_eq!(
            attrs.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            vec!["xmlns:xsi", "b", "a"]
        );
        assert_eq!(root.attribute("a").as_deref(), Some("<>&'\"AB"));
        assert_eq!(root.text().as_deref(), Some("hello & not-an-entity"));
        assert_eq!(root.children_named("child").len(), 1);
        assert!(root.first_named("child").unwrap().text().is_none());
    }

    #[test]
    fn raw_text_keeps_whitespace() {
        let xml = "<root>\n  <a>  spaced  </a><b> </b><c><![CDATA[ x ]]>\n</c><d/></root>";
        let raw = MNode::parse_text_raw("raw.xml", xml).unwrap();
        let text = |name: &str| raw.first_named(name).and_then(|child| child.text());
        assert_eq!(text("a").as_deref(), Some("  spaced  "));
        assert_eq!(text("b").as_deref(), Some(" "));
        assert_eq!(text("c").as_deref(), Some(" x \n"));
        assert_eq!(text("d"), None);
        let trimmed = MNode::parse_text("trimmed.xml", xml).unwrap();
        assert_eq!(
            trimmed.first_named("a").and_then(|child| child.text()).as_deref(),
            Some("spaced")
        );
        assert!(trimmed.first_named("b").and_then(|child| child.text()).is_none());
    }

    #[test]
    fn writer_cdata_round_trips() {
        let root = MNode::parse_text("t.xml", r#"<root b="1" a="2">a &lt; b &amp; c<child>x</child></root>"#).unwrap();
        let again = MNode::parse_text("t.xml", &root.write_string()).unwrap();
        assert!(root.same_structure(&again));
        assert_eq!(again.text().as_deref(), Some("a < b & c"));
        assert_eq!(again.attributes()[0].0, "b");
    }

    #[test]
    fn writer_splits_cdata_end_in_text() {
        let root = MNode::parse_text("t.xml", "<root>a ]]&gt; b]]&gt;<child/></root>").unwrap();
        assert_eq!(root.text().as_deref(), Some("a ]]> b]]>"));
        let written = root.write_string();
        let again = MNode::parse_text("t.xml", &written).unwrap();
        assert_eq!(again.text().as_deref(), Some("a ]]> b]]>"), "{written}");
        assert_eq!(again.children().len(), 1);
    }

    #[test]
    fn doctype_and_unknown_entity_report_line_and_column() {
        let err = expect_parse_err(MNode::parse_text("bad.xml", "<root>\n  <!DOCTYPE foo>\n</root>"));
        assert!(err.message().contains("DOCTYPE"));
        let loc = err.location().unwrap();
        assert_eq!(loc.file.as_deref(), Some("bad.xml"));
        assert_eq!(loc.line, 2);
        assert_eq!(loc.column, 3);

        let err = expect_parse_err(MNode::parse_text("bad.xml", "<root>&foo;</root>"));
        assert!(err.message().contains("unknown entity"));
        assert_eq!(err.location().unwrap().line, 1);
    }

    #[test]
    fn mismatched_end_tag_names_the_element() {
        let err = expect_parse_err(MNode::parse_text("bad.xml", "<root>\n  <child>\n</root>"));
        assert!(err.message().contains("</root>"));
        assert!(err.message().contains("<child>"));
        assert_eq!(err.location().unwrap().line, 3);
    }

    #[test]
    fn root_only_stops_after_the_start_tag() {
        let root = MNode::parse_root_only("t.xml", r#"<root a="1"><child/></root>"#).unwrap();
        assert_eq!(root.attribute("a").as_deref(), Some("1"));
        assert!(root.children().is_empty());
    }

    #[test]
    fn merge_children_by_key_and_single_child() {
        let base = MNode::parse_text(
            "b.xml",
            r#"<root><cache-list warm-on-start="true"><cache name="entity.definition" value-type="Entity"/></cache-list><user-facade><password min-length="8"/></user-facade></root>"#,
        )
        .unwrap();
        let over = MNode::parse_text(
            "o.xml",
            r#"<root><cache-list warm-on-start="false"><cache name="entity.definition" expire-time-idle="30"/><cache name="new.one"/></cache-list><user-facade><password min-length="12"><extra/></password></user-facade></root>"#,
        )
        .unwrap();
        base.merge_child_with_child_key(&over, "cache-list", "cache", &["name"], None);
        base.merge_child_with_child_key(&over, "user-facade", "password", &[], None);
        let cache_list = base.first_named("cache-list").unwrap();
        assert_eq!(cache_list.attribute("warm-on-start").as_deref(), Some("false"));
        let entity = cache_list
            .first_named_attrs("cache", &[("name", "entity.definition")])
            .unwrap();
        assert_eq!(entity.attribute("expire-time-idle").as_deref(), Some("30"));
        assert_eq!(entity.attribute("value-type").as_deref(), Some("Entity"));
        assert!(cache_list.first_named_attrs("cache", &[("name", "new.one")]).is_some());
        let password = base
            .first_named("user-facade")
            .unwrap()
            .first_named("password")
            .unwrap();
        assert_eq!(password.attribute("min-length").as_deref(), Some("12"));
        assert!(password.first_named("extra").is_some());
    }

    #[test]
    fn component_version_attribute_is_not_expanded() {
        let path = workspace_root().join("runtime/base-component/framework/component.xml");
        let root = MNode::parse_file(&path).unwrap();
        assert_eq!(root.attribute("name").as_deref(), Some("framework"));
        assert_eq!(root.attribute("version").as_deref(), Some("${moqui_version}"));
    }

    #[test]
    fn corpus_parses_and_catalog_doctype_is_rejected() {
        let root = workspace_root();
        let mut files = Vec::new();
        collect_xml(&root.join("runtime"), &mut files);
        assert!(files.len() > 100, "copied {} files", files.len());
        let mut parsed = 0;
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            if path.ends_with("framework-catalog.xml") {
                let err = expect_parse_err(MNode::parse_text(&path.display().to_string(), &text));
                assert!(err.message().contains("DOCTYPE"), "{path:?} {err}");
                continue;
            }
            MNode::parse_text(&path.display().to_string(), &text)
                .unwrap_or_else(|err| panic!("parse {} failed: {err}", path.display()));
            parsed += 1;
        }
        assert!(parsed > 100);
    }

    #[test]
    fn dev_conf_overlay_changes_the_sections_it_names() {
        let root = workspace_root();
        let base = MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).unwrap();
        let over = MNode::parse_file(root.join("runtime/conf/MoquiDevConf.xml")).unwrap();
        merge_dev_conf(&base, &over);

        let purpose = base
            .children_named("default-property")
            .into_iter()
            .find(|node| node.attribute("name").as_deref() == Some("instance_purpose"))
            .unwrap();
        assert_eq!(purpose.attribute("value").as_deref(), Some("dev"));
        let locale = base
            .children_named("default-property")
            .into_iter()
            .find(|node| node.attribute("name").as_deref() == Some("default_locale"))
            .unwrap();
        assert_eq!(locale.attribute("value").as_deref(), Some("en_US"));

        let caches = base.first_named("cache-list").unwrap();
        assert_eq!(caches.attribute("warm-on-start").as_deref(), Some("false"));
        let entity = caches
            .first_named_attrs("cache", &[("name", "entity.definition")])
            .unwrap();
        assert_eq!(entity.attribute("expire-time-idle").as_deref(), Some("30"));
        assert!(entity.attribute("value-type").is_some());

        let screen = base
            .first_named("artifact-execution-facade")
            .unwrap()
            .first_named_attrs("artifact-execution", &[("type", "AT_XML_SCREEN")])
            .unwrap();
        assert_eq!(screen.attribute("tarpit-enabled").as_deref(), Some("false"));

        let webapp = base
            .first_named("webapp-list")
            .unwrap()
            .first_named_attrs("webapp", &[("name", "webroot")])
            .unwrap();
        let servlet = webapp.first_named_attrs("servlet", &[("name", "H2Console")]).unwrap();
        assert_eq!(
            servlet.attribute("class").as_deref(),
            Some("org.h2.server.web.JakartaWebServlet")
        );
        assert!(servlet.first_named("url-pattern").is_some());
    }

    #[test]
    fn mantle_udm_parses_when_the_sibling_tree_is_present() {
        let path = workspace_root().join("../moqui/runtime/component/mantle-udm");
        if !path.is_dir() {
            return;
        }
        let mut files = Vec::new();
        collect_xml(&path, &mut files);
        assert!(!files.is_empty());
        for file in files {
            MNode::parse_file(&file).unwrap_or_else(|err| panic!("parse {} failed: {err}", file.display()));
        }
    }

    fn merge_dev_conf(base: &MNode, over: &MNode) {
        base.merge_children_by_keys(over, "default-property", &["name"], None);
        base.merge_child_with_child_key(over, "cache-list", "cache", &["name"], None);
        if let (Some(stats), Some(override_stats)) =
            (base.first_named("server-stats"), over.first_named("server-stats"))
        {
            stats.copy_attributes_from(&override_stats);
            for child in override_stats.children_named("artifact-stats") {
                let found = stats
                    .children_named("artifact-stats")
                    .into_iter()
                    .find(|node| same_attr(node, &child, "type") && same_attr(node, &child, "sub-type"));
                if let Some(base_child) = found {
                    base_child.copy_attributes_from(&child);
                } else {
                    stats.append_node(child.deep_copy());
                }
            }
        }
        base.merge_child_with_child_key(
            over,
            "webapp-list",
            "webapp",
            &["name"],
            Some(&mut |base_web, over_web| {
                base_web.merge_children_by_keys(over_web, "servlet", &["name"], None);
            }),
        );
        base.merge_child_with_child_key(over, "artifact-execution-facade", "artifact-execution", &["type"], None);
    }

    fn same_attr(left: &MNode, right: &MNode, name: &str) -> bool {
        non_empty(left.attribute(name)) == non_empty(right.attribute(name))
    }

    fn collect_xml(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_xml(&path, out);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "xml" || ext == "xsd")
            {
                out.push(path);
            }
        }
    }
}
