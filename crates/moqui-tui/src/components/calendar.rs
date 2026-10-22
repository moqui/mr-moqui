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

//! `calendar`: a month's events as an agenda by day, with the month grid above it from lg. PgUp and PgDn (or `[` and
//! `]`) show the month before or after, `t` this month; the month is a request parameter, so the transition answers
//! the events of the month asked.
//!
//! Props, each with its default: `source` (the transition answering `{month, events}`), `events` ("events"), `start`
//! ("start", `yyyy-MM-dd HH:mm`), `end` ("end"), `title` ("title"), `kind` ("kind": meeting, call, deadline, done),
//! `month` ("month", the parameter and the answer's key, `yyyy-MM`), `open` (a url with `{id}`).

use std::collections::BTreeMap;

use jiff::civil::Date;
use serde_json::Value;

use super::{fit, list, prop, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;

pub struct Calendar;

struct Event {
    id: String,
    /// The event's own screen, when the data gives one (`_open`, a list view's row link).
    open: String,
    day: Option<Date>,
    start: String,
    end: String,
    title: String,
    kind: String,
}

fn events(props: &Value, state: &ComponentState) -> Vec<Event> {
    let mut out: Vec<Event> = list(props, state, prop(props, "events", "events"))
        .into_iter()
        .map(|event| {
            let start = text_of(event.get(prop(props, "start", "start")));
            Event {
                id: text_of(event.get("id")),
                open: text_of(event.get("_open")),
                day: start.get(..10).and_then(|day| day.parse::<Date>().ok()),
                end: text_of(event.get(prop(props, "end", "end"))),
                title: text_of(event.get(prop(props, "title", "title"))),
                kind: text_of(event.get(prop(props, "kind", "kind"))),
                start,
            }
        })
        .collect();
    out.sort_by(|a, b| a.start.cmp(&b.start));
    out
}

/// The month shown: the state's parameter, else the data's, else that of the first event (a list view's events come
/// with the props), else this month.
fn month(props: &Value, state: &ComponentState) -> Date {
    let name = prop(props, "month", "month");
    let text = state
        .parameters
        .get(name)
        .cloned()
        .or_else(|| state.data.as_ref().map(|data| text_of(data.get(name))))
        .filter(|text| !text.is_empty())
        .or_else(|| {
            events(props, state)
                .first()
                .and_then(|event| event.start.get(..7))
                .map(str::to_string)
        })
        .unwrap_or_default();
    format!("{text}-01")
        .parse::<Date>()
        .unwrap_or_else(|_| jiff::Zoned::now().date().first_of_month())
}

fn role_of_kind(kind: &str) -> Role {
    match kind {
        "meeting" => Role::Primary,
        "call" => Role::Info,
        "deadline" => Role::Warning,
        "done" => Role::Positive,
        _ => Role::Text,
    }
}

/// `09:00–10:00` from the start and end, or the start's time alone.
fn hours(event: &Event) -> String {
    let time = |text: &str| text.get(11..16).unwrap_or("").to_string();
    let (from, to) = (time(&event.start), time(&event.end));
    match (from.is_empty(), to.is_empty() || to == from) {
        (true, _) => String::new(),
        (false, true) => from,
        (false, false) => format!("{from}–{to}"),
    }
}

impl Component for Calendar {
    fn name(&self) -> &'static str {
        "calendar"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, wide: bool) -> Vec<Line> {
        let shown = month(props, state);
        let mut lines = vec![vec![
            Span::new("◂ ", Role::Muted),
            Span::new(shown.strftime("%B %Y").to_string(), Role::Text).bold(),
            Span::new(" ▸   PgUp/PgDn month · t today", Role::Muted),
        ]];
        if let Some(line) = waiting(state) {
            lines.push(line);
            return lines;
        }
        let all = events(props, state);
        let current = all.get(state.at).and_then(|event| event.day);
        if wide {
            // The month grid: a week a line, a day's number and the count of its events.
            lines.push(vec![Span::new(
                "  Mo     Tu     We     Th     Fr     Sa     Su",
                Role::Muted,
            )]);
            let lead = usize::try_from(shown.weekday().to_monday_zero_offset()).unwrap_or(0);
            let days = usize::try_from(shown.days_in_month()).unwrap_or(30);
            let today = jiff::Zoned::now().date();
            let mut line: Line = Vec::new();
            for cell in 0..lead + days {
                if cell % 7 == 0 && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                if cell < lead {
                    line.push(Span::new("       ", Role::Text));
                    continue;
                }
                let day = shown
                    .checked_add(jiff::Span::new().days(i64::try_from(cell - lead).unwrap_or(0)))
                    .unwrap_or(shown);
                let count = all.iter().filter(|event| event.day == Some(day)).count();
                let mark = if count > 0 {
                    format!("•{count}")
                } else {
                    String::new()
                };
                let mut span = Span::new(format!("{:>4}{:<3}", day.day(), mark), Role::Text);
                if Some(day) == current {
                    span = span.reverse();
                } else if day == today {
                    span = span.underline();
                }
                line.push(span);
            }
            if !line.is_empty() {
                lines.push(line);
            }
            lines.push(Vec::new());
        }
        if all.is_empty() {
            lines.push(vec![Span::new("No events this month", Role::Muted)]);
            return lines;
        }
        let width = usize::from(width);
        let mut last_day = None;
        for (at, event) in all.iter().enumerate() {
            if event.day != last_day {
                last_day = event.day;
                let heading = event
                    .day
                    .map_or_else(|| event.start.clone(), |day| day.strftime("%a %d %b").to_string());
                lines.push(vec![Span::new(heading, Role::Text).bold()]);
            }
            let time = fit(&hours(event), 12);
            let mut title = Span::new(
                fit(&event.title, width.saturating_sub(16 + event.kind.len())),
                Role::Text,
            );
            if at == state.at {
                title = title.reverse();
            }
            lines.push(vec![
                Span::new(format!("  {time}"), Role::Muted),
                title,
                Span::new(format!(" {}", event.kind), role_of_kind(&event.kind)),
            ]);
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        let all = events(props, state);
        let name = prop(props, "month", "month").to_string();
        let shown = month(props, state);
        let to_month = |state: &mut ComponentState, date: Date| {
            state
                .parameters
                .insert(name.clone(), date.strftime("%Y-%m").to_string());
            state.at = 0;
            Some(vec![Act::Reload])
        };
        match press.key {
            Key::Down if state.at + 1 < all.len() => state.at += 1,
            Key::Up if state.at > 0 => state.at -= 1,
            Key::PageDown | Key::Char(']') => {
                return to_month(state, shown.checked_add(jiff::Span::new().months(1)).unwrap_or(shown));
            }
            Key::PageUp | Key::Char('[') => {
                return to_month(state, shown.checked_add(jiff::Span::new().months(-1)).unwrap_or(shown));
            }
            Key::Char('t') if press.is_plain() => return to_month(state, jiff::Zoned::now().date().first_of_month()),
            Key::Enter => {
                let event = all.get(state.at)?;
                if !event.open.is_empty() {
                    return Some(vec![Act::Follow(event.open.clone())]);
                }
                let open = prop(props, "open", "");
                if open.is_empty() {
                    return Some(vec![Act::Message(format!("{} {}", event.start, event.title))]);
                }
                return Some(vec![Act::Follow(open.replace("{id}", &event.id))]);
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Up/Down", "Previous / next event"),
            ("PgUp/PgDn", "Previous / next month"),
            ("t", "This month"),
            ("Enter", "Open the event"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::line_text;

    #[test]
    fn events_list_by_day_and_the_month_moves() {
        let data = serde_json::json!({"month": "2026-10", "events": [
            {"id": "b", "title": "Demo", "kind": "meeting", "start": "2026-10-19 15:00", "end": "2026-10-19 16:00"},
            {"id": "a", "title": "Call Acme", "kind": "call", "start": "2026-10-02 14:00", "end": "2026-10-02 15:00"}
        ]});
        let props = serde_json::json!({"source": "events"});
        let mut state = ComponentState {
            data: Some(data),
            ..ComponentState::default()
        };
        let lines: Vec<String> = Calendar
            .lines(&props, &state, 60, false)
            .iter()
            .map(line_text)
            .collect();
        assert!(lines[0].contains("October 2026"));
        assert_eq!(lines[1], "Fri 02 Oct");
        assert!(lines[2].contains("14:00–15:00") && lines[2].contains("Call Acme") && lines[2].ends_with("call"));
        let grid: Vec<String> = Calendar.lines(&props, &state, 60, true).iter().map(line_text).collect();
        assert!(grid.iter().any(|line| line.contains("2•1")), "{grid:?}");
        let acts = Calendar.key(&props, &mut state, &KeyPress::plain(Key::PageDown), false);
        assert_eq!(acts, Some(vec![Act::Reload]));
        assert_eq!(state.parameters.get("month").map(String::as_str), Some("2026-11"));
        assert_eq!(
            Calendar.source(&props, &state),
            Some(("events".to_string(), state.parameters.clone()))
        );
    }
}
