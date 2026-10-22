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

//! `board`: rows as cards in columns by group (a pipeline by stage). From lg the columns stand side by side; below, the
//! groups are headings over their cards. `m` moves the highlighted card to another group through the `move` transition.
//!
//! Props, each with its default: `source` (the transition answering `{groups, rows}`), `groups` ("groups"), `rows`
//! ("rows"), `groupId` ("id"), `groupLabel` ("label", else "name"), `groupColor` ("color"), `groupOf` ("group", the row's
//! group id), `id` ("id"), `title` ("title"), `fields` (the row values shown under the title), `move` (the transition a
//! move posts to), `moveId` ("id") and `moveGroup` ("group", the posted names), `open` (a url with `{id}`, the card's
//! screen).

use std::collections::BTreeMap;

use serde_json::Value;

use super::{fit, list, prop, role_of_color, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;

pub struct Board;

/// A board's groups and cards, read from the data with the props' names.
struct Read<'a> {
    groups: Vec<Group>,
    cards: Vec<Card<'a>>,
}

struct Group {
    id: String,
    label: String,
    role: Role,
}

struct Card<'a> {
    id: String,
    title: String,
    fields: Vec<String>,
    group: usize,
    row: &'a Value,
}

fn read<'a>(props: &'a Value, state: &'a ComponentState) -> Read<'a> {
    let group_id = prop(props, "groupId", "id");
    let group_color = prop(props, "groupColor", "color");
    let groups: Vec<Group> = list(props, state, prop(props, "groups", "groups"))
        .into_iter()
        .map(|group| {
            let mut label = text_of(group.get(prop(props, "groupLabel", "label")));
            if label.is_empty() {
                label = text_of(group.get("name"));
            }
            let id = text_of(group.get(group_id));
            Group {
                label: if label.is_empty() { id.clone() } else { label },
                id,
                role: role_of_color(&text_of(group.get(group_color))),
            }
        })
        .collect();
    let field_names: Vec<&str> = props
        .get("fields")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let group_of = prop(props, "groupOf", "group");
    let mut cards: Vec<Card<'a>> = Vec::new();
    for (at, group) in groups.iter().enumerate() {
        for row in list(props, state, prop(props, "rows", "rows")) {
            if text_of(row.get(group_of)) != group.id {
                continue;
            }
            cards.push(Card {
                id: text_of(row.get(prop(props, "id", "id"))),
                title: text_of(row.get(prop(props, "title", "title"))),
                fields: field_names
                    .iter()
                    .map(|name| text_of(row.get(*name)))
                    .filter(|text| !text.is_empty())
                    .collect(),
                group: at,
                row,
            });
        }
    }
    Read { groups, cards }
}

impl Board {
    fn card_lines(card: &Card<'_>, width: usize, current: bool) -> [Line; 2] {
        let mut title = Span::new(fit(&card.title, width), Role::Text);
        if current {
            title = title.reverse();
        }
        [
            vec![title],
            vec![Span::new(fit(&card.fields.join(" · "), width), Role::Muted)],
        ]
    }
}

impl Component for Board {
    fn name(&self) -> &'static str {
        "board"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let board = read(props, state);
        if board.groups.is_empty() {
            return vec![vec![Span::new("No groups", Role::Muted)]];
        }
        let width = usize::from(width);
        let mut lines = Vec::new();
        if let (Some(target), Some(card)) = (state.choosing, board.cards.get(state.at)) {
            let mut line = vec![Span::new(format!("Move {} to: ", card.title), Role::Info)];
            for (at, group) in board.groups.iter().enumerate() {
                let span = Span::new(format!(" {} ", group.label), group.role);
                line.push(if at == target { span.reverse().bold() } else { span });
            }
            lines.push(line);
            lines.push(vec![Span::new("←/→ choose · Enter moves · Esc cancels", Role::Muted)]);
        }
        let count = board.groups.len();
        let column = width.saturating_sub(count.saturating_sub(1)) / count.max(1);
        if wide && column >= 16 {
            // Columns side by side: the headings, then the cards row by row.
            let mut heading = Vec::new();
            for (at, group) in board.groups.iter().enumerate() {
                let n = board.cards.iter().filter(|card| card.group == at).count();
                heading.push(Span::new(fit(&format!("{} · {n}", group.label), column), group.role).bold());
                heading.push(Span::new(" ", Role::Text));
            }
            lines.push(heading);
            let deepest = (0..count)
                .map(|at| board.cards.iter().filter(|card| card.group == at).count())
                .max()
                .unwrap_or(0);
            for row in 0..deepest {
                let mut pair: [Line; 2] = [Vec::new(), Vec::new()];
                for at in 0..count {
                    let found = board
                        .cards
                        .iter()
                        .enumerate()
                        .filter(|(_, card)| card.group == at)
                        .nth(row);
                    let parts = match found {
                        Some((index, card)) => Self::card_lines(card, column, index == state.at),
                        None => [
                            vec![Span::new(" ".repeat(column), Role::Text)],
                            vec![Span::new(" ".repeat(column), Role::Text)],
                        ],
                    };
                    for (line, part) in pair.iter_mut().zip(parts) {
                        line.extend(part);
                        line.push(Span::new(" ", Role::Text));
                    }
                }
                lines.extend(pair);
            }
            return lines;
        }
        // Groups as headings over their cards.
        for (at, group) in board.groups.iter().enumerate() {
            let n = board.cards.iter().filter(|card| card.group == at).count();
            lines.push(vec![Span::new(format!("{} · {n}", group.label), group.role).bold()]);
            for (index, card) in board.cards.iter().enumerate().filter(|(_, card)| card.group == at) {
                let [title, fields] = Self::card_lines(card, width.saturating_sub(2), index == state.at);
                let mut first = vec![Span::new("  ", Role::Text)];
                first.extend(title);
                lines.push(first);
                if !card.fields.is_empty() {
                    let mut second = vec![Span::new("  ", Role::Text)];
                    second.extend(fields);
                    lines.push(second);
                }
            }
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, wide: bool) -> Option<Vec<Act>> {
        // The cards are read from a copy, so the state can change while they are in hand.
        let seen = state.clone();
        let board = read(props, &seen);
        let last = board.cards.len().saturating_sub(1);
        if let Some(target) = state.choosing {
            let groups = board.groups.len().max(1);
            match press.key {
                Key::Left | Key::Up => state.choosing = Some((target + groups - 1) % groups),
                Key::Right | Key::Down | Key::Tab => state.choosing = Some((target + 1) % groups),
                Key::Escape | Key::F(12) => state.choosing = None,
                Key::Enter => {
                    state.choosing = None;
                    let (Some(card), Some(group)) = (board.cards.get(state.at), board.groups.get(target)) else {
                        return Some(Vec::new());
                    };
                    if card.group == target {
                        return Some(Vec::new());
                    }
                    let fields = BTreeMap::from([
                        (prop(props, "moveId", "id").to_string(), card.id.clone()),
                        (prop(props, "moveGroup", "group").to_string(), group.id.clone()),
                    ]);
                    return Some(vec![Act::Post {
                        url: prop(props, "move", "").to_string(),
                        fields,
                    }]);
                }
                _ => {}
            }
            return Some(Vec::new());
        }
        let current = board.cards.get(state.at).map_or(0, |card| card.group);
        let same_group = |group: usize| -> Vec<usize> {
            board
                .cards
                .iter()
                .enumerate()
                .filter(|(_, card)| card.group == group)
                .map(|(index, _)| index)
                .collect()
        };
        match press.key {
            Key::Down if state.at < last => state.at += 1,
            Key::Up if state.at > 0 => state.at -= 1,
            Key::Right | Key::Left if wide && press.is_plain() => {
                let rank = same_group(current)
                    .iter()
                    .position(|index| *index == state.at)
                    .unwrap_or(0);
                let groups = board.groups.len();
                let step = |group: usize| {
                    if press.key == Key::Right {
                        (group + 1) % groups.max(1)
                    } else {
                        (group + groups.max(1) - 1) % groups.max(1)
                    }
                };
                // The next group that has a card, at the same depth or its last card.
                let mut group = step(current);
                for _ in 0..groups {
                    let cards = same_group(group);
                    if let Some(index) = cards.get(rank).or(cards.last()) {
                        state.at = *index;
                        break;
                    }
                    group = step(group);
                }
            }
            Key::Char('m') if press.is_plain() => {
                if prop(props, "move", "").is_empty() {
                    return Some(vec![Act::Message("This board does not move cards.".to_string())]);
                }
                if !board.cards.is_empty() {
                    state.choosing = Some(current);
                }
            }
            Key::Enter => {
                let open = prop(props, "open", "");
                let Some(card) = board.cards.get(state.at) else {
                    return Some(Vec::new());
                };
                if open.is_empty() {
                    return Some(vec![Act::Message(card.title.clone())]);
                }
                let mut url = open.replace("{id}", &card.id);
                // Any other `{name}` is the row's value.
                if let Some(row) = card.row.as_object() {
                    for (name, value) in row {
                        url = url.replace(&format!("{{{name}}}"), &text_of(Some(value)));
                    }
                }
                return Some(vec![Act::Follow(url)]);
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Up/Down", "Previous / next card"),
            ("Left/Right", "Next column (wide)"),
            ("m", "Move the card to another column"),
            ("Enter", "Open the card"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::line_text;

    fn data() -> Value {
        serde_json::json!({
            "stages": [{"id": "q", "name": "Qualify", "color": "grey-6"}, {"id": "w", "name": "Won", "color": "positive"}],
            "rows": [
                {"id": "O1", "title": "Acme", "amount": 100, "stage": "q"},
                {"id": "O2", "title": "Beta", "amount": 20, "stage": "w"},
                {"id": "O3", "title": "Gamma", "amount": 5, "stage": "q"}
            ]
        })
    }

    fn props() -> Value {
        serde_json::json!({"source": "boardRows", "groups": "stages", "groupOf": "stage", "fields": ["amount"],
            "move": "moveCard", "moveGroup": "stage", "open": "../Detail?id={id}"})
    }

    #[test]
    fn cards_group_under_their_column_and_move() {
        let mut state = ComponentState {
            data: Some(data()),
            ..ComponentState::default()
        };
        let narrow: Vec<String> = Board.lines(&props(), &state, 40, false).iter().map(line_text).collect();
        assert_eq!(narrow[0], "Qualify · 2");
        assert!(narrow[1].starts_with("  Acme"));
        assert!(narrow.iter().any(|line| line == "Won · 1"));
        let wide = Board.lines(&props(), &state, 80, true);
        assert!(line_text(&wide[0]).contains("Won · 1"));
        // Down to Gamma, the second card of Qualify; m then Right then Enter posts the move to Won.
        let press = |key| KeyPress::plain(key);
        assert_eq!(
            Board.key(&props(), &mut state, &press(Key::Down), false),
            Some(Vec::new())
        );
        Board.key(&props(), &mut state, &press(Key::Char('m')), false);
        Board.key(&props(), &mut state, &press(Key::Right), false);
        let acts = Board.key(&props(), &mut state, &press(Key::Enter), false).unwrap();
        assert_eq!(
            acts,
            [Act::Post {
                url: "moveCard".to_string(),
                fields: BTreeMap::from([
                    ("id".to_string(), "O3".to_string()),
                    ("stage".to_string(), "w".to_string())
                ]),
            }]
        );
        assert_eq!(
            Board.key(&props(), &mut state, &press(Key::Enter), false),
            Some(vec![Act::Follow("../Detail?id=O3".to_string())])
        );
        // Past the last card the key is the screen's.
        state.at = 2;
        assert_eq!(Board.key(&props(), &mut state, &press(Key::Down), false), None);
    }
}
