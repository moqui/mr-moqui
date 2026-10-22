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

//! The levels on screen and the history (D2). Each level is its own request: the leaf, and above it the parents between
//! the app root and the leaf (a record header above its tabs). A navigation keeps the parents whose path and parameters
//! are unchanged and asks only for the levels from the first one that changed. The levels down to the app root are the
//! drawer's, so they are not drawn.

use std::collections::BTreeMap;

use crate::model::ScreenModel;

/// The screens down to the app root (`apps` and the app) are drawn as the drawer, not as levels.
pub const LEVEL_FLOOR: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub model: ScreenModel,
    pub parameters: BTreeMap<String, String>,
}

/// A screen and its parameters: what a history entry or a reload asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub parameters: BTreeMap<String, String>,
}

impl Location {
    /// A url as the server writes it, `/apps/x/Detail?enumId=A`, split into its path and parameters.
    pub fn of_url(url: &str) -> Self {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        Self {
            path: path.to_string(),
            parameters: query_parameters(query),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Nav {
    parents: Vec<Level>,
    leaf: Level,
    history: Vec<Location>,
    floor: usize,
}

impl Nav {
    pub fn new(model: ScreenModel) -> Self {
        Self {
            parents: Vec::new(),
            leaf: Level {
                parameters: model.parameters.clone(),
                model,
            },
            history: Vec::new(),
            floor: LEVEL_FLOOR,
        }
    }

    /// How many leading path names are the drawer's. Fixture paths with no app root use 0.
    pub fn set_floor(&mut self, floor: usize) {
        self.floor = floor;
    }

    pub fn leaf(&self) -> &ScreenModel {
        &self.leaf.model
    }

    pub fn leaf_mut(&mut self) -> &mut ScreenModel {
        &mut self.leaf.model
    }

    pub fn leaf_parameters(&self) -> &BTreeMap<String, String> {
        &self.leaf.parameters
    }

    pub fn location(&self) -> Location {
        Location {
            path: self.leaf.model.screen.path.clone(),
            parameters: self.leaf.parameters.clone(),
        }
    }

    pub fn parents(&self) -> &[Level] {
        &self.parents
    }

    /// The models on screen, outermost first.
    pub fn models(&self) -> impl Iterator<Item = &ScreenModel> {
        self.parents
            .iter()
            .map(|level| &level.model)
            .chain(std::iter::once(&self.leaf.model))
    }

    /// The models on screen, to change: a dynamic container's body goes in.
    pub fn models_mut(&mut self) -> impl Iterator<Item = &mut ScreenModel> {
        self.parents
            .iter_mut()
            .map(|level| &mut level.model)
            .chain(std::iter::once(&mut self.leaf.model))
    }

    /// The screens asked for, newest first, for the palette's recent group.
    pub fn history(&self) -> &[Location] {
        &self.history
    }

    pub fn paths(&self) -> Vec<&str> {
        self.models().map(|model| model.screen.path.as_str()).collect()
    }

    /// A new leaf, after a level request. Parents that are still above it with the same parameters stay; the others go.
    /// Returns the parent paths to ask for, outermost first.
    pub fn set_leaf(&mut self, model: ScreenModel, parameters: BTreeMap<String, String>) -> Vec<String> {
        let wanted = parent_paths(&model, self.floor);
        self.parents
            .retain(|level| wanted.contains(&level.model.screen.path) && level.parameters == parameters);
        let missing = wanted
            .into_iter()
            .filter(|path| !self.parents.iter().any(|level| &level.model.screen.path == path))
            .collect();
        self.leaf = Level { model, parameters };
        missing
    }

    /// A parent level arrived. It goes in path order; one no longer above the leaf is dropped.
    pub fn set_parent(&mut self, model: ScreenModel, parameters: BTreeMap<String, String>) {
        let path = model.screen.path.clone();
        if !parent_paths(&self.leaf.model, self.floor).contains(&path) || parameters != self.leaf.parameters {
            return;
        }
        self.parents.retain(|level| level.model.screen.path != path);
        self.parents.push(Level { model, parameters });
        self.parents.sort_by_key(|level| level.model.screen.path.len());
    }

    /// Remember where the user was, before a navigation.
    pub fn push_history(&mut self) {
        let here = self.location();
        if self.history.last() != Some(&here) {
            self.history.push(here);
        }
    }

    pub fn pop_history(&mut self) -> Option<Location> {
        self.history.pop()
    }

    pub fn has_history(&self) -> bool {
        !self.history.is_empty()
    }
}

/// The paths of the levels between the app root and the leaf, outermost first.
fn parent_paths(model: &ScreenModel, floor: usize) -> Vec<String> {
    let names = &model.screen.path_list;
    if names.len() <= floor + 1 {
        return Vec::new();
    }
    (floor + 1..names.len())
        .map(|end| format!("/{}", names[..end].join("/")))
        .collect()
}

/// The path that asks for one level alone: `.tui` on its last name stops the walk down the default subscreens.
pub fn level_path(path: &str) -> String {
    if path.ends_with(".tui") || path == "/" || path.is_empty() {
        path.to_string()
    } else {
        format!("{path}.tui")
    }
}

pub fn query_parameters(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

/// `%XX` and `+` decoded; a `%` not followed by two hex digits stays as it is.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes.get(index + 1..index + 3).and_then(|pair| {
                    let digit = |byte: u8| char::from(byte).to_digit(16);
                    Some(digit(pair[0])? * 16 + digit(pair[1])?)
                });
                if let Some(byte) = hex.and_then(|value| u8::try_from(value).ok()) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                out.push(b'%');
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(path: &str) -> ScreenModel {
        let names: Vec<String> = path.split('/').filter(|n| !n.is_empty()).map(str::to_string).collect();
        ScreenModel {
            v: 1,
            screen: crate::model::ScreenMeta {
                path: path.to_string(),
                path_list: names,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn parents_between_the_app_root_and_the_leaf_are_asked_for_once() {
        let mut nav = Nav::new(model("/apps/app/Find"));
        let params = BTreeMap::from([("partyId".to_string(), "P1".to_string())]);
        let missing = nav.set_leaf(model("/apps/app/Party/Edit"), params.clone());
        assert_eq!(missing, ["/apps/app/Party"]);
        nav.set_parent(model("/apps/app/Party"), params.clone());
        assert_eq!(nav.paths(), ["/apps/app/Party", "/apps/app/Party/Edit"]);
        // A sibling tab of the same record keeps the header.
        assert!(nav.set_leaf(model("/apps/app/Party/Roles"), params.clone()).is_empty());
        // Another record asks for it again.
        let other = BTreeMap::from([("partyId".to_string(), "P2".to_string())]);
        assert_eq!(nav.set_leaf(model("/apps/app/Party/Roles"), other), ["/apps/app/Party"]);
    }

    #[test]
    fn decoding_keeps_a_stray_percent_and_multibyte_text() {
        assert_eq!(decode("a%20b+c"), "a b c");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%é1"), "%é1");
        assert_eq!(decode("%C3%A9"), "é");
        assert_eq!(level_path("/apps/x/Party"), "/apps/x/Party.tui");
    }
}
