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

pub use parse::{FlatElement, XmlEvent, XmlEvents};

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::{Rc, Weak};

use crate::error::Error;

/// One element. Clones share the same node, the way a Java reference does.
#[derive(Clone)]
pub struct MNode {
    inner: Rc<RefCell<NodeInner>>,
}

/// Attribute names (interned) and values, in document order. A node has few, so a list is smaller and as quick to
/// search as a map.
pub(crate) type Attributes = Vec<(Rc<str>, Box<str>)>;

pub(crate) struct NodeInner {
    /// Interned: the elements of one name share it, as Java's `MNode` interns its names.
    name: Rc<str>,
    attributes: Attributes,
    text: Option<Box<str>>,
    children: Vec<MNode>,
    parent: Option<Weak<RefCell<NodeInner>>>,
    /// Shared by the nodes of one document.
    location: Option<Rc<str>>,
    line: u32,
    column: u32,
}

/// Called with the base node and the override node when a merge visits both.
type NodeMerger<'a> = dyn FnMut(&MNode, &MNode) + 'a;

/// Distinct names kept per thread. Past this a name is not interned, so a document of made-up names cannot grow the
/// set without bound.
const NAMES_CAP: usize = 16_384;

/// How much text one thread keeps parsed for [`MNode::parse_shared`], and how many documents.
const SHARED_BYTES: usize = 1024 * 1024;
const SHARED_COUNT: usize = 64;

thread_local! {
    static NAMES: RefCell<HashSet<Rc<str>>> = RefCell::new(HashSet::new());
    /// Documents this thread parsed for [`MNode::parse_shared`], by their text, the most recently used last.
    static SHARED: RefCell<Vec<(Rc<str>, MNode)>> = const { RefCell::new(Vec::new()) };
}

/// The one `Rc` of `name` on this thread, element and attribute names alike.
pub(crate) fn intern(name: &str) -> Rc<str> {
    NAMES.with(|names| {
        let mut names = names.borrow_mut();
        if let Some(known) = names.get(name) {
            return Rc::clone(known);
        }
        let made: Rc<str> = Rc::from(name);
        if names.len() < NAMES_CAP {
            names.insert(Rc::clone(&made));
        }
        made
    })
}

impl MNode {
    pub(crate) fn from_parts(
        name: Rc<str>,
        attributes: Attributes,
        text: Option<Box<str>>,
        children: Vec<MNode>,
        location: Option<Rc<str>>,
        line: u32,
        column: u32,
    ) -> Self {
        let node = Self {
            inner: Rc::new(RefCell::new(NodeInner {
                name,
                attributes,
                text,
                children: Vec::new(),
                parent: None,
                location,
                line,
                column,
            })),
        };
        if !children.is_empty() {
            let parent = Rc::downgrade(&node.inner);
            for child in &children {
                let mut inner = child.inner.borrow_mut();
                inner.parent = Some(Weak::clone(&parent));
                if inner.location.is_none() {
                    inner.location.clone_from(&node.inner.borrow().location);
                }
            }
            node.inner.borrow_mut().children = children;
        }
        node
    }

    /// Build an element. Used by tests and by code that assembles a tree without a file.
    pub fn new(name: impl Into<String>) -> Self {
        Self::from_parts(intern(&name.into()), Vec::new(), None, Vec::new(), None, 0, 0)
    }

    pub fn parse_text(location: &str, text: &str) -> Result<Self, Error> {
        parse::parse_document(Some(location), text, false)
    }

    /// Parse keeping each element's text as written, whitespace included, as SAX `characters` gives it; the entity
    /// data loader reads field text this way, as Java's does.
    pub fn parse_text_raw(location: &str, text: &str) -> Result<Self, Error> {
        parse::parse_document_with(Some(location), text, false, true)
    }

    /// The root of `text` when it has no child elements, its name and attributes borrowed from the text, without a
    /// node; `None` when it has child elements. The document is checked as [`Self::parse_text_raw`] checks it.
    pub fn parse_flat<'a>(location: &str, text: &'a str) -> Result<Option<FlatElement<'a>>, Error> {
        parse::parse_flat(Some(location), text)
    }

    /// [`Self::parse_text`] of `text`, the same tree for every reader of the same text on this thread, so readers of
    /// one document (a cached definition's XML, the merged conf) do not each parse it. The tree is shared: read it,
    /// never change it or append it to another tree, since a change would show to the next reader. Parse a tree of
    /// one's own to change. A text is found by its content; up to 64 documents and 1 MB of text are kept per thread,
    /// the least recently used dropped first.
    pub fn parse_shared(location: &str, text: &str) -> Result<Self, Error> {
        let found = SHARED.with(|shared| {
            let mut shared = shared.borrow_mut();
            let index = shared.iter().position(|(held, _)| **held == *text)?;
            let entry = shared.remove(index);
            let node = entry.1.clone();
            shared.push(entry);
            Some(node)
        });
        if let Some(node) = found {
            return Ok(node);
        }
        let node = Self::parse_text(location, text)?;
        if text.len() <= SHARED_BYTES {
            SHARED.with(|shared| {
                let mut shared = shared.borrow_mut();
                let mut held = shared.iter().map(|(kept, _)| kept.len()).sum::<usize>() + text.len();
                while !shared.is_empty() && (held > SHARED_BYTES || shared.len() >= SHARED_COUNT) {
                    held -= shared.remove(0).0.len();
                }
                shared.push((Rc::from(text), node.clone()));
            });
        }
        Ok(node)
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
        self.inner.borrow().name.to_string()
    }

    /// The element's interned name, shared rather than copied: `match &*node.name_rc() { ... }`.
    pub fn name_rc(&self) -> Rc<str> {
        Rc::clone(&self.inner.borrow().name)
    }

    /// `body` with the element's name, borrowed.
    pub fn with_name<R>(&self, body: impl FnOnce(&str) -> R) -> R {
        body(&self.inner.borrow().name)
    }

    /// Whether the element is named `name`, without copying its name.
    pub fn has_name(&self, name: &str) -> bool {
        *self.inner.borrow().name == *name
    }

    pub fn set_name(&self, name: impl Into<String>) {
        self.inner.borrow_mut().name = intern(&name.into());
    }

    pub fn file_location(&self) -> Option<String> {
        self.inner.borrow().location.as_deref().map(str::to_string)
    }

    pub fn line(&self) -> u32 {
        self.inner.borrow().line
    }

    pub fn column(&self) -> u32 {
        self.inner.borrow().column
    }

    pub fn attribute(&self, name: &str) -> Option<String> {
        self.with_attribute(name, |value| value.map(str::to_string))
    }

    /// `body` with the value of attribute `name`, borrowed.
    pub fn with_attribute<R>(&self, name: &str, body: impl FnOnce(Option<&str>) -> R) -> R {
        let inner = self.inner.borrow();
        body(find_attribute(&inner.attributes, name))
    }

    /// Whether attribute `name` is `value`, without copying it.
    pub fn attribute_is(&self, name: &str, value: &str) -> bool {
        self.with_attribute(name, |held| held == Some(value))
    }

    pub fn has_attribute(&self, name: &str) -> bool {
        self.with_attribute(name, |held| held.is_some())
    }

    pub fn attributes(&self) -> Vec<(String, String)> {
        self.inner
            .borrow()
            .attributes
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// Each attribute in document order, borrowed: [`Self::attributes`] without the copies.
    pub fn for_each_attribute(&self, mut body: impl FnMut(&str, &str)) {
        for (name, value) in &self.inner.borrow().attributes {
            body(name, value);
        }
    }

    pub fn set_attribute(&self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value: Box<str> = value.into().into_boxed_str();
        let mut inner = self.inner.borrow_mut();
        match inner.attributes.iter_mut().find(|(key, _)| **key == *name) {
            Some(slot) => slot.1 = value,
            None => inner.attributes.push((intern(&name), value)),
        }
    }

    /// Copy attribute values from `other` onto this node. Existing attributes not in `other` stay.
    pub fn copy_attributes_from(&self, other: &MNode) {
        if Rc::ptr_eq(&self.inner, &other.inner) {
            return;
        }
        let other = other.inner.borrow();
        let mut inner = self.inner.borrow_mut();
        for (key, value) in &other.attributes {
            match inner.attributes.iter_mut().find(|(held, _)| **held == **key) {
                Some(slot) => slot.1.clone_from(value),
                None => inner.attributes.push((Rc::clone(key), value.clone())),
            }
        }
    }

    pub fn text(&self) -> Option<String> {
        self.inner.borrow().text.as_deref().map(str::to_string)
    }

    /// `body` with the element's text, borrowed.
    pub fn with_text<R>(&self, body: impl FnOnce(Option<&str>) -> R) -> R {
        body(self.inner.borrow().text.as_deref())
    }

    pub fn set_text(&self, text: Option<String>) {
        self.inner.borrow_mut().text = text
            .filter(|value| !value.trim().is_empty())
            .map(String::into_boxed_str);
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

    /// Each child in order, without copying the list. `body` may read the children, and this node, but not change
    /// this node.
    pub fn for_each_child(&self, mut body: impl FnMut(&MNode)) {
        for child in &self.inner.borrow().children {
            body(child);
        }
    }

    /// Each child in order, without copying the list: [`Self::children`] for a loop whose body uses `?`, `continue` or
    /// `return`. The list is read at each step and no borrow is held between steps, so the body may read this node and
    /// its children; a loop that adds or removes this node's children takes [`Self::children`], a copy.
    pub fn child_iter(&self) -> impl Iterator<Item = MNode> + '_ {
        (0..).map_while(move |index| self.inner.borrow().children.get(index).cloned())
    }

    /// [`Self::child_iter`] of the children named `name`: [`Self::children_named`] without the copied list.
    pub fn named_iter<'a>(&'a self, name: &'a str) -> impl Iterator<Item = MNode> + 'a {
        self.child_iter().filter(move |child| child.has_name(name))
    }

    /// How many children the element has.
    pub fn child_count(&self) -> usize {
        self.inner.borrow().children.len()
    }

    pub fn children_named(&self, name: &str) -> Vec<MNode> {
        self.inner
            .borrow()
            .children
            .iter()
            .filter(|child| child.has_name(name))
            .cloned()
            .collect()
    }

    pub fn first_child(&self) -> Option<MNode> {
        self.inner.borrow().children.first().cloned()
    }

    pub fn first_named(&self, name: &str) -> Option<MNode> {
        self.inner
            .borrow()
            .children
            .iter()
            .find(|child| child.has_name(name))
            .cloned()
    }

    pub fn first_named_attrs(&self, name: &str, attrs: &[(&str, &str)]) -> Option<MNode> {
        self.inner
            .borrow()
            .children
            .iter()
            .find(|child| child.has_name(name) && attrs.iter().all(|(key, value)| child.attribute_is(key, value)))
            .cloned()
    }

    /// The first child `pred` takes. `pred` may read the children, and this node, but not change this node.
    pub fn first_where(&self, mut pred: impl FnMut(&MNode) -> bool) -> Option<MNode> {
        let count = self.child_count();
        (0..count).find_map(|index| {
            let child = self.inner.borrow().children.get(index).cloned()?;
            pred(&child).then_some(child)
        })
    }

    pub fn append(&self, name: impl Into<String>) -> MNode {
        let child = MNode::new(name);
        self.append_node(child.clone());
        child
    }

    pub fn append_node(&self, child: MNode) {
        {
            let mut held = child.inner.borrow_mut();
            held.parent = Some(Rc::downgrade(&self.inner));
            if held.location.is_none() {
                held.location.clone_from(&self.inner.borrow().location);
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
        let mut inner = self.inner.borrow_mut();
        let Some(slot) = inner.children.get_mut(index) else {
            return Err(Error::new(format!("child index {index} is past the end")));
        };
        child.inner.borrow_mut().parent = Some(Rc::downgrade(&self.inner));
        *slot = child;
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
        for child in self.child_iter() {
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
        Self::from_parts(name, attributes, text, copied_children, location, line, column)
    }

    /// Same structure, ignoring file location and shared identity.
    pub fn same_structure(&self, other: &MNode) -> bool {
        let same_node = {
            let (mine, theirs) = (self.inner.borrow(), other.inner.borrow());
            mine.name == theirs.name && mine.attributes == theirs.attributes && mine.text == theirs.text
        };
        if !same_node {
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
        // Read in place: a child is its own cell, so this node's borrow is held across the children's.
        let inner = self.inner.borrow();
        out.push('<');
        out.push_str(&inner.name);
        for (key, value) in &inner.attributes {
            out.push(' ');
            out.push_str(key);
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
        let children = &inner.children;
        if inner.text.is_some() || !children.is_empty() {
            out.push('>');
            if let Some(text) = &inner.text {
                out.push_str("<![CDATA[");
                let mut pieces = text.split("]]>");
                if let Some(first) = pieces.next() {
                    out.push_str(first);
                }
                for piece in pieces {
                    out.push_str("]]]]><![CDATA[>");
                    out.push_str(piece);
                }
                out.push_str("]]>");
            }
            if !children.is_empty() {
                for child in children {
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
            out.push_str(&inner.name);
            out.push('>');
        } else {
            out.push_str("/>");
        }
    }
}

fn find_attribute<'a>(attributes: &'a Attributes, name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| **key == *name)
        .map(|(_, value)| &**value)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests;
