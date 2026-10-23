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

//! The node data of a screen `tree`, a port of `ScreenTree.sendSubNodeJson`: the jstree JSON the html and qvt clients
//! read from `{screen}/actions/{tree}` with `treeNodeId` (`#` for the root), `treeNodeName` and `treeOpenPath`.
//!
//! Each `tree-sub-node` runs its condition and actions, then names a `tree-node` and a list (`nodeList` by default);
//! each entry is a node with the `id` and `text` of the node's `link` or `label`, the link's URL, and its children when
//! the node is on `treeOpenPath` (otherwise only whether it has any).

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::definition::{Actions, Condition, TreeDef};
use super::render::RenderState;
use super::sri::NodeLit;
use crate::context::Error;
use crate::script::{self, ContextFrame};

struct SubNode {
    node_name: String,
    list: String,
    condition: Option<Condition>,
    actions: Option<Actions>,
}

struct TreeNode {
    name: String,
    condition: Option<Condition>,
    actions: Option<Actions>,
    link: Option<MNode>,
    label: Option<MNode>,
    sub_nodes: Vec<SubNode>,
}

struct Tree {
    nodes: Vec<TreeNode>,
    sub_nodes: Vec<SubNode>,
}

fn condition_of(node: &MNode, label: &str) -> Result<Option<Condition>, Error> {
    match node.first_named("condition") {
        Some(condition) => Condition::from_element(&condition, label),
        None => Ok(None),
    }
}

fn actions_of(node: &MNode, label: &str) -> Result<Option<Actions>, Error> {
    match node.first_named("actions") {
        Some(actions) => Actions::from_node(&actions, label).map(Some),
        None => Ok(None),
    }
}

fn sub_nodes(node: &MNode, label: &str) -> Result<Vec<SubNode>, Error> {
    node.children_named("tree-sub-node")
        .iter()
        .map(|sub| {
            let node_name = sub.attribute("node-name").unwrap_or_default();
            let label = format!("{label}.subnode.{node_name}");
            Ok(SubNode {
                list: sub
                    .attribute("list")
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "nodeList".to_string()),
                condition: condition_of(sub, &label)?,
                actions: actions_of(sub, &label)?,
                node_name,
            })
        })
        .collect()
}

impl Tree {
    fn of(def: &TreeDef, location: &str) -> Result<Self, Error> {
        let node = def.node.node()?;
        let label = format!("{location}.tree.{}", def.name);
        let mut nodes = Vec::new();
        for tree_node in node.children_named("tree-node") {
            let name = tree_node.attribute("name").unwrap_or_default();
            let node_label = format!("{label}.node.{name}");
            nodes.push(TreeNode {
                condition: condition_of(&tree_node, &node_label)?,
                actions: actions_of(&tree_node, &node_label)?,
                link: tree_node.first_named("link"),
                label: tree_node.first_named("label"),
                sub_nodes: sub_nodes(&tree_node, &node_label)?,
                name,
            });
        }
        Ok(Self {
            sub_nodes: sub_nodes(&node, &label)?,
            nodes,
        })
    }

    fn node(&self, name: &str) -> Option<&TreeNode> {
        self.nodes.iter().find(|node| node.name == name)
    }
}

fn context_text(name: &str) -> String {
    script::context_value(name)
        .filter(|value| !value.is_null())
        .map(|value| moqui_lib::object::to_plain_string(&value))
        .unwrap_or_default()
}

/// The element as the URL builder takes it, with its children (its `parameter` elements).
fn node_lit(node: &MNode) -> NodeLit {
    NodeLit {
        name: node.name(),
        attrs: node.attributes().into_iter().collect(),
        text: node.text().unwrap_or_default(),
        children: node.children().iter().map(node_lit).collect(),
        parent: None,
    }
}

fn holds(condition: Option<&Condition>) -> Result<bool, Error> {
    match condition {
        Some(condition) => super::render::check_condition(condition),
        None => Ok(true),
    }
}

/// `ScreenTree.sendSubNodeJson`: the child nodes of the request's `treeNodeId`.
///
/// # Errors
///
/// Returns an error when a condition, an action, a list expression or a link fails.
pub(crate) fn sub_node_json(state: &RenderState, def: &TreeDef, location: &str) -> Result<Value, Error> {
    let tree = Tree::of(def, location)?;
    let node_id = context_text("treeNodeId");
    let subs: &[SubNode] = if node_id == "#" || node_id.is_empty() {
        &tree.sub_nodes
    } else {
        let name = context_text("treeNodeName");
        match tree.node(&name) {
            Some(node) => &node.sub_nodes,
            None => {
                tracing::warn!(
                    "No treeNodeName passed in request for child nodes for node [{node_id}] in tree [{}], using first node in tree definition.",
                    def.name
                );
                tree.nodes.first().map_or(&[][..], |node| &node.sub_nodes)
            }
        }
    };
    let open_path = context_text("treeOpenPath");
    Ok(Value::List(child_nodes(state, &tree, subs, &open_path, true)?))
}

fn child_nodes(
    state: &RenderState,
    tree: &Tree,
    subs: &[SubNode],
    open_path: &str,
    recurse: bool,
) -> Result<Vec<Value>, Error> {
    let mut out = Vec::new();
    for sub in subs {
        if !holds(sub.condition.as_ref())? {
            continue;
        }
        if let Some(actions) = &sub.actions {
            script::run_actions(&actions.xml)?;
        }
        let Some(tree_node) = tree.node(&sub.node_name) else {
            continue;
        };
        let list = script::eval_value(&sub.list)?;
        let items = script::value_items(&list).unwrap_or_default();
        let count = items.len();
        for (index, entry) in items.into_iter().enumerate() {
            let frame = ContextFrame::push();
            let built = (|| -> Result<Option<Value>, Error> {
                script::set_context_name("nodeList_entry", entry.clone())?;
                script::set_context_name("nodeList_index", Value::I64(i64::try_from(index).unwrap_or(i64::MAX)))?;
                script::set_context_name("nodeList_has_next", Value::Bool(index + 1 < count))?;
                if !holds(tree_node.condition.as_ref())? {
                    return Ok(None);
                }
                if let Some(actions) = &tree_node.actions {
                    script::run_actions(&actions.xml)?;
                }
                let Some(shown) = tree_node.link.as_ref().or(tree_node.label.as_ref()) else {
                    return Ok(None);
                };
                let id = script::expand(&shown.attribute("id").unwrap_or_default())?;
                let text = script::expand_localized(&shown.attribute("text").unwrap_or_default())?;
                let mut link = None;
                if let Some(link_node) = &tree_node.link {
                    let lit = node_lit(link_node);
                    let url_type = Some(link_node.attribute("url-type").unwrap_or_default())
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "transition".to_string());
                    // ScreenTree passes `expand-transition-url`, "true" when not set.
                    let expand = link_node
                        .attribute("expand-transition-url")
                        .filter(|text| !text.is_empty())
                        .is_none_or(|text| text == "true");
                    let url = super::urls::make_url_by_type(
                        state,
                        &link_node.attribute("url").unwrap_or_default(),
                        &url_type,
                        Some(&lit),
                        expand,
                    )?;
                    let url_text = if link_node.attribute("url-noparam").as_deref() == Some("true") {
                        url.path()
                    } else {
                        url.path_with_params()
                    };
                    let load_id = link_node.attribute("dynamic-load-id").unwrap_or_default();
                    let mut attrs = IndexMap::new();
                    attrs.insert(
                        "href".to_string(),
                        Value::string(if load_id.is_empty() {
                            url_text.clone()
                        } else {
                            load_href(&load_id, &url_text)
                        }),
                    );
                    attrs.insert("loadId".to_string(), Value::string(load_id));
                    attrs.insert("urlText".to_string(), Value::string(url_text));
                    link = Some(attrs);
                }
                let open = !open_path.is_empty() && !id.is_empty() && open_path.starts_with(&id);
                let children = if recurse {
                    let _frame = ContextFrame::push();
                    script::set_context_name("treeNodeId", Value::string(id.clone()))?;
                    Some(child_nodes(state, tree, &tree_node.sub_nodes, open_path, open)?)
                } else {
                    None
                };
                let mut node = IndexMap::new();
                node.insert("id".to_string(), Value::string(id.clone()));
                node.insert("text".to_string(), Value::string(text));
                let mut li = IndexMap::new();
                li.insert("treeNodeName".to_string(), Value::string(tree_node.name.clone()));
                node.insert("li_attr".to_string(), Value::Map(li));
                if let Some(link) = link {
                    node.insert("a_attr".to_string(), Value::Map(link));
                }
                let has_children = children.as_ref().is_some_and(|list| !list.is_empty());
                if open {
                    let mut state_map = IndexMap::new();
                    state_map.insert("opened".to_string(), Value::Bool(true));
                    state_map.insert("selected".to_string(), Value::Bool(open_path == id));
                    node.insert("state".to_string(), Value::Map(state_map));
                    node.insert("children".to_string(), Value::List(children.unwrap_or_default()));
                } else {
                    node.insert("children".to_string(), Value::Bool(has_children));
                }
                Ok(Some(Value::Map(node)))
            })();
            drop(frame);
            if let Some(node) = built? {
                out.push(node);
            }
        }
    }
    Ok(out)
}

/// The `javascript:` href that loads `url_text` into the element `load_id`, as Java's ScreenTree makes it. Both values are
/// JSON strings in the script, and a browser percent-decodes a `javascript:` URL before it runs it, so each `%` is encoded.
fn load_href(load_id: &str, url_text: &str) -> String {
    let quote = |text: &str| serde_json::Value::from(text).to_string().replace('%', "%25");
    format!(
        "javascript:{{$('#' + {}).load({}); void(0);}}",
        quote(load_id),
        quote(url_text)
    )
}

#[cfg(test)]
mod tests {
    use super::load_href;

    #[test]
    fn load_href_quotes_the_url_as_a_js_string() {
        assert_eq!(
            load_href("Tree", "/apps/x/Edit/a');alert(1);//?q=%22"),
            r#"javascript:{$('#' + "Tree").load("/apps/x/Edit/a');alert(1);//?q=%2522"); void(0);}"#
        );
        assert_eq!(
            load_href("T", "/a?x=\"b\""),
            r#"javascript:{$('#' + "T").load("/a?x=\"b\""); void(0);}"#
        );
    }
}
