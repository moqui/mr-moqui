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

//! The frame of section 1.2 and the canvas: the title line, the drawer, the levels, the message line and the key line.
//! Every widget follows the vocabulary of section 1.4, and every color cue has a glyph or text beside it.

use std::cell::Cell as Shared;

use revue::layout::Rect;
use revue::widget::RenderContext;

use crate::components::Line;
use crate::forms::FocusKind;
use crate::keys::Action;
use crate::layout::{
    display_width, dotted_label, frame_with, layout_levels, truncate_width, Breakpoints, FormLine, Frame, Inline,
    LabelMode, LaidBox, LaidField, LaidList, LaidOut, LaidStatus, LaidTree, Piece, TreeMark, WidthClass,
};
use crate::model::{Cell, Node, Widget};
use crate::session::Session;
use crate::theme::{Glyph, Role, Theme};

use super::overlays;
use super::paint::{Hits, Look, Painter, Spots, Target};

/// What the view keeps between frames that a frame reads and writes: the canvas scroll (free while the wheel moved
/// it), and the click areas it records.
pub struct Kept<'a> {
    pub scroll: &'a Shared<usize>,
    pub free_scroll: bool,
    pub hits: &'a mut Hits,
}

/// Draw the whole screen.
pub fn draw(
    session: &Session,
    theme: &Theme,
    breakpoints: &Breakpoints,
    kept: Kept,
    clock: Option<&str>,
    log: Option<&std::collections::VecDeque<String>>,
    ctx: &mut RenderContext,
) {
    let Kept {
        scroll,
        free_scroll,
        hits,
    } = kept;
    let width = ctx.area.width;
    let height = ctx.area.height;
    if width < 10 || height < 3 {
        return;
    }
    // The Assist pane: a right column while the canvas beside it stays md, else over the canvas while it has the keys.
    let assist = crate::assist::draw::place(session, width, breakpoints);
    let pane_columns = assist.as_ref().filter(|at| at.docked).map_or(0, |at| at.width);
    let mut framed = frame_with(width - pane_columns, breakpoints, session.drawer_choice());
    framed.terminal = width;
    let roomy = height >= 10;
    let (body_top, mut body_bottom) = if roomy {
        (2, height - 3)
    } else {
        (1, height.saturating_sub(2))
    };
    // The log pane: a bottom split of 10 rows over the message line (section 1.2).
    let pane = log
        .filter(|_| body_bottom > body_top + 14)
        .map(|lines| (body_bottom - 10, lines));
    if let Some((top, _)) = pane {
        body_bottom = top;
    }
    let mut spots = Spots::new();
    let mut paint = Painter {
        ctx,
        theme,
        top: body_top,
        bottom: body_bottom,
        scroll: 0,
        left: 0,
        right: width,
        spots: &mut spots,
        hits,
        aim: None,
    };
    title_line(&mut paint, session, &framed, clock);
    if roomy {
        rule(&mut paint, 1, width, &framed, Joint::Top);
        rule(&mut paint, height - 3, width, &framed, Joint::Bottom);
        if let Some(at) = assist.as_ref().filter(|at| at.x > 0) {
            let (top, bottom) = if paint.theme.ascii { ("+", "+") } else { ("┬", "┴") };
            paint.fixed(at.x, 1, top, Look::role(Role::Border), 1);
            paint.fixed(at.x, height - 3, bottom, Look::role(Role::Border), 1);
        }
    }
    let canvas_x = framed.drawer;
    let laid = layout_levels(
        session.nav().models(),
        framed.canvas.saturating_sub(2),
        breakpoints,
        session,
    );
    let body_rows = usize::from(body_bottom.saturating_sub(body_top));
    let target = focus_rows(&laid, session);
    let mut top = scroll.get().min(laid.height().saturating_sub(1));
    // The wheel scrolled: the canvas stays where it went, no further than its last page, until a key or a click.
    let target = if free_scroll {
        top = top.min(laid.height().saturating_sub(body_rows));
        None
    } else {
        target
    };
    if let Some((y, h)) = target {
        if y < top {
            top = y;
        } else if y + h > top + body_rows {
            top = (y + h).saturating_sub(body_rows);
        }
    }
    scroll.set(top);
    paint.scroll = top;
    paint.left = canvas_x + 1;
    paint.right = (width - pane_columns).saturating_sub(1).max(canvas_x + 2);
    let mut y = 0;
    for piece in &laid.pieces {
        draw_piece(&mut paint, session, piece, canvas_x + 1, y);
        y += piece.height();
    }
    paint.left = 0;
    paint.right = width;
    if let Some((top, lines)) = pane {
        log_pane(&mut paint, top, top + 10, width, lines);
    }
    if framed.drawer > 0 || session.drawer_focus().is_some() {
        drawer(&mut paint, session, &framed, body_top, body_bottom);
    }
    if let Some(at) = &assist {
        crate::assist::draw::pane(&mut paint, session, at, body_top, body_bottom);
    }
    if height >= 2 {
        message_line(&mut paint, session, height - 2, width);
        key_line(&mut paint, session, height - 1, width);
    }
    // The overlays keep to the canvas left of the pane.
    let canvas_width = assist
        .as_ref()
        .map_or(framed.canvas, |at| at.x.saturating_sub(canvas_x).max(10));
    let canvas = Rect::new(canvas_x, body_top, canvas_width, body_bottom - body_top);
    overlays::draw(&mut paint, session, breakpoints, canvas, framed.class);
}

#[derive(Clone, Copy)]
enum Joint {
    Top,
    Bottom,
}

fn rule(paint: &mut Painter, row: u16, width: u16, framed: &Frame, joint: Joint) {
    let line = paint.theme.glyph(Glyph::Rule).repeat(usize::from(width));
    paint.fixed(0, row, &line, Look::role(Role::Border), usize::from(width));
    if framed.drawer > 0 {
        let mark = match (paint.theme.ascii, joint) {
            (true, _) => "+",
            (false, Joint::Top) => "┬",
            (false, Joint::Bottom) => "┴",
        };
        paint.fixed(framed.drawer.saturating_sub(1), row, mark, Look::role(Role::Border), 1);
    }
}

/// `≡ Gallery ▸ Detail · Enumeration` on the left; the user, the clock and the spinner on the right.
fn title_line(paint: &mut Painter, session: &Session, framed: &Frame, clock: Option<&str>) {
    let width = usize::from(framed.terminal);
    let crumb = format!(" {} ", paint.theme.glyph(Glyph::Crumb));
    let names = session.crumbs();
    let mut left = String::from(" ");
    if framed.drawer == 0 {
        left.push_str(paint.theme.glyph(Glyph::Menu));
        left.push(' ');
    }
    left.push_str(&names.join(&crumb));
    let spin = if session.is_busy() {
        format!("{}  ", spinner(paint.theme.ascii))
    } else {
        String::new()
    };
    // The counters: `Today 3  Chat 1  Notif 2`, or `T3 C1 N2` on a narrow screen or when the long form would cut the
    // crumbs; then the user and the clock, dropped in that order while the crumbs still do not fit.
    let order = |name: &str| {
        ["today", "chat", "notifications"]
            .iter()
            .position(|known| *known == name)
            .unwrap_or(9)
    };
    let mut counts: Vec<(&String, &i64)> = session.counts().iter().collect();
    counts.sort_by_key(|(name, _)| order(name));
    let (mut long, mut short) = (String::new(), String::new());
    for (name, count) in counts {
        if *count == 0 {
            continue;
        }
        let word = match name.as_str() {
            "notifications" => "Notif".to_string(),
            other => {
                let mut chars = other.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
        };
        short.push_str(&format!("{}{count} ", word.chars().next().unwrap_or(' ')));
        long.push_str(&format!("{word} {count}  "));
    }
    if !short.is_empty() {
        short.push(' ');
    }
    let user = &session.shell().user;
    let user = if user.is_empty() {
        String::new()
    } else {
        format!("{user}  ")
    };
    let clock = match clock {
        Some(clock) => format!("{clock} "),
        None => format!("{} ", jiff::Zoned::now().strftime("%a %H:%M")),
    };
    let mut choices = Vec::new();
    if framed.class >= WidthClass::Md {
        choices.push(format!("{spin}{long}{user}{clock}"));
    }
    choices.push(format!("{spin}{short}{user}{clock}"));
    choices.push(format!("{spin}{short}{clock}"));
    choices.push(format!("{spin}{short}"));
    let fits = |right: &String| display_width(&left) + display_width(right) < width;
    let right = match choices.iter().position(fits) {
        Some(index) => choices.swap_remove(index),
        None => choices.pop().unwrap_or_default(),
    };
    let room = width.saturating_sub(display_width(&right) + 1);
    paint.fixed(0, 0, &truncate_width(&left, room), Look::role(Role::Text).bold(), room);
    // A click on the crumbs is the palette; on the `≡` of a hidden drawer, the drawer.
    paint.area(Rect::new(0, 0, u16::try_from(room).unwrap_or(0), 1), Target::Title);
    if framed.drawer == 0 {
        paint.area(Rect::new(1, 0, 1, 1), Target::DrawerToggle);
    }
    let x = u16::try_from(width.saturating_sub(display_width(&right))).unwrap_or(0);
    paint.fixed(x, 0, &right, Look::role(Role::Muted), display_width(&right));
}

fn spinner(ascii: bool) -> &'static str {
    let frames: &[&str] = if ascii {
        &["|", "/", "-", "\\"]
    } else {
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    };
    let tick = usize::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() / 100)
            .unwrap_or(0)
            % 64,
    )
    .unwrap_or(0);
    frames[tick % frames.len()]
}

/// The app menu tree: `▾` items with their screens, `▶` the screen shown.
fn drawer(paint: &mut Painter, session: &Session, framed: &Frame, top: u16, bottom: u16) {
    let width = if framed.drawer > 0 {
        framed.drawer
    } else {
        26.min(framed.terminal)
    };
    let rows = session.drawer_rows();
    let focus = session.drawer_focus();
    paint.area(Rect::new(0, top, width, bottom.saturating_sub(top)), Target::Block);
    for row in top..bottom {
        paint.fill_row(0, row, usize::from(width), Look::default());
        let side = if paint.theme.ascii { "|" } else { "│" };
        paint.fixed(width.saturating_sub(1), row, side, Look::role(Role::Border), 1);
    }
    let paths = session.paths();
    for (at, (title, url, depth)) in rows.iter().enumerate() {
        let Some(row) = top
            .checked_add(u16::try_from(at).unwrap_or(u16::MAX))
            .filter(|row| *row < bottom)
        else {
            break;
        };
        let path = url.split('?').next().unwrap_or(url);
        let active = paths
            .iter()
            .any(|shown| *shown == path || shown.starts_with(&format!("{path}/")));
        let heading = url.is_empty();
        let mark = match (*depth, active) {
            _ if heading => " ",
            (0, _) => paint.theme.glyph(Glyph::Open),
            (_, true) => paint.theme.glyph(Glyph::Active),
            _ => " ",
        };
        let text = format!(" {}{mark} {title}", "  ".repeat(*depth));
        let look = if heading {
            Look::role(Role::Muted).bold()
        } else if active {
            Look::role(Role::Primary).bold()
        } else {
            Look::role(Role::Text)
        };
        paint.fixed(
            0,
            row,
            &text,
            look.focused(focus == Some(at)),
            usize::from(width.saturating_sub(1)),
        );
        if !heading {
            paint.area(Rect::new(0, row, width.saturating_sub(1), 1), Target::Drawer(at));
        }
    }
}

/// The newest log lines, warnings and errors in their colors.
fn log_pane(paint: &mut Painter, top: u16, bottom: u16, width: u16, lines: &std::collections::VecDeque<String>) {
    let title = format!("{} Log · Alt+L closes ", paint.theme.glyph(Glyph::Rule));
    let rule = format!("{title}{}", paint.theme.glyph(Glyph::Rule).repeat(usize::from(width)));
    paint.fixed(0, top, &rule, Look::role(Role::Border), usize::from(width));
    let rows = usize::from(bottom - top - 1);
    for (at, line) in lines.iter().skip(lines.len().saturating_sub(rows)).enumerate() {
        let row = top + 1 + u16::try_from(at).unwrap_or(0);
        paint.fill_row(0, row, usize::from(width), Look::default());
        let look = if line.contains(" ERROR ") {
            Look::role(Role::Negative)
        } else if line.contains(" WARN ") {
            Look::role(Role::Warning)
        } else {
            Look::role(Role::Muted)
        };
        paint.fixed(1, row, line, look, usize::from(width.saturating_sub(1)));
    }
}

fn message_line(paint: &mut Painter, session: &Session, row: u16, width: u16) {
    let mark = session
        .focus()
        .and_then(|item| session.mark(&item.key))
        .map(str::to_string);
    let (text, look) = if !session.message().is_empty() {
        let look = if session.message_is_error() {
            Look::role(Role::Negative).bold()
        } else {
            Look::role(Role::Positive)
        };
        let lead = if session.message_is_error() { "! " } else { "" };
        (format!(" {lead}{}", session.message()), look)
    } else if let Some(mark) = mark {
        (format!(" ! {mark}"), Look::role(Role::Negative))
    } else {
        // The focused field's full title, which the label may have cut, and its tooltip.
        let hint = session
            .focus()
            .and_then(|item| session.index().fields.get(&item.node))
            .map(|field| {
                if field.tooltip.is_empty() {
                    field.title.clone()
                } else {
                    format!("{} · {}", field.title, field.tooltip)
                }
            })
            .unwrap_or_default();
        (format!(" {hint}"), Look::role(Role::Muted))
    };
    paint.fixed(0, row, &text, look, usize::from(width));
}

/// The keys that apply to the focused element, then the global ones, cut to the width (section 1.2).
fn key_line(paint: &mut Painter, session: &Session, row: u16, width: u16) {
    if session.assist_focused() {
        let line = crate::assist::draw::key_line(session);
        paint.fixed(0, row, &line, Look::role(Role::Muted), usize::from(width));
        return;
    }
    let keys = session.keys();
    let mut actions = vec![Action::Help];
    match session.focus_kind() {
        Some(FocusKind::Text) => {
            if matches!(
                session.field_widget(session.focus_id()),
                Some(Widget::DateTime { .. } | Widget::DateFind { .. } | Widget::TextFind { .. })
            ) {
                actions.push(Action::Prompt);
            }
            actions.push(Action::Submit);
        }
        Some(FocusKind::DropDown | FocusKind::Check) => actions.push(Action::Prompt),
        Some(FocusKind::List) => {
            let list = session.focus_id();
            if session
                .index()
                .rows
                .contains_key(&(list.to_string(), session.row_at(list)))
            {
                actions.push(Action::EditRow);
            }
            actions.extend([Action::PageNext, Action::Sort, Action::Find]);
            if session.list(list).is_some_and(|list| list.views.is_some()) {
                actions.push(Action::View);
            }
            if session.list(list).is_some_and(|list| list.row_selection.is_some()) {
                actions.push(Action::Select);
            }
            actions.push(Action::Actions);
        }
        Some(FocusKind::Menu) => actions.push(Action::Actions),
        Some(FocusKind::Component) if session.view_of(session.focus_id()).is_some() => actions.push(Action::View),
        _ => {}
    }
    actions.extend([
        Action::Back,
        Action::Refresh,
        Action::Drawer,
        Action::Cancel,
        Action::NumberLinks,
        Action::Quit,
        Action::Palette,
        Action::QuickCreate,
        Action::Mouse,
    ]);
    let mut line = String::new();
    let mut entries = Vec::new();
    for action in actions {
        let Some(hint) = keys.hint(action) else {
            continue;
        };
        if display_width(&line) + display_width(&hint) + 2 > usize::from(width) {
            break;
        }
        line.push(' ');
        entries.push((display_width(&line), display_width(&hint), action));
        line.push_str(&hint);
        line.push(' ');
    }
    paint.fixed(0, row, &line, Look::role(Role::Muted), usize::from(width));
    // A click on an entry presses its key.
    for (start, used, action) in entries {
        let (Ok(start), Ok(used)) = (u16::try_from(start), u16::try_from(used)) else {
            continue;
        };
        paint.area(Rect::new(start, row, used, 1), Target::KeyLine(action));
    }
}

// ---- the focus row, for scrolling ----

fn focus_rows(laid: &LaidOut, session: &Session) -> Option<(usize, usize)> {
    let item = session.focus()?;
    let mut y = 0;
    for piece in &laid.pieces {
        if let Some(found) = find_in(piece, &item.node, session, y) {
            return Some(found);
        }
        y += piece.height();
    }
    None
}

fn find_in(piece: &Piece, id: &str, session: &Session, y: usize) -> Option<(usize, usize)> {
    match piece {
        Piece::Stack(pieces) => {
            let mut at = y;
            for piece in pieces {
                if let Some(found) = find_in(piece, id, session, at) {
                    return Some(found);
                }
                at += piece.height();
            }
            None
        }
        Piece::Row(placed) => placed.iter().find_map(|item| find_in(&item.piece, id, session, y)),
        Piece::Box(laid) => {
            let mut at = y + 1 + usize::from(!laid.toolbar.is_empty());
            if laid.toolbar.iter().any(|item| item.id() == id) || laid.id == id {
                return Some((y, 2));
            }
            for piece in &laid.body {
                if let Some(found) = find_in(piece, id, session, at) {
                    return Some(found);
                }
                at += piece.height();
            }
            None
        }
        Piece::Form(form) => {
            let mut at = y;
            for line in &form.lines {
                let height = match line {
                    FormLine::Field(field) => field.height(),
                    FormLine::Cells(cells) => cells.iter().map(|(_, f)| f.height()).max().unwrap_or(1),
                    FormLine::Text(text) => text.lines.len(),
                    FormLine::Buttons(_) | FormLine::Group(_) => 1,
                };
                let found = match line {
                    FormLine::Field(field) => field.id == id,
                    FormLine::Cells(cells) => cells.iter().any(|(_, field)| field.id == id),
                    FormLine::Buttons(items) => items.iter().any(|item| item.id() == id),
                    FormLine::Text(_) | FormLine::Group(_) => false,
                };
                if found {
                    return Some((at, height));
                }
                at += height;
            }
            None
        }
        Piece::Inline(items) => items.iter().any(|item| item.id() == id).then_some((y, 1)),
        Piece::List(list) if list.id == id => {
            let row = session.row_at(id);
            let height = list.heights.get(row).copied().unwrap_or(1);
            // The header with the first row, else the row.
            Some(if row == 0 {
                (y, 2 + height)
            } else {
                (y + 2 + list.row_top(row), height)
            })
        }
        Piece::Tree(tree) if tree.id == id => {
            let at = session.tree(id).map_or(0, |state| state.at);
            Some((y + at.min(tree.rows.len().saturating_sub(1)), 1))
        }
        Piece::Lines(lines) if lines.id == id => {
            // The highlighted line of a component, else its top.
            let at = lines
                .lines
                .iter()
                .position(|line| line.iter().any(|span| span.reverse))
                .unwrap_or(0);
            Some((y + at, 1))
        }
        _ => None,
    }
}

// ---- pieces ----

pub fn draw_piece(paint: &mut Painter, session: &Session, piece: &Piece, x: u16, y: usize) {
    match piece {
        Piece::Stack(pieces) => {
            let mut at = y;
            for piece in pieces {
                draw_piece(paint, session, piece, x, at);
                at += piece.height();
            }
        }
        Piece::Row(placed) => {
            for item in placed {
                draw_piece(paint, session, &item.piece, x + item.x, y);
            }
        }
        Piece::Box(laid) => draw_box(paint, session, laid, x, y, piece.height()),
        Piece::Form(form) => {
            let mut at = y;
            for line in &form.lines {
                at += draw_form_line(paint, session, line, x, at);
            }
        }
        Piece::Inline(items) => {
            draw_inline(paint, session, items, x, y);
        }
        Piece::Text(text) => {
            let look = text_look(&text.style);
            for (at, line) in text.lines.iter().enumerate() {
                let line = if text.style == "image" {
                    format!("{} {line}", paint.theme.glyph(Glyph::Image))
                } else if text.style == "crumbs" {
                    line.replace(" ▸ ", &format!(" {} ", paint.theme.glyph(Glyph::Crumb)))
                } else {
                    line.clone()
                };
                paint.text(x, y + at, &line, look, usize::from(paint.right.saturating_sub(x)));
            }
        }
        Piece::List(list) => draw_list(paint, session, list, x, y),
        Piece::Tree(tree) => draw_tree(paint, session, tree, x, y),
        Piece::Status(status) => draw_status(paint, status, x, y),
        Piece::Lines(lines) => {
            let focused = session.focus_id() == lines.id;
            paint.target(Target::Focus(lines.id.clone()), |paint| {
                for (at, line) in lines.lines.iter().enumerate() {
                    draw_line(paint, line, x, y + at, focused);
                }
            });
        }
        Piece::Tabs(tabs) => {
            let mut col = x;
            for (at, (title, active)) in tabs.items.iter().enumerate() {
                let text = format!("{}:{title}", at + 1);
                let look = if *active {
                    Look::role(Role::Primary).bold().reverse()
                } else {
                    Look::role(Role::Text)
                };
                let url = tabs.urls.get(at).cloned().unwrap_or_default();
                let used = paint.target(Target::Tab(url), |paint| {
                    paint.text(col, y, &text, look, usize::from(paint.right.saturating_sub(col)))
                });
                col = col.saturating_add(u16::try_from(used + 2).unwrap_or(u16::MAX));
            }
        }
        Piece::Placeholder { text, url, .. } => {
            let line = format!(
                "{} {} has a browser-only part{}",
                paint.theme.glyph(Glyph::Image),
                if text.is_empty() { "This screen" } else { text },
                if url.is_empty() {
                    String::new()
                } else {
                    " · o=Open in browser".to_string()
                }
            );
            paint.text(
                x,
                y,
                &line,
                Look::role(Role::Muted).italic(),
                usize::from(paint.right.saturating_sub(x)),
            );
        }
        Piece::Error { text, .. } => {
            paint.text(
                x,
                y,
                &format!("! {text}"),
                Look::role(Role::Negative),
                usize::from(paint.right.saturating_sub(x)),
            );
        }
    }
}

fn text_look(style: &str) -> Look {
    match style {
        "h1" => Look::role(Role::Text).bold().underline(),
        "h2" | "h3" => Look::role(Role::Text).bold(),
        "h4" | "h5" | "h6" | "strong" => Look::role(Role::Text).bold(),
        "pre" | "code" => Look::role(Role::Info),
        "quote" => Look::role(Role::Muted).italic(),
        "i" | "em" => Look::role(Role::Text).italic(),
        "rule" => Look::role(Role::Border),
        "crumbs" => Look::role(Role::Muted),
        _ => Look::role(Role::Text),
    }
}

fn draw_box(paint: &mut Painter, session: &Session, laid: &LaidBox, x: u16, y: usize, height: usize) {
    let edge = Role::of_type(&laid.kind);
    // A collapsible box shows its state before the title; its title is where its focus is.
    let mark = if !laid.collapsible {
        ""
    } else if laid.collapsed {
        paint.theme.glyph(Glyph::Closed)
    } else {
        paint.theme.glyph(Glyph::Open)
    };
    let title = match (laid.title.is_empty(), mark.is_empty()) {
        (true, true) => String::new(),
        (true, false) => format!(" {mark} "),
        (false, true) => format!(" {} ", laid.title),
        (false, false) => format!(" {mark} {} ", laid.title),
    };
    paint.frame(x, y, laid.w, height, &title, edge);
    if laid.collapsible && !title.is_empty() {
        paint.canvas_area(x + 2, y, display_width(&title), 1, Target::Focus(laid.id.clone()));
    }
    if laid.collapsible && session.focus_id() == laid.id && !title.is_empty() {
        let width = display_width(&title);
        paint.text(x + 2, y, &title, Look::role(Role::Text).bold().reverse(), width);
    }
    if laid.collapsed {
        return;
    }
    let inner_x = x + 2;
    let mut at = y + 1;
    if !laid.toolbar.is_empty() {
        draw_inline(paint, session, &laid.toolbar, inner_x, at);
        at += 1;
    }
    let right = paint.right;
    paint.right = (x + laid.w).saturating_sub(1).min(right);
    for piece in &laid.body {
        draw_piece(paint, session, piece, inner_x, at);
        at += piece.height();
    }
    paint.right = right;
}

fn draw_form_line(paint: &mut Painter, session: &Session, line: &FormLine, x: u16, y: usize) -> usize {
    match line {
        FormLine::Field(field) => paint.target(Target::Focus(field.id.clone()), |paint| {
            draw_field(paint, session, field, x, y)
        }),
        FormLine::Cells(cells) => cells
            .iter()
            .map(|(cell_x, field)| {
                paint.target(Target::Focus(field.id.clone()), |paint| {
                    draw_field(paint, session, field, x + cell_x, y)
                })
            })
            .max()
            .unwrap_or(1),
        FormLine::Buttons(items) => {
            draw_inline(paint, session, items, x, y);
            1
        }
        FormLine::Group(title) => {
            paint.text(
                x,
                y,
                title,
                Look::role(Role::Text).bold(),
                usize::from(paint.right.saturating_sub(x)),
            );
            1
        }
        FormLine::Text(text) => {
            for (at, row) in text.lines.iter().enumerate() {
                paint.text(
                    x,
                    y + at,
                    row,
                    text_look(&text.style),
                    usize::from(paint.right.saturating_sub(x)),
                );
            }
            text.lines.len()
        }
    }
}

/// One field: its label (dot leaders, `*` when required), then its value in the widget's look. Returns its rows.
fn draw_field(paint: &mut Painter, session: &Session, field: &LaidField, x: u16, y: usize) -> usize {
    let Some(info) = session.index().fields.get(&field.id) else {
        return 0;
    };
    if !session.visible(&field.id) {
        // A hidden field keeps its rows so the layout does not jump while typing; it is blank.
        return field.height();
    }
    let focused_node = session.focus().is_some_and(|item| item.node == field.id);
    let required = if info.required { "* " } else { "" };
    let (value_x, value_y) = match field.mode {
        LabelMode::Beside => {
            let label = dotted_label(&format!("{required}{}", field.title), field.label_width + 2);
            let label_look = if focused_node {
                Look::role(Role::Text).bold()
            } else {
                Look::role(Role::Text)
            };
            paint.text(x, y, &label, label_look, field.label_width + 2);
            paint.text(
                x + u16::try_from(field.label_width + 2).unwrap_or(0),
                y,
                ":",
                Look::role(Role::Muted),
                1,
            );
            (x + u16::try_from(field.label_width + 4).unwrap_or(0), y)
        }
        LabelMode::Above => {
            paint.text(
                x,
                y,
                &format!("{required}{}", field.title),
                Look::role(Role::Text).bold(),
                usize::from(paint.right.saturating_sub(x)),
            );
            (x + 2, y + 1)
        }
    };
    paint.spot(&field.id, value_x, value_y);
    draw_value(paint, session, field, &info.widget, value_x, value_y);
    field.height()
}

fn draw_value(paint: &mut Painter, session: &Session, field: &LaidField, widget: &Widget, x: u16, y: usize) {
    let id = &field.id;
    let width = field.value_width.max(4);
    let focus_key = session.focus_id();
    let after = |paint: &mut Painter, used: usize, key: &str| {
        let mut col = x + u16::try_from(used).unwrap_or(0);
        if session.is_dirty(key) {
            col += u16::try_from(paint.text(col, y, paint.theme.glyph(Glyph::Changed), Look::role(Role::Changed), 1))
                .unwrap_or(0);
        }
        if session.mark(key).is_some() {
            col += u16::try_from(paint.text(
                col,
                y,
                paint.theme.glyph(Glyph::Error),
                Look::role(Role::Negative).bold(),
                1,
            ))
            .unwrap_or(0);
        }
        // A value Assist filled is tagged, so color is not the only cue (plan 4.11).
        if session.assist_filled(key) {
            paint.text(col, y, " Assist", Look::role(Role::Info), 7);
        }
    };
    match widget {
        Widget::TextLine { input_type, .. } => {
            let used = input(paint, session, id, x, y, width, input_type == "number", false);
            after(paint, used, id);
        }
        Widget::Password {} => {
            let used = input(paint, session, id, x, y, width, false, true);
            after(paint, used, id);
        }
        Widget::DateTime { .. } => {
            let used = input(paint, session, id, x, y, width, false, false);
            after(paint, used, id);
            if focus_key == id.as_str() {
                paint.text(
                    x + u16::try_from(used + 2).unwrap_or(0),
                    y,
                    "F4",
                    Look::role(Role::Muted),
                    2,
                );
            }
        }
        Widget::TextFind { .. } => {
            let used = input(paint, session, id, x, y, width.saturating_sub(10), false, false);
            after(paint, used, id);
            let op = session.buffer(&format!("{id}#op"));
            let not = session.buffer(&format!("{id}#not")) == "Y";
            if op != "contains" || not {
                let chip = format!("{}{op}", if not { "not " } else { "" });
                paint.text(
                    x + u16::try_from(used + 2).unwrap_or(0),
                    y,
                    &chip,
                    Look::role(Role::Info),
                    12,
                );
            }
        }
        Widget::DateFind { .. } | Widget::RangeFind { .. } | Widget::DatePeriod { .. } => {
            let half = width.saturating_sub(3) / 2;
            let from = format!("{id}#from");
            let thru = format!("{id}#thru");
            let used = input(paint, session, &from, x, y, half, false, false);
            after(paint, used, &from);
            let thru_x = x + u16::try_from(half + 3).unwrap_or(0);
            paint.text(
                x + u16::try_from(half + 1).unwrap_or(0),
                y,
                "–",
                Look::role(Role::Muted),
                1,
            );
            paint.spot(&thru, thru_x, y);
            let used = input(paint, session, &thru, thru_x, y, half, false, false);
            after(paint, used + usize::from(thru_x - x), &thru);
        }
        Widget::TextArea { editor_type, .. } if editor_type == "md" && session.previewing(id) => {
            // The preview of the source, drawn as markdown-view draws it.
            let room = usize::from(paint.right.saturating_sub(x)).saturating_sub(3);
            let (blocks, _) = crate::markdown::blocks(session.buffer(id));
            let mut lines = Vec::new();
            for block in &blocks {
                let lead = match block.kind.as_str() {
                    "li" => format!(
                        "{}{} ",
                        "  ".repeat(usize::from(block.depth).saturating_sub(1)),
                        block.marker
                    ),
                    "quote" => "│ ".to_string(),
                    _ => String::new(),
                };
                for line in crate::layout::wrap(&format!("{lead}{}", block.text), room) {
                    lines.push((line, block.kind.as_str()));
                }
            }
            // PgUp, PgDn and the wheel scroll the preview.
            let top = session.preview_top(id).min(lines.len().saturating_sub(field.rows));
            for row in 0..field.rows {
                paint.text(
                    x,
                    y + row,
                    area_bar(paint, row, field.rows, top, lines.len()),
                    Look::role(Role::Info),
                    1,
                );
                if let Some((line, kind)) = lines.get(top + row) {
                    paint.text(x + 1, y + row, line, text_look(kind), room);
                }
            }
            paint.text(
                x + u16::try_from(room.saturating_sub(10)).unwrap_or(0),
                y,
                " F4=Source ",
                Look::role(Role::Muted),
                11,
            );
        }
        Widget::TextArea {
            read_only, editor_type, ..
        } => {
            let text = session.buffer(id);
            let lines: Vec<&str> = text.split('\n').collect();
            let focused = focus_key == id.as_str();
            let (cursor_row, cursor_col) = crate::edit::row_col(text, session.cursor(id));
            let room = usize::from(paint.right.saturating_sub(x)).saturating_sub(3);
            // Markdown source is drawn styled (3.8): its text is the value, each character in the look of its part.
            let looks = (editor_type == "md").then(|| crate::markdown::source_looks(text));
            // The lines from the area's top, which follows the cursor; a text longer than the rows scrolls.
            let top = session.area_top(id).min(lines.len().saturating_sub(field.rows));
            let mut line_start: usize = lines.iter().take(top).map(|line| line.chars().count() + 1).sum();
            for shown in 0..field.rows {
                let row = top + shown;
                let line = lines.get(row).copied().unwrap_or("");
                let y = y + shown;
                let bar = area_bar(paint, shown, field.rows, top, lines.len());
                paint.text(x, y, bar, Look::role(Role::Field), 1);
                let look = if *read_only {
                    Look::role(Role::Text)
                } else {
                    Look::role(Role::Field)
                };
                match &looks {
                    Some(looks) => source_line(paint, x + 1, y, line, &looks[line_start.min(looks.len())..], room),
                    None => {
                        paint.text(x + 1, y, &truncate_width(line, room), look, room);
                    }
                }
                line_start += line.chars().count() + 1;
                if focused && row == cursor_row {
                    let ch = line.chars().nth(cursor_col).unwrap_or(' ').to_string();
                    let cx = x
                        + 1
                        + u16::try_from(display_width(&line.chars().take(cursor_col).collect::<String>())).unwrap_or(0);
                    paint.text(cx, y, &ch, Look::role(Role::Focus).reverse(), 1);
                }
            }
            // The changed and error marks after the text's room, not over its first column.
            after(paint, room + 1, id);
        }
        Widget::DropDown { options, multiple, .. } => {
            let value = session.buffer(id);
            let label = if *multiple {
                value
                    .split(',')
                    .filter(|part| !part.is_empty())
                    .map(|part| label_of(options, part))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                label_of(options, value)
            };
            let inner = width.saturating_sub(4);
            let shown = format!(
                "[{:<inner$} {}]",
                truncate_width(&label, inner),
                paint.theme.glyph(Glyph::DropDown)
            );
            let focused = focus_key == id.as_str();
            let used = paint.text(x, y, &shown, Look::role(Role::Field).focused(focused), width);
            after(paint, used, id);
        }
        Widget::Check { options, .. } => {
            let value = session.buffer(id);
            let focused = focus_key == id.as_str();
            let mut col = x;
            for choice in options {
                let on = value.split(',').any(|part| part == choice.value);
                let mark = if on { "[x]" } else { "[ ]" };
                let text = if options.len() == 1 && choice.label.is_empty() {
                    mark.to_string()
                } else {
                    format!("{mark} {}", choice.label)
                };
                let used = paint.text(
                    col,
                    y,
                    &text,
                    Look::role(Role::Field).focused(focused),
                    usize::from(paint.right.saturating_sub(col)),
                );
                col += u16::try_from(used + 2).unwrap_or(0);
            }
            after(paint, usize::from(col - x).saturating_sub(2), id);
        }
        Widget::Radio { options, .. } => {
            let value = session.buffer(id);
            let focused = focus_key == id.as_str();
            let mut col = x;
            for choice in options {
                let on = choice.value == value;
                let dot = if paint.theme.ascii { "*" } else { "•" };
                let text = format!("({}) {}", if on { dot } else { " " }, choice.label);
                let used = paint.text(
                    col,
                    y,
                    &text,
                    Look::role(Role::Field).focused(focused && on),
                    usize::from(paint.right.saturating_sub(col)),
                );
                col += u16::try_from(used + 2).unwrap_or(0);
            }
        }
        Widget::Display { text, .. } => {
            let room = usize::from(paint.right.saturating_sub(x));
            let text = session.shown_text(id).unwrap_or(text);
            for (at, line) in crate::layout::wrap(text, room).iter().enumerate().take(field.rows) {
                paint.text(x, y + at, line, Look::role(Role::Text), room);
            }
        }
        Widget::Editable { text, .. } => {
            // Click to edit: the value, underlined as something to act on; Enter edits it.
            let room = usize::from(paint.right.saturating_sub(x));
            let shown = if text.is_empty() { "(click to edit)" } else { text };
            let focused = focus_key == id.as_str();
            paint.text(x, y, shown, Look::role(Role::Text).underline().focused(focused), room);
        }
        Widget::Label { text, style } => {
            paint.text(x, y, text, text_look(style), usize::from(paint.right.saturating_sub(x)));
        }
        Widget::Image { alt, .. } => {
            let text = format!("{} {alt}", paint.theme.glyph(Glyph::Image));
            paint.text(
                x,
                y,
                &text,
                Look::role(Role::Muted),
                usize::from(paint.right.saturating_sub(x)),
            );
        }
        Widget::File { .. } => {
            paint.text(
                x,
                y,
                "[ choose… ]  upload in the browser",
                Look::role(Role::Muted),
                usize::from(paint.right.saturating_sub(x)),
            );
        }
        Widget::Hidden { .. } | Widget::Submit { .. } | Widget::Reset { .. } => {}
    }
}

pub fn label_of(options: &[crate::model::Choice], value: &str) -> String {
    options
        .iter()
        .find(|choice| choice.value == value)
        .map_or_else(|| value.to_string(), |choice| choice.label.clone())
}

/// `[value____]` in the field color; focused, reverse video with the cursor cell marked. A number is right-aligned and
/// a password shows dots. Returns the columns used.
#[expect(clippy::too_many_arguments)]
fn input(
    paint: &mut Painter,
    session: &Session,
    key: &str,
    x: u16,
    y: usize,
    width: usize,
    right: bool,
    secret: bool,
) -> usize {
    let inner = width.saturating_sub(2).max(1);
    let raw = session.buffer(key);
    let text: String = if secret {
        let dot = if paint.theme.ascii { '*' } else { '•' };
        std::iter::repeat_n(dot, raw.chars().count()).collect()
    } else {
        raw.to_string()
    };
    let focused = session.focus_id() == key;
    let chars: Vec<char> = text.chars().collect();
    let cursor = session.cursor(key).min(chars.len());
    // Keep the cursor inside the field: drop columns from the left until the text before it, and the cursor cell, fit.
    let mut start = 0;
    if focused {
        let width_of = |from: usize| {
            chars[from..cursor]
                .iter()
                .map(|ch| crate::layout::char_width(*ch))
                .sum::<usize>()
                + 1
        };
        while start < cursor && width_of(start) > inner {
            start += 1;
        }
    }
    let visible: String = chars[start..].iter().collect();
    let shown = truncate_width(&visible, inner);
    let pad = inner.saturating_sub(display_width(&shown));
    let body = if right && !focused {
        format!("{}{shown}", " ".repeat(pad))
    } else {
        format!("{shown}{}", "_".repeat(pad))
    };
    let look = Look::role(Role::Field).focused(focused);
    paint.text(x, y, "[", look, 1);
    paint.text(x + 1, y, &body, look, inner);
    paint.text(x + 1 + u16::try_from(inner).unwrap_or(0), y, "]", look, 1);
    if focused {
        // The cursor cell: the focus color, not reversed, inside the reversed field.
        let before: String = chars[start..cursor].iter().collect();
        let cx = x + 1 + u16::try_from(display_width(&before)).unwrap_or(0);
        let under = chars.get(cursor).map_or_else(|| " ".to_string(), char::to_string);
        if usize::from(cx - x) <= inner {
            paint.text(cx, y, &under, Look::role(Role::Focus), 1);
        }
    }
    inner + 2
}

/// One line of Markdown source in runs of the same look: headings bold in the primary color, strong bold, emphasis
/// italic, code in the info color, links underlined, quotes and markers muted.
/// The left bar of a text area's row: `▲` on the first row with lines above, `▼` on the last with lines below.
fn area_bar(paint: &Painter, shown: usize, rows: usize, top: usize, lines: usize) -> &'static str {
    let ascii = paint.theme.ascii;
    if shown == 0 && top > 0 {
        if ascii {
            "^"
        } else {
            "▲"
        }
    } else if shown + 1 == rows && top + rows < lines {
        if ascii {
            "v"
        } else {
            "▼"
        }
    } else if ascii {
        "|"
    } else {
        "│"
    }
}

fn source_line(paint: &mut Painter, x: u16, y: usize, line: &str, looks: &[crate::markdown::SourceLook], room: usize) {
    let look_of = |part: &crate::markdown::SourceLook| {
        let mut look = if part.marker || part.quote {
            Look::role(Role::Muted)
        } else if part.code {
            Look::role(Role::Info)
        } else if part.heading || part.link {
            Look::role(Role::Primary)
        } else {
            Look::role(Role::Field)
        };
        if part.heading || part.strong {
            look = look.bold();
        }
        if part.emphasis {
            look = look.italic();
        }
        if part.link && !part.marker {
            look = look.underline();
        }
        look
    };
    let mut col = 0;
    let mut run = String::new();
    let mut current: Option<crate::markdown::SourceLook> = None;
    let flush = |paint: &mut Painter, run: &mut String, part: Option<crate::markdown::SourceLook>, col: &mut usize| {
        if let Some(part) = part.filter(|_| !run.is_empty()) {
            let left = room.saturating_sub(*col);
            *col += paint.text(x + u16::try_from(*col).unwrap_or(0), y, run, look_of(&part), left);
        }
        run.clear();
    };
    for (at, ch) in line.chars().enumerate() {
        let part = looks.get(at).copied().unwrap_or_default();
        if current != Some(part) {
            flush(paint, &mut run, current, &mut col);
            current = Some(part);
        }
        run.push(ch);
    }
    flush(paint, &mut run, current, &mut col);
}

/// Links and buttons on one line, two columns apart.
fn draw_inline(paint: &mut Painter, session: &Session, items: &[Inline], x: u16, y: usize) {
    let mut col = x;
    for item in items {
        let focused = session.focus_id() == item.id();
        let number = session
            .link_number(item.id())
            .map(|n| format!("[{n}]"))
            .unwrap_or_default();
        let (text, look) = match item {
            Inline::Link {
                text, btn_type, button, ..
            } => {
                let role = Role::of_type(btn_type).unwrap_or(Role::Primary);
                if btn_type == "image" {
                    (
                        format!("{number}{} {text}", paint.theme.glyph(Glyph::Image)),
                        Look::role(Role::Primary).underline(),
                    )
                } else if *button {
                    (format!("{number}[ {text} ]"), Look::role(role))
                } else {
                    (format!("{number}{text}"), Look::role(Role::Primary).underline())
                }
            }
            Inline::Menu { text, .. } => (
                format!("[ {text} {} ]", paint.theme.glyph(Glyph::DropDown)),
                Look::role(Role::Secondary),
            ),
            Inline::Button { id, text, primary } => {
                let dirty = session
                    .index()
                    .form_of(id)
                    .is_some_and(|form| session.form_dirty(&form.id));
                if *primary {
                    // Dim while the form is unchanged, strong once a field changes (the outline-until-changed rule).
                    let look = if dirty {
                        Look::role(Role::Primary).bold()
                    } else {
                        Look::role(Role::Primary).dim()
                    };
                    (format!("< {text} >"), look)
                } else {
                    (format!("[ {text} ]"), Look::role(Role::Secondary))
                }
            }
            Inline::Dialog { text, .. } => (format!("{number}[ {text}… ]"), Look::role(Role::Secondary)),
        };
        paint.spot(item.id(), col, y);
        let url = match item {
            Inline::Link { id, .. } => session
                .index()
                .links
                .get(id)
                .map(|link| link.url.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let room = usize::from(paint.right.saturating_sub(col));
        let used = paint.target(Target::Focus(item.id().to_string()), |paint| {
            if super::paint::safe_link(&url) {
                paint.link_text(col, y, &text, look.focused(focused), room, &url)
            } else {
                paint.text(col, y, &text, look.focused(focused), room)
            }
        });
        col = col.saturating_add(u16::try_from(used + 2).unwrap_or(u16::MAX));
    }
}

/// A list: header with sort marks, rule, rows with the current row's `▌` and the selection column, the first column
/// fixed while the rest scroll sideways, and the paging footer.
fn draw_list(paint: &mut Painter, session: &Session, laid: &LaidList, x: u16, y: usize) {
    let Some(list) = session.list(&laid.id) else {
        return;
    };
    let focused = session.focus_id() == laid.id;
    let current = session.row_at(&laid.id);
    let selected = session.selected(&laid.id);
    let room = usize::from(paint.right.saturating_sub(x));
    let scroll = session.hscroll(&laid.id).min(laid.columns.len().saturating_sub(1));
    // The columns shown: the first, then from the scrolled one on.
    let mut shown: Vec<usize> = Vec::new();
    if !laid.columns.is_empty() {
        shown.push(0);
        shown.extend((1 + scroll)..laid.columns.len());
    }
    let gutter = 2 + if laid.selection { 4 } else { 0 };
    let mut used = gutter;
    let mut fits = Vec::new();
    for at in &shown {
        let width = laid.columns[*at].width;
        if used + width > room && !fits.is_empty() {
            break;
        }
        used += width + 1;
        fits.push(*at);
    }
    let more_right = fits.last().is_some_and(|last| *last + 1 < laid.columns.len());
    let total = if list.paging.count > 0 {
        list.paging.count as usize
    } else {
        list.rows.len()
    };
    // Header.
    let order = session.order_by();
    let mut col = x + u16::try_from(gutter).unwrap_or(0);
    for at in &fits {
        let column = &laid.columns[*at];
        let source = &list.columns[*at];
        let mut title = column.title.clone();
        if !source.order_by.is_empty() {
            let bare = source.order_by.trim_start_matches('^');
            if order.trim_start_matches('^') == bare {
                title.push_str(paint.theme.glyph(Glyph::Up));
            } else if order
                .strip_prefix('-')
                .is_some_and(|name| name.trim_start_matches('^') == bare)
            {
                title.push_str(paint.theme.glyph(Glyph::Down));
            }
        }
        let text = fit(&title, column.width, column.right);
        paint.text(col, y, &text, Look::role(Role::Text).bold(), column.width);
        if !source.order_by.is_empty() {
            paint.canvas_area(
                col,
                y,
                column.width,
                1,
                Target::ListSort {
                    list: laid.id.clone(),
                    column: *at,
                },
            );
        }
        col += u16::try_from(column.width + 1).unwrap_or(0);
    }
    if scroll > 0 {
        paint.text(x, y, paint.theme.glyph(Glyph::ScrollLeft), Look::role(Role::Muted), 1);
    }
    if more_right {
        let edge = x + u16::try_from(room.saturating_sub(1)).unwrap_or(0);
        paint.text(
            edge,
            y,
            paint.theme.glyph(Glyph::ScrollRight),
            Look::role(Role::Muted),
            1,
        );
    }
    let rule_width = used.min(room);
    paint.text(
        x,
        y + 1,
        &paint.theme.glyph(Glyph::Rule).repeat(rule_width),
        Look::role(Role::Border),
        rule_width,
    );
    // Rows: a row takes the lines of its tallest cell, as a column that stacks several fields draws them under each other.
    if list.rows.is_empty() {
        paint.text(x + 2, y + 2, "No rows match", Look::role(Role::Muted).italic(), room);
    }
    for (at, row) in list.rows.iter().enumerate() {
        let row_y = y + 2 + laid.row_top(at);
        let height = laid.heights.get(at).copied().unwrap_or(1);
        let is_current = focused && at == current;
        paint.canvas_area(
            x,
            row_y,
            used.min(room),
            height,
            Target::ListRow {
                list: laid.id.clone(),
                row: at,
            },
        );
        if is_current {
            for line in 0..height {
                paint.text(
                    x,
                    row_y + line,
                    paint.theme.glyph(Glyph::Row),
                    Look::role(Role::Primary).bold(),
                    1,
                );
            }
        }
        let mut col = x + 2;
        if laid.selection {
            let id = if row.select_id.is_empty() {
                &row.id
            } else {
                &row.select_id
            };
            let on = selected.is_some_and(|set| set.contains(id));
            paint.text(col, row_y, if on { "[x]" } else { "[ ]" }, Look::role(Role::Field), 3);
            paint.canvas_area(
                col,
                row_y,
                3,
                1,
                Target::ListSelect {
                    list: laid.id.clone(),
                    row: at,
                },
            );
            col += 4;
        }
        for column_at in &fits {
            let column = &laid.columns[*column_at];
            if let Some(cell) = row.cells.get(*column_at) {
                for (line, part) in cell.parts().into_iter().enumerate().take(height) {
                    let (text, look) = cell_part(paint, session, part);
                    let look = if is_current { look.on(Role::Selection) } else { look };
                    // A link of a cell is its own target over the row's: a click follows that link. The first column
                    // of a row with a link of its own is that link (the record, as the browser's first column).
                    let link = match part {
                        Cell::Node(node) => match node.as_ref() {
                            Node::Link { id, .. } => Some(id.clone()),
                            _ => None,
                        },
                        _ if *column_at == 0 && crate::link::row_link(row).is_some() => Some(String::new()),
                        _ => None,
                    };
                    let draw = |paint: &mut Painter| {
                        paint.text(
                            col,
                            row_y + line,
                            &fit(&text, column.width, column.right),
                            look,
                            column.width,
                        )
                    };
                    match link {
                        Some(link) => {
                            paint.target(
                                Target::RowLink {
                                    list: laid.id.clone(),
                                    row: at,
                                    link,
                                },
                                draw,
                            );
                        }
                        None => {
                            draw(paint);
                        }
                    }
                }
            }
            col += u16::try_from(column.width + 1).unwrap_or(0);
        }
    }
    // Footer: `Rows 21–40 of 312 · Page 2/16`.
    let footer_y = y + 2 + laid.body_rows().max(1);
    let page = session.page_index(&laid.id) + 1;
    let pages = session.page_count(&laid.id);
    let low = list
        .paging
        .range_low
        .map_or_else(|| (page - 1) * list.paging.size.max(1) as usize + 1, |low| low as usize);
    let high = list
        .paging
        .range_high
        .map_or(low + list.rows.len().saturating_sub(1), |high| high as usize);
    let footer = if list.rows.is_empty() {
        format!("Rows 0 of {total}")
    } else {
        format!("Rows {low}–{high} of {total} · Page {page}/{pages}")
    };
    paint.text(x + 2, footer_y, &footer, Look::role(Role::Muted), room);
    if let Some(find) = filter_summary(session, list) {
        let col = x + 2 + u16::try_from(display_width(&footer) + 3).unwrap_or(0);
        paint.text(
            col,
            footer_y,
            &find,
            Look::role(Role::Info),
            room.saturating_sub(usize::from(col - x)),
        );
    }
}

/// `Status = Approved` for each header field with a value, as qvt's filter summary.
fn filter_summary(session: &Session, list: &crate::model::FormList) -> Option<String> {
    let mut parts = Vec::new();
    crate::model::walk(&list.header_fields, &mut |node| {
        if let Node::Field { id, title, .. } = node {
            let value = session.buffer(id);
            let from = session.buffer(&format!("{id}#from"));
            let thru = session.buffer(&format!("{id}#thru"));
            if !value.is_empty() {
                parts.push(format!("{title} = {value}"));
            } else if !from.is_empty() || !thru.is_empty() {
                parts.push(format!("{title} {from}–{thru}"));
            }
        }
    });
    (!parts.is_empty()).then(|| format!("Find: {}", parts.join(", ")))
}

/// A part of a list cell as drawn: a link with its number, a field of a row form with its current value and marks, or
/// text.
fn cell_part(paint: &Painter, session: &Session, part: &Cell) -> (String, Look) {
    let text_look = Look::role(Role::Text);
    let Cell::Node(node) = part else {
        return (part.text().to_string(), text_look);
    };
    match node.as_ref() {
        Node::Link { id, text, .. } => {
            let number = session.link_number(id).map(|n| format!("[{n}]")).unwrap_or_default();
            (format!("{number}{text}"), Look::role(Role::Primary).underline())
        }
        Node::Text { text, .. } => (text.clone(), text_look),
        Node::DynamicDialog { button, title, .. } | Node::Dialog { button, title, .. } => {
            let text = if button.is_empty() { title } else { button };
            (format!("[ {text}… ]"), Look::role(Role::Secondary))
        }
        Node::Field { id, w, .. } => {
            let value = session.buffer(id);
            let shown = match w {
                Widget::DropDown { .. } | Widget::Radio { .. } => label_of(session.index().choices(id), value),
                Widget::Check { options, .. } => {
                    let on = options
                        .iter()
                        .any(|choice| value.split(',').any(|part| part == choice.value));
                    if on { "[x]" } else { "[ ]" }.to_string()
                }
                Widget::Submit { text, .. } | Widget::Reset { text } => format!("[ {text} ]"),
                Widget::Display { text, .. } => session.shown_text(id).unwrap_or(text).to_string(),
                Widget::Editable { text, .. } | Widget::Label { text, .. } => text.clone(),
                _ => value.to_string(),
            };
            let mut marks = String::new();
            if session.is_dirty(id) {
                marks.push_str(paint.theme.glyph(Glyph::Changed));
            }
            if session.mark(id).is_some() {
                marks.push_str(paint.theme.glyph(Glyph::Error));
            }
            let look = if w.is_display() {
                text_look
            } else if session.mark(id).is_some() {
                Look::role(Role::Negative)
            } else if session.is_dirty(id) {
                Look::role(Role::Changed)
            } else {
                Look::role(Role::Field)
            };
            (format!("{shown}{marks}"), look)
        }
        _ => (part.text().to_string(), text_look),
    }
}

/// A tree's rows: `▾` an open node, `▸` a closed one, `·` a leaf, the current row reversed while the tree has focus.
fn draw_tree(paint: &mut Painter, session: &Session, tree: &LaidTree, x: u16, y: usize) {
    let room = usize::from(paint.right.saturating_sub(x));
    if tree.rows.is_empty() {
        paint.text(x, y, "Loading…", Look::role(Role::Muted).italic(), room);
        return;
    }
    let focused = session.focus_id() == tree.id;
    let at = session.tree(&tree.id).map_or(0, |state| state.at);
    for (index, row) in tree.rows.iter().enumerate() {
        let mark = match row.mark {
            TreeMark::Open => paint.theme.glyph(Glyph::Open),
            TreeMark::Closed => paint.theme.glyph(Glyph::Closed),
            TreeMark::Leaf => paint.theme.glyph(Glyph::Leaf),
            TreeMark::Loading => paint.theme.glyph(Glyph::Ellipsis),
        };
        let text = format!("{}{mark} {}", "  ".repeat(row.depth), row.text);
        let look = if row.url.is_empty() {
            Look::role(Role::Text)
        } else {
            Look::role(Role::Primary).underline()
        };
        paint.target(
            Target::TreeRow {
                tree: tree.id.clone(),
                row: index,
            },
            |paint| paint.text(x, y + index, &text, look.focused(focused && index == at), room),
        );
    }
}

/// `✓ Placed ─ ● Approved ─ ○ Shipped`: the stages, done, current and to come, or the status alone.
fn draw_status(paint: &mut Painter, status: &LaidStatus, x: u16, y: usize) {
    let room = usize::from(paint.right.saturating_sub(x));
    let mut col = x;
    if !status.text.is_empty() && (status.stages.is_empty() || status.text.starts_with("Step")) {
        let used = paint.text(col, y, &status.text, Look::role(Role::Text).bold(), room);
        col += u16::try_from(used + 2).unwrap_or(0);
    }
    for (at, stage) in status.stages.iter().enumerate() {
        if at > 0 {
            let rule = format!(" {} ", paint.theme.glyph(Glyph::Rule));
            col += u16::try_from(paint.text(col, y, &rule, Look::role(Role::Border), 3)).unwrap_or(0);
        }
        let (glyph, look) = match stage.state.as_str() {
            "done" => (Glyph::Done, Look::role(Role::Muted)),
            "current" => (Glyph::Current, Look::role(Role::Primary).bold()),
            _ => (Glyph::Todo, Look::role(Role::Muted)),
        };
        let text = format!("{} {}", paint.theme.glyph(glyph), stage.label);
        let left = room.saturating_sub(usize::from(col - x));
        col += u16::try_from(paint.text(col, y, &text, look, left)).unwrap_or(0);
    }
}

/// A component's line: its spans in their roles; a reversed span shows the component's choice while it has focus.
fn draw_line(paint: &mut Painter, line: &Line, x: u16, y: usize, focused: bool) {
    let mut col = x;
    for span in line {
        let mut look = Look::role(span.role);
        if span.bold {
            look = look.bold();
        }
        if span.underline {
            look = look.underline();
        }
        if span.reverse && focused {
            look = look.reverse();
        } else if span.reverse {
            look = look.underline();
        }
        let left = usize::from(paint.right.saturating_sub(col));
        if left == 0 {
            break;
        }
        col += u16::try_from(paint.text(col, y, &span.text, look, left)).unwrap_or(0);
    }
}

fn fit(text: &str, width: usize, right: bool) -> String {
    let shown = truncate_width(text, width);
    let pad = width.saturating_sub(display_width(&shown));
    if right {
        format!("{}{shown}", " ".repeat(pad))
    } else {
        format!("{shown}{}", " ".repeat(pad))
    }
}
