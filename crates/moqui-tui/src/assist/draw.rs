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

//! The Assist pane drawn: a 48-column right column while the canvas beside it stays md (168 columns with the app drawer
//! hidden, 194 with it), else over the canvas while it has the keys. From the top: the title, the thread (messages, step
//! rows, done lines, the canvas line), the cards waiting for the user, the status line and the composer.

use revue::layout::Rect;

use super::state::{Row, RowKind};
use super::{Area, Card, PANE_WIDTH};
use crate::layout::{display_width, truncate_width, wrap, Breakpoints};
use crate::session::Session;
use crate::theme::{Glyph, Role};
use crate::ui::paint::{Look, Painter};

/// Where the pane goes: its first column and width, and whether it docks beside the canvas.
pub struct PaneAt {
    pub x: u16,
    pub width: u16,
    pub docked: bool,
}

/// The pane on a terminal `width` wide, or None when it does not show. It docks while the canvas left beside it is md,
/// with the app drawer when the user keeps the drawer open; otherwise it covers the canvas's right side while it has
/// the keys (all of it below 80 columns).
pub fn place(session: &Session, width: u16, breakpoints: &Breakpoints) -> Option<PaneAt> {
    let assist = session.assist();
    if !assist.open {
        return None;
    }
    let room = width.saturating_sub(PANE_WIDTH);
    let drawer = if session.drawer_choice() == Some(true) {
        breakpoints.drawer
    } else {
        0
    };
    if room >= breakpoints.md + drawer {
        return Some(PaneAt {
            x: room,
            width: PANE_WIDTH,
            docked: true,
        });
    }
    if !assist.focused {
        return None;
    }
    let pane = if width < breakpoints.sm {
        width
    } else {
        PANE_WIDTH.min(width)
    };
    Some(PaneAt {
        x: width - pane,
        width: pane,
        docked: false,
    })
}

/// The key line while the pane has the keys.
pub fn key_line(session: &Session) -> String {
    match session.assist().area {
        Area::Composer => " Enter=Send  F4=Lines  Tab=Next  Alt+X=Stop  F10=Assist  F2=Close  Esc=Screen",
        Area::Cards => " Enter=Review  Up/Down=Card  Tab=Next  o=Canvas  F10=Assist  Esc=Screen",
        Area::Transcript => " Up/Down/PgUp/PgDn=Scroll  Tab=Next  o=Canvas  F10=Assist  Esc=Screen",
    }
    .to_string()
}

/// Draw the pane in rows `top..bottom`.
pub fn pane(paint: &mut Painter, session: &Session, at: &PaneAt, top: u16, bottom: u16) {
    let (left, right) = (paint.left, paint.right);
    paint.left = 0;
    paint.right = at.x + at.width;
    let height = bottom.saturating_sub(top);
    paint.clear(Rect::new(at.x, top, at.width, height));
    let side = if paint.theme.ascii { "|" } else { "│" };
    for row in top..bottom {
        paint.fixed(at.x, row, side, Look::role(Role::Border), 1);
    }
    let x = at.x + 2;
    let room = usize::from(at.width.saturating_sub(3));
    if height >= 6 && room >= 10 {
        draw_body(paint, session, x, room, top, bottom);
    }
    paint.left = left;
    paint.right = right;
}

fn draw_body(paint: &mut Painter, session: &Session, x: u16, room: usize, top: u16, bottom: u16) {
    let assist = session.assist();
    let thread = &assist.thread;
    let focused = session.assist_focused();
    // the title
    let title = if thread.title.is_empty() {
        "New conversation"
    } else {
        thread.title.as_str()
    };
    let used = paint.fixed(x, top, "Assist", Look::role(Role::Primary).bold(), room);
    paint.fixed(
        x + u16::try_from(used).unwrap_or(0),
        top,
        &format!(" · {title}"),
        Look::role(Role::Muted),
        room.saturating_sub(used),
    );
    let rule = paint.theme.glyph(Glyph::Rule).repeat(room);
    paint.fixed(x, top + 1, &rule, Look::role(Role::Border), room);
    // the composer and the status line
    let composer_row = bottom - 1;
    let on_composer = focused && assist.area == Area::Composer;
    let mark = format!("{} ", paint.theme.glyph(Glyph::Crumb));
    let field = Look::role(Role::Field).focused(on_composer);
    paint.fill_row(x, composer_row, room, field);
    paint.fixed(x, composer_row, &mark, field, room);
    let inner = room.saturating_sub(display_width(&mark));
    let text_x = x + u16::try_from(display_width(&mark)).unwrap_or(2);
    if assist.composer.is_empty() && !on_composer {
        paint.fixed(text_x, composer_row, "Ask Assist (F2)", Look::role(Role::Muted), inner);
    } else {
        // the end of a long message, the cursor shown
        let chars: Vec<char> = assist.composer.chars().collect();
        let cursor = assist.cursor.min(chars.len());
        let mut start = 0;
        while start < cursor && display_width(&chars[start..cursor].iter().collect::<String>()) + 1 > inner {
            start += 1;
        }
        let shown: String = chars[start..].iter().collect();
        paint.fixed(text_x, composer_row, &truncate_width(&shown, inner), field, inner);
        if on_composer {
            let before: String = chars[start..cursor].iter().collect();
            let cx = text_x + u16::try_from(display_width(&before)).unwrap_or(0);
            let under = chars.get(cursor).map_or_else(|| " ".to_string(), char::to_string);
            paint.fixed(cx, composer_row, &under, Look::role(Role::Focus), 1);
        }
    }
    let status_row = composer_row - 1;
    let (status, look) = match &assist.bootstrap {
        Some(boot) if !boot.enabled => (boot.reason.clone(), Look::role(Role::Negative)),
        _ if !thread.owner.following && !thread.owner.client_id.is_empty() && thread.active_run.is_some() => {
            ("Following in another window".to_string(), Look::role(Role::Muted))
        }
        _ => (thread.status(), Look::role(Role::Info)),
    };
    paint.fixed(x, status_row, &truncate_width(&status, room), look, room);
    // the cards, above the status line, at most half the pane
    let thread_top = top + 2;
    let mut cards = card_lines(session, room);
    let most = usize::from(status_row.saturating_sub(thread_top)) / 2;
    if cards.len() > most {
        let focus = cards.iter().position(|line| line.2).unwrap_or(0);
        let start = focus.min(cards.len() - most);
        cards = cards.into_iter().skip(start).take(most).collect();
    }
    let cards_top = status_row - u16::try_from(cards.len()).unwrap_or(0);
    for (at, (line, look, _)) in cards.iter().enumerate() {
        let row = cards_top + u16::try_from(at).unwrap_or(0);
        paint.fixed(x, row, line, *look, room);
    }
    // the thread, its end at the bottom, scrolled up in the transcript
    let lines = thread_lines(&thread.rows(), room);
    let shown = usize::from(cards_top.saturating_sub(thread_top));
    if lines.is_empty() && shown > 0 {
        let hint = "Ask about this screen, or tell Assist what to do.";
        for (at, line) in wrap(hint, room).iter().enumerate().take(shown) {
            let row = thread_top + u16::try_from(at).unwrap_or(0);
            paint.fixed(x, row, line, Look::role(Role::Muted), room);
        }
        return;
    }
    let scroll = assist.scroll.min(lines.len().saturating_sub(shown));
    let end = lines.len() - scroll;
    let start = end.saturating_sub(shown);
    for (at, (line, look)) in lines[start..end].iter().enumerate() {
        let row = thread_top + u16::try_from(at).unwrap_or(0);
        paint.fixed(x, row, line, *look, room);
    }
    if start > 0 && focused && assist.area == Area::Transcript {
        let up = paint.theme.glyph(Glyph::Up);
        paint.fixed(
            x + u16::try_from(room).unwrap_or(1) - 1,
            thread_top,
            up,
            Look::role(Role::Muted),
            1,
        );
    }
}

/// The thread's rows wrapped to the pane: each paragraph of a message on its own lines, a blank line before each
/// message of the user's but the first.
fn thread_lines(rows: &[Row], room: usize) -> Vec<(String, Look)> {
    let mut lines = Vec::new();
    for row in rows {
        let look = match row.kind {
            RowKind::User => Look::role(Role::Text).bold(),
            RowKind::Assistant => Look::role(Role::Text),
            RowKind::Step => Look::role(Role::Muted),
            RowKind::StepFailed | RowKind::Error => Look::role(Role::Negative),
            RowKind::Note | RowKind::Canvas => Look::role(Role::Info),
            RowKind::Done => Look::role(Role::Positive),
        };
        if row.kind == RowKind::User && !lines.is_empty() {
            lines.push((String::new(), look));
        }
        for paragraph in row.text.split('\n') {
            if paragraph.trim().is_empty() {
                lines.push((String::new(), look));
                continue;
            }
            lines.extend(wrap(paragraph, room).into_iter().map(|line| (line, look)));
        }
    }
    lines
}

/// The cards waiting, as lines with their look and whether the line is of the focused card.
fn card_lines(session: &Session, room: usize) -> Vec<(String, Look, bool)> {
    let assist = session.assist();
    let thread = &assist.thread;
    let on_cards = session.assist_focused() && assist.area == Area::Cards;
    let current = assist.card();
    let mut lines = Vec::new();
    for card in assist.cards() {
        let focused = on_cards && current == Some(card);
        let mut push = |text: &str, look: Look| {
            for line in wrap(text, room) {
                lines.push((line, look, focused));
            }
        };
        push("", Look::default());
        match card {
            Card::Approval(at) => {
                let card = &thread.approvals[at];
                let kind = if card.classification.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", card.classification)
                };
                push(
                    &format!("! Approval{kind}"),
                    Look::role(Role::Warning).bold().focused(focused),
                );
                push(&card.summary, Look::role(Role::Text));
                for change in &card.changes {
                    let shown = |value: &str| {
                        if value.is_empty() {
                            "(empty)".to_string()
                        } else {
                            value.to_string()
                        }
                    };
                    push(
                        &format!(
                            "{}: {} → {}",
                            change.label,
                            shown(&change.current),
                            shown(&change.proposed)
                        ),
                        Look::role(Role::Changed),
                    );
                }
                let buttons = if card.can_edit {
                    "[ Approve ]  [ Edit in screen ]  [ Reject ]"
                } else {
                    "[ Approve ]  [ Reject ]"
                };
                lines.push((truncate_width(buttons, room), Look::role(Role::Primary), focused));
            }
            Card::Question(at) => {
                let card = &thread.questions[at];
                push(
                    &format!("? {}", card.text),
                    Look::role(Role::Info).bold().focused(focused),
                );
                for (n, (label, recommended)) in card.options.iter().enumerate() {
                    let mark = if *recommended { " (recommended)" } else { "" };
                    push(&format!("  {}. {label}{mark}", n + 1), Look::role(Role::Text));
                }
                if card.free_text || card.options.is_empty() {
                    push("  Other: type an answer", Look::role(Role::Muted));
                }
            }
            Card::Handoff(at) => {
                let card = &thread.handoffs[at];
                push(
                    &format!("Assist filled {} — review, then Enter on Save", card.form),
                    Look::role(Role::Warning).bold().focused(focused),
                );
                for (label, value) in &card.values {
                    push(&format!("  {label}: {value}"), Look::role(Role::Changed));
                }
            }
        }
    }
    lines
}
