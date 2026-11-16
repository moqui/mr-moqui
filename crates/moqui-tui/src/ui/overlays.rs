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

//! What draws over the canvas: dialogs and the find dialog (90% wide, full screen at xs), the drop-down list under its
//! field, the date picker, the confirmation, the help, the info box and the login.

use revue::layout::Rect;

use crate::keys::{Action, KeyScheme, ACTIONS};
use crate::layout::{display_width, layout_nodes, truncate_width, Breakpoints, WidthClass};
use crate::model::{walk, Node};
use crate::session::{Columns, Editor, Menu, Overlay, PromptThen, Session};
use crate::state::ConfirmKind;
use crate::theme::Role;

use super::draw::draw_piece;
use super::paint::{Look, Painter, Target};

pub fn draw(paint: &mut Painter, session: &Session, breakpoints: &Breakpoints, canvas: Rect, class: WidthClass) {
    for overlay in session.overlays() {
        match overlay {
            Overlay::Dialog(id) => {
                let title = session.index().dialogs.get(id).cloned().unwrap_or_default();
                if let Some(mut nodes) = dialog_nodes(session, id) {
                    if nodes.is_empty() {
                        nodes.push(Node::Text {
                            id: format!("{id}#loading"),
                            text: "Loading…".to_string(),
                            style: "quote".to_string(),
                        });
                    }
                    body_box(paint, session, breakpoints, canvas, class, &title, &nodes);
                }
            }
            Overlay::Row(list, row) => {
                let nodes = session.row_nodes(list, *row);
                let title = format!("Row {} · Enter or Ctrl+S saves · Esc closes", row + 1);
                body_box(paint, session, breakpoints, canvas, class, &title, &nodes);
            }
            Overlay::Menu(menu) => menu_box(paint, menu, canvas),
            Overlay::Palette(_) => palette(paint, session, canvas),
            Overlay::Search(text) => {
                let line = format!("Find: {text}_   Enter finds · n next · N previous · Esc");
                let width = canvas
                    .width
                    .min(u16::try_from(display_width(&line) + 4).unwrap_or(u16::MAX));
                let rect = Rect::new(canvas.x, (canvas.y + canvas.height).saturating_sub(3), width, 3);
                paint.clear(rect);
                paint.fixed_frame(rect, "");
                paint.fixed(
                    rect.x + 2,
                    rect.y + 1,
                    &line,
                    Look::role(Role::Info),
                    usize::from(width - 4),
                );
            }
            Overlay::Messages => {
                // each message wrapped to the box, so a long one (a summary and its details, ACC-UI-028) is read whole
                let room = usize::from(canvas.width.saturating_sub(8)).max(20);
                let mut lines: Vec<String> = session
                    .history()
                    .iter()
                    .flat_map(|message| {
                        let text = if message.error {
                            format!("! {}", message.text)
                        } else {
                            message.text.clone()
                        };
                        crate::layout::wrap(&text, room)
                    })
                    .collect();
                if lines.is_empty() {
                    lines.push("No messages yet.".to_string());
                }
                let room = usize::from(canvas.height.saturating_sub(2));
                if lines.len() > room {
                    lines.drain(..lines.len() - room);
                }
                small_box(paint, canvas, "Messages", &lines);
            }
            Overlay::Editor(editor) => editor_box(paint, session, editor, canvas),
            Overlay::Columns(columns) => columns_box(paint, columns, canvas),
            Overlay::Prompt(prompt) => {
                let mut lines = match prompt.then {
                    PromptThen::AssistText => {
                        // a message of several lines, the cursor at its end
                        let text = format!("{}_", prompt.text);
                        text.split('\n').map(|line| format!("{line:<50}")).collect()
                    }
                    _ => vec![format!("[{:<30}]", format!("{}_", prompt.text))],
                };
                lines.push(String::new());
                lines.push(
                    match prompt.then {
                        PromptThen::SaveFind(_) => "Enter=Save  Esc=Cancel",
                        PromptThen::AssistText => "Alt+Enter=Send  Esc=Back to the composer",
                        _ => "Enter=Send  Esc=Cancel",
                    }
                    .to_string(),
                );
                small_box(paint, canvas, &prompt.title, &lines);
            }
            Overlay::Find(list) => {
                if let Some(found) = session.list(list) {
                    body_box(
                        paint,
                        session,
                        breakpoints,
                        canvas,
                        class,
                        "Find",
                        &found.header_fields.clone(),
                    );
                }
            }
            Overlay::DropDown(_) => dropdown(paint, session, canvas),
            Overlay::Picker(picker) => date_picker(paint, picker, canvas, paint_spot(paint, &picker.key)),
            Overlay::Confirm(confirm) => {
                let (title, mut lines) = match &confirm.kind {
                    ConfirmKind::Leave => ("Confirm", vec!["Leave without saving the changes?".to_string()]),
                    ConfirmKind::Ask(text) => ("Confirm", vec![text.clone()]),
                    ConfirmKind::Approval(card) => ("Assist asks to", card.lines.clone()),
                };
                // The choices as buttons, the chosen one reversed as a focused button is everywhere else.
                let choices: Vec<String> = confirm.choices().iter().map(|choice| format!("[ {choice} ]")).collect();
                lines.push(String::new());
                lines.push(choices.join("  "));
                let rect = small_box(paint, canvas, title, &lines);
                let y = rect.y + u16::try_from(lines.len()).unwrap_or(u16::MAX);
                if y + 1 < rect.y + rect.height {
                    let mut x = rect.x + 2;
                    let right = rect.x + rect.width.saturating_sub(2);
                    for (at, (choice, button)) in confirm.choices().iter().zip(&choices).enumerate() {
                        let room = usize::from(right.saturating_sub(x));
                        let look = Look::role(Role::Primary).focused(*choice == confirm.choice());
                        paint.target(Target::ConfirmChoice(at), |paint| paint.fixed(x, y, button, look, room));
                        x = x.saturating_add(u16::try_from(display_width(button) + 2).unwrap_or(u16::MAX));
                    }
                }
            }
            Overlay::Help => help(paint, session, canvas),
            Overlay::Info => {
                let link = session
                    .focus()
                    .and_then(|item| session.index().links.get(&item.key))
                    .map(|link| format!("Link: {}", link.url));
                let mut lines = vec![
                    format!("Screen: {}", session.path()),
                    format!("Address: {}", session.location_url()),
                    format!("Title: {}", session.model().screen.title),
                ];
                lines.extend(link);
                // A remote client knows only the components built into it (section 2.9).
                lines.push(format!("Components: {}", crate::components::names().join(", ")));
                small_box(paint, canvas, "Info", &lines);
            }
            Overlay::Login(login) => {
                let mark = |at: usize| if login.at == at { ">" } else { " " };
                let labels = login.labels();
                // the labels dotted to one width, the dots lined up from the right: "Username . : ["
                let width = labels.iter().map(|label| display_width(label)).max().unwrap_or(0) + 2;
                let mut lines: Vec<String> = labels
                    .iter()
                    .enumerate()
                    .map(|(at, label)| {
                        let value = if login.masked(at) {
                            "•".repeat(login.field(at).chars().count())
                        } else {
                            login.field(at).to_string()
                        };
                        let mut dotted = format!("{label} ");
                        while display_width(&dotted) < width {
                            dotted.push_str(if (width - display_width(&dotted)).is_multiple_of(2) {
                                " "
                            } else {
                                "."
                            });
                        }
                        format!("{} {dotted} : [{value:<20}]", mark(at))
                    })
                    .collect();
                let fields = lines.len();
                lines.push(String::new());
                let title = match &login.setup {
                    Some(setup) => {
                        lines.insert(fields, format!("  The password: {}", setup.hint()));
                        lines.push("Enter=Next  Ctrl+S=Create and log in  Tab=Next  Esc=Cancel".to_string());
                        "Welcome to your new system: create the first administrator"
                    }
                    None => {
                        lines.push(if login.code_wanted && !login.sendable.is_empty() {
                            "Enter=Log in  Alt+S=Send code  Tab=Next  Esc=Cancel".to_string()
                        } else {
                            "Enter=Log in  Tab=Next  Esc=Cancel".to_string()
                        });
                        "Log in"
                    }
                };
                let error = !login.error.is_empty();
                if error {
                    lines.insert(0, format!("! {}", login.error));
                }
                let rect = small_box(paint, canvas, title, &lines);
                // The fields, and the key hint line as the button.
                let first = rect.y + 1 + u16::from(error);
                let inner = rect.width.saturating_sub(2);
                for at in 0..fields {
                    let row = first + u16::try_from(at).unwrap_or(0);
                    paint.area(Rect::new(rect.x + 1, row, inner, 1), Target::LoginField(at));
                }
                let button = first + u16::try_from(lines.len() - 1 - usize::from(error)).unwrap_or(0);
                paint.area(Rect::new(rect.x + 1, button, inner, 1), Target::LoginField(fields));
            }
            Overlay::Drawer(_) => {}
        }
    }
}

fn paint_spot(paint: &Painter, key: &str) -> Option<(u16, u16)> {
    paint.spots.get(key).copied()
}

/// The body of a dialog: a container-dialog's or a dynamic-dialog's of the screen, or one the session added.
fn dialog_nodes(session: &Session, id: &str) -> Option<Vec<Node>> {
    if let Some(extra) = session.extras().iter().find(|extra| extra.id == id) {
        return Some(extra.c.clone());
    }
    let mut found = None;
    for model in session.nav().models() {
        walk(&model.c, &mut |node| match node {
            Node::Dialog { id: dialog, c, .. } | Node::DynamicDialog { id: dialog, c, .. } if dialog == id => {
                found = Some(c.clone());
            }
            _ => {}
        });
    }
    found
}

/// A dialog's body, laid out at its own width and drawn in a box over the canvas.
fn body_box(
    paint: &mut Painter,
    session: &Session,
    breakpoints: &Breakpoints,
    canvas: Rect,
    class: WidthClass,
    title: &str,
    nodes: &[Node],
) {
    let width = if class == WidthClass::Xs {
        canvas.width
    } else {
        canvas.width * 9 / 10
    };
    let laid = layout_nodes(nodes, width.saturating_sub(4), breakpoints, session);
    let height = u16::try_from(laid.height() + 2).unwrap_or(u16::MAX).min(canvas.height);
    let x = canvas.x + (canvas.width - width) / 2;
    let y = if class == WidthClass::Xs {
        canvas.y
    } else {
        canvas.y + (canvas.height - height) / 2
    };
    let rect = Rect::new(x, y, width, height);
    clear_around(paint, rect, canvas);
    paint.fixed_frame(rect, &format!(" {title} "));
    let saved = (paint.top, paint.bottom, paint.scroll, paint.left, paint.right);
    paint.top = y + 1;
    paint.bottom = y + height.saturating_sub(1);
    paint.scroll = 0;
    paint.left = x + 1;
    paint.right = x + width.saturating_sub(1);
    let mut at = 0;
    for piece in &laid.pieces {
        draw_piece(paint, session, piece, x + 2, at);
        at += piece.height();
    }
    (paint.top, paint.bottom, paint.scroll, paint.left, paint.right) = saved;
}

/// Blank a dialog's box and a column and a row around it, inside the canvas less its left edge (the drawer's border), so
/// the text under it does not run into its frame.
fn clear_around(paint: &mut Painter, rect: Rect, canvas: Rect) {
    let left = rect.x.saturating_sub(1).max(canvas.x + 1).min(rect.x);
    let top = rect.y.saturating_sub(1).max(canvas.y).min(rect.y);
    let right = (rect.x + rect.width + 1)
        .min(canvas.x + canvas.width)
        .max(rect.x + rect.width);
    let bottom = (rect.y + rect.height + 1)
        .min(canvas.y + canvas.height)
        .max(rect.y + rect.height);
    paint.clear(Rect::new(left, top, right - left, bottom - top));
}

/// A small centered box of text lines; answers where it is.
fn small_box(paint: &mut Painter, canvas: Rect, title: &str, lines: &[String]) -> Rect {
    let widest = lines
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0)
        .max(display_width(title) + 4);
    let width = u16::try_from(widest + 4).unwrap_or(u16::MAX).min(canvas.width);
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(canvas.height);
    let rect = Rect::new(
        canvas.x + (canvas.width - width) / 2,
        canvas.y + (canvas.height - height) / 2,
        width,
        height,
    );
    clear_around(paint, rect, canvas);
    paint.fixed_frame(rect, &format!(" {title} "));
    for (at, line) in lines.iter().enumerate() {
        let row = rect.y + 1 + u16::try_from(at).unwrap_or(0);
        if row + 1 >= rect.y + rect.height {
            break;
        }
        let look = if line.starts_with('!') {
            Look::role(Role::Negative).bold()
        } else {
            Look::role(Role::Text)
        };
        paint.fixed(rect.x + 2, row, line, look, usize::from(width.saturating_sub(4)));
    }
    rect
}

/// The open drop-down: its entries under the field, the typed filter at the top, the highlighted one reversed.
fn dropdown(paint: &mut Painter, session: &Session, canvas: Rect) {
    let Some(drop) = session.dropdown() else {
        return;
    };
    let choices = session.dropdown_choices();
    let current = session.buffer(&drop.field).to_string();
    let multiple = session.is_multi_choice(&drop.field);
    let widest = choices
        .iter()
        .map(|choice| display_width(&choice.label))
        .max()
        .unwrap_or(10)
        .clamp(12, 50);
    // A multiple choice has a check mark column and a hint line, which sets its least width.
    let width = u16::try_from(if multiple {
        (widest + 6).max(display_width(MULTI_HINT) + 3)
    } else {
        widest + 4
    })
    .unwrap_or(40);
    let visible = choices.len().clamp(1, 10);
    let height =
        u16::try_from(visible + 2 + usize::from(!drop.filter.is_empty()) + usize::from(multiple)).unwrap_or(12);
    let (sx, sy) = paint_spot(paint, &drop.field).unwrap_or((canvas.x + 2, canvas.y));
    let below = sy + 1;
    let y = if below + height <= canvas.y + canvas.height {
        below
    } else {
        (sy).saturating_sub(height).max(canvas.y)
    };
    let x = sx.min((canvas.x + canvas.width).saturating_sub(width));
    let rect = Rect::new(x, y, width, height);
    paint.clear(rect);
    paint.fixed_frame(rect, "");
    let mut row = y + 1;
    if !drop.filter.is_empty() {
        paint.fixed(
            x + 1,
            row,
            &format!("/{}", drop.filter),
            Look::role(Role::Info),
            usize::from(width - 2),
        );
        row += 1;
    }
    let start = drop.at.saturating_sub(visible - 1);
    for (at, choice) in choices.iter().enumerate().skip(start).take(visible) {
        let on = if multiple {
            current.split(',').any(|part| part == choice.value)
        } else {
            current == choice.value
        };
        let mark = if multiple {
            if on {
                "[x] "
            } else {
                "[ ] "
            }
        } else if on {
            "• "
        } else {
            "  "
        };
        let label = if choice.label.is_empty() {
            "(none)"
        } else {
            &choice.label
        };
        let text = format!("{mark}{}", truncate_width(label, widest));
        let look = Look::role(Role::Text).focused(at == drop.at);
        paint.fill_row(x + 1, row, usize::from(width - 2), look);
        paint.fixed(x + 1, row, &text, look, usize::from(width - 2));
        paint.area(Rect::new(x + 1, row, width - 2, 1), Target::Choice(at));
        row += 1;
    }
    if choices.is_empty() {
        paint.fixed(
            x + 1,
            row,
            if session.dropdown_waits_for_text() {
                "type to search"
            } else {
                "no matches"
            },
            Look::role(Role::Muted).italic(),
            usize::from(width - 2),
        );
    }
    if multiple {
        paint.fixed(
            x + 1,
            y + height - 2,
            MULTI_HINT,
            Look::role(Role::Muted),
            usize::from(width - 2),
        );
    }
}

const MULTI_HINT: &str = "Space=Toggle  Enter=Done";

/// A month grid at the field: arrows move a day or a week, PgUp and PgDn a month, Home today, Enter picks.
fn date_picker(paint: &mut Painter, picker: &crate::session::Picker, canvas: Rect, spot: Option<(u16, u16)>) {
    let width = 24;
    let height = 10;
    let (sx, sy) = spot.unwrap_or((canvas.x + 2, canvas.y));
    let y = if sy + 1 + height <= canvas.y + canvas.height {
        sy + 1
    } else {
        sy.saturating_sub(height).max(canvas.y)
    };
    let x = sx.min((canvas.x + canvas.width).saturating_sub(width));
    let rect = Rect::new(x, y, width, height);
    paint.clear(rect);
    let title = picker.date.strftime("%B %Y").to_string();
    paint.fixed_frame(rect, &title);
    paint.fixed(x + 1, y + 1, "Mo Tu We Th Fr Sa Su", Look::role(Role::Muted), 22);
    let first = picker.date.first_of_month();
    let lead = usize::try_from(first.weekday().to_monday_zero_offset()).unwrap_or(0);
    let days = usize::try_from(picker.date.days_in_month()).unwrap_or(30);
    let today = jiff::Zoned::now().date();
    for day in 1..=days {
        let cell = lead + day - 1;
        let row = y + 2 + u16::try_from(cell / 7).unwrap_or(0);
        let col = x + 1 + u16::try_from((cell % 7) * 3).unwrap_or(0);
        let date = first
            .checked_add(jiff::Span::new().days(i64::try_from(day - 1).unwrap_or(0)))
            .unwrap_or(first);
        let look = if date == picker.date {
            Look::role(Role::Primary).bold().reverse()
        } else if date == today {
            Look::role(Role::Primary).underline()
        } else {
            Look::role(Role::Text)
        };
        paint.fixed(col, row, &format!("{day:>2}"), look, 2);
        paint.area(Rect::new(col, row, 2, 1), Target::PickerDay(date));
    }
    paint.fixed(
        x + 1,
        y + height - 2,
        "Enter=Pick  Home=Today",
        Look::role(Role::Muted),
        22,
    );
}

/// F1: the screen's documents by number, the focused component's keys, then every action with its keys, from the merged
/// keymap, so a theme's keys show.
fn help(paint: &mut Painter, session: &Session, canvas: Rect) {
    let keys = session.keys();
    let mut lines = Vec::new();
    let palette = keys
        .chords(Action::Palette)
        .first()
        .map_or_else(|| "Ctrl+K".to_string(), |press| press.label());
    lines.push(format!(
        "Keys: {}. To change: {palette}, then Keys.",
        session.key_scheme().title()
    ));
    lines.push(String::new());
    if !session.docs().is_empty() {
        lines.push("Documents (press the number):".to_string());
        for (at, (title, _)) in session.docs().iter().enumerate().take(9) {
            lines.push(format!("  {}  {title}", at + 1));
        }
        lines.push(String::new());
    }
    let component = session
        .focus()
        .and_then(|item| session.index().components.get(&item.key))
        .and_then(|info| crate::components::find(&info.name));
    if session.focus().is_some_and(|item| session.markdown_field(&item.key)) {
        lines.push("This Markdown field:".to_string());
        for (key, what) in [
            ("F4", "preview, and the source again"),
            ("Alt+B", "bold the word at the cursor"),
            ("Alt+I", "italic"),
            ("Alt+K", "link: [word](), the url next"),
        ] {
            lines.push(format!("  {key:<14} {what}"));
        }
        lines.push(String::new());
    }
    if session.focused_is_text() {
        lines.push("This text field:".to_string());
        let select_all = if session.key_scheme() == KeyScheme::Modern {
            "Ctrl+A selects all; "
        } else {
            ""
        };
        for (key, what) in [
            ("Shift+arrows", format!("select; {select_all}Ctrl+Left/Right a word")),
            (
                "Ctrl+C, X, V",
                "copy, cut (all of it with nothing selected), paste".to_string(),
            ),
            ("Ctrl+Z, Ctrl+Y", "undo, redo".to_string()),
            (
                "Alt+Backspace",
                "delete a word (Ctrl+Backspace, Ctrl+Del); Ctrl+U to the line's start".to_string(),
            ),
        ] {
            lines.push(format!("  {key:<14} {what}"));
        }
        lines.push(String::new());
    }
    if let Some(component) = component {
        lines.push(format!("This {}:", component.name()));
        for (key, what) in component.help() {
            lines.push(format!("  {key:<14} {what}"));
        }
        lines.push(String::new());
    }
    for action in ACTIONS {
        let chords: Vec<String> = keys.chords(action).iter().map(|press| press.label()).collect();
        if chords.is_empty() {
            continue;
        }
        lines.push(format!("{:<16} {}", action.title(), chords.join(", ")));
    }
    lines.push(format!("{:<16} Alt+1…Alt+9", "Tab 1 to 9"));
    lines.push(String::new());
    lines.push("Shift+Left/Right scrolls a wide list. Any key closes.".to_string());
    let mouse = keys
        .chords(Action::Mouse)
        .first()
        .map_or_else(|| "Alt+P".to_string(), |press| press.label());
    lines.push(if session.mouse_on() {
        format!("Mouse: click, wheel, drag to copy; {mouse} gives the terminal its own selection.")
    } else {
        format!("Mouse off: the terminal selects and copies; {mouse} turns it on.")
    });
    let room = usize::from(canvas.height.saturating_sub(2));
    if lines.len() > room {
        lines.truncate(room);
    }
    small_box(paint, canvas, "Keys", &lines);
}

/// A menu over the canvas: its title, its entries, the highlighted one reversed. A button's menu opens under its
/// button (above it when there is no room below), as a drop-down's list does; any other in the middle.
fn menu_box(paint: &mut Painter, menu: &Menu, canvas: Rect) {
    let widest = menu
        .items
        .iter()
        .map(|item| display_width(&item.label))
        .chain(std::iter::once(display_width(&menu.title) + 4))
        .max()
        .unwrap_or(10)
        .clamp(16, 60);
    let width = u16::try_from(widest + 4).unwrap_or(64).min(canvas.width);
    let visible = menu
        .items
        .len()
        .clamp(1, usize::from(canvas.height.saturating_sub(3)).max(1));
    let height = u16::try_from(visible + 2).unwrap_or(12).min(canvas.height);
    let anchored = Some(menu.anchor.as_str())
        .filter(|key| !key.is_empty())
        .and_then(|key| paint_spot(paint, key));
    let (x, y) = match anchored {
        Some((sx, sy)) => {
            let below = sy + 1;
            let y = if below + height <= canvas.y + canvas.height {
                below
            } else {
                sy.saturating_sub(height).max(canvas.y)
            };
            (sx.max(canvas.x).min((canvas.x + canvas.width).saturating_sub(width)), y)
        }
        None => (
            canvas.x + (canvas.width - width) / 2,
            canvas.y + (canvas.height - height) / 2,
        ),
    };
    let rect = Rect::new(x, y, width, height);
    paint.clear(rect);
    paint.fixed_frame(rect, &format!(" {} ", menu.title));
    let start = menu.at.saturating_sub(visible - 1);
    for (row, (at, item)) in menu.items.iter().enumerate().skip(start).take(visible).enumerate() {
        let y = rect.y + 1 + u16::try_from(row).unwrap_or(0);
        let look = Look::role(Role::Text).focused(at == menu.at);
        paint.fill_row(rect.x + 1, y, usize::from(width - 2), look);
        paint.fixed(rect.x + 2, y, &item.label, look, usize::from(width - 4));
        paint.area(Rect::new(rect.x + 1, y, width - 2, 1), Target::MenuItem(at));
    }
}

/// The palette at the top of the canvas: what is typed, then the rows under their groups.
fn palette(paint: &mut Painter, session: &Session, canvas: Rect) {
    let Some(Overlay::Palette(palette)) = session.top_overlay() else {
        return;
    };
    let rows = session.palette_rows();
    let width = canvas.width.min(80).max(canvas.width * 7 / 10).min(canvas.width);
    let room = usize::from(canvas.height.saturating_sub(4)).max(1);
    let mut lines: Vec<(String, Look, bool, Option<usize>)> = Vec::new();
    let mut group = "";
    for (at, row) in rows.iter().enumerate() {
        if row.group != group {
            group = &row.group;
            lines.push((group.to_string(), Look::role(Role::Muted).bold(), false, None));
        }
        let text = if row.subtitle.is_empty() {
            format!("  {}", row.title)
        } else {
            format!("  {}  · {}", row.title, row.subtitle)
        };
        lines.push((
            text,
            Look::role(Role::Text).focused(at == palette.at),
            at == palette.at,
            Some(at),
        ));
    }
    if rows.is_empty() {
        lines.push((
            "  Nothing matches yet".to_string(),
            Look::role(Role::Muted).italic(),
            false,
            None,
        ));
    }
    // Keep the highlighted row in view.
    let current = lines.iter().position(|(_, _, on, _)| *on).unwrap_or(0);
    let start = current.saturating_sub(room.saturating_sub(1));
    let shown: Vec<_> = lines.into_iter().skip(start).take(room).collect();
    let height = u16::try_from(shown.len() + 3).unwrap_or(12).min(canvas.height);
    let rect = Rect::new(canvas.x + (canvas.width - width) / 2, canvas.y, width, height);
    paint.clear(rect);
    let title = if palette.go { " Go to " } else { " Go to anything " };
    paint.fixed_frame(rect, title);
    let prompt = format!("{} {}_", if palette.go { ">" } else { "?" }, palette.text);
    paint.fixed(
        rect.x + 2,
        rect.y + 1,
        &prompt,
        Look::role(Role::Field),
        usize::from(width - 4),
    );
    for (row, (text, look, _, at)) in shown.iter().enumerate() {
        let y = rect.y + 2 + u16::try_from(row).unwrap_or(0);
        paint.fill_row(rect.x + 1, y, usize::from(width - 2), *look);
        paint.fixed(rect.x + 2, y, text, *look, usize::from(width - 4));
        if let Some(at) = at {
            paint.area(Rect::new(rect.x + 1, y, width - 2, 1), Target::PaletteRow(*at));
        }
    }
}

/// A click-to-edit value's editor: the text with its cursor, or a select's options.
fn editor_box(paint: &mut Painter, session: &Session, editor: &Editor, canvas: Rect) {
    let title = session
        .index()
        .fields
        .get(&editor.field)
        .map(|field| field.title.clone())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Edit".to_string());
    let mut lines = Vec::new();
    match session.field_widget(&editor.field) {
        Some(crate::model::Widget::Editable { options: choices, .. }) if !choices.is_empty() => {
            for (at, choice) in choices.iter().enumerate() {
                let mark = if at == editor.at { ">" } else { " " };
                lines.push(format!("{mark} {}", choice.label));
            }
        }
        _ => {
            let before: String = editor.text.chars().take(editor.cursor).collect();
            let after: String = editor.text.chars().skip(editor.cursor).collect();
            for line in format!("{before}▏{after}").split('\n') {
                lines.push(truncate_width(line, 70));
            }
        }
    }
    lines.push(String::new());
    lines.push("Enter=Save  Esc=Cancel".to_string());
    small_box(paint, canvas, &title, &lines);
}

/// The columns of a list: each field shown in its column or not, the highlighted one reversed.
fn columns_box(paint: &mut Painter, columns: &Columns, canvas: Rect) {
    let mut lines = Vec::new();
    for (at, entry) in columns.entries.iter().enumerate() {
        let mark = if at == columns.at { ">" } else { " " };
        let place = entry
            .column
            .map_or_else(|| "hidden  ".to_string(), |n| format!("column {:<2}", n + 1));
        lines.push(format!(
            "{mark} [{}] {place} {}",
            if entry.column.is_some() { "x" } else { " " },
            entry.title
        ));
    }
    lines.push(String::new());
    lines.push("Space=Show/hide  Alt+Up/Down=Move  Enter=Save  r=Reset  Esc".to_string());
    let room = usize::from(canvas.height.saturating_sub(2));
    if lines.len() > room {
        let start = columns.at.saturating_sub(room.saturating_sub(3));
        let hint = lines.split_off(lines.len() - 2);
        lines = lines.into_iter().skip(start).take(room.saturating_sub(2)).collect();
        lines.extend(hint);
    }
    small_box(paint, canvas, "Columns", &lines);
}
