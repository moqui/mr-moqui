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

//! The structure of a screen that has state of its own: trees, whose nodes load as they open; collapsible boxes;
//! components and their data; a wizard's Next; the editor of a list's row; the editor of a click-to-edit value. And
//! what a screen asks for once it has come.

use super::*;
use crate::components::Act;
use crate::layout::{Shown, TreeMark, TreeRow};

/// A tree's nodes as loaded, which are open, and the highlighted row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeState {
    pub roots: Vec<TreeNode>,
    pub open: BTreeSet<String>,
    pub loading: BTreeSet<String>,
    pub at: usize,
}

/// A node of a tree, as `m-tree-item` reads it: children as a list when they came with it, else whether it has any.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeNode {
    pub id: String,
    pub text: String,
    pub url: String,
    /// The `treeNodeName` its children are asked for with.
    pub name: String,
    pub children: Option<Vec<TreeNode>>,
    pub folder: bool,
}

impl TreeNode {
    fn of(value: &serde_json::Value) -> Option<Self> {
        let id = shell::json_text(value, "id");
        if id.is_empty() {
            return None;
        }
        let attr = |name: &str, key: &str| {
            value
                .get(name)
                .map(|map| shell::json_text(map, key))
                .unwrap_or_default()
        };
        let mut url = attr("a_attr", "urlText");
        if url.is_empty() {
            url = attr("a_attr", "href");
        }
        let (children, folder) = match value.get("children") {
            Some(serde_json::Value::Array(items)) => {
                let list: Vec<Self> = items.iter().filter_map(Self::of).collect();
                let folder = !list.is_empty();
                (Some(list), folder)
            }
            Some(serde_json::Value::Bool(flag)) => (None, *flag),
            _ => (Some(Vec::new()), false),
        };
        Some(Self {
            text: crate::model::clean_text(&shell::json_text(value, "text")),
            url,
            name: attr("li_attr", "treeNodeName"),
            children,
            folder,
            id,
        })
    }

    fn find_mut<'a>(nodes: &'a mut [Self], id: &str) -> Option<&'a mut Self> {
        for node in nodes {
            if node.id == id {
                return Some(node);
            }
            if let Some(found) = node.children.as_deref_mut().and_then(|kids| Self::find_mut(kids, id)) {
                return Some(found);
            }
        }
        None
    }
}

impl TreeState {
    /// The rows as drawn: each loaded node, and under an open one its children.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.add_rows(&self.roots, 0, &mut rows);
        rows
    }

    fn add_rows(&self, nodes: &[TreeNode], depth: usize, rows: &mut Vec<TreeRow>) {
        for node in nodes {
            let open = self.open.contains(&node.id);
            let mark = if self.loading.contains(&node.id) {
                TreeMark::Loading
            } else if !node.folder {
                TreeMark::Leaf
            } else if open {
                TreeMark::Open
            } else {
                TreeMark::Closed
            };
            rows.push(TreeRow {
                key: node.id.clone(),
                depth,
                text: node.text.clone(),
                url: node.url.clone(),
                mark,
            });
            if open {
                if let Some(children) = &node.children {
                    self.add_rows(children, depth + 1, rows);
                }
            }
        }
    }
}

/// The key of a tree request: the tree, then the node whose children are asked for (`#` for the roots).
fn tree_key(tree: &str, node: &str) -> String {
    format!("{tree}\u{1f}{node}")
}

impl Shown for Session {
    fn link_number(&self, id: &str) -> Option<usize> {
        Session::link_number(self, id)
    }

    fn collapsed(&self, id: &str, initial: &str) -> bool {
        self.collapsed.get(id).copied().unwrap_or(initial == "closed")
    }

    fn tree_rows(&self, id: &str) -> Vec<TreeRow> {
        self.trees.get(id).map(TreeState::rows).unwrap_or_default()
    }

    fn component_lines(
        &self,
        id: &str,
        name: &str,
        props: &serde_json::Value,
        width: u16,
        wide: bool,
    ) -> Option<Vec<crate::components::Line>> {
        let component = crate::components::find(name)?;
        let state = self.components.get(id).cloned().unwrap_or_default();
        Some(component.lines(props, &state, width, wide))
    }
}

impl Session {
    // ---- what the view reads ----

    pub fn tree(&self, id: &str) -> Option<&TreeState> {
        self.trees.get(id)
    }

    pub fn component_state(&self, id: &str) -> Option<&ComponentState> {
        self.components.get(id)
    }

    /// A display's text from its value url, when one came.
    pub fn shown_text(&self, field: &str) -> Option<&str> {
        self.shown_text.get(field).map(String::as_str)
    }

    pub fn is_collapsed(&self, id: &str) -> bool {
        let initial = self.index.boxes.get(id).map(String::as_str).unwrap_or("");
        Shown::collapsed(self, id, initial)
    }

    /// The fields of the row being edited, and the row's list and index.
    pub fn row_editing(&self) -> Option<(&str, usize)> {
        self.overlays.iter().rev().find_map(|overlay| match overlay {
            Overlay::Row(list, row) => Some((list.as_str(), *row)),
            _ => None,
        })
    }

    // ---- after a screen comes ----

    /// What a screen asks for once it is here: its menu data (documents, saved finds) when it is another screen, the
    /// counters on a navigation, the bodies of its dynamic containers, its trees' roots and its components' data.
    pub(super) fn after_screen(&mut self, navigate: bool, path_changed: bool) {
        self.load_parts(navigate, None);
        if path_changed {
            self.docs.clear();
            self.finds.clear();
            self.request_menu();
        }
        if navigate && !self.counts.is_empty() {
            self.read_counts();
        }
    }

    /// The parts that load on their own: dynamic containers, trees and components. For a screen, all of them; for the
    /// body of a dynamic dialog or container (`within`, its id), only the new ones inside it, so a body does not ask
    /// for itself again.
    pub(super) fn load_parts(&mut self, fresh: bool, within: Option<&str>) {
        let inside = |id: &str| within.is_none_or(|prefix| id.starts_with(&format!("{prefix}/")));
        let containers: Vec<(String, String)> = self
            .index
            .dynamic
            .iter()
            .filter(|(id, info)| !info.dialog && !info.url.is_empty() && inside(id))
            .map(|(id, info)| (id.clone(), info.url.clone()))
            .collect();
        for (id, url) in containers {
            self.request_fragment(&id, &url);
        }
        let trees: Vec<String> = self.index.trees.keys().filter(|id| inside(id)).cloned().collect();
        for id in trees {
            if !self.trees.contains_key(&id) {
                self.trees.insert(id.clone(), TreeState::default());
                self.request_tree(&id, "#", "");
            }
        }
        let components: Vec<String> = self.index.components.keys().filter(|id| inside(id)).cloned().collect();
        for id in components {
            if within.is_some() && self.components.contains_key(&id) {
                continue;
            }
            if fresh || !self.components.contains_key(&id) {
                self.components.insert(id.clone(), ComponentState::default());
            }
            self.request_component(&id);
        }
    }

    // ---- boxes ----

    pub(super) fn toggle_box(&mut self, id: &str) {
        let closed = self.is_collapsed(id);
        self.collapsed.insert(id.to_string(), !closed);
        self.rebuild_focus();
    }

    // ---- trees ----

    fn request_tree(&mut self, tree: &str, node: &str, name: &str) {
        let Some(info) = self.index.trees.get(tree).cloned() else {
            return;
        };
        let mut parameters = info.parameters.clone();
        parameters.insert("treeNodeId".to_string(), node.to_string());
        parameters.insert("treeOpenPath".to_string(), info.open_path.clone());
        if !name.is_empty() {
            parameters.insert("treeNodeName".to_string(), name.to_string());
        }
        if node != "#" {
            if let Some(state) = self.trees.get_mut(tree) {
                state.loading.insert(node.to_string());
            }
        }
        self.lanes.push(
            Purpose::Tree(tree_key(tree, node)),
            LinkCall::Json {
                url: info.url,
                method: "post".to_string(),
                parameters,
            },
        );
    }

    pub(super) fn tree_reply(&mut self, key: &str, result: Result<serde_json::Value, LinkError>) {
        let (tree, node) = key.split_once('\u{1f}').unwrap_or((key, "#"));
        let Some(state) = self.trees.get_mut(tree) else {
            return;
        };
        state.loading.remove(node);
        let nodes: Vec<TreeNode> = match result {
            Ok(serde_json::Value::Array(items)) => items.iter().filter_map(TreeNode::of).collect(),
            Ok(_) => {
                // An answer that is not a list leaves the node to ask again on the next open, as m-tree-item does.
                return;
            }
            Err(err) => {
                self.set_message(format!("The tree did not load: {err}"), true);
                return;
            }
        };
        if node == "#" {
            // The nodes on the open path come with their children: those start open.
            fn opened(nodes: &[TreeNode], open: &mut BTreeSet<String>) {
                for node in nodes {
                    if let Some(children) = node.children.as_ref().filter(|kids| !kids.is_empty()) {
                        open.insert(node.id.clone());
                        opened(children, open);
                    }
                }
            }
            opened(&nodes, &mut state.open);
            state.roots = nodes;
        } else if let Some(found) = TreeNode::find_mut(&mut state.roots, node) {
            found.folder = !nodes.is_empty();
            found.children = Some(nodes);
            state.open.insert(node.to_string());
        }
    }

    /// Up and Down move through a tree's rows; Right opens a node (asking for its children the first time), Left
    /// closes it or goes to its parent. Past the first or last row the keys move the focus on.
    pub(super) fn tree_key(&mut self, press: &KeyPress) -> bool {
        let Some(id) = self
            .focus()
            .filter(|item| item.kind == FocusKind::Tree)
            .map(|item| item.key.clone())
        else {
            return false;
        };
        if !press.is_plain() {
            return false;
        }
        let Some(state) = self.trees.get(&id) else {
            return false;
        };
        let rows = state.rows();
        let at = state.at.min(rows.len().saturating_sub(1));
        let Some(row) = rows.get(at).cloned() else {
            return false;
        };
        match press.key {
            Key::Up if at > 0 => self.set_tree_at(&id, at - 1),
            Key::Down if at + 1 < rows.len() => self.set_tree_at(&id, at + 1),
            Key::Right => match row.mark {
                TreeMark::Closed => self.open_tree_node(&id, &row.key),
                TreeMark::Open | TreeMark::Loading => {
                    if at + 1 < rows.len() {
                        self.set_tree_at(&id, at + 1);
                    }
                }
                TreeMark::Leaf => self.follow_url(&row.url),
            },
            Key::Left => {
                if row.mark == TreeMark::Open {
                    if let Some(state) = self.trees.get_mut(&id) {
                        state.open.remove(&row.key);
                    }
                } else if let Some(parent) = rows[..at].iter().rposition(|other| other.depth < row.depth) {
                    self.set_tree_at(&id, parent);
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        true
    }

    fn set_tree_at(&mut self, id: &str, at: usize) {
        if let Some(state) = self.trees.get_mut(id) {
            state.at = at;
        }
    }

    fn open_tree_node(&mut self, tree: &str, node: &str) {
        let Some(state) = self.trees.get_mut(tree) else {
            return;
        };
        let Some(found) = TreeNode::find_mut(&mut state.roots, node) else {
            return;
        };
        if found.children.is_some() {
            state.open.insert(node.to_string());
            return;
        }
        let name = found.name.clone();
        self.request_tree(tree, node, &name);
    }

    /// Enter on a tree: follow the node's link, or open or close a node without one.
    pub(super) fn tree_enter(&mut self, id: &str) {
        let Some(state) = self.trees.get(id) else {
            return;
        };
        let rows = state.rows();
        let Some(row) = rows.get(state.at.min(rows.len().saturating_sub(1))).cloned() else {
            return;
        };
        if !row.url.is_empty() {
            self.follow_url(&row.url);
            return;
        }
        match row.mark {
            TreeMark::Closed => self.open_tree_node(id, &row.key),
            TreeMark::Open => {
                if let Some(state) = self.trees.get_mut(id) {
                    state.open.remove(&row.key);
                }
            }
            TreeMark::Leaf | TreeMark::Loading => {}
        }
    }

    // ---- components ----

    /// The url of a component's transition: a name or a relative path goes under the screen's path (its `base`).
    fn component_url(base: &str, url: &str) -> String {
        if url.starts_with('/') || url.contains("://") || base.is_empty() {
            url.to_string()
        } else {
            format!("{}/{url}", base.trim_end_matches('/'))
        }
    }

    pub(super) fn request_component(&mut self, id: &str) {
        let Some(info) = self.index.components.get(id).cloned() else {
            return;
        };
        let Some(component) = crate::components::find(&info.name) else {
            return;
        };
        let state = self.components.entry(id.to_string()).or_default();
        let Some((url, parameters)) = component.source(&info.props, state) else {
            return;
        };
        state.loading = true;
        let base = if info.base.is_empty() {
            self.path().to_string()
        } else {
            info.base
        };
        self.lanes.push(
            Purpose::Component(id.to_string()),
            LinkCall::Json {
                url: Self::component_url(&base, &url),
                method: "get".to_string(),
                parameters,
            },
        );
    }

    pub(super) fn component_reply(&mut self, id: &str, result: Result<serde_json::Value, LinkError>) {
        let Some(state) = self.components.get_mut(id) else {
            return;
        };
        state.loading = false;
        match result {
            Ok(value) => {
                state.data = Some(value);
                state.error.clear();
            }
            Err(err) => state.error = err.to_string(),
        }
    }

    /// The focused component's keys come before the screen's: its own moves, then what it asks for.
    pub(super) fn component_key(&mut self, press: &KeyPress) -> bool {
        let Some(id) = self
            .focus()
            .filter(|item| item.kind == FocusKind::Component)
            .map(|item| item.key.clone())
        else {
            return false;
        };
        let Some(info) = self.index.components.get(&id).cloned() else {
            return false;
        };
        let Some(component) = crate::components::find(&info.name) else {
            return false;
        };
        let wide = self.columns >= 180;
        let state = self.components.entry(id.clone()).or_default();
        let Some(acts) = component.key(&info.props, state, press, wide) else {
            return false;
        };
        let base = if info.base.is_empty() {
            self.path().to_string()
        } else {
            info.base
        };
        for act in acts {
            match act {
                Act::Reload => self.request_component(&id),
                Act::Post { url, fields } => self.post_simple(&Self::component_url(&base, &url), fields),
                Act::Follow(url) => self.follow_url(&url),
                Act::Message(text) => self.set_message(text, false),
            }
        }
        true
    }

    // ---- a wizard ----

    /// Next: the step's changed form is saved first and the next step follows its answer; with nothing changed, the
    /// next step at once (`m-subscreens-wizard`).
    pub(super) fn wizard_next(&mut self) {
        let Some(wizard) = self.index.wizard.clone() else {
            return;
        };
        let Some(next) = wizard.steps.get(wizard.active + 1).map(|step| step.url.clone()) else {
            // The last step: its finish is its form's submit.
            if let Some(form) = self.first_dirty_form() {
                self.submit_form(&form, None);
            }
            return;
        };
        match self.first_dirty_form() {
            Some(form) => self.submit_form(&form, Some(Then::Navigate(next))),
            None => self.navigate(&next, true),
        }
    }

    // ---- the row editor ----

    /// `e` or Enter on a row with fields: the row's fields in a dialog, saved with Enter or Ctrl+S as the row's form, or
    /// as the list's multi form with every row.
    pub(super) fn edit_row(&mut self) {
        let Some(list) = self.target_list() else {
            return;
        };
        let row = self.row_at(&list);
        if !self.index.rows.contains_key(&(list.clone(), row)) {
            self.set_message("This row has nothing to edit.".to_string(), false);
            return;
        }
        self.overlays.push(Overlay::Row(list, row));
        self.rebuild_focus();
        self.focus_first_in_scope();
    }

    // ---- click to edit ----

    pub(super) fn open_editor(&mut self, field: &str) {
        let Some(Widget::Editable {
            text, value, options, ..
        }) = self.field_widget(field).cloned()
        else {
            return;
        };
        let current = value.unwrap_or(text);
        let at = options.iter().position(|choice| choice.value == current).unwrap_or(0);
        self.overlays.push(Overlay::Editor(Editor {
            field: field.to_string(),
            cursor: current.chars().count(),
            text: current,
            at,
        }));
    }

    /// Typing edits the value (Up and Down choose for a select); Enter posts it to the value's transition.
    pub(super) fn editor_key(&mut self, mut editor: Editor, press: &KeyPress) {
        self.overlays.pop();
        let Some(Widget::Editable {
            url,
            parameters,
            parameter_name,
            widget_type,
            options,
            ..
        }) = self.field_widget(&editor.field).cloned()
        else {
            return;
        };
        let multiline = widget_type == "textarea";
        match &press.key {
            Key::Escape | Key::F(12) => return,
            Key::Up | Key::Down if !options.is_empty() => {
                let last = options.len() - 1;
                editor.at = if press.key == Key::Up {
                    editor.at.saturating_sub(1)
                } else {
                    (editor.at + 1).min(last)
                };
                editor.text = options[editor.at].value.clone();
            }
            Key::Enter if !multiline || press.alt => {
                let mut fields = parameters;
                fields.insert(
                    if parameter_name.is_empty() {
                        "value".to_string()
                    } else {
                        parameter_name
                    },
                    editor.text.clone(),
                );
                self.post_simple(&url, fields);
                return;
            }
            _ if press.ctrl && press.key == Key::Char('s') => {
                let mut fields = parameters;
                fields.insert(
                    if parameter_name.is_empty() {
                        "value".to_string()
                    } else {
                        parameter_name
                    },
                    editor.text.clone(),
                );
                self.post_simple(&url, fields);
                return;
            }
            _ if options.is_empty() => {
                let limits = Limits {
                    multiline,
                    ..Limits::default()
                };
                edit::apply(&mut editor.text, &mut editor.cursor, press, &limits);
            }
            _ => {}
        }
        self.overlays.push(Overlay::Editor(editor));
    }
}
