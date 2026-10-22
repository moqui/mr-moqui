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

//! Width classes and the layout of section 1.3, in canvas columns, with no Revue import (D3). [`layout`] turns the
//! levels' nodes into [`Piece`]s that know their width and their height in rows; the view draws them and scrolls the
//! canvas to the focus. Revue's CSS does not lay out grids or flex, so all of it is here.

use std::collections::BTreeMap;

use crate::components::Line;
use crate::model::{Cell, FormList, HtmlBlock, HtmlLink, Node, ScreenModel, StatusStage, Widget};

/// Rows of a list page when the document has no page size.
pub const PAGE_ROWS: usize = 10;

const COL_CAP: usize = 40;
/// A box narrower than this falls to its own row rather than squeezing.
const BOX_MIN: u16 = 40;
const LABEL_CAP: usize = 24;
const LABEL_CAP_SM: usize = 18;
/// A `field-row` keeps its cells side by side while each gets its label plus this many columns.
const CELL_VALUE_MIN: usize = 24;
/// The width of a text field with no size or maxlength.
const VALUE_DEFAULT: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WidthClass {
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
}

impl WidthClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Xs => "xs",
            Self::Sm => "sm",
            Self::Md => "md",
            Self::Lg => "lg",
            Self::Xl => "xl",
        }
    }

    fn stacks_boxes(self) -> bool {
        self <= Self::Sm
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelMode {
    Above,
    Beside,
}

impl LabelMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Beside => "beside",
        }
    }
}

/// The width classes and the drawer as a theme sets them (`STRT_TUI_BREAKPOINTS`), with the defaults of section 1.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoints {
    pub sm: u16,
    pub md: u16,
    pub lg: u16,
    pub xl: u16,
    pub drawer: u16,
}

impl Default for Breakpoints {
    fn default() -> Self {
        Self {
            sm: 80,
            md: 120,
            lg: 180,
            xl: 240,
            drawer: DRAWER_WIDTH,
        }
    }
}

impl Breakpoints {
    /// The theme's values over the defaults. A value out of order is left out, with the reason.
    pub fn with(values: &BTreeMap<String, u16>) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut skipped = Vec::new();
        for (name, value) in values {
            match name.as_str() {
                "sm" => out.sm = *value,
                "md" => out.md = *value,
                "lg" => out.lg = *value,
                "xl" => out.xl = *value,
                "drawer" => out.drawer = (*value).clamp(12, 60),
                other => skipped.push(format!("breakpoint {other} is not known")),
            }
        }
        if !(out.sm < out.md && out.md < out.lg && out.lg < out.xl) {
            skipped.push("breakpoints are not in order sm < md < lg < xl; the defaults are used".to_string());
            let drawer = out.drawer;
            out = Self {
                drawer,
                ..Self::default()
            };
        }
        (out, skipped)
    }

    pub fn class(&self, canvas: u16) -> WidthClass {
        if canvas < self.sm {
            WidthClass::Xs
        } else if canvas < self.md {
            WidthClass::Sm
        } else if canvas < self.lg {
            WidthClass::Md
        } else if canvas < self.xl {
            WidthClass::Lg
        } else {
            WidthClass::Xl
        }
    }

    /// The terminal width from which the drawer docks: the canvas left is still md.
    pub fn dock_at(&self) -> u16 {
        self.md + self.drawer
    }
}

/// Terminal width at which the drawer docks with the default breakpoints. The leftover canvas is then at least 120.
pub const DRAWER_DOCK_AT: u16 = 146;

/// Columns the docked drawer takes.
pub const DRAWER_WIDTH: u16 = 26;

/// Drawer and the canvas the width class is computed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub terminal: u16,
    pub drawer: u16,
    pub canvas: u16,
    pub class: WidthClass,
}

/// The frame with the default breakpoints and the drawer docked by width.
pub fn frame(terminal: u16) -> Frame {
    frame_with(terminal, &Breakpoints::default(), None)
}

/// The drawer docks when the canvas left beside it is still md, or as the user chose with F9 (D7).
pub fn frame_with(terminal: u16, breakpoints: &Breakpoints, drawer: Option<bool>) -> Frame {
    let docked = drawer.unwrap_or(terminal >= breakpoints.dock_at()) && terminal > breakpoints.drawer + 20;
    let drawer = if docked { breakpoints.drawer } else { 0 };
    let canvas = terminal.saturating_sub(drawer);
    Frame {
        terminal,
        drawer,
        canvas,
        class: breakpoints.class(canvas),
    }
}

pub fn width_class(canvas: u16) -> WidthClass {
    Breakpoints::default().class(canvas)
}

/// Beside-label column. `xs` is 0 because the label sits above the value. `sm` caps at 18, other classes at 24.
pub fn label_column_width(titles: &[&str], class: WidthClass) -> usize {
    if class == WidthClass::Xs {
        return 0;
    }
    let cap = if class == WidthClass::Sm {
        LABEL_CAP_SM
    } else {
        LABEL_CAP
    };
    // Room for the required mark before the title.
    titles
        .iter()
        .map(|title| display_width(title).min(cap))
        .max()
        .unwrap_or(0)
}

/// Title padded with spaced dot leaders out to `width` columns, as `Order date . . . .` in the mockups; the view puts
/// ` : ` after it. A width of 0 returns the title.
pub fn dotted_label(title: &str, width: usize) -> String {
    if width == 0 {
        return title.to_string();
    }
    // A title cut at the column still leaves the space before the colon.
    let shown = if display_width(title) >= width {
        truncate_width(title, width.saturating_sub(1))
    } else {
        title.to_string()
    };
    let start = display_width(&shown);
    let mut out = shown;
    // Dots on every other column counted from the right, so the leaders of a column of labels line up.
    for col in start..width {
        out.push(if col > start && (width - col).is_multiple_of(2) {
            '.'
        } else {
            ' '
        });
    }
    out
}

// ---- what the session adds ----

/// What changes the layout and only the session knows: a box the user closed, the rows of a tree that are open, a
/// component's lines. [`Plain`] is a screen as it first comes.
pub trait Shown {
    /// Whether a collapsible box (one with an `initial`) is closed now.
    fn collapsed(&self, _id: &str, initial: &str) -> bool {
        initial == "closed"
    }

    /// The rows of a tree as they show: its loaded nodes, with the children of the open ones.
    fn tree_rows(&self, _id: &str) -> Vec<TreeRow> {
        Vec::new()
    }

    /// The number a link shows before its text while links are numbered (`#`), so a line of buttons wraps with room
    /// for it.
    fn link_number(&self, _id: &str) -> Option<usize> {
        None
    }

    /// A component's lines at `width`; None when no component of its name is registered.
    fn component_lines(
        &self,
        _id: &str,
        _name: &str,
        _props: &serde_json::Value,
        _width: u16,
        _wide: bool,
    ) -> Option<Vec<Line>> {
        None
    }
}

/// A screen as the server sent it: boxes as their `initial` says, trees not loaded, components with no data.
pub struct Plain;

impl Shown for Plain {
    fn component_lines(
        &self,
        _id: &str,
        name: &str,
        props: &serde_json::Value,
        width: u16,
        wide: bool,
    ) -> Option<Vec<Line>> {
        crate::components::find(name)
            .map(|component| component.lines(props, &crate::components::ComponentState::default(), width, wide))
    }
}

/// One row of a tree as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// The node's id, which its children are asked for by.
    pub key: String,
    pub depth: usize,
    pub text: String,
    pub url: String,
    pub mark: TreeMark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeMark {
    Open,
    Closed,
    Leaf,
    /// Its children were asked for and have not come.
    Loading,
}

// ---- the pieces ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidOut {
    pub class: WidthClass,
    pub canvas: u16,
    pub pieces: Vec<Piece>,
}

impl LaidOut {
    pub fn height(&self) -> usize {
        self.pieces.iter().map(Piece::height).sum()
    }
}

/// Something drawn on the canvas, `w` columns wide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Pieces one under the other.
    Stack(Vec<Piece>),
    /// Pieces side by side, each at `x` within the row.
    Row(Vec<Placed>),
    Box(LaidBox),
    Form(LaidForm),
    /// Links and buttons on one line.
    Inline(Vec<Inline>),
    Text(LaidText),
    List(LaidList),
    Tabs(LaidTabs),
    Tree(LaidTree),
    /// The stages of a status bar on one line.
    Status(LaidStatus),
    /// A component's lines.
    Lines(LaidLines),
    /// A browser-only part, or a node this client has no view for.
    Placeholder {
        id: String,
        text: String,
        url: String,
    },
    Error {
        id: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub x: u16,
    pub w: u16,
    pub piece: Piece,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidBox {
    pub id: String,
    pub title: String,
    /// The box `type`: a heavy left edge in its color.
    pub kind: String,
    pub w: u16,
    pub toolbar: Vec<Inline>,
    pub body: Vec<Piece>,
    /// The box has an `initial`, so Enter on its title opens or closes it.
    pub collapsible: bool,
    /// Closed: the title and the bottom edge only.
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidTree {
    pub id: String,
    pub rows: Vec<TreeRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidStatus {
    pub id: String,
    pub text: String,
    pub stages: Vec<StatusStage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidLines {
    pub id: String,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidForm {
    pub id: String,
    pub name: String,
    pub lines: Vec<FormLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormLine {
    Field(LaidField),
    /// A `field-row` whose cells fit side by side.
    Cells(Vec<(u16, LaidField)>),
    Buttons(Vec<Inline>),
    /// A `field-group` title.
    Group(String),
    Text(LaidText),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidField {
    pub id: String,
    pub title: String,
    pub label_width: usize,
    pub mode: LabelMode,
    /// Columns for the value, brackets included.
    pub value_width: usize,
    /// Rows of the value: a text area's rows, else 1.
    pub rows: usize,
}

impl LaidField {
    /// Rows: the label's above, and the value's.
    pub fn height(&self) -> usize {
        match self.mode {
            LabelMode::Above => 1 + self.rows,
            LabelMode::Beside => self.rows,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Link {
        id: String,
        text: String,
        btn_type: String,
        button: bool,
    },
    /// A submit or reset field. `primary` is the form's submit: dim until the form changes (style guide §4.6).
    Button { id: String, text: String, primary: bool },
    /// The button that opens a container-dialog or a dynamic-dialog.
    Dialog { id: String, text: String },
    /// A button-menu: `[ More ▾ ]`, Enter opens its items.
    Menu { id: String, text: String },
}

impl Inline {
    pub fn id(&self) -> &str {
        match self {
            Self::Link { id, .. } | Self::Button { id, .. } | Self::Dialog { id, .. } | Self::Menu { id, .. } => id,
        }
    }

    /// Columns as `draw_inline` draws it: `[ x ]`, `[ x… ]`, `[ x ▾ ]`, `< x >`, a plain link or an image's glyph and
    /// alt text, after its `[n]` while links are numbered.
    pub fn width(&self, number: Option<usize>) -> usize {
        let number = number.map_or(0, |n| n.to_string().len() + 2);
        match self {
            Self::Link {
                text, btn_type, button, ..
            } => {
                number
                    + display_width(text)
                    + if btn_type == "image" {
                        2
                    } else if *button {
                        4
                    } else {
                        0
                    }
            }
            Self::Button { text, .. } => display_width(text) + 4,
            Self::Dialog { text, .. } => number + display_width(text) + 5,
            Self::Menu { text, .. } => display_width(text) + 6,
        }
    }
}

/// Links and buttons as lines that each fit `width`, two columns apart, as many to a line as fit; one too wide for any
/// line has a line of its own and is cut when drawn.
fn wrap_inline(items: Vec<Inline>, ctx: &Ctx<'_>, width: u16) -> Vec<Vec<Inline>> {
    let width = usize::from(width);
    let mut lines: Vec<Vec<Inline>> = Vec::new();
    let mut used = 0;
    for item in items {
        let w = item.width(ctx.shown.link_number(item.id()));
        match lines.last_mut() {
            Some(line) if used + 2 + w <= width => {
                used += 2 + w;
                line.push(item);
            }
            _ => {
                used = w;
                lines.push(vec![item]);
            }
        }
    }
    lines
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidText {
    pub id: String,
    /// `h1` to `h6`, `pre`, `quote`, `li`, `rule`, `code`, or empty for plain text.
    pub style: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidList {
    pub id: String,
    pub name: String,
    pub columns: Vec<LaidCol>,
    /// Total width wider than the canvas: the first column stays and the rest scroll.
    pub scroll: bool,
    pub rows: usize,
    pub selection: bool,
    /// The lines of each row: more than one where a column stacks several fields.
    pub heights: Vec<usize>,
}

impl LaidList {
    pub fn height(&self) -> usize {
        // Header, rule, the rows (or the empty line), footer.
        2 + self.body_rows().max(1) + 1
    }

    /// The lines the rows take.
    pub fn body_rows(&self) -> usize {
        self.heights.iter().sum()
    }

    /// The line within the body where row `at` starts.
    pub fn row_top(&self, at: usize) -> usize {
        self.heights.iter().take(at).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidCol {
    pub title: String,
    pub width: usize,
    pub right: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaidTabs {
    pub id: String,
    pub items: Vec<(String, bool)>,
    /// Each tab's url, for a click.
    pub urls: Vec<String>,
}

impl Piece {
    pub fn height(&self) -> usize {
        match self {
            Self::Stack(pieces) => pieces.iter().map(Self::height).sum(),
            Self::Row(placed) => placed.iter().map(|p| p.piece.height()).max().unwrap_or(0),
            Self::Box(laid) => {
                if laid.collapsed {
                    2
                } else {
                    2 + usize::from(!laid.toolbar.is_empty()) + laid.body.iter().map(Self::height).sum::<usize>()
                }
            }
            Self::Form(form) => form.lines.iter().map(form_line_height).sum(),
            Self::Text(text) => text.lines.len(),
            Self::List(list) => list.height(),
            Self::Tree(tree) => tree.rows.len().max(1),
            Self::Lines(lines) => lines.lines.len().max(1),
            Self::Inline(_) | Self::Tabs(_) | Self::Status(_) | Self::Placeholder { .. } | Self::Error { .. } => 1,
        }
    }
}

fn form_line_height(line: &FormLine) -> usize {
    match line {
        FormLine::Field(field) => field.height(),
        FormLine::Cells(cells) => cells.iter().map(|(_, field)| field.height()).max().unwrap_or(1),
        FormLine::Buttons(_) | FormLine::Group(_) => 1,
        FormLine::Text(text) => text.lines.len(),
    }
}

/// Lay out the levels, outermost first, on a canvas of `canvas` columns.
pub fn layout_levels<'a>(
    models: impl IntoIterator<Item = &'a ScreenModel>,
    canvas: u16,
    breakpoints: &Breakpoints,
    shown: &dyn Shown,
) -> LaidOut {
    let ctx = Ctx::new(breakpoints.class(canvas), shown);
    let mut pieces = Vec::new();
    for model in models {
        pieces.extend(place_nodes(&model.c, &ctx, canvas));
    }
    LaidOut {
        class: ctx.class,
        canvas,
        pieces,
    }
}

/// Nodes on their own, as a dialog or the find dialog lays out its body.
pub fn layout_nodes(nodes: &[Node], width: u16, breakpoints: &Breakpoints, shown: &dyn Shown) -> LaidOut {
    let ctx = Ctx::new(breakpoints.class(width), shown);
    LaidOut {
        class: ctx.class,
        canvas: width,
        pieces: place_nodes(nodes, &ctx, width),
    }
}

/// One level.
pub fn layout(model: &ScreenModel, canvas: u16) -> LaidOut {
    layout_levels(std::iter::once(model), canvas, &Breakpoints::default(), &Plain)
}

#[derive(Clone, Copy)]
struct Ctx<'a> {
    class: WidthClass,
    mode: LabelMode,
    shown: &'a dyn Shown,
}

impl<'a> Ctx<'a> {
    fn new(class: WidthClass, shown: &'a dyn Shown) -> Self {
        Self {
            class,
            mode: if class == WidthClass::Xs {
                LabelMode::Above
            } else {
                LabelMode::Beside
            },
            shown,
        }
    }
}

fn place_nodes(nodes: &[Node], ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut index = 0;
    while index < nodes.len() {
        // Boxes in a row go side by side when the class and the 40-column rule allow.
        if matches!(nodes[index], Node::Box { .. }) {
            let start = index;
            while index < nodes.len() && matches!(nodes[index], Node::Box { .. }) {
                index += 1;
            }
            pieces.extend(place_boxes(&nodes[start..index], ctx, width));
            continue;
        }
        // Fields outside a form, one after another, line their labels up as one form does.
        if matches!(nodes[index], Node::Field { .. }) {
            let start = index;
            while index < nodes.len() && matches!(nodes[index], Node::Field { .. }) {
                index += 1;
            }
            pieces.push(Piece::Form(LaidForm {
                id: String::new(),
                name: String::new(),
                lines: form_lines(&nodes[start..index], ctx, width),
            }));
            continue;
        }
        // Links and dialog buttons next to each other share a line.
        if inline_of(&nodes[index]).is_some() {
            let mut line = Vec::new();
            while let Some(item) = nodes.get(index).and_then(inline_of) {
                line.push(item);
                index += 1;
            }
            pieces.extend(wrap_inline(line, ctx, width).into_iter().map(Piece::Inline));
            continue;
        }
        pieces.extend(place_one(&nodes[index], ctx, width));
        index += 1;
    }
    pieces
}

/// The id of reference link `n` (from 1) of an `html` or `markdown` node.
pub fn ref_id(node: &str, n: usize) -> String {
    format!("{node}#ref{n}")
}

/// The ids of a wizard's Back and Next buttons.
pub fn wizard_back(id: &str) -> String {
    format!("{id}#back")
}

pub fn wizard_next(id: &str) -> String {
    format!("{id}#next")
}

/// The steps of a wizard as status stages, and the active step.
pub fn wizard_steps(items: &[crate::model::SubscreenItem]) -> (Vec<StatusStage>, usize) {
    let active = items.iter().position(|item| item.active).unwrap_or(0);
    let stages = items
        .iter()
        .enumerate()
        .map(|(at, item)| StatusStage {
            status_id: item.url.clone(),
            label: item.title.clone(),
            state: match at.cmp(&active) {
                std::cmp::Ordering::Less => "done",
                std::cmp::Ordering::Equal => "current",
                std::cmp::Ordering::Greater => "todo",
            }
            .to_string(),
        })
        .collect();
    (stages, active)
}

fn inline_of(node: &Node) -> Option<Inline> {
    match node {
        Node::Link {
            id,
            text,
            kind,
            btn_type,
            disabled: false,
            ..
        } => Some(Inline::Link {
            id: id.clone(),
            text: text.clone(),
            btn_type: btn_type.clone(),
            // A link with a button type is a button, as a status bar's transitions are (style guide §4.6).
            button: kind.contains("button") || kind == "hidden-form" || !btn_type.is_empty(),
        }),
        Node::Dialog { id, button, title, .. } => Some(Inline::Dialog {
            id: id.clone(),
            text: if button.is_empty() {
                title.clone()
            } else {
                button.clone()
            },
        }),
        Node::DynamicDialog { id, button, title, .. } => Some(Inline::Dialog {
            id: id.clone(),
            text: if button.is_empty() {
                title.clone()
            } else {
                button.clone()
            },
        }),
        Node::ButtonMenu { id, text, .. } => Some(Inline::Menu {
            id: id.clone(),
            text: if text.is_empty() {
                "More".to_string()
            } else {
                text.clone()
            },
        }),
        Node::Image { id, alt, url } if !url.is_empty() => Some(Inline::Link {
            id: id.clone(),
            text: alt.clone(),
            btn_type: "image".to_string(),
            button: false,
        }),
        _ => None,
    }
}

fn place_one(node: &Node, ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    match node {
        Node::Row { c, .. } => place_row(c, ctx, width),
        Node::Section { c, .. }
        | Node::Container { c, .. }
        | Node::Fragment { c, .. }
        | Node::Active { c, .. }
        | Node::Region { c, .. }
        | Node::Col { c, .. }
        | Node::DynamicContainer { c, .. } => place_nodes(c, ctx, width),
        Node::Tree { id, .. } => vec![Piece::Tree(LaidTree {
            id: id.clone(),
            rows: ctx.shown.tree_rows(id),
        })],
        Node::Subscreen {
            id,
            kind,
            items,
            finish_text,
            c,
        } => place_subscreens(id, kind, items, finish_text, c, ctx, width),
        Node::Panel { c, .. } => place_panel(c, ctx, width),
        Node::Split { side_width, c, .. } => place_split(c, *side_width, ctx, width),
        Node::Dashboard { c, .. } => place_dashboard(c, ctx, width),
        Node::Box { .. } => place_boxes(std::slice::from_ref(node), ctx, width),
        Node::Form { id, name, c, .. } => vec![Piece::Form(LaidForm {
            id: id.clone(),
            name: name.clone(),
            lines: form_lines(c, ctx, width),
        })],
        Node::FormList { list } => {
            let mut pieces = vec![Piece::List(lay_list(list, width))];
            // The multi form's buttons, once under its rows.
            pieces.extend(place_nodes(&list.buttons, ctx, width));
            pieces.extend(place_nodes(&list.c, ctx, width));
            pieces
        }
        Node::Field { .. } => {
            // A field outside a form: laid as a one-field form so it draws the same way.
            vec![Piece::Form(LaidForm {
                id: String::new(),
                name: String::new(),
                lines: form_lines(std::slice::from_ref(node), ctx, width),
            })]
        }
        Node::Text { id, text, style } => vec![Piece::Text(text_piece(id, text, style, width))],
        Node::Markdown { id, text } => {
            let (blocks, links) = crate::markdown::blocks(text);
            let mut pieces = html_pieces(id, &blocks, width);
            pieces.extend(references(id, &links, width));
            pieces
        }
        Node::Html { id, blocks, links } => {
            let mut pieces = html_pieces(id, blocks, width);
            pieces.extend(references(id, links, width));
            pieces
        }
        Node::NumberCard { id, title, value, url } => {
            let body = if url.is_empty() {
                Piece::Text(LaidText {
                    id: format!("{id}.value"),
                    style: "h3".to_string(),
                    lines: vec![value.clone()],
                })
            } else {
                // The value is the card's link: Enter follows it.
                Piece::Inline(vec![Inline::Link {
                    id: id.clone(),
                    text: value.clone(),
                    btn_type: String::new(),
                    button: false,
                }])
            };
            vec![Piece::Box(LaidBox {
                id: format!("{id}.card"),
                title: title.clone(),
                kind: String::new(),
                w: width.min(30),
                toolbar: Vec::new(),
                body: vec![body],
                collapsible: false,
                collapsed: false,
            })]
        }
        Node::StatusBar { id, text, stages, c } => {
            let mut pieces = vec![Piece::Status(LaidStatus {
                id: id.clone(),
                text: text.clone(),
                stages: stages.clone(),
            })];
            pieces.extend(place_nodes(c, ctx, width));
            pieces
        }
        Node::Image { id, alt, .. } => vec![Piece::Text(LaidText {
            id: id.clone(),
            style: "image".to_string(),
            lines: vec![alt.clone()],
        })],
        Node::Component { id, name, props, .. } => {
            match ctx
                .shown
                .component_lines(id, name, props, width, ctx.class >= WidthClass::Lg)
            {
                Some(lines) => vec![Piece::Lines(LaidLines { id: id.clone(), lines })],
                None => vec![Piece::Placeholder {
                    id: id.clone(),
                    text: format!("The {name} component, which this client does not have,"),
                    url: String::new(),
                }],
            }
        }
        Node::Placeholder { id, text, url } => vec![Piece::Placeholder {
            id: id.clone(),
            text: text.clone(),
            url: url.clone(),
        }],
        Node::Error { id, text } => wrap(text, usize::from(width).saturating_sub(2))
            .into_iter()
            .map(|line| Piece::Error {
                id: id.clone(),
                text: line,
            })
            .collect(),
        Node::Link { .. } | Node::Dialog { .. } | Node::DynamicDialog { .. } | Node::ButtonMenu { .. } => {
            inline_of(node)
                .map(|item| vec![Piece::Inline(vec![item])])
                .unwrap_or_default()
        }
    }
}

/// Tabs over the active subscreen; a wizard's steps with Back and Next; a stack's trail. A popup panel shows no bar: its
/// siblings are in the drawer.
fn place_subscreens(
    id: &str,
    kind: &str,
    items: &[crate::model::SubscreenItem],
    finish_text: &str,
    c: &[Node],
    ctx: &Ctx<'_>,
    width: u16,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    match kind {
        "wizard" if !items.is_empty() => {
            let (stages, active) = wizard_steps(items);
            let title = items.get(active).map(|item| item.title.as_str()).unwrap_or("");
            pieces.push(Piece::Status(LaidStatus {
                id: id.to_string(),
                text: format!("Step {} of {} · {title}", active + 1, items.len()),
                stages,
            }));
            pieces.extend(place_nodes(c, ctx, width));
            let mut buttons = Vec::new();
            if active > 0 {
                buttons.push(Inline::Link {
                    id: wizard_back(id),
                    text: "Back".to_string(),
                    btn_type: String::new(),
                    button: true,
                });
            }
            let last = active + 1 == items.len();
            if !last || !finish_text.is_empty() {
                buttons.push(Inline::Link {
                    id: wizard_next(id),
                    text: if last {
                        finish_text.to_string()
                    } else {
                        "Next".to_string()
                    },
                    btn_type: "primary".to_string(),
                    button: true,
                });
            }
            if !buttons.is_empty() {
                pieces.extend(wrap_inline(buttons, ctx, width).into_iter().map(Piece::Inline));
            }
        }
        "stack" if !items.is_empty() => {
            let active = items.iter().position(|item| item.active).unwrap_or(items.len() - 1);
            let trail: Vec<&str> = items[..=active].iter().map(|item| item.title.as_str()).collect();
            pieces.push(Piece::Text(LaidText {
                id: id.to_string(),
                style: "crumbs".to_string(),
                lines: vec![truncate_width(&trail.join(" ▸ "), usize::from(width))],
            }));
            pieces.extend(place_nodes(c, ctx, width));
        }
        _ => {
            if kind != "popup" && items.len() > 1 {
                pieces.push(Piece::Tabs(LaidTabs {
                    id: id.to_string(),
                    items: items.iter().map(|item| (item.title.clone(), item.active)).collect(),
                    urls: items.iter().map(|item| item.url.clone()).collect(),
                }));
            }
            pieces.extend(place_nodes(c, ctx, width));
        }
    }
    pieces
}

/// The numbered links of an `html` or `markdown` node, at its end as Lynx lists References: each one a link to follow.
fn references(id: &str, links: &[HtmlLink], width: u16) -> Vec<Piece> {
    if links.is_empty() {
        return Vec::new();
    }
    let mut pieces = vec![Piece::Text(LaidText {
        id: format!("{id}#refs"),
        style: "h6".to_string(),
        lines: vec!["References".to_string()],
    })];
    for (at, link) in links.iter().enumerate() {
        let text = if link.text.is_empty() { &link.url } else { &link.text };
        pieces.push(Piece::Inline(vec![Inline::Link {
            id: ref_id(id, at + 1),
            text: truncate_width(&format!("{}. {text}", at + 1), usize::from(width)),
            btn_type: String::new(),
            button: false,
        }]));
    }
    pieces
}

fn text_piece(id: &str, text: &str, style: &str, width: u16) -> LaidText {
    let width = usize::from(width);
    let lines = if style == "pre" || style == "code" {
        text.lines().map(|line| truncate_width(line, width)).collect()
    } else {
        text.split('\n').flat_map(|line| wrap(line, width)).collect()
    };
    LaidText {
        id: id.to_string(),
        style: style.to_string(),
        lines,
    }
}

fn html_pieces(id: &str, blocks: &[HtmlBlock], width: u16) -> Vec<Piece> {
    let width = usize::from(width);
    blocks
        .iter()
        .enumerate()
        .map(|(at, block)| {
            let indent = usize::from(block.depth) * 2;
            let room = width.saturating_sub(indent + 2).max(10);
            let lead = match block.kind.as_str() {
                "li" => format!("{}{} ", " ".repeat(indent.saturating_sub(2)), block.marker),
                "quote" => format!("{}│ ", " ".repeat(indent.saturating_sub(2))),
                _ => " ".repeat(indent),
            };
            let pad = " ".repeat(display_width(&lead));
            let mut lines = Vec::new();
            if block.kind == "rule" {
                lines.push("─".repeat(width.min(40)));
            } else if block.kind == "pre" {
                lines.extend(
                    block
                        .text
                        .lines()
                        .map(|line| truncate_width(&format!("{lead}{line}"), width)),
                );
            } else {
                for (n, line) in block.text.split('\n').flat_map(|line| wrap(line, room)).enumerate() {
                    lines.push(format!("{}{line}", if n == 0 { &lead } else { &pad }));
                }
            }
            Piece::Text(LaidText {
                id: format!("{id}.{at}"),
                style: block.kind.clone(),
                lines,
            })
        })
        .collect()
}

fn place_boxes(nodes: &[Node], ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let slots = box_slots(nodes.len(), ctx.class, width);
    let boxes: Vec<(u16, u16, Piece)> = nodes
        .iter()
        .zip(slots)
        .filter_map(|(node, (x, w))| {
            let Node::Box {
                id,
                title,
                kind,
                initial,
                c,
            } = node
            else {
                return None;
            };
            let collapsed = !initial.is_empty() && ctx.shown.collapsed(id, initial);
            let inner = w.saturating_sub(4);
            let mut toolbar = Vec::new();
            let mut body_nodes = Vec::new();
            for child in c {
                match child {
                    Node::Region { name, c, .. } if name == "toolbar" => {
                        toolbar.extend(c.iter().filter_map(inline_of));
                    }
                    other => body_nodes.push(other.clone()),
                }
            }
            Some((
                x,
                w,
                Piece::Box(LaidBox {
                    id: id.clone(),
                    title: title.clone(),
                    kind: kind.clone(),
                    w,
                    toolbar,
                    body: if collapsed {
                        Vec::new()
                    } else {
                        place_nodes(&body_nodes, ctx, inner)
                    },
                    collapsible: !initial.is_empty(),
                    collapsed,
                }),
            ))
        })
        .collect();
    if boxes.iter().any(|(x, _, _)| *x > 0) {
        vec![Piece::Row(
            boxes.into_iter().map(|(x, w, piece)| Placed { x, w, piece }).collect(),
        )]
    } else {
        boxes.into_iter().map(|(_, _, piece)| piece).collect()
    }
}

fn box_slots(count: usize, class: WidthClass, canvas: u16) -> Vec<(u16, u16)> {
    if count == 0 {
        return Vec::new();
    }
    let count_u = u16::try_from(count).unwrap_or(u16::MAX);
    if !class.stacks_boxes() && count > 1 {
        let gutters = count_u.saturating_sub(1);
        let inner = canvas.saturating_sub(gutters);
        let share = inner / count_u;
        if share >= BOX_MIN {
            let remainder = inner - share * count_u;
            let mut slots = Vec::with_capacity(count);
            let mut x: u16 = 0;
            for index in 0..count_u {
                let w = if index + 1 == count_u { share + remainder } else { share };
                slots.push((x, w));
                x = x.saturating_add(w).saturating_add(1);
            }
            return slots;
        }
    }
    vec![(0, canvas); count]
}

fn place_row(children: &[Node], ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let columns: Vec<&Node> = children
        .iter()
        .filter(|node| matches!(node, Node::Col { .. }))
        .collect();
    if columns.is_empty() {
        return place_nodes(children, ctx, width);
    }
    let spans: Vec<Option<u16>> = columns.iter().map(|node| span_at(node, ctx.class)).collect();
    let mut pieces = Vec::new();
    for line in span_lines(&spans) {
        pieces.extend(place_line(&spans[line.clone()], &columns[line], ctx, width));
    }
    pieces
}

/// Splits the columns into lines as Bootstrap wraps them: a column whose span would take its line past 12 starts the
/// next line.
fn span_lines(spans: &[Option<u16>]) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut used = 0u16;
    for (index, span) in spans.iter().enumerate() {
        let span = span.unwrap_or(0).min(12);
        if index > start && used + span > 12 {
            lines.push(start..index);
            start = index;
            used = 0;
        }
        used += span;
    }
    lines.push(start..spans.len());
    lines
}

/// A column and its width.
type Sized<'a> = (&'a Node, u16);

/// One line of columns. Below md they stack. A column narrower than 40 is not squeezed: the others stay a row, and it
/// follows on a line of its own at full width.
fn place_line(spans: &[Option<u16>], columns: &[&Node], ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let body = |node: &Node, w: u16| place_nodes(node.children(), ctx, w);
    if ctx.class.stacks_boxes() || columns.len() == 1 {
        return columns.iter().flat_map(|node| body(node, width)).collect();
    }
    let widths = span_widths(spans, width);
    let (wide, narrow): (Vec<Sized<'_>>, Vec<Sized<'_>>) =
        columns.iter().copied().zip(widths).partition(|(_, w)| *w >= BOX_MIN);
    let mut pieces = Vec::new();
    if let [(node, _)] = wide.as_slice() {
        pieces.extend(body(node, width));
    } else if !wide.is_empty() {
        let mut x = 0u16;
        let row = wide
            .into_iter()
            .map(|(node, w)| {
                let placed = Placed {
                    x,
                    w,
                    piece: Piece::Stack(body(node, w)),
                };
                x = x.saturating_add(w).saturating_add(1);
                placed
            })
            .collect();
        pieces.push(Piece::Row(row));
    }
    for (node, _) in narrow {
        pieces.extend(body(node, width));
    }
    pieces
}

fn span_at(node: &Node, class: WidthClass) -> Option<u16> {
    let Node::Col { xs, sm, md, lg, .. } = node else {
        return None;
    };
    let filled = |text: &str| (!text.is_empty()).then(|| text.to_string());
    let text = match class {
        WidthClass::Xs => filled(xs),
        WidthClass::Sm => filled(sm).or_else(|| filled(xs)),
        WidthClass::Md => filled(md).or_else(|| filled(sm)).or_else(|| filled(xs)),
        WidthClass::Lg | WidthClass::Xl => filled(lg)
            .or_else(|| filled(md))
            .or_else(|| filled(sm))
            .or_else(|| filled(xs)),
    }?;
    text.parse::<u16>().ok().filter(|span| *span > 0)
}

fn span_widths(spans: &[Option<u16>], canvas: u16) -> Vec<u16> {
    let count = u16::try_from(spans.len()).unwrap_or(u16::MAX);
    let inner = canvas.saturating_sub(count.saturating_sub(1));
    let specified: u32 = spans.iter().copied().flatten().map(u32::from).sum();
    let flex_count = u16::try_from(spans.iter().filter(|span| span.is_none()).count()).unwrap_or(0);
    let specified_width = inner.saturating_mul(u16::try_from(specified.min(12)).unwrap_or(12)) / 12;
    let flex_total = inner.saturating_sub(specified_width);
    let flex_share = flex_total.checked_div(flex_count).unwrap_or(0);
    let mut flex_used = 0u16;
    let mut flex_seen = 0u16;
    spans
        .iter()
        .map(|span| match span {
            Some(span) => inner.saturating_mul((*span).min(12)) / 12,
            None => {
                flex_seen += 1;
                if flex_seen == flex_count {
                    flex_total.saturating_sub(flex_used)
                } else {
                    flex_used = flex_used.saturating_add(flex_share);
                    flex_share
                }
            }
        })
        .collect()
}

fn region<'a>(children: &'a [Node], name: &str) -> Option<(&'a [Node], u32)> {
    children.iter().find_map(|node| match node {
        Node::Region {
            name: found, size, c, ..
        } if found == name => Some((c.as_slice(), *size)),
        _ => None,
    })
}

/// A panel: header, the left/center/right columns side by side from md (a side's size in pixels ÷ 8), and footer. Below
/// md everything stacks, since a terminal cannot scroll the page sideways.
fn place_panel(children: &[Node], ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let mut pieces = Vec::new();
    if let Some((header, _)) = region(children, "header") {
        pieces.extend(place_nodes(header, ctx, width));
    }
    let left = region(children, "left");
    let center = region(children, "center");
    let right = region(children, "right");
    if ctx.class.stacks_boxes() {
        for part in [left, center, right].into_iter().flatten() {
            pieces.extend(place_nodes(part.0, ctx, width));
        }
    } else {
        let side = |part: Option<(&[Node], u32)>| {
            part.map(|(_, size)| {
                u16::try_from(if size == 0 { 180 } else { size } / 8)
                    .unwrap_or(22)
                    .clamp(12, width / 3)
            })
        };
        let lw = side(left).unwrap_or(0);
        let rw = side(right).unwrap_or(0);
        let gaps = u16::from(left.is_some()) + u16::from(right.is_some());
        let cw = width.saturating_sub(lw + rw + gaps);
        let mut row = Vec::new();
        let mut x = 0;
        for (part, w) in [(left, lw), (center, cw), (right, rw)] {
            if let Some((nodes, _)) = part {
                row.push(Placed {
                    x,
                    w,
                    piece: Piece::Stack(place_nodes(nodes, ctx, w)),
                });
                x += w + 1;
            }
        }
        if !row.is_empty() {
            pieces.push(Piece::Row(row));
        }
    }
    if let Some((footer, _)) = region(children, "footer") {
        pieces.extend(place_nodes(footer, ctx, width));
    }
    pieces
}

/// A split: side and main together from md, the side `side-width` pixels ÷ 8 wide; below md the side list, then main.
fn place_split(children: &[Node], side_width: u32, ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let side = region(children, "side").map(|(nodes, _)| nodes).unwrap_or(&[]);
    let main = region(children, "main").map(|(nodes, _)| nodes).unwrap_or(&[]);
    if ctx.class.stacks_boxes() {
        let mut pieces = place_nodes(side, ctx, width);
        pieces.extend(place_nodes(main, ctx, width));
        return pieces;
    }
    let sw = u16::try_from(if side_width == 0 { 280 } else { side_width } / 8)
        .unwrap_or(35)
        .clamp(20, width / 2);
    let mw = width.saturating_sub(sw + 1);
    vec![Piece::Row(vec![
        Placed {
            x: 0,
            w: sw,
            piece: Piece::Stack(place_nodes(side, ctx, sw)),
        },
        Placed {
            x: sw + 1,
            w: mw,
            piece: Piece::Stack(place_nodes(main, ctx, mw)),
        },
    ])]
}

/// Cards of a dashboard in rows, about 24 columns each.
fn place_dashboard(children: &[Node], ctx: &Ctx<'_>, width: u16) -> Vec<Piece> {
    let per_row = usize::from((width / 25).max(1));
    let card_w = (width / u16::try_from(per_row).unwrap_or(1)).saturating_sub(1).max(10);
    let mut pieces = Vec::new();
    for chunk in children.chunks(per_row) {
        let row: Vec<Placed> = chunk
            .iter()
            .enumerate()
            .map(|(at, node)| Placed {
                x: u16::try_from(at).unwrap_or(0) * (card_w + 1),
                w: card_w,
                piece: Piece::Stack(place_one(node, ctx, card_w)),
            })
            .collect();
        pieces.push(Piece::Row(row));
    }
    pieces
}

// ---- forms ----

fn form_lines(nodes: &[Node], ctx: &Ctx<'_>, width: u16) -> Vec<FormLine> {
    let mut titles = Vec::new();
    crate::model::walk(nodes, &mut |node| {
        if let Node::Field { title, w, required, .. } = node {
            if !matches!(w, Widget::Submit { .. } | Widget::Reset { .. } | Widget::Hidden { .. }) {
                // The required mark is part of the label.
                titles.push(if *required { format!("* {title}") } else { title.clone() });
            }
        }
    });
    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let label_width = label_column_width(&refs, ctx.class);
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    collect_form(nodes, ctx, width, label_width, &mut lines, &mut buttons);
    if !buttons.is_empty() {
        lines.extend(wrap_inline(buttons, ctx, width).into_iter().map(FormLine::Buttons));
    }
    lines
}

fn collect_form(
    nodes: &[Node],
    ctx: &Ctx<'_>,
    width: u16,
    label_width: usize,
    lines: &mut Vec<FormLine>,
    buttons: &mut Vec<Inline>,
) {
    let mut primary_seen = false;
    for node in nodes {
        match node {
            Node::Field { id, title, w, .. } => match w {
                Widget::Submit { text, .. } => {
                    buttons.push(Inline::Button {
                        id: id.clone(),
                        text: if text.is_empty() { title.clone() } else { text.clone() },
                        primary: !primary_seen,
                    });
                    primary_seen = true;
                }
                Widget::Reset { text } => buttons.push(Inline::Button {
                    id: id.clone(),
                    text: if text.is_empty() { title.clone() } else { text.clone() },
                    primary: false,
                }),
                Widget::Hidden { .. } => {}
                _ => lines.push(FormLine::Field(lay_field(id, title, w, ctx.mode, label_width, width))),
            },
            Node::Row { c, .. } => {
                let fields: Vec<&Node> = c.iter().filter(|n| matches!(n, Node::Field { .. })).collect();
                let count = u16::try_from(fields.len().max(1)).unwrap_or(1);
                let cell_w = width.saturating_sub(count - 1) / count;
                let fits = fields.len() > 1
                    && ctx.mode == LabelMode::Beside
                    && usize::from(cell_w) >= label_width + 3 + CELL_VALUE_MIN;
                if fits {
                    let cells = fields
                        .iter()
                        .enumerate()
                        .filter_map(|(at, node)| match node {
                            Node::Field { id, title, w, .. } => Some((
                                u16::try_from(at).unwrap_or(0) * (cell_w + 1),
                                lay_field(id, title, w, ctx.mode, label_width, cell_w),
                            )),
                            _ => None,
                        })
                        .collect();
                    lines.push(FormLine::Cells(cells));
                } else {
                    collect_form(c, ctx, width, label_width, lines, buttons);
                }
            }
            Node::Box { title, c, .. } => {
                if !title.is_empty() {
                    lines.push(FormLine::Group(title.clone()));
                }
                collect_form(c, ctx, width, label_width, lines, buttons);
            }
            Node::Text { id, text, style } => lines.push(FormLine::Text(text_piece(id, text, style, width))),
            Node::Link { .. } | Node::Dialog { .. } => buttons.extend(inline_of(node)),
            other => collect_form(other.children(), ctx, width, label_width, lines, buttons),
        }
    }
}

fn lay_field(id: &str, title: &str, widget: &Widget, mode: LabelMode, label_width: usize, width: u16) -> LaidField {
    let room = match mode {
        LabelMode::Beside => usize::from(width).saturating_sub(label_width + 4),
        LabelMode::Above => usize::from(width).saturating_sub(2),
    }
    .max(8);
    let natural = match widget {
        Widget::TextLine { size, .. } if *size > 0 => usize::try_from(*size).unwrap_or(VALUE_DEFAULT) + 2,
        Widget::TextArea { .. } => room,
        Widget::DateTime { .. } => 20,
        Widget::DateFind { .. } | Widget::DatePeriod { .. } => 30,
        Widget::RangeFind { .. } => 24,
        Widget::Check { options, .. } | Widget::Radio { options, .. } => options
            .iter()
            .map(|choice| display_width(&choice.label) + 6)
            .sum::<usize>()
            .max(4),
        Widget::DropDown { options, .. } => {
            options
                .iter()
                .map(|choice| display_width(&choice.label))
                .max()
                .unwrap_or(10)
                .clamp(10, 40)
                + 4
        }
        Widget::Display { text, .. } | Widget::Label { text, .. } => display_width(text).max(1),
        _ => VALUE_DEFAULT + 2,
    };
    let rows = match widget {
        Widget::TextArea { .. } => text_area_rows(widget),
        Widget::Display { text, .. } => wrap(text, room).len().max(1),
        _ => 1,
    };
    LaidField {
        id: id.to_string(),
        title: match mode {
            LabelMode::Above => truncate_width(title, usize::from(width)),
            LabelMode::Beside => title.to_string(),
        },
        label_width,
        mode,
        value_width: natural.min(room),
        rows,
    }
}

// ---- lists ----

fn lay_list(list: &FormList, canvas: u16) -> LaidList {
    let mut columns = Vec::new();
    let mut total = 0usize;
    let selection = list.row_selection.is_some();
    let gutter = 2 + if selection { 4 } else { 0 };
    for (index, column) in list.columns.iter().enumerate() {
        let printed = column
            .print_width
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|width| *width > 0);
        let width = match (printed, column.print_width_type.as_str()) {
            (Some(percent), "percent") => (usize::from(canvas) * percent / 100).max(display_width(&column.title)),
            (Some(chars), _) => chars,
            (None, _) => {
                let mut width = display_width(&column.title).min(COL_CAP);
                for row in &list.rows {
                    if let Some(cell) = row.cells.get(index) {
                        for part in cell.parts() {
                            width = width.max(cell_width(part).min(COL_CAP));
                        }
                    }
                }
                width
            }
        };
        if index > 0 {
            total += 1;
        }
        total += width;
        columns.push(LaidCol {
            title: column.title.clone(),
            width,
            right: column.align == "right",
        });
    }
    LaidList {
        id: list.id.clone(),
        name: list.name.clone(),
        columns,
        scroll: total + gutter > usize::from(canvas),
        rows: list.rows.len(),
        selection,
        heights: list
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.parts().len())
                    .max()
                    .unwrap_or(1)
                    .max(1)
            })
            .collect(),
    }
}

/// The columns a cell part takes: its text, or an input of a row form, which keeps room to type.
fn cell_width(part: &Cell) -> usize {
    match part {
        Cell::Node(node) => match node.as_ref() {
            Node::Field { w, .. } => match w {
                Widget::Submit { text, .. } | Widget::Reset { text } => display_width(text) + 4,
                Widget::Display { text, .. } | Widget::Label { text, .. } | Widget::Editable { text, .. } => {
                    display_width(text)
                }
                Widget::DropDown { options, value, .. } => {
                    let label = options
                        .iter()
                        .find(|choice| &choice.value == value)
                        .map_or(value.as_str(), |choice| choice.label.as_str());
                    display_width(label).max(10) + 4
                }
                Widget::Check { .. } => 3,
                other => display_width(other.value()).max(10) + 2,
            },
            Node::DynamicDialog { button, title, .. } | Node::Dialog { button, title, .. } => {
                display_width(if button.is_empty() { title } else { button }) + 5
            }
            _ => display_width(part.text()),
        },
        _ => display_width(part.text()),
    }
}

/// The rows a text area shows: its `rows` (2 to 12), or a read-only one's lines up to 12. A longer text scrolls.
pub fn text_area_rows(widget: &Widget) -> usize {
    match widget {
        Widget::TextArea {
            rows, value, read_only, ..
        } => {
            if *read_only {
                value.lines().count().clamp(1, 12)
            } else {
                usize::try_from(*rows).unwrap_or(3).clamp(2, 12)
            }
        }
        _ => 1,
    }
}

// ---- text ----

pub fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

pub fn char_width(c: char) -> usize {
    let cp = c as u32;
    if (0x0300..=0x036F).contains(&cp) || cp == 0x200D || (0xFE00..=0xFE0F).contains(&cp) {
        return 0;
    }
    if is_wide(cp) {
        return 2;
    }
    1
}

fn is_wide(cp: u32) -> bool {
    (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE10..=0xFE19).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F300..=0x1F64F).contains(&cp)
        || (0x1F680..=0x1F6FF).contains(&cp)
        || (0x1F900..=0x1F9FF).contains(&cp)
        || (0x1FA00..=0x1FAFF).contains(&cp)
        || (0x20000..=0x3FFFD).contains(&cp)
}

/// At most `max` columns; a cut text ends in `…`.
pub fn truncate_width(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(text) <= max {
        return text.to_string();
    }
    let keep = max - 1;
    let mut width = 0;
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        let next = char_width(ch);
        if width + next > keep {
            break;
        }
        width += next;
        end = index + ch.len_utf8();
    }
    let mut out = text[..end].to_string();
    out.push('…');
    out
}

/// Words to lines of at most `width` columns. A word longer than a line is cut.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word_w = display_width(word);
        let line_w = display_width(&line);
        if line.is_empty() {
            line = truncate_width(word, width);
        } else if line_w + 1 + word_w <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line = truncate_width(word, width);
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// A text outline of the layout, for tests: one line per piece with its place and size.
pub fn outline(model: &ScreenModel, canvas: u16) -> String {
    outline_of(&layout(model, canvas))
}

pub fn outline_of(laid: &LaidOut) -> String {
    let mut out = format!("class={} canvas={}\n", laid.class.name(), laid.canvas);
    for piece in &laid.pieces {
        write_piece(&mut out, piece, "");
    }
    out
}

fn write_piece(out: &mut String, piece: &Piece, indent: &str) {
    let nested = format!("{indent}  ");
    match piece {
        Piece::Stack(pieces) => pieces.iter().for_each(|piece| write_piece(out, piece, indent)),
        Piece::Row(placed) => {
            out.push_str(&format!("{indent}row\n"));
            for item in placed {
                out.push_str(&format!("{indent}col x={} w={}\n", item.x, item.w));
                write_piece(out, &item.piece, &nested);
            }
        }
        Piece::Box(laid) => {
            let closed = if laid.collapsed { " closed" } else { "" };
            out.push_str(&format!("{indent}box {} w={}{closed}\n", laid.title, laid.w));
            if !laid.toolbar.is_empty() {
                let ids: Vec<&str> = laid.toolbar.iter().map(Inline::id).collect();
                out.push_str(&format!("{nested}toolbar {}\n", ids.join(" ")));
            }
            laid.body.iter().for_each(|piece| write_piece(out, piece, &nested));
        }
        Piece::Form(form) => {
            out.push_str(&format!("{indent}form {}\n", form.name));
            for line in &form.lines {
                match line {
                    FormLine::Field(field) => out.push_str(&format!(
                        "{nested}field {} {} label={} value={}\n",
                        field.title,
                        field.mode.name(),
                        field.label_width,
                        field.value_width
                    )),
                    FormLine::Cells(cells) => {
                        let parts: Vec<String> = cells.iter().map(|(x, f)| format!("{}@{x}", f.title)).collect();
                        out.push_str(&format!("{nested}cells {}\n", parts.join(" ")));
                    }
                    FormLine::Buttons(items) => {
                        let ids: Vec<&str> = items.iter().map(Inline::id).collect();
                        out.push_str(&format!("{nested}buttons {}\n", ids.join(" ")));
                    }
                    FormLine::Group(title) => out.push_str(&format!("{nested}group {title}\n")),
                    FormLine::Text(text) => out.push_str(&format!("{nested}text {}\n", text.lines.join(" / "))),
                }
            }
        }
        Piece::Inline(items) => {
            let ids: Vec<&str> = items.iter().map(Inline::id).collect();
            out.push_str(&format!("{indent}inline {}\n", ids.join(" ")));
        }
        Piece::Text(text) => out.push_str(&format!("{indent}text {}\n", text.lines.join(" / "))),
        Piece::List(list) => {
            out.push_str(&format!(
                "{indent}list {} scroll={} rows={}\n",
                list.name, list.scroll, list.rows
            ));
            for col in &list.columns {
                let right = if col.right { " right" } else { "" };
                out.push_str(&format!("{nested}col {} w={}{right}\n", col.title, col.width));
            }
        }
        Piece::Tabs(tabs) => {
            let items: Vec<String> = tabs
                .items
                .iter()
                .map(|(title, active)| if *active { format!("[{title}]") } else { title.clone() })
                .collect();
            out.push_str(&format!("{indent}tabs {}\n", items.join(" ")));
        }
        Piece::Tree(tree) => {
            out.push_str(&format!("{indent}tree {} rows={}\n", tree.id, tree.rows.len()));
        }
        Piece::Status(status) => {
            let stages: Vec<String> = status
                .stages
                .iter()
                .map(|stage| format!("{}:{}", stage.label, stage.state))
                .collect();
            out.push_str(&format!("{indent}status {} {}\n", status.text, stages.join(" ")));
        }
        Piece::Lines(lines) => {
            for line in &lines.lines {
                out.push_str(&format!("{indent}line {}\n", crate::components::line_text(line)));
            }
        }
        Piece::Placeholder { text, .. } => out.push_str(&format!("{indent}placeholder {text}\n")),
        Piece::Error { text, .. } => out.push_str(&format!("{indent}error {text}\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_and_a_long_word_is_cut() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abc…"]);
        assert_eq!(wrap("", 4), [""]);
    }

    #[test]
    fn breakpoints_follow_the_theme_and_refuse_a_bad_order() {
        let (bp, skipped) = Breakpoints::with(&BTreeMap::from([("md".to_string(), 110)]));
        assert_eq!(bp.class(115), WidthClass::Md);
        assert!(skipped.is_empty());
        let (bp, skipped) = Breakpoints::with(&BTreeMap::from([("md".to_string(), 300)]));
        assert_eq!(bp, Breakpoints::default());
        assert_eq!(skipped.len(), 1);
    }
}
