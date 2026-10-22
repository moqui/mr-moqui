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

//! The fields, forms, lists, links and dialogs of the levels on screen, found once per model change, with what a form
//! posts and the checks it makes before a submit (plans/archive/Screen-TUI-Impl.md 1.7, 2.3).
//!
//! A field's value is in the edit buffers under its id. A find field has parts, under `id#part`: a date or range find
//! `#from` and `#thru`, a text find `#op`, `#not` and `#ic`, a drop-down with `show-not` `#not`.

use std::collections::BTreeMap;

use crate::model::{walk, Choice, FormList, Node, Rule, ScreenModel, VisibleWhen, Widget};

/// Which overlay a focusable item belongs to. A dialog, the find dialog of a list, or the editor of a list's row keeps
/// focus inside it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    #[default]
    Main,
    Dialog(String),
    Find(String),
    /// The fields of row `n` of a list, in the row editor.
    Row(String, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusKind {
    /// A text input: text line, text area, password, date, a find part.
    Text,
    DropDown,
    Check,
    Radio,
    Button,
    Link,
    List,
    /// The button that opens a dialog: a container-dialog, a dynamic-dialog, a row action.
    Dialog,
    /// A button-menu: Enter opens its items.
    Menu,
    Tree,
    /// The title of a collapsible box: Enter opens or closes it.
    Toggle,
    /// A click-to-edit value: Enter opens its editor.
    Edit,
    Component,
}

/// One item of the focus order, which is the reading order of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusItem {
    /// The node id, or `field id#part` for a part of a find field.
    pub key: String,
    /// The node the item is drawn in: the field of a part.
    pub node: String,
    pub kind: FocusKind,
    pub scope: Scope,
    /// The collapsible boxes around it: while one is closed the item is not in the focus order.
    pub boxes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldInfo {
    pub id: String,
    pub name: String,
    pub title: String,
    /// The form the field posts with, or a list whose header (find) field it is.
    pub form: Option<String>,
    pub list: Option<String>,
    pub required: bool,
    pub maxlength: u32,
    pub rules: Vec<Rule>,
    pub tooltip: String,
    pub visible_when: Option<VisibleWhen>,
    pub widget: Widget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormInfo {
    pub id: String,
    pub name: String,
    pub transition_url: String,
    pub method: String,
    pub hidden: Vec<(String, String)>,
    pub submit_message: String,
    pub background_reload_id: String,
    pub background_hide_id: String,
    /// Field ids in document order.
    pub fields: Vec<String>,
    /// The form of a row action: it posts the selected rows of this list too.
    pub selection: Option<String>,
    pub multipart: bool,
    /// Its id in a qvt render of the same screen ([`Node::Form`]'s `dom_id`; a list row's `<list>_<row>`).
    pub dom_id: String,
    /// Its transition is not read-only.
    pub writes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkInfo {
    pub id: String,
    pub text: String,
    pub url: String,
    pub kind: String,
    pub method: String,
    pub confirm: String,
    pub disabled: bool,
    /// It runs a transition that is not read-only.
    pub writes: bool,
}

impl LinkInfo {
    /// A link that runs a transition rather than opening a screen: a hidden-form link posts, as `m-form-link` does.
    pub fn posts(&self) -> bool {
        self.kind == "hidden-form" || self.kind == "hidden-form-link" || self.method.eq_ignore_ascii_case("post")
    }
}

/// A subscreen tab, for Alt+1 to Alt+9.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    pub title: String,
    pub url: String,
    pub active: bool,
}

/// A dynamic dialog or container: what its body loads from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicInfo {
    pub url: String,
    /// A dialog opens on Enter; a container loads with the screen.
    pub dialog: bool,
}

/// A button-menu: its text and its items' link ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuInfo {
    pub text: String,
    pub links: Vec<String>,
}

/// A tree: where its nodes load from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeInfo {
    pub url: String,
    pub open_path: String,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentInfo {
    pub name: String,
    pub props: serde_json::Value,
    /// The screen path its transitions are under.
    pub base: String,
}

/// A wizard's steps: Back and Next are its links `#back` and `#next`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WizardInfo {
    pub id: String,
    pub steps: Vec<TabInfo>,
    pub active: usize,
}

/// The fields of a list row, and the form they post with: the row's own, or the multi form of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowInfo {
    pub form: String,
    pub fields: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub fields: BTreeMap<String, FieldInfo>,
    pub forms: BTreeMap<String, FormInfo>,
    pub lists: BTreeMap<String, FormList>,
    pub links: BTreeMap<String, LinkInfo>,
    /// Link ids in reading order, row links included: link `[n]` of `#` numbering is `link_order[n - 1]`.
    pub link_order: Vec<String>,
    /// Dialog id to title.
    pub dialogs: BTreeMap<String, String>,
    /// A row action's dialog id to its list.
    pub dialog_lists: BTreeMap<String, String>,
    pub dynamic: BTreeMap<String, DynamicInfo>,
    pub menus: BTreeMap<String, MenuInfo>,
    pub trees: BTreeMap<String, TreeInfo>,
    pub components: BTreeMap<String, ComponentInfo>,
    /// A collapsible box's id to its `initial`.
    pub boxes: BTreeMap<String, String>,
    /// The fields of each list row that has a form, by list id and row.
    pub rows: BTreeMap<(String, usize), RowInfo>,
    /// The dialogs of each list row, by list id and row.
    pub row_dialogs: BTreeMap<(String, usize), Vec<String>>,
    pub tabs: Vec<TabInfo>,
    pub wizard: Option<WizardInfo>,
    /// Every focusable item in reading order, in every scope.
    pub order: Vec<FocusItem>,
}

/// Where a node is: the scope it is focused in, the form it posts with, the list whose find it is, the list whose rows
/// a row action takes, and the collapsible boxes around it.
#[derive(Clone, Debug, Default)]
struct At {
    scope: Scope,
    form: Option<String>,
    list: Option<String>,
    selection: Option<String>,
    boxes: Vec<String>,
}

/// A dialog the session adds that is not in a screen: a quick create, the tray, a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Extra {
    pub id: String,
    pub title: String,
    pub c: Vec<Node>,
}

impl Index {
    /// Index the levels from the outermost to the leaf.
    pub fn of<'a>(levels: impl IntoIterator<Item = &'a ScreenModel>) -> Self {
        Self::with_extras(levels, &[])
    }

    /// The levels, and the dialogs the session holds outside them.
    pub fn with_extras<'a>(levels: impl IntoIterator<Item = &'a ScreenModel>, extras: &[Extra]) -> Self {
        let mut index = Self::default();
        for model in levels {
            index.add(&model.c, &At::default());
        }
        for extra in extras {
            index.dialogs.insert(extra.id.clone(), extra.title.clone());
            let at = At {
                scope: Scope::Dialog(extra.id.clone()),
                ..At::default()
            };
            index.add(&extra.c, &at);
        }
        index
    }

    fn add(&mut self, nodes: &[Node], at: &At) {
        for node in nodes {
            match node {
                Node::Form {
                    id,
                    name,
                    multipart,
                    transition_url,
                    method,
                    hidden,
                    submit_message,
                    background_reload_id,
                    background_hide_id,
                    dom_id,
                    writes,
                    c,
                    ..
                } => {
                    let mut fields = Vec::new();
                    walk(c, &mut |inner| {
                        if let Node::Field { id, .. } = inner {
                            fields.push(id.clone());
                        }
                    });
                    self.forms.insert(
                        id.clone(),
                        FormInfo {
                            id: id.clone(),
                            name: name.clone(),
                            transition_url: transition_url.clone(),
                            method: if method.is_empty() {
                                "post".to_string()
                            } else {
                                method.clone()
                            },
                            hidden: hidden
                                .iter()
                                .map(|item| (item.name.clone(), item.value.clone()))
                                .collect(),
                            submit_message: submit_message.clone(),
                            background_reload_id: background_reload_id.clone(),
                            background_hide_id: background_hide_id.clone(),
                            fields,
                            selection: at.selection.clone(),
                            multipart: *multipart,
                            dom_id: dom_id.clone(),
                            writes: *writes,
                        },
                    );
                    let inner = At {
                        form: Some(id.clone()),
                        list: None,
                        ..at.clone()
                    };
                    self.add(c, &inner);
                }
                Node::Field { .. } => self.add_field(node, at),
                Node::Link {
                    id,
                    text,
                    url,
                    kind,
                    method,
                    confirm,
                    disabled,
                    writes,
                    ..
                } => {
                    self.add_link(id, text, url, kind, method, confirm, *disabled, *writes, true);
                    if !disabled {
                        self.push(id, FocusKind::Link, at);
                    }
                }
                Node::FormList { list } => self.add_list(list, at),
                Node::Dialog { id, title, list, c, .. } => {
                    self.dialogs.insert(id.clone(), title.clone());
                    self.push(id, FocusKind::Dialog, at);
                    let mut inner = At {
                        scope: Scope::Dialog(id.clone()),
                        ..at.clone()
                    };
                    if !list.is_empty() {
                        self.dialog_lists.insert(id.clone(), list.clone());
                        inner.selection = Some(list.clone());
                    }
                    self.add(c, &inner);
                }
                Node::DynamicDialog {
                    id,
                    url,
                    button,
                    title,
                    c,
                } => {
                    let shown = if title.is_empty() { button } else { title };
                    self.dialogs.insert(id.clone(), shown.clone());
                    self.dynamic.insert(
                        id.clone(),
                        DynamicInfo {
                            url: url.clone(),
                            dialog: true,
                        },
                    );
                    self.push(id, FocusKind::Dialog, at);
                    let inner = At {
                        scope: Scope::Dialog(id.clone()),
                        ..at.clone()
                    };
                    self.add(c, &inner);
                }
                Node::DynamicContainer { id, url, c } => {
                    self.dynamic.insert(
                        id.clone(),
                        DynamicInfo {
                            url: url.clone(),
                            dialog: false,
                        },
                    );
                    self.add(c, at);
                }
                Node::ButtonMenu { id, text, c } => {
                    let mut links = Vec::new();
                    walk(c, &mut |inner| {
                        if let Node::Link {
                            id,
                            text,
                            url,
                            kind,
                            method,
                            confirm,
                            disabled,
                            writes,
                            ..
                        } = inner
                        {
                            self.add_link(id, text, url, kind, method, confirm, *disabled, *writes, false);
                            links.push(id.clone());
                        }
                    });
                    self.menus.insert(
                        id.clone(),
                        MenuInfo {
                            text: text.clone(),
                            links,
                        },
                    );
                    self.push(id, FocusKind::Menu, at);
                }
                Node::Tree {
                    id,
                    url,
                    open_path,
                    parameters,
                    ..
                } => {
                    self.trees.insert(
                        id.clone(),
                        TreeInfo {
                            url: url.clone(),
                            open_path: open_path.clone(),
                            parameters: parameters.clone(),
                        },
                    );
                    self.push(id, FocusKind::Tree, at);
                }
                Node::Box { id, initial, c, .. } if !initial.is_empty() => {
                    self.boxes.insert(id.clone(), initial.clone());
                    self.push(id, FocusKind::Toggle, at);
                    let mut inner = at.clone();
                    inner.boxes.push(id.clone());
                    self.add(c, &inner);
                }
                Node::Subscreen { id, kind, items, c, .. } => {
                    // The deepest tab bar wins: Alt+N picks among the tabs nearest the content.
                    let tabs: Vec<TabInfo> = items
                        .iter()
                        .map(|item| TabInfo {
                            title: item.title.clone(),
                            url: item.url.clone(),
                            active: item.active,
                        })
                        .collect();
                    if kind == "wizard" {
                        self.add(c, at);
                        let (_, active) = crate::layout::wizard_steps(items);
                        for (link, step, text) in [
                            (crate::layout::wizard_back(id), active.checked_sub(1), "Back"),
                            (crate::layout::wizard_next(id), Some(active + 1), "Next"),
                        ] {
                            if let Some(step) = step.and_then(|step| tabs.get(step)) {
                                self.add_link(&link, text, &step.url, "", "", "", false, false, true);
                                self.push(&link, FocusKind::Link, at);
                            }
                        }
                        self.wizard = Some(WizardInfo {
                            id: id.clone(),
                            steps: tabs,
                            active,
                        });
                        continue;
                    }
                    if tabs.len() > 1 && kind != "stack" {
                        self.tabs = tabs;
                    }
                    self.add(c, at);
                }
                Node::NumberCard { id, value, url, .. } if !url.is_empty() => {
                    self.add_link(id, value, url, "", "", "", false, false, true);
                    self.push(id, FocusKind::Link, at);
                }
                Node::Image { id, alt, url } if !url.is_empty() => {
                    self.add_link(id, alt, url, "image", "", "", false, false, true);
                    self.push(id, FocusKind::Link, at);
                }
                Node::Html { id, links, .. } => self.add_references(id, links, at),
                Node::Markdown { id, text } => {
                    let (_, links) = crate::markdown::blocks(text);
                    self.add_references(id, &links, at);
                }
                Node::Component { id, name, props, base } => {
                    self.components.insert(
                        id.clone(),
                        ComponentInfo {
                            name: name.clone(),
                            props: props.clone(),
                            base: base.clone(),
                        },
                    );
                    self.push(id, FocusKind::Component, at);
                }
                other => self.add(other.children(), at),
            }
        }
    }

    fn add_field(&mut self, node: &Node, at: &At) {
        let Node::Field {
            id,
            name,
            title,
            required,
            maxlength,
            rules,
            tooltip,
            visible_when,
            w,
        } = node
        else {
            return;
        };
        self.fields.insert(
            id.clone(),
            FieldInfo {
                id: id.clone(),
                name: name.clone(),
                title: title.clone(),
                form: at.form.clone(),
                list: at.list.clone(),
                required: *required,
                maxlength: *maxlength,
                rules: rules.clone(),
                tooltip: tooltip.clone(),
                visible_when: visible_when.clone(),
                widget: w.clone(),
            },
        );
        for (key, kind) in focus_parts(id, w) {
            self.order.push(FocusItem {
                key,
                node: id.clone(),
                kind,
                scope: at.scope.clone(),
                boxes: at.boxes.clone(),
            });
        }
    }

    /// A list: its rows' links, and when it posts, the fields of each row with the form they go in (the row's own, or
    /// the list's multi form), each row's fields focused in its row editor.
    fn add_list(&mut self, list: &FormList, at: &At) {
        self.lists.insert(list.id.clone(), list.clone());
        self.push(&list.id, FocusKind::List, at);
        let posts = !list.transition_url.is_empty();
        let mut multi_fields = Vec::new();
        let mut multi_hidden: Vec<(String, String)> = list
            .hidden
            .iter()
            .map(|item| (item.name.clone(), item.value.clone()))
            .collect();
        for (row_at, row) in list.rows.iter().enumerate() {
            let form = if list.multi {
                list.id.clone()
            } else {
                format!("{}#{row_at}", list.id)
            };
            let row_scope = At {
                scope: Scope::Row(list.id.clone(), row_at),
                form: Some(form.clone()),
                list: None,
                ..at.clone()
            };
            let mut fields = Vec::new();
            for cell in &row.cells {
                for inner in cell.nodes() {
                    match inner {
                        Node::Link {
                            id,
                            text,
                            url,
                            kind,
                            method,
                            confirm,
                            disabled,
                            writes,
                            ..
                        } => self.add_link(id, text, url, kind, method, confirm, *disabled, *writes, true),
                        Node::Field { id, .. } if posts => {
                            self.add_field(inner, &row_scope);
                            fields.push(id.clone());
                        }
                        // A dialog of the row (a View per row): not in the focus order, F10 offers the current row's.
                        Node::DynamicDialog {
                            id,
                            url,
                            button,
                            title,
                            c,
                        } => {
                            let shown = if title.is_empty() { button } else { title };
                            self.dialogs.insert(id.clone(), shown.clone());
                            self.dynamic.insert(
                                id.clone(),
                                DynamicInfo {
                                    url: url.clone(),
                                    dialog: true,
                                },
                            );
                            self.row_dialogs
                                .entry((list.id.clone(), row_at))
                                .or_default()
                                .push(id.clone());
                            let inner_at = At {
                                scope: Scope::Dialog(id.clone()),
                                form: None,
                                list: None,
                                ..at.clone()
                            };
                            self.add(c, &inner_at);
                        }
                        // A container-dialog of the row (an Edit per row): its body came with the row; F10 offers it.
                        Node::Dialog {
                            id, button, title, c, ..
                        } => {
                            let shown = if title.is_empty() { button } else { title };
                            self.dialogs.insert(id.clone(), shown.clone());
                            self.row_dialogs
                                .entry((list.id.clone(), row_at))
                                .or_default()
                                .push(id.clone());
                            let inner_at = At {
                                scope: Scope::Dialog(id.clone()),
                                form: None,
                                list: None,
                                ..at.clone()
                            };
                            self.add(c, &inner_at);
                        }
                        _ => {}
                    }
                }
            }
            if fields.is_empty() {
                continue;
            }
            let row_hidden = row.hidden.iter().map(|item| (item.name.clone(), item.value.clone()));
            if list.multi {
                multi_fields.extend(fields.iter().cloned());
                multi_hidden.extend(row_hidden);
            } else {
                let mut hidden = multi_hidden.clone();
                hidden.extend(row_hidden);
                let mut row_form = list_form(list, &form, fields.clone(), hidden);
                if !list.dom_id.is_empty() {
                    row_form.dom_id = format!("{}_{row_at}", list.dom_id);
                }
                self.forms.insert(form.clone(), row_form);
            }
            self.rows.insert((list.id.clone(), row_at), RowInfo { form, fields });
        }
        if list.multi && posts {
            let buttons = At {
                form: Some(list.id.clone()),
                list: None,
                ..at.clone()
            };
            for node in &list.buttons {
                if let Node::Field { id, .. } = node {
                    self.add_field(node, &buttons);
                    multi_fields.push(id.clone());
                }
            }
            self.forms
                .insert(list.id.clone(), list_form(list, &list.id, multi_fields, multi_hidden));
        }
        let find = At {
            scope: Scope::Find(list.id.clone()),
            form: None,
            list: Some(list.id.clone()),
            selection: None,
            boxes: at.boxes.clone(),
        };
        self.add(&list.header_fields, &find);
        self.add(&list.c, at);
    }

    /// The numbered links of an `html` or `markdown` node, each a link to follow.
    fn add_references(&mut self, id: &str, links: &[crate::model::HtmlLink], at: &At) {
        for (n, link) in links.iter().enumerate() {
            let key = crate::layout::ref_id(id, n + 1);
            self.add_link(&key, &link.text, &link.url, "", "", "", false, false, true);
            self.push(&key, FocusKind::Link, at);
        }
    }

    /// A link the screen has. One shown on the screen is numbered for `#`; a menu's item is not, as it is not shown.
    #[expect(clippy::too_many_arguments)]
    fn add_link(
        &mut self,
        id: &str,
        text: &str,
        url: &str,
        kind: &str,
        method: &str,
        confirm: &str,
        disabled: bool,
        writes: bool,
        numbered: bool,
    ) {
        if numbered && !disabled && !url.is_empty() && !self.links.contains_key(id) {
            self.link_order.push(id.to_string());
        }
        self.links.insert(
            id.to_string(),
            LinkInfo {
                id: id.to_string(),
                text: text.to_string(),
                url: url.to_string(),
                kind: kind.to_string(),
                method: method.to_string(),
                confirm: confirm.to_string(),
                disabled,
                writes,
            },
        );
    }

    fn push(&mut self, id: &str, kind: FocusKind, at: &At) {
        self.order.push(FocusItem {
            key: id.to_string(),
            node: id.to_string(),
            kind,
            scope: at.scope.clone(),
            boxes: at.boxes.clone(),
        });
    }

    /// The form a field or button posts with.
    pub fn form_of(&self, field_id: &str) -> Option<&FormInfo> {
        self.fields
            .get(field_id)
            .and_then(|field| field.form.as_ref())
            .and_then(|form| self.forms.get(form))
            .or_else(|| self.forms.get(field_id))
    }

    /// The values every field starts with, by buffer key.
    pub fn initial_values(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for field in self.fields.values() {
            for (key, value) in initial_parts(&field.id, &field.widget) {
                out.insert(key, value);
            }
        }
        out
    }

    /// Whether a field shows: its `visible-when` field (in the same form) has the value it names.
    pub fn visible(&self, field_id: &str, values: &dyn Fn(&str) -> String) -> bool {
        let Some(field) = self.fields.get(field_id) else {
            return true;
        };
        let Some(when) = &field.visible_when else {
            return true;
        };
        let other = self
            .fields
            .values()
            .find(|other| other.name == when.field && other.form == field.form);
        let value = other.map(|other| values(&other.id)).unwrap_or_default();
        if when.value.is_empty() {
            !value.is_empty()
        } else {
            value.split(',').any(|part| part.trim() == when.value)
        }
    }

    /// The field ids whose `visible-when` names `name`.
    pub fn watched_by(&self, name: &str) -> bool {
        self.fields
            .values()
            .any(|field| field.visible_when.as_ref().is_some_and(|when| when.field == name))
    }

    /// The drop-downs that depend on the field `name`.
    pub fn dependents(&self, name: &str) -> Vec<String> {
        self.fields
            .values()
            .filter(|field| match &field.widget {
                Widget::DropDown {
                    options_url,
                    depends_on,
                    ..
                } => !options_url.is_empty() && depends_on.values().any(|dep| names_field(dep, name, &field.name)),
                _ => false,
            })
            .map(|field| field.id.clone())
            .collect()
    }

    /// The url and parameters of a drop-down's options: its fixed parameters, then each depends-on field's value.
    pub fn option_query(
        &self,
        field_id: &str,
        values: &dyn Fn(&str) -> String,
    ) -> Option<(String, BTreeMap<String, String>)> {
        let field = self.fields.get(field_id)?;
        let Widget::DropDown {
            options_url,
            options_parameters,
            depends_on,
            ..
        } = &field.widget
        else {
            return None;
        };
        if options_url.is_empty() {
            return None;
        }
        let mut params = options_parameters.clone();
        for (parameter, name) in depends_on {
            if let Some(other) = self.sibling(field, name) {
                params.insert(parameter.clone(), values(&other.id));
            }
        }
        Some((options_url.clone(), params))
    }

    /// The field named `name` in the form of `of`. In a multi form a row's fields are `name_N`, and what a row's field
    /// depends on is named without the row, so the row's own field of that name is found.
    pub fn sibling(&self, of: &FieldInfo, name: &str) -> Option<&FieldInfo> {
        let same = |other: &&FieldInfo, wanted: &str| other.form == of.form && other.name == wanted;
        self.fields.values().find(|other| same(other, name)).or_else(|| {
            let suffix = row_suffix(&of.name);
            if suffix.is_empty() {
                return None;
            }
            let wanted = format!("{name}{suffix}");
            self.fields.values().find(|other| same(other, &wanted))
        })
    }

    pub fn choices(&self, field_id: &str) -> &[Choice] {
        match self.fields.get(field_id).map(|field| &field.widget) {
            Some(Widget::DropDown { options, .. } | Widget::Radio { options, .. } | Widget::Check { options, .. }) => {
                options
            }
            _ => &[],
        }
    }

    pub fn set_choices(&mut self, field_id: &str, choices: Vec<Choice>) {
        if let Some(FieldInfo {
            widget: Widget::DropDown { options, .. },
            ..
        }) = self.fields.get_mut(field_id)
        {
            *options = choices;
        }
    }
}

/// The `_N` of a multi form's row field (`orgId_3`), else empty.
pub fn row_suffix(name: &str) -> &str {
    match name.rfind('_') {
        Some(at) if at + 1 < name.len() && name[at + 1..].chars().all(|ch| ch.is_ascii_digit()) => &name[at..],
        _ => "",
    }
}

/// Whether a depends-on entry `dep` of the field `target` names the field `changed`: by its name, or in a multi form
/// row by its name without the row's `_N`.
pub fn names_field(dep: &str, changed: &str, target: &str) -> bool {
    if dep == changed {
        return true;
    }
    let suffix = row_suffix(changed);
    !suffix.is_empty() && row_suffix(target) == suffix && changed.strip_suffix(suffix) == Some(dep)
}

/// The form of a list's row, or of all its rows (multi): it posts to the list's transition.
fn list_form(list: &FormList, id: &str, fields: Vec<String>, hidden: Vec<(String, String)>) -> FormInfo {
    FormInfo {
        id: id.to_string(),
        name: list.name.clone(),
        transition_url: list.transition_url.clone(),
        method: "post".to_string(),
        hidden,
        submit_message: String::new(),
        background_reload_id: String::new(),
        background_hide_id: String::new(),
        fields,
        selection: None,
        multipart: false,
        dom_id: list.dom_id.clone(),
        writes: list.writes,
    }
}

/// The focusable parts of a field: one item, two for a from/thru find, none for a display or hidden field.
fn focus_parts(id: &str, widget: &Widget) -> Vec<(String, FocusKind)> {
    let one = |kind| vec![(id.to_string(), kind)];
    match widget {
        Widget::TextLine { .. } | Widget::TextFind { .. } | Widget::DateTime { .. } | Widget::Password {} => {
            one(FocusKind::Text)
        }
        Widget::TextArea { read_only, .. } => {
            if *read_only {
                Vec::new()
            } else {
                one(FocusKind::Text)
            }
        }
        Widget::DateFind { .. } | Widget::RangeFind { .. } | Widget::DatePeriod { .. } => vec![
            (format!("{id}#from"), FocusKind::Text),
            (format!("{id}#thru"), FocusKind::Text),
        ],
        Widget::DropDown { .. } => one(FocusKind::DropDown),
        Widget::Check { .. } => one(FocusKind::Check),
        Widget::Radio { .. } => one(FocusKind::Radio),
        Widget::Submit { .. } | Widget::Reset { .. } => one(FocusKind::Button),
        Widget::Editable { .. } => one(FocusKind::Edit),
        Widget::Display { .. }
        | Widget::Hidden { .. }
        | Widget::File { .. }
        | Widget::Label { .. }
        | Widget::Image { .. } => Vec::new(),
    }
}

/// The buffer keys and starting values of a field.
fn initial_parts(id: &str, widget: &Widget) -> Vec<(String, String)> {
    let main = |value: &str| vec![(id.to_string(), value.to_string())];
    let part = |name: &str, value: &str| (format!("{id}#{name}"), value.to_string());
    let yes = |flag: bool| if flag { "Y" } else { "N" };
    match widget {
        Widget::TextLine { value, .. }
        | Widget::TextArea { value, .. }
        | Widget::DateTime { value, .. }
        | Widget::Check { value, .. }
        | Widget::Radio { value, .. }
        | Widget::Hidden { value } => main(value),
        Widget::Password {} => main(""),
        Widget::TextFind { value, op, not, ic, .. } => vec![
            (id.to_string(), value.clone()),
            part("op", if op.is_empty() { "contains" } else { op }),
            part("not", yes(*not)),
            part("ic", yes(*ic)),
        ],
        Widget::DropDown {
            value, show_not, not, ..
        } => {
            let mut out = main(value);
            if *show_not {
                out.push(part("not", yes(*not)));
            }
            out
        }
        Widget::DateFind { from, thru, .. }
        | Widget::RangeFind { from, thru }
        | Widget::DatePeriod { from, thru, .. } => vec![part("from", from), part("thru", thru)],
        Widget::Display { value, .. } => value.as_deref().map(main).unwrap_or_default(),
        Widget::Submit { .. }
        | Widget::Reset { .. }
        | Widget::File { .. }
        | Widget::Label { .. }
        | Widget::Image { .. }
        | Widget::Editable { .. } => Vec::new(),
    }
}

/// The request parameters of one field, from its buffers.
pub fn field_parameters(field: &FieldInfo, values: &dyn Fn(&str) -> String) -> Vec<(String, String)> {
    let id = &field.id;
    let name = &field.name;
    let part = |suffix: &str| values(&format!("{id}#{suffix}"));
    match &field.widget {
        Widget::TextLine { .. }
        | Widget::DateTime { .. }
        | Widget::Check { .. }
        | Widget::Radio { .. }
        | Widget::Hidden { .. }
        | Widget::Password {} => vec![(name.clone(), values(id))],
        Widget::TextArea { read_only, .. } => {
            if *read_only {
                Vec::new()
            } else {
                vec![(name.clone(), values(id))]
            }
        }
        Widget::TextFind { .. } => vec![
            (name.clone(), values(id)),
            (format!("{name}_op"), part("op")),
            (format!("{name}_not"), part("not")),
            (format!("{name}_ic"), part("ic")),
        ],
        Widget::DropDown { show_not, multiple, .. } => {
            let mut out = vec![(name.clone(), values(id))];
            if *multiple && values(id).contains(',') {
                out.push((format!("{name}_op"), "in".to_string()));
            }
            if *show_not {
                out.push((format!("{name}_not"), part("not")));
            }
            out
        }
        Widget::DateFind { .. } | Widget::RangeFind { .. } | Widget::DatePeriod { .. } => vec![
            (format!("{name}_from"), part("from")),
            (format!("{name}_thru"), part("thru")),
        ],
        Widget::Display { value, .. } => {
            if value.is_some() {
                vec![(name.clone(), values(id))]
            } else {
                Vec::new()
            }
        }
        Widget::Submit { .. }
        | Widget::Reset { .. }
        | Widget::File { .. }
        | Widget::Label { .. }
        | Widget::Image { .. }
        | Widget::Editable { .. } => Vec::new(),
    }
}

/// The request parameter names of a field with the buffer key each one is posted from: `name` from the field's own
/// key, `name_from` from `id#from`. What the browser's form names a value maps to a key by this.
pub fn parameter_keys(field: &FieldInfo) -> Vec<(String, String)> {
    let keys = field_keys(field);
    field_parameters(field, &|key| key.to_string())
        .into_iter()
        .filter(|(_, key)| keys.contains(key))
        .collect()
}

/// The buffer keys of a field, so a save drops exactly its edits.
pub fn field_keys(field: &FieldInfo) -> Vec<String> {
    initial_parts(&field.id, &field.widget)
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// What a form posts: its hidden values, then each shown field. An empty optional value the user did not touch is left
/// out, so an untouched empty field does not write "" into a foreign key.
pub fn posted(
    index: &Index,
    field_ids: &[String],
    hidden: &[(String, String)],
    values: &dyn Fn(&str) -> String,
    dirty: &dyn Fn(&str) -> bool,
) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = hidden.iter().cloned().collect();
    for id in field_ids {
        let Some(field) = index.fields.get(id) else {
            continue;
        };
        if !index.visible(id, values) {
            continue;
        }
        let keys = field_keys(field);
        let touched = keys.iter().any(|key| dirty(key));
        for (name, value) in field_parameters(field, values) {
            if value.is_empty() && !field.required && !touched {
                continue;
            }
            out.insert(name, value);
        }
    }
    out
}

/// The checks of section 1.7 made before a submit, as `(buffer key, message)`: required, length, the field's rules, and
/// a date that does not read in its format.
pub fn check(index: &Index, field_ids: &[String], values: &dyn Fn(&str) -> String) -> Vec<(String, String)> {
    let mut marks = Vec::new();
    for id in field_ids {
        let Some(field) = index.fields.get(id) else {
            continue;
        };
        if !index.visible(id, values) || matches!(field.widget, Widget::Display { .. } | Widget::Hidden { .. }) {
            continue;
        }
        let value = values(id);
        if field.required && value.trim().is_empty() && !matches!(field.widget, Widget::Submit { .. }) {
            marks.push((id.clone(), "A value is required".to_string()));
            continue;
        }
        let limit = usize::try_from(field.maxlength).unwrap_or(usize::MAX);
        if field.maxlength > 0 && value.chars().count() > limit {
            marks.push((id.clone(), format!("At most {limit} characters")));
            continue;
        }
        if let Some(message) = field.rules.iter().find_map(|rule| broken_rule(rule, &value)) {
            marks.push((id.clone(), message));
            continue;
        }
        if let Widget::DateTime { format, date_type, .. } = &field.widget {
            if !value.is_empty() && !date_reads(&value, format, date_type) {
                marks.push((
                    id.clone(),
                    format!("Not a date in the form {}", shown_format(format, date_type)),
                ));
            }
        }
    }
    marks
}

fn broken_rule(rule: &Rule, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let message = |text: &str, fallback: &str| {
        if text.is_empty() {
            fallback.to_string()
        } else {
            text.to_string()
        }
    };
    match rule {
        Rule::Number { message: text } => parse_number(value)
            .is_none()
            .then(|| message(text, "Please enter a valid number")),
        Rule::Integer { message: text } => {
            let whole = value.trim().trim_start_matches(['-', '+']);
            (whole.is_empty() || !whole.chars().all(|ch| ch.is_ascii_digit() || ch == ','))
                .then(|| message(text, "Please enter a valid whole number"))
        }
        Rule::Pattern { pattern, message: text } => match regex::Regex::new(pattern) {
            Ok(re) => (!re.is_match(value)).then(|| message(text, "The value is not in the right form")),
            // A JavaScript pattern this engine does not read: the server checks it again.
            Err(_) => None,
        },
        Rule::Range {
            min,
            min_eq,
            max,
            max_eq,
            message: text,
        } => {
            let Some(number) = parse_number(value) else {
                return Some(message(text, "Please enter a valid number"));
            };
            let bound = |text: &Option<String>| text.as_deref().and_then(parse_number);
            let low = bound(min).is_some_and(|low| if *min_eq { number < low } else { number <= low });
            let high = bound(max).is_some_and(|high| if *max_eq { number > high } else { number >= high });
            (low || high).then(|| message(text, "The number is out of range"))
        }
        Rule::Other => None,
    }
}

/// A number as a person types one: digits with grouping commas, a point, a sign.
fn parse_number(text: &str) -> Option<f64> {
    text.trim().replace(',', "").parse::<f64>().ok()
}

/// The Java date pattern as strftime, or None when it has a letter this does not map.
fn strftime_of(format: &str) -> Option<String> {
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let mut run = 1;
        while index + run < chars.len() && chars[index + run] == ch {
            run += 1;
        }
        let piece = match (ch, run) {
            ('y', 4) => "%Y",
            ('y', 2) => "%y",
            ('M', 2) => "%m",
            ('M', 3) => "%b",
            ('d', 2) => "%d",
            ('H', 2) => "%H",
            ('h', 2) => "%I",
            ('m', 2) => "%M",
            ('s', 2) => "%S",
            ('a', 1) => "%p",
            (letter, _) if letter.is_ascii_alphabetic() => return None,
            _ => "",
        };
        if piece.is_empty() {
            for _ in 0..run {
                out.push(ch);
            }
        } else {
            out.push_str(piece);
        }
        index += run;
    }
    Some(out)
}

fn date_pattern(format: &str, date_type: &str) -> String {
    if !format.is_empty() {
        return format.to_string();
    }
    match date_type {
        "date" => "yyyy-MM-dd".to_string(),
        "time" => "HH:mm".to_string(),
        _ => "yyyy-MM-dd HH:mm".to_string(),
    }
}

pub fn shown_format(format: &str, date_type: &str) -> String {
    date_pattern(format, date_type)
}

/// Whether `value` reads as a date, time or timestamp in `format` (a Java pattern). A pattern this does not map is
/// not checked here; the server checks it.
pub fn date_reads(value: &str, format: &str, date_type: &str) -> bool {
    let pattern = date_pattern(format, date_type);
    let Some(strf) = strftime_of(&pattern) else {
        return true;
    };
    let value = value.trim();
    match date_type {
        "date" => jiff::civil::Date::strptime(&strf, value).is_ok(),
        "time" => jiff::civil::Time::strptime(&strf, value).is_ok(),
        _ => {
            jiff::civil::DateTime::strptime(&strf, value).is_ok()
                // A timestamp field also takes a date alone, as the server does.
                || jiff::civil::Date::strptime("%Y-%m-%d", value).is_ok()
        }
    }
}

/// The date a field's text names, for the picker to start at; today when it names none.
pub fn date_of(value: &str, format: &str, date_type: &str) -> jiff::civil::Date {
    let pattern = date_pattern(format, date_type);
    let parsed = strftime_of(&pattern).and_then(|strf| match date_type {
        "date" => jiff::civil::Date::strptime(&strf, value.trim()).ok(),
        "time" => None,
        _ => jiff::civil::DateTime::strptime(&strf, value.trim())
            .map(|dt| dt.date())
            .ok()
            .or_else(|| jiff::civil::Date::strptime("%Y-%m-%d", value.trim()).ok()),
    });
    parsed.unwrap_or_else(|| jiff::Zoned::now().date())
}

/// `date` written in the field's format; a timestamp keeps the time it had, or midnight.
pub fn date_text(date: jiff::civil::Date, previous: &str, format: &str, date_type: &str) -> String {
    let pattern = date_pattern(format, date_type);
    let Some(strf) = strftime_of(&pattern) else {
        return date.to_string();
    };
    match date_type {
        "date" => date.strftime(&strf).to_string(),
        _ => {
            let time = jiff::civil::DateTime::strptime(&strf, previous.trim())
                .map(|dt| dt.time())
                .unwrap_or(jiff::civil::Time::midnight());
            date.to_datetime(time).strftime(&strf).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_dates_are_checked() {
        let number = Rule::Number { message: String::new() };
        assert_eq!(broken_rule(&number, "1,234.5"), None);
        assert!(broken_rule(&number, "abc").is_some());
        let pattern = Rule::Pattern {
            pattern: r"^\d*$".to_string(),
            message: "digits".to_string(),
        };
        assert_eq!(broken_rule(&pattern, "12a").as_deref(), Some("digits"));
        let range = Rule::Range {
            min: Some("1".to_string()),
            min_eq: true,
            max: Some("10".to_string()),
            max_eq: false,
            message: String::new(),
        };
        assert_eq!(broken_rule(&range, "1"), None);
        assert!(broken_rule(&range, "10").is_some());
        assert!(date_reads("2026-10-20", "", "date"));
        assert!(!date_reads("2026-13-40", "", "date"));
        assert!(date_reads("2026-10-20 14:30", "", "date-time"));
        assert!(date_reads("10/20/2026", "MM/dd/yyyy", "date"));
        assert!(
            date_reads("whatever", "EEE d", "date"),
            "an unmapped pattern is the server's to check"
        );
        let day = jiff::civil::date(2026, 10, 21);
        assert_eq!(date_text(day, "2026-10-20 14:30", "", "date-time"), "2026-10-21 14:30");
    }
}
