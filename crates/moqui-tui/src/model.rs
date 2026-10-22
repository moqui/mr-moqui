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

//! Screen model subset from `plans/archive/Screen-TUI-Impl.md` section 2.3.
//!
//! Node names match that section (`box`, `formList`, `textLine`). Unknown object fields are ignored so a later key does
//! not break a fixture. A node of an unknown `t`, or a widget of an unknown `kind`, becomes a placeholder
//! ([`ScreenModel::from_json`]). A `v` other than 1 is refused.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Model version this crate reads.
pub const MODEL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    Json(String),
    Version(u32),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(text) => write!(f, "screen model json: {text}"),
            Self::Version(v) => write!(f, "screen model version {v} is not {MODEL_VERSION}"),
        }
    }
}

/// One screen level. The envelope around `c`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenModel {
    pub v: u32,
    pub screen: ScreenMeta,
    /// Request parameters. Credential names are already gone.
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub messages: Messages,
    #[serde(default)]
    pub focus: String,
    #[serde(default)]
    pub c: Vec<Node>,
}

impl ScreenModel {
    /// Read a document. Every string is cleaned first ([`clean_text`], D9), and a node or widget of a kind this client
    /// does not know becomes a placeholder, so one new node type does not lose the page.
    pub fn from_json(text: &str) -> Result<Self, ModelError> {
        let mut value: serde_json::Value =
            serde_json::from_str(text).map_err(|err| ModelError::Json(err.to_string()))?;
        clean_value(&mut value);
        let model: Self = serde_json::from_value(value).map_err(|err| ModelError::Json(err.to_string()))?;
        if model.v != MODEL_VERSION {
            return Err(ModelError::Version(model.v));
        }
        Ok(model)
    }
}

/// The `t` values of [`Node`].
const NODE_TYPES: [&str; 30] = [
    "section",
    "container",
    "box",
    "row",
    "col",
    "dialog",
    "subscreens",
    "active",
    "fragment",
    "region",
    "text",
    "link",
    "form",
    "formList",
    "field",
    "placeholder",
    "error",
    "buttonMenu",
    "panel",
    "split",
    "dashboard",
    "numberCard",
    "statusBar",
    "dynamicDialog",
    "dynamicContainer",
    "tree",
    "image",
    "html",
    "markdown",
    "component",
];

/// The `kind` values of [`Widget`].
const WIDGET_KINDS: [&str; 19] = [
    "textLine",
    "textArea",
    "textFind",
    "dropDown",
    "dateTime",
    "dateFind",
    "datePeriod",
    "rangeFind",
    "file",
    "display",
    "password",
    "check",
    "radio",
    "hidden",
    "submit",
    "reset",
    "label",
    "image",
    "editable",
];

/// Text as it may be drawn: C0 and C1 controls other than tab and newline, and the bidirectional overrides and
/// isolates, are dropped, so a value cannot drive the terminal or reorder what is shown around it (D9).
pub fn clean_text(text: &str) -> String {
    text.chars().filter(|ch| !is_unsafe(*ch)).collect()
}

fn is_unsafe(ch: char) -> bool {
    let code = u32::from(ch);
    (code < 0x20 && ch != '\t' && ch != '\n')
        || (0x7F..0xA0).contains(&code)
        || (0x202A..=0x202E).contains(&code)
        || (0x2066..=0x2069).contains(&code)
}

fn clean_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => {
            if text.chars().any(is_unsafe) {
                *text = clean_text(text);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(clean_value),
        serde_json::Value::Object(map) => {
            if map.keys().any(|key| key.chars().any(is_unsafe)) {
                let entries = std::mem::take(map);
                *map = entries
                    .into_iter()
                    .map(|(key, item)| (clean_text(&key), item))
                    .collect();
            }
            map.values_mut().for_each(clean_value);
            let unknown_node = map
                .get("t")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|t| !NODE_TYPES.contains(&t));
            if unknown_node {
                let t = map
                    .get("t")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let id = map.get("id").cloned().unwrap_or_default();
                *map = serde_json::Map::new();
                map.insert("t".to_string(), "placeholder".into());
                map.insert("id".to_string(), id);
                map.insert("text".to_string(), t.into());
            } else if let Some(serde_json::Value::Object(widget)) = map.get_mut("w") {
                let unknown_widget = widget
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|kind| !WIDGET_KINDS.contains(&kind));
                if unknown_widget {
                    let kind = widget
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    *widget = serde_json::Map::new();
                    widget.insert("kind".to_string(), "display".into());
                    widget.insert("text".to_string(), format!("[{kind}]").into());
                }
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenMeta {
    pub path: String,
    #[serde(default)]
    pub path_list: Vec<String>,
    #[serde(default)]
    pub title: String,
    /// Index of the always-standalone screen in `path_list`. The root screen is 0.
    #[serde(default)]
    pub standalone: i64,
    /// The screen's `render-modes`; `all` when it names none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub render_modes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Messages {
    #[serde(default)]
    pub info: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub validation_errors: Vec<ValidationError>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    #[serde(default)]
    pub form: String,
    #[serde(default)]
    pub field: String,
    #[serde(default)]
    pub message: String,
}

/// A hidden input posted with a form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiddenValue {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// One tab on a subscreens node.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubscreenItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub active: bool,
}

/// A stage of a status bar: `done`, `current` or `todo`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusStage {
    #[serde(default)]
    pub status_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub state: String,
}

/// A node the client can draw. Names are the `t` values from section 2.3.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Node {
    Section {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        iterate: bool,
        #[serde(default)]
        c: Vec<Node>,
    },
    Container {
        id: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Box {
        id: String,
        #[serde(default)]
        title: String,
        #[serde(default, rename = "type")]
        kind: String,
        #[serde(default)]
        initial: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Row {
        id: String,
        #[serde(default)]
        xs: String,
        #[serde(default)]
        sm: String,
        #[serde(default)]
        md: String,
        #[serde(default)]
        lg: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Col {
        id: String,
        #[serde(default)]
        xs: String,
        #[serde(default)]
        sm: String,
        #[serde(default)]
        md: String,
        #[serde(default)]
        lg: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Dialog {
        id: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        button: String,
        /// A row-selection action of this list: its forms post the selected rows' ids too.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        list: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    /// The subscreens of a level: `tab`, `popup`, `wizard` (a step per subscreen) or `stack`.
    #[serde(rename = "subscreens", rename_all = "camelCase")]
    Subscreen {
        id: String,
        #[serde(default)]
        kind: String,
        #[serde(default)]
        items: Vec<SubscreenItem>,
        /// A wizard's caption for its last step's button.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        finish_text: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Active {
        id: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Fragment {
        #[serde(default)]
        id: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    /// A named part of its parent: a box `toolbar`; a panel `header`, `left`, `center`, `right` or `footer`; a split
    /// `side` or `main`. `size` is a panel side's width in pixels.
    Region {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        size: u32,
        #[serde(default)]
        c: Vec<Node>,
    },
    Text {
        #[serde(default)]
        id: String,
        text: String,
        #[serde(default)]
        style: String,
    },
    #[serde(rename_all = "camelCase")]
    Link {
        id: String,
        text: String,
        url: String,
        #[serde(default)]
        kind: String,
        #[serde(default)]
        method: String,
        #[serde(default)]
        confirm: String,
        /// `primary`, `success`, `info`, `warning`, `danger`, or empty.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        btn_type: String,
        /// A Material Symbols name.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        icon: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        badge: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tooltip: String,
        /// The user may not follow it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        disabled: bool,
        /// It runs a transition that is not read-only, as qvt's `read-only` attribute tells: an agent's press of it
        /// waits for the user's approval (plans/LLM-A2A-Assist.md 4.7). The server's gate stays the authority.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        writes: bool,
    },
    Form {
        id: String,
        name: String,
        #[serde(default)]
        kind: String,
        /// The form holds a file field, so it posts as multipart.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        multipart: bool,
        #[serde(default, rename = "transitionUrl")]
        transition_url: String,
        #[serde(default)]
        method: String,
        #[serde(default)]
        hidden: Vec<HiddenValue>,
        #[serde(default, rename = "submitMessage")]
        submit_message: String,
        #[serde(default, rename = "backgroundReloadId")]
        background_reload_id: String,
        #[serde(default, rename = "backgroundHideId")]
        background_hide_id: String,
        /// The id of this form in a qvt render of the same screen (its name, then `_` and the section entry index
        /// inside a section-iterate): what the browser's state and a follow name it by.
        #[serde(default, rename = "domId", skip_serializing_if = "String::is_empty")]
        dom_id: String,
        /// Its transition is not read-only: a submit by an agent waits for the user's approval.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        writes: bool,
        #[serde(default)]
        c: Vec<Node>,
    },
    FormList {
        #[serde(flatten)]
        list: FormList,
    },
    #[serde(rename_all = "camelCase")]
    Field {
        id: String,
        name: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        required: bool,
        #[serde(default)]
        maxlength: u32,
        /// Checks made before a submit, other than `required`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rules: Vec<Rule>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tooltip: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visible_when: Option<VisibleWhen>,
        w: Widget,
    },
    Placeholder {
        #[serde(default)]
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        url: String,
    },
    Error {
        #[serde(default)]
        id: String,
        #[serde(default)]
        text: String,
    },
    ButtonMenu {
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Panel {
        id: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    #[serde(rename_all = "camelCase")]
    Split {
        id: String,
        /// The side's width in pixels.
        #[serde(default, skip_serializing_if = "is_zero")]
        side_width: u32,
        /// The parameter that holds the selected record's id.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        select_parameter: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    Dashboard {
        id: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    NumberCard {
        id: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        url: String,
    },
    /// The stages of a status, and its transitions as links (`c`), each posting the new status.
    StatusBar {
        id: String,
        /// The current status, for a bar with no stages.
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stages: Vec<StatusStage>,
        #[serde(default)]
        c: Vec<Node>,
    },
    DynamicDialog {
        id: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        button: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        title: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    DynamicContainer {
        id: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        c: Vec<Node>,
    },
    /// A tree whose nodes load from `url`: the roots with `treeNodeId=#`, a node's children with its id.
    #[serde(rename_all = "camelCase")]
    Tree {
        id: String,
        #[serde(default)]
        url: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        open_path: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        parameters: BTreeMap<String, String>,
        #[serde(default)]
        c: Vec<Node>,
    },
    Image {
        id: String,
        #[serde(default)]
        alt: String,
        #[serde(default)]
        url: String,
    },
    /// HTML converted on the server: blocks of text, and the links numbered `[n]` in them.
    Html {
        id: String,
        #[serde(default)]
        blocks: Vec<HtmlBlock>,
        #[serde(default)]
        links: Vec<HtmlLink>,
    },
    Markdown {
        id: String,
        #[serde(default)]
        text: String,
    },
    /// A client component of a `tui` text (section 2.9): its registered name, its props, and the screen path its
    /// transitions are under.
    Component {
        #[serde(default)]
        id: String,
        name: String,
        #[serde(default)]
        props: serde_json::Value,
        #[serde(default)]
        base: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormList {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub rows: Vec<ListRow>,
    #[serde(default)]
    pub paging: Paging,
    /// Find fields drawn above the rows, not as a second form.
    #[serde(default)]
    pub header_fields: Vec<Node>,
    /// Rows can be selected, and the selected rows' `select_id`s are posted as `parameter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_selection: Option<RowSelection>,
    /// Where a row's form, or the multi form of all rows, posts. Empty for a list that only shows.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transition_url: String,
    /// One form of every row, its fields named `name_N` (qvt's multi `m-form`); else each row is its own form.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
    /// The id of the list's form in a qvt render (the multi `m-form`; a row's form is this, `_` and the row, the find
    /// form this and `_header`), as [`Node::Form`]'s `dom_id`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dom_id: String,
    /// Its transition is not read-only: a row's or the multi form's submit by an agent waits for the user's approval.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writes: bool,
    /// Posted with each row form, or once with the multi form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<HiddenValue>,
    /// The submit buttons of the multi form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_finds: Option<Box<SavedFinds>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select_columns: Option<Box<SelectColumns>>,
    /// The list as a file or as text: csv, xlsx, text, pdf.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<Export>,
    /// The other ways to show the rows: a board, a calendar, a tree (`views`, style-guide §8.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub views: Option<Box<ListViews>>,
    #[serde(default)]
    pub c: Vec<Node>,
}

/// A list's views: the kinds in the form's order (`list`, `board`, `calendar`, `tree`), the one to show, the user's
/// preference it is kept in (`LIST_VIEW_…`, as qvt keeps it), and the props of the `board`, `calendar` and `tree` components
/// that draw the page's rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListViews {
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub chosen: String,
    #[serde(default)]
    pub key: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub board: serde_json::Value,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub calendar: serde_json::Value,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub tree: serde_json::Value,
}

/// The user's saved finds of a list (`formSaveFind`): each applies by its url; a new one, an update, a default or a
/// delete posts `form_location` and the find's `parameters` to `save_url`, as qvt's saved-finds dialog does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFinds {
    #[serde(default)]
    pub save_url: String,
    #[serde(default)]
    pub form_location: String,
    /// The current find's parameters, which a new or updated find saves.
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    /// The screen with no find applied (`formListFindId=_clear`).
    #[serde(default)]
    pub clear_url: String,
    #[serde(default)]
    pub finds: Vec<SavedFind>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFind {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The screen with the find applied.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub active: bool,
    /// The user's default find of the screen.
    #[serde(default)]
    pub default: bool,
    /// The user's own find, which the user may update or delete.
    #[serde(default)]
    pub mine: bool,
}

/// The columns of a list as the user may arrange them (`formSelectColumns`): the shown columns in order, each its
/// fields, and the fields not shown. Saving posts them as qvt's `columnsTree`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectColumns {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub form_location: String,
    /// The find parameters, posted back so the list keeps its find.
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub shown: Vec<Vec<ColumnChoice>>,
    #[serde(default)]
    pub hidden: Vec<ColumnChoice>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnChoice {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
}

/// One export of a list: `csv`, `xlsx`, `text`, `pdf`, or `all` for every row as csv.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Export {
    pub kind: String,
    #[serde(default)]
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowSelection {
    #[serde(default)]
    pub id_field: String,
    #[serde(default)]
    pub parameter: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub list_mode: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub title: String,
    /// The `orderByField` value that sorts on this column, empty when it does not sort.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub order_by: String,
    /// `print-width`: characters, or a percent of the line with `print_width_type` `percent`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub print_width: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub print_width_type: String,
    /// `right` for a column of numbers.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub align: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListRow {
    /// The row's `id-field` value, else its index on the page.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The row-selection `id-field` value of the row.
    #[serde(default, rename = "selectId", skip_serializing_if = "String::is_empty")]
    pub select_id: String,
    pub cells: Vec<Cell>,
    /// The row's values posted with its form and not shown: its hidden fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<HiddenValue>,
}

/// A list cell: plain text, one node (a link, or a field of a row form), or the parts of a column that stacks several
/// fields, one under the other.
///
/// The node is boxed so a cell is not the size of every node variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Cell {
    Text(String),
    Node(Box<Node>),
    Stack(Vec<Cell>),
}

impl Cell {
    /// The cell's text as one line: a stack's first part.
    pub fn text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::Node(node) => match node.as_ref() {
                Node::Link { text, .. } | Node::Text { text, .. } => text,
                Node::Field { w, .. } => w.value(),
                _ => "",
            },
            Self::Stack(parts) => parts.first().map_or("", Self::text),
        }
    }

    /// The parts drawn one under the other: a stack's, else the cell itself.
    pub fn parts(&self) -> Vec<&Cell> {
        match self {
            Self::Stack(parts) => parts.iter().flat_map(Self::parts).collect(),
            other => vec![other],
        }
    }

    pub fn link_url(&self) -> Option<&str> {
        match self {
            Self::Node(node) => match node.as_ref() {
                Node::Link { url, .. } => Some(url),
                _ => None,
            },
            Self::Stack(parts) => parts.iter().find_map(Self::link_url),
            Self::Text(_) => None,
        }
    }

    /// The nodes in the cell.
    pub fn nodes(&self) -> Vec<&Node> {
        match self {
            Self::Node(node) => vec![node.as_ref()],
            Self::Stack(parts) => parts.iter().flat_map(Self::nodes).collect(),
            Self::Text(_) => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paging {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub size: u32,
    #[serde(default)]
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_index: Option<u32>,
    /// The 1-based number of the page's first and last row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_low: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_high: Option<u32>,
}

/// A check of a field's value before a submit. A pattern is a JavaScript regular expression; one Rust's `regex` cannot
/// read is skipped by the client, and the server checks it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Rule {
    Number {
        #[serde(default)]
        message: String,
    },
    Integer {
        #[serde(default)]
        message: String,
    },
    Pattern {
        pattern: String,
        #[serde(default)]
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Range {
        #[serde(default)]
        min: Option<String>,
        #[serde(default)]
        min_eq: bool,
        #[serde(default)]
        max: Option<String>,
        #[serde(default)]
        max_eq: bool,
        #[serde(default)]
        message: String,
    },
    /// A kind this client does not know: not checked here.
    #[serde(other)]
    Other,
}

/// One block of an `html` node: `p`, `h1`…`h6`, `li`, `quote`, `pre`, `row` or `rule`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlBlock {
    pub kind: String,
    #[serde(default)]
    pub text: String,
    /// List or quote nesting.
    #[serde(default)]
    pub depth: u8,
    /// A list item's `•` or `1.`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub marker: String,
}

/// Link `[n]` of an `html` node is `links[n - 1]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlLink {
    #[serde(default)]
    pub text: String,
    pub url: String,
}

/// Show the field only while another field of its form has a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleWhen {
    pub field: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Widget {
    #[serde(rename_all = "camelCase")]
    TextLine {
        #[serde(default)]
        value: String,
        /// `number` right-aligns the value.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        input_type: String,
        /// An Inputmask pattern: `9` a digit, `a` a letter, `*` either; other characters are typed for the user.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        mask: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        size: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        prefix: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        disabled: bool,
        /// `default-transition`: the value asked for when a `depends_on` field changes and this one is empty.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        default_url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        default_parameters: BTreeMap<String, String>,
        /// Request parameter name to the field whose value it sends.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        depends_on: BTreeMap<String, String>,
    },
    #[serde(rename_all = "camelCase")]
    TextArea {
        #[serde(default)]
        value: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        rows: u32,
        /// `md` is Markdown source; `html`, `rich` and `rich-light` come with `read_only` (D12).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        editor_type: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        read_only: bool,
    },
    /// A text find posts `name`, `name_op`, `name_not` and `name_ic`.
    #[serde(rename_all = "camelCase")]
    TextFind {
        #[serde(default)]
        value: String,
        #[serde(default)]
        op: String,
        #[serde(default)]
        not: bool,
        #[serde(default)]
        ic: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hide_options: bool,
    },
    #[serde(rename_all = "camelCase")]
    DropDown {
        #[serde(default)]
        value: String,
        #[serde(default)]
        options: Vec<Choice>,
        #[serde(default)]
        options_url: String,
        /// Request parameter name to the field whose value it sends.
        #[serde(default)]
        depends_on: BTreeMap<String, String>,
        /// Fixed parameters of the options transition.
        #[serde(default)]
        options_parameters: BTreeMap<String, String>,
        /// The options transition filters by a `term` parameter: an `ac-transition` text line is such a drop-down.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        server_search: bool,
        /// With `server_search`, the fewest characters typed before a search.
        #[serde(default, skip_serializing_if = "is_zero")]
        min_length: u32,
        /// Ask for the options even when a `depends_on` field is empty.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        depends_optional: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        multiple: bool,
        /// Free text is allowed as well as the options.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        combo: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_empty: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        submit_on_select: bool,
        /// A find drop-down that also posts `name_not`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        show_not: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        not: bool,
    },
    #[serde(rename_all = "camelCase")]
    DateTime {
        #[serde(default)]
        value: String,
        /// `date`, `time`, or `date-time`.
        #[serde(default, rename = "type")]
        date_type: String,
        /// A Java date pattern such as `yyyy-MM-dd HH:mm`.
        #[serde(default)]
        format: String,
    },
    /// Posts `name_from` and `name_thru`.
    #[serde(rename_all = "camelCase")]
    DateFind {
        #[serde(default, rename = "type")]
        date_type: String,
        #[serde(default)]
        format: String,
        #[serde(default)]
        from: String,
        #[serde(default)]
        thru: String,
    },
    /// Posts `name_period`, `name_poffset`, `name_pdate`, `name_from` and `name_thru`.
    #[serde(rename_all = "camelCase")]
    DatePeriod {
        #[serde(default)]
        period: String,
        #[serde(default)]
        poffset: String,
        #[serde(default)]
        pdate: String,
        #[serde(default)]
        from: String,
        #[serde(default)]
        thru: String,
        #[serde(default)]
        allow_empty: bool,
        #[serde(default)]
        time: bool,
    },
    /// Posts `name_from` and `name_thru`.
    RangeFind {
        #[serde(default)]
        from: String,
        #[serde(default)]
        thru: String,
    },
    /// Upload is off where the client runs on the server (SSH, browser terminal).
    File {
        #[serde(default)]
        multiple: bool,
        #[serde(default)]
        accept: String,
    },
    /// `value` is the plain value posted with the form (`also-hidden`); absent, nothing is posted. With `value_url` the
    /// text and value are asked for again when a `depends_on` field changes (`dynamic-transition`).
    #[serde(rename_all = "camelCase")]
    Display {
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        value_url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        value_parameters: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        depends_on: BTreeMap<String, String>,
    },
    /// No value. A password in the document would be a secret, so a `value` key is ignored.
    Password {},
    Check {
        #[serde(default)]
        value: String,
        #[serde(default)]
        options: Vec<Choice>,
    },
    Radio {
        #[serde(default)]
        value: String,
        #[serde(default)]
        options: Vec<Choice>,
    },
    Hidden {
        #[serde(default)]
        value: String,
    },
    Submit {
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        confirm: String,
    },
    Reset {
        #[serde(default)]
        text: String,
    },
    Label {
        #[serde(default)]
        text: String,
        #[serde(default)]
        style: String,
    },
    Image {
        #[serde(default)]
        alt: String,
        #[serde(default)]
        url: String,
    },
    /// Click to edit (`m-editable`): the text, and Enter opens a one-field editor that posts `parameter_name` to `url`.
    #[serde(rename_all = "camelCase")]
    Editable {
        #[serde(default)]
        text: String,
        #[serde(default)]
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        parameters: BTreeMap<String, String>,
        #[serde(default)]
        parameter_name: String,
        /// `text`, `textarea` or `select`.
        #[serde(default)]
        widget_type: String,
        /// The value to edit, when it is not the text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<Choice>,
    },
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl Widget {
    /// The value the field holds as one string: the input's value, a display's text, a button's caption.
    pub fn value(&self) -> &str {
        match self {
            Self::TextLine { value, .. }
            | Self::TextArea { value, .. }
            | Self::TextFind { value, .. }
            | Self::DropDown { value, .. }
            | Self::DateTime { value, .. }
            | Self::Check { value, .. }
            | Self::Radio { value, .. }
            | Self::Hidden { value } => value,
            Self::Display { text, .. }
            | Self::Submit { text, .. }
            | Self::Reset { text }
            | Self::Label { text, .. }
            | Self::Editable { text, .. } => text,
            Self::DateFind { from, .. } | Self::DatePeriod { from, .. } | Self::RangeFind { from, .. } => from,
            Self::Password {} | Self::File { .. } | Self::Image { .. } => "",
        }
    }

    pub fn is_editable_text(&self) -> bool {
        match self {
            Self::TextLine { .. } | Self::TextFind { .. } | Self::DateTime { .. } | Self::Password {} => true,
            Self::TextArea { read_only, .. } => !read_only,
            _ => false,
        }
    }

    /// Shown, not edited: a display, a label, an image.
    pub fn is_display(&self) -> bool {
        matches!(
            self,
            Self::Display { .. } | Self::Label { .. } | Self::Image { .. } | Self::Editable { .. }
        )
    }
}

impl Node {
    pub fn id(&self) -> &str {
        match self {
            Self::FormList { list } => &list.id,
            Self::Section { id, .. }
            | Self::Container { id, .. }
            | Self::Box { id, .. }
            | Self::Row { id, .. }
            | Self::Col { id, .. }
            | Self::Dialog { id, .. }
            | Self::Subscreen { id, .. }
            | Self::Active { id, .. }
            | Self::Fragment { id, .. }
            | Self::Region { id, .. }
            | Self::Text { id, .. }
            | Self::Link { id, .. }
            | Self::Form { id, .. }
            | Self::Field { id, .. }
            | Self::Placeholder { id, .. }
            | Self::Error { id, .. }
            | Self::ButtonMenu { id, .. }
            | Self::Panel { id, .. }
            | Self::Split { id, .. }
            | Self::Dashboard { id, .. }
            | Self::NumberCard { id, .. }
            | Self::StatusBar { id, .. }
            | Self::DynamicDialog { id, .. }
            | Self::DynamicContainer { id, .. }
            | Self::Tree { id, .. }
            | Self::Image { id, .. }
            | Self::Html { id, .. }
            | Self::Markdown { id, .. }
            | Self::Component { id, .. } => id,
        }
    }

    /// Child nodes in document order. List row cells are not children; [`walk`] visits those too.
    pub fn children(&self) -> &[Node] {
        match self {
            Self::Section { c, .. }
            | Self::Container { c, .. }
            | Self::Box { c, .. }
            | Self::Row { c, .. }
            | Self::Col { c, .. }
            | Self::Dialog { c, .. }
            | Self::Subscreen { c, .. }
            | Self::Active { c, .. }
            | Self::Fragment { c, .. }
            | Self::Region { c, .. }
            | Self::Form { c, .. }
            | Self::ButtonMenu { c, .. }
            | Self::Panel { c, .. }
            | Self::Split { c, .. }
            | Self::Dashboard { c, .. }
            | Self::DynamicDialog { c, .. }
            | Self::DynamicContainer { c, .. }
            | Self::Tree { c, .. }
            | Self::StatusBar { c, .. } => c,
            Self::FormList { list } => &list.c,
            Self::Text { .. }
            | Self::Link { .. }
            | Self::Field { .. }
            | Self::Placeholder { .. }
            | Self::Error { .. }
            | Self::NumberCard { .. }
            | Self::Image { .. }
            | Self::Html { .. }
            | Self::Markdown { .. }
            | Self::Component { .. } => &[],
        }
    }

    /// Mutable children. The empty cases return an empty slice.
    pub fn children_mut(&mut self) -> &mut [Node] {
        match self {
            Self::Section { c, .. }
            | Self::Container { c, .. }
            | Self::Box { c, .. }
            | Self::Row { c, .. }
            | Self::Col { c, .. }
            | Self::Dialog { c, .. }
            | Self::Subscreen { c, .. }
            | Self::Active { c, .. }
            | Self::Fragment { c, .. }
            | Self::Region { c, .. }
            | Self::Form { c, .. }
            | Self::ButtonMenu { c, .. }
            | Self::Panel { c, .. }
            | Self::Split { c, .. }
            | Self::Dashboard { c, .. }
            | Self::DynamicDialog { c, .. }
            | Self::DynamicContainer { c, .. }
            | Self::Tree { c, .. }
            | Self::StatusBar { c, .. } => c,
            Self::FormList { list } => &mut list.c,
            Self::Text { .. }
            | Self::Link { .. }
            | Self::Field { .. }
            | Self::Placeholder { .. }
            | Self::Error { .. }
            | Self::NumberCard { .. }
            | Self::Image { .. }
            | Self::Html { .. }
            | Self::Markdown { .. }
            | Self::Component { .. } => &mut [],
        }
    }
}

impl Node {
    /// The node's id, to change it.
    pub fn id_mut(&mut self) -> &mut String {
        match self {
            Self::FormList { list } => &mut list.id,
            Self::Section { id, .. }
            | Self::Container { id, .. }
            | Self::Box { id, .. }
            | Self::Row { id, .. }
            | Self::Col { id, .. }
            | Self::Dialog { id, .. }
            | Self::Subscreen { id, .. }
            | Self::Active { id, .. }
            | Self::Fragment { id, .. }
            | Self::Region { id, .. }
            | Self::Text { id, .. }
            | Self::Link { id, .. }
            | Self::Form { id, .. }
            | Self::Field { id, .. }
            | Self::Placeholder { id, .. }
            | Self::Error { id, .. }
            | Self::ButtonMenu { id, .. }
            | Self::Panel { id, .. }
            | Self::Split { id, .. }
            | Self::Dashboard { id, .. }
            | Self::NumberCard { id, .. }
            | Self::StatusBar { id, .. }
            | Self::DynamicDialog { id, .. }
            | Self::DynamicContainer { id, .. }
            | Self::Tree { id, .. }
            | Self::Image { id, .. }
            | Self::Html { id, .. }
            | Self::Markdown { id, .. }
            | Self::Component { id, .. } => id,
        }
    }
}

/// Visit every node to change it, list cells, find fields and buttons included.
pub fn walk_mut(nodes: &mut [Node], visit: &mut impl FnMut(&mut Node)) {
    for node in nodes {
        visit(node);
        if let Node::FormList { list } = node {
            for row in &mut list.rows {
                for cell in &mut row.cells {
                    cell_nodes_mut(cell, visit);
                }
            }
            walk_mut(&mut list.header_fields, visit);
            walk_mut(&mut list.buttons, visit);
        }
        walk_mut(node.children_mut(), visit);
    }
}

fn cell_nodes_mut(cell: &mut Cell, visit: &mut impl FnMut(&mut Node)) {
    match cell {
        Cell::Node(node) => {
            visit(node);
            walk_mut(node.children_mut(), visit);
        }
        Cell::Stack(parts) => parts.iter_mut().for_each(|part| cell_nodes_mut(part, visit)),
        Cell::Text(_) => {}
    }
}

/// Put `prefix` before every id in `nodes`, and before the list a row action names: the body of a dynamic dialog or
/// container is its own render, so its ids may be the screen's too.
pub fn prefix_ids(nodes: &mut [Node], prefix: &str) {
    walk_mut(nodes, &mut |node| {
        let id = node.id_mut();
        if !id.is_empty() {
            *id = format!("{prefix}{id}");
        }
        if let Node::Dialog { list, .. } = node {
            if !list.is_empty() {
                *list = format!("{prefix}{list}");
            }
        }
    });
}

/// The node with `id`, to change it.
pub fn find_mut<'a>(nodes: &'a mut [Node], id: &str) -> Option<&'a mut Node> {
    for node in nodes {
        if node.id() == id {
            return Some(node);
        }
        if let Some(found) = find_mut(node.children_mut(), id) {
            return Some(found);
        }
    }
    None
}

/// A node from JSON the client did not get in a screen (a document for the help), read as a screen's nodes are.
///
/// # Errors
///
/// Returns the reason when the value is not a node.
pub fn node_of(mut value: serde_json::Value) -> Result<Node, ModelError> {
    clean_value(&mut value);
    serde_json::from_value(value).map_err(|err| ModelError::Json(err.to_string()))
}

/// Walk every node, including nested children and list-cell nodes.
pub fn walk<'a>(nodes: &'a [Node], visit: &mut impl FnMut(&'a Node)) {
    for node in nodes {
        visit(node);
        if let Node::FormList { list } = node {
            for row in &list.rows {
                for cell in &row.cells {
                    for inner in cell.nodes() {
                        visit(inner);
                        walk(inner.children(), visit);
                    }
                }
            }
            walk(&list.header_fields, visit);
            walk(&list.buttons, visit);
        }
        walk(node.children(), visit);
    }
}
