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

//! What the drawer says about the user's page (plan 4.11 "Follow on the page", 7.1): the browser state a turn or a
//! `context` call carries, read leniently and capped, with secret-named values dropped; from it the sync-in state of
//! the conversation's screen session ([`DrawerState::browser_state`]) and the `<context>` tail of the message
//! ([`DrawerState::tail_spec`]). And the follow stamp the agent's `ui.*` events carry ([`follow_stamp`]): the follow
//! owner's client id and the page version it reported, so only that tab applies them, and only for that page.

use std::collections::BTreeMap;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::entity::EntityFacade;
use crate::llm::prompt::{TailFilter, TailForm, TailRecord, TailSelection, TailSpec};
use crate::llm::screen::{BrowserForm, BrowserState, ScreenState};

/// The path is cut here.
const MAX_PATH: usize = 2000;
/// Page parameters kept, and their values' length.
const MAX_PARAMETERS: usize = 50;
const MAX_PARAMETER_CHARS: usize = 1000;
/// Forms kept, values per form, and a value's length.
pub const MAX_FORMS: usize = 20;
pub const MAX_FORM_VALUES: usize = 100;
pub const MAX_VALUE_CHARS: usize = 4000;
/// Open dialogs kept.
const MAX_DIALOGS: usize = 10;
/// Selections, filters and form summaries kept, and ids per selection.
const MAX_LISTED: usize = 10;
const MAX_IDS: usize = 20;

/// A record the page is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrawerRecord {
    pub entity_name: String,
    pub id: String,
    pub label: String,
}

/// The browser state of a drawer call (docs/Assist.md, "The page state").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrawerState {
    /// The screen path, `/qapps` written as `/apps` (the screen sessions' tree); empty when the client sent none or
    /// one that is not a plain path from the root.
    pub path: String,
    pub parameters: BTreeMap<String, String>,
    pub title: String,
    pub forms: Vec<BrowserForm>,
    pub dialogs: Vec<String>,
    pub record: Option<DrawerRecord>,
    pub selections: Vec<TailSelection>,
    pub filters: Vec<TailFilter>,
    pub form_info: Vec<TailForm>,
    /// A canvas open in the drawer: its title and version.
    pub canvas: Option<(String, u32)>,
}

/// A JSON value as one line of text: a string as it is, a number or boolean written, anything else empty.
fn scalar(value: &Json) -> Option<String> {
    match value {
        Json::String(text) => Some(text.clone()),
        Json::Number(number) => Some(number.to_string()),
        Json::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn text_at(value: &Json, key: &str, max: usize) -> String {
    value
        .get(key)
        .and_then(scalar)
        .map(|text| cut(text.trim(), max))
        .unwrap_or_default()
}

/// Whether a field name is one whose value never leaves the browser: [`crate::llm::tools::data::secret_name`] or one
/// of the conf's `redact-keys`.
pub fn secret_field(name: &str, extra: &[String]) -> bool {
    crate::llm::tools::data::secret_name(name) || extra.iter().any(|key| key.eq_ignore_ascii_case(name))
}

/// A path as the screen sessions take it: `/qapps/…` is `/apps/…`; one that is not a plain path from the root
/// ([`crate::llm::screen::path_problem`]) is none.
pub fn session_path(path: &str) -> Option<String> {
    let path = path.trim();
    let bare = path.split(['?', '#']).next().unwrap_or(path);
    if bare.is_empty() || crate::llm::screen::path_problem(bare).is_some() {
        return None;
    }
    let bare = cut(bare, MAX_PATH);
    Some(match bare.strip_prefix("/qapps") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("/apps{rest}"),
        _ => bare,
    })
}

fn string_map(value: Option<&Json>, max: usize, max_chars: usize, extra: &[String]) -> BTreeMap<String, String> {
    value
        .and_then(Json::as_object)
        .map(|map| {
            map.iter()
                .filter(|(name, _)| !name.is_empty() && !secret_field(name, extra))
                .filter_map(|(name, value)| scalar(value).map(|text| (cut(name, 200), cut(&text, max_chars))))
                .take(max)
                .collect()
        })
        .unwrap_or_default()
}

fn strings(value: Option<&Json>, max: usize) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(scalar)
                .map(|text| cut(text.trim(), 200))
                .filter(|text| !text.is_empty())
                .take(max)
                .collect()
        })
        .unwrap_or_default()
}

impl DrawerState {
    /// The state of a `state` parameter: an object, read leniently (a field of the wrong type is left out), capped,
    /// and with every secret-named value dropped (`extra` are the conf's `redact-keys`). Anything else is no state.
    pub fn of(value: &Json, extra: &[String]) -> Self {
        if !value.is_object() {
            return Self::default();
        }
        let path_text = value.get("path").and_then(Json::as_str).unwrap_or_default();
        let forms = value
            .get("forms")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|form| {
                        let dom_id = text_at(form, "domId", 200);
                        if dom_id.is_empty() {
                            return None;
                        }
                        let values = string_map(form.get("values"), MAX_FORM_VALUES, MAX_VALUE_CHARS, extra);
                        let dirty = strings(form.get("dirty"), MAX_FORM_VALUES)
                            .into_iter()
                            .filter(|name| values.contains_key(name))
                            .collect();
                        Some(BrowserForm { dom_id, values, dirty })
                    })
                    .take(MAX_FORMS)
                    .collect()
            })
            .unwrap_or_default();
        let record = value
            .get("record")
            .filter(|record| record.is_object())
            .and_then(|record| {
                let entity_name = text_at(record, "entityName", 200);
                let id = text_at(record, "id", 200);
                (!entity_name.is_empty() && !id.is_empty()).then(|| DrawerRecord {
                    entity_name,
                    id,
                    label: text_at(record, "label", 200),
                })
            });
        let listed = |key: &str| -> Vec<&Json> {
            value
                .get(key)
                .and_then(Json::as_array)
                .map(|items| items.iter().filter(|item| item.is_object()).take(MAX_LISTED).collect())
                .unwrap_or_default()
        };
        let selections = listed("selections")
            .into_iter()
            .map(|item| {
                let ids = strings(item.get("ids"), MAX_IDS);
                let total = item
                    .get("total")
                    .and_then(Json::as_u64)
                    .and_then(|total| usize::try_from(total).ok())
                    .unwrap_or(ids.len());
                TailSelection {
                    form: text_at(item, "form", 200),
                    ids,
                    total,
                }
            })
            .filter(|selection| !selection.ids.is_empty())
            .collect();
        let filters = listed("filters")
            .into_iter()
            .map(|item| TailFilter {
                form: text_at(item, "form", 200),
                values: string_map(item.get("values"), 20, 200, extra).into_iter().collect(),
            })
            .filter(|filter| !filter.values.is_empty())
            .collect();
        let form_info = listed("formInfo")
            .into_iter()
            .map(|item| TailForm {
                name: text_at(item, "name", 200),
                kind: text_at(item, "kind", 20),
                fields: strings(item.get("fields"), 20),
                rows: item
                    .get("rows")
                    .and_then(Json::as_u64)
                    .and_then(|rows| usize::try_from(rows).ok()),
            })
            .filter(|form| !form.name.is_empty())
            .collect();
        let canvas = value
            .get("canvas")
            .filter(|canvas| canvas.is_object())
            .and_then(|canvas| {
                let title = text_at(canvas, "title", 200);
                let version = canvas
                    .get("version")
                    .and_then(Json::as_u64)
                    .and_then(|version| u32::try_from(version).ok())
                    .unwrap_or(1);
                (!title.is_empty()).then_some((title, version))
            });
        Self {
            path: session_path(path_text).unwrap_or_default(),
            parameters: string_map(value.get("parameters"), MAX_PARAMETERS, MAX_PARAMETER_CHARS, extra),
            title: text_at(value, "title", 300),
            forms,
            dialogs: strings(value.get("dialogs"), MAX_DIALOGS),
            record,
            selections,
            filters,
            form_info,
            canvas,
        }
    }

    /// What the screen session takes (sync-in): the path, its parameters, the forms' changed values and the dialogs.
    /// `None` without a path.
    pub fn browser_state(&self) -> Option<BrowserState> {
        if self.path.is_empty() {
            return None;
        }
        Some(BrowserState {
            path: self.path.clone(),
            parameters: self.parameters.clone(),
            forms: self.forms.clone(),
            dialogs: self.dialogs.clone(),
        })
    }

    /// The state a new conversation keeps, so its first screen session opens on the user's page.
    pub fn screen_state(&self) -> Option<ScreenState> {
        let state = self.browser_state()?;
        Some(ScreenState {
            path: state.path,
            parameters: state.parameters,
            values: state
                .forms
                .iter()
                .map(|form| (form.dom_id.clone(), form.dirty_values()))
                .filter(|(_, values)| !values.is_empty())
                .collect(),
        })
    }

    /// The tail of a message sent from this page at `now`.
    pub fn tail_spec(&self, now: Option<jiff::Zoned>, week_start: &str) -> TailSpec {
        let page = (!self.path.is_empty()).then(|| {
            let url = ScreenState {
                path: self.path.clone(),
                parameters: self.parameters.clone(),
                values: BTreeMap::new(),
            }
            .url();
            (url, self.title.clone())
        });
        TailSpec {
            now,
            week_start: week_start.to_string(),
            page,
            record: self.record.as_ref().map(|record| TailRecord {
                entity_name: record.entity_name.clone(),
                id: record.id.clone(),
                label: record.label.clone(),
            }),
            selections: self.selections.clone(),
            filters: self.filters.clone(),
            forms: self.form_info.clone(),
            canvas: self.canvas.clone(),
            skills: Vec::new(),
            headless: false,
        }
    }
}

/// The current time in the user's time zone (the session's, else the server's), for the tail's `now` line.
pub fn user_now() -> jiff::Zoned {
    let zone = crate::user::time_zone()
        .ok()
        .flatten()
        .filter(|zone| !zone.is_empty())
        .and_then(|zone| jiff::tz::TimeZone::get(&zone).ok());
    match zone {
        Some(zone) => jiff::Timestamp::now().to_zoned(zone),
        None => jiff::Zoned::now(),
    }
}

/// The first day of the week for a locale: Sunday where the calendar starts there (the United States, Canada,
/// Japan, Brazil and a few more), else Monday.
pub fn week_start(locale: &str) -> &'static str {
    let region = locale.split(['_', '-']).nth(1).unwrap_or_default().to_ascii_uppercase();
    match region.as_str() {
        "US" | "CA" | "JP" | "BR" | "MX" | "IL" | "PH" | "KR" | "TW" | "HK" | "IN" | "ZA" | "AU" => "Sunday",
        _ => "Monday",
    }
}

/// Who follows a conversation's agent, as its `ui.*` events say: the follow owner's client id and the page version
/// its tab last reported (`AgentConversation.clientId`, `contextVersion`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FollowStamp {
    pub client_id: Option<String>,
    pub context_version: i64,
}

impl FollowStamp {
    /// `payload` with `clientId` and `contextVersion` added (an object only).
    pub fn apply(&self, payload: &mut Json) {
        if let Json::Object(map) = payload {
            map.insert("clientId".to_string(), json!(self.client_id));
            map.insert("contextVersion".to_string(), json!(self.context_version));
        }
    }
}

/// The follow stamp of a conversation now; a read that fails is logged and stamps nothing anyone follows.
pub fn follow_stamp(facade: &EntityFacade, conversation_id: &str) -> FollowStamp {
    let read = crate::llm::run::quietly(|| {
        facade
            .find(crate::llm::run::CONVERSATION)?
            .eq("conversationId", Value::string(conversation_id))
            .disable_authz()
            .one()
    });
    match read {
        Ok(Some(row)) => FollowStamp {
            client_id: crate::llm::text_of(&row, "clientId"),
            context_version: crate::llm::int_of(&row, "contextVersion").unwrap_or(0),
        },
        Ok(None) => FollowStamp::default(),
        Err(error) => {
            tracing::error!("the follow owner of agent conversation {conversation_id} could not be read: {error}");
            FollowStamp::default()
        }
    }
}
