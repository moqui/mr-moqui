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

//! `ask` (plans/LLM-A2A-Assist.md 7.3): the agent asks the user before it goes on, with 2 to 4 short options, the one it
//! recommends first, or a few fields to fill; the user may also type an answer. It is offered where someone can answer
//! (`question.inline` in the drawer and a chat room, `question.data` for A2A, where the question is the
//! `input-required` data part); a job has no `ask`. The call stores an `AgentQuestion` (the one write of this read tool,
//! with the fence lifted for it: run bookkeeping, as the budget question is) and pauses the run with a
//! `question.asked` event; the answer (`submit#AgentInput` of kind answer, or a message) resumes it, and the model reads
//! what the user chose. A form's fields are typed ([`FieldType`], Twenty's `request_form`): text, a long text, a number,
//! a date, a yes-or-no, a choice of options, or a record picker that searches as the search box does; each may be
//! required. A form's answer is checked against its fields before the run takes it ([`check_answer`]), and the model
//! reads the values normalized.

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::format::ToolError;
use super::schema::{object, Property};
use super::{caps, AgentTool, Classification, Pause, PauseKind, ToolInput, ToolOutcome, ToolSpec};
use crate::entity::fence;
use crate::llm::run::context::TurnContext;

const QUESTION: &str = "moqui.agent.AgentQuestion";
const MAX_OPTIONS: usize = 4;
const MAX_FIELDS: usize = 6;
const OPTION_CAP: usize = 80;

pub struct Ask(ToolSpec);

impl Default for Ask {
    fn default() -> Self {
        let mut schema = object(&[
            Property::required("question", "string", ""),
            Property::optional("options", "array", "2 to 4 short answers, recommended first"),
            Property::optional("multi", "boolean", "More than one may be chosen"),
            Property::optional("fields", "array", "A form instead: names or {name, label, type, required, options, search}"),
        ]);
        schema["properties"]["options"]["items"] = json!({"type": "string"});
        schema["properties"]["fields"]["items"] = json!({
            "type": ["object", "string"],
            "properties": {
                "name": {"type": "string"},
                "label": {"type": "string"},
                "type": {"type": "string", "enum": FieldType::ALL.iter().map(|kind| kind.as_str()).collect::<Vec<_>>()},
                "required": {"type": "boolean"},
                "options": {"type": "array", "items": {"type": "string"}},
                "search": {"type": "string", "description": "record: words to search"},
            },
        });
        Self(ToolSpec {
            may_pause: true,
            ..ToolSpec::new(
                "ask",
                "Ask the user before you go on: 2 to 4 short options (they may also type an answer), or a few fields to fill. Your last action in the turn.",
                schema,
                Classification::Read,
            )
            .needs_any(&[caps::QUESTION_INLINE, caps::QUESTION_DATA])
        })
    }
}

/// The type of a form field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldType {
    #[default]
    Text,
    /// Several lines.
    Textarea,
    Number,
    Integer,
    /// `YYYY-MM-DD`.
    Date,
    /// `YYYY-MM-DD HH:mm`.
    DateTime,
    /// Yes or no.
    Boolean,
    /// One of the field's options.
    Choice,
    /// A record picked by searching, as the search box does; the answer is its id and label.
    Record,
}

impl FieldType {
    pub const ALL: [FieldType; 9] = [
        FieldType::Text,
        FieldType::Textarea,
        FieldType::Number,
        FieldType::Integer,
        FieldType::Date,
        FieldType::DateTime,
        FieldType::Boolean,
        FieldType::Choice,
        FieldType::Record,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Textarea => "textarea",
            FieldType::Number => "number",
            FieldType::Integer => "integer",
            FieldType::Date => "date",
            FieldType::DateTime => "date-time",
            FieldType::Boolean => "boolean",
            FieldType::Choice => "choice",
            FieldType::Record => "record",
        }
    }

    /// A type as the model wrote it, its common other names included; an unknown one is text.
    pub fn parse(text: &str) -> Self {
        let text = text.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        match text.as_str() {
            "textarea" | "long-text" | "multiline" | "memo" => FieldType::Textarea,
            "number" | "decimal" | "float" | "amount" | "currency" => FieldType::Number,
            "integer" | "int" | "long" | "count" => FieldType::Integer,
            "date" => FieldType::Date,
            "date-time" | "datetime" | "timestamp" => FieldType::DateTime,
            "boolean" | "bool" | "checkbox" | "yes-no" => FieldType::Boolean,
            "choice" | "select" | "drop-down" | "dropdown" | "enum" | "radio" => FieldType::Choice,
            "record" | "picker" | "lookup" | "reference" => FieldType::Record,
            _ => FieldType::Text,
        }
    }
}

/// A field the user fills: its name, label and type, whether it is required, the options of a choice, and what a record
/// picker searches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub kind: FieldType,
    pub required: bool,
    pub options: Vec<String>,
    /// A record picker's search words (the search box's groups, such as people or records).
    pub search: Option<String>,
}

impl Field {
    /// A text field from what the model wrote: `name`, `Label`, or `name: Label`.
    pub fn of(text: &str) -> Self {
        let (name, label) = match text.split_once(':') {
            Some((name, label)) => (name.trim(), label.trim()),
            None => (text.trim(), text.trim()),
        };
        Self {
            name: camel(name),
            label: label.to_string(),
            ..Self::default()
        }
    }

    /// A field from an object: `{name, label, type, required, options, search}`; a name is made from the label when
    /// it has none.
    pub fn of_json(value: &Json) -> Option<Self> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let label = text("label");
        let name = text("name").or_else(|| label.clone())?;
        let mut options: Vec<String> = Vec::new();
        for option in value.get("options").and_then(Json::as_array).into_iter().flatten() {
            let option = match option {
                Json::String(text) => text.trim().to_string(),
                other => other
                    .get("label")
                    .and_then(Json::as_str)
                    .map_or_else(|| other.to_string(), |text| text.trim().to_string()),
            };
            if !option.is_empty() && !options.iter().any(|known| known.eq_ignore_ascii_case(&option)) {
                options.push(super::format::cut(&option, OPTION_CAP));
            }
        }
        let kind = match text("type") {
            Some(kind) => FieldType::parse(&kind),
            None if !options.is_empty() => FieldType::Choice,
            None => FieldType::Text,
        };
        let required = match value.get("required") {
            Some(Json::Bool(flag)) => *flag,
            Some(Json::String(text)) => matches!(text.trim().to_ascii_lowercase().as_str(), "true" | "y" | "yes"),
            _ => false,
        };
        Some(Self {
            label: label.unwrap_or_else(|| name.clone()),
            name: camel(&name),
            kind: if kind == FieldType::Choice && options.is_empty() {
                FieldType::Text
            } else {
                kind
            },
            required,
            options,
            search: text("search").or_else(|| text("entity")),
        })
    }

    pub fn to_json(&self) -> Json {
        let mut field = json!({"name": self.name, "label": self.label, "type": self.kind.as_str()});
        if self.required {
            field["required"] = json!(true);
        }
        if !self.options.is_empty() {
            field["options"] = json!(self.options);
        }
        if let Some(search) = &self.search {
            field["search"] = json!(search);
        }
        field
    }

    /// The value as the field takes it, normalized (a number as a number, a yes-or-no as a boolean, a choice as its
    /// option, a record as `{id, label}`), or why it does not fit. `None` is no value.
    fn check(&self, value: &Json) -> Result<Option<Json>, String> {
        let text = match value {
            Json::Null => String::new(),
            Json::String(text) => text.trim().to_string(),
            Json::Object(_) if self.kind == FieldType::Record => String::new(),
            other => other.to_string(),
        };
        if text.is_empty() && !(self.kind == FieldType::Record && value.is_object()) {
            return Ok(None);
        }
        let label = &self.label;
        match self.kind {
            FieldType::Text | FieldType::Textarea => Ok(Some(json!(text))),
            FieldType::Number => text
                .replace(',', "")
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .and_then(serde_json::Number::from_f64)
                .map(|number| Some(Json::Number(number)))
                .ok_or_else(|| format!("{label} is a number")),
            FieldType::Integer => text
                .replace(',', "")
                .parse::<i64>()
                .map(|number| Some(json!(number)))
                .map_err(|_| format!("{label} is a whole number")),
            FieldType::Date => text
                .parse::<jiff::civil::Date>()
                .map(|date| Some(json!(date.to_string())))
                .map_err(|_| format!("{label} is a date as YYYY-MM-DD")),
            FieldType::DateTime => text
                .replacen(' ', "T", 1)
                .parse::<jiff::civil::DateTime>()
                .map(|at| Some(json!(at.strftime("%Y-%m-%d %H:%M").to_string())))
                .map_err(|_| format!("{label} is a date and time as YYYY-MM-DD HH:mm")),
            FieldType::Boolean => match text.to_ascii_lowercase().as_str() {
                "true" | "y" | "yes" | "on" | "1" => Ok(Some(json!(true))),
                "false" | "n" | "no" | "off" | "0" => Ok(Some(json!(false))),
                _ => Err(format!("{label} is yes or no")),
            },
            FieldType::Choice => self
                .options
                .iter()
                .find(|option| option.eq_ignore_ascii_case(&text))
                .map(|option| Some(json!(option)))
                .ok_or_else(|| format!("{label} is one of {}", self.options.join(", "))),
            FieldType::Record => {
                let (id, shown) = match value {
                    Json::Object(map) => (
                        map.get("id").or_else(|| map.get("value")).map(|id| match id {
                            Json::String(text) => text.trim().to_string(),
                            other => other.to_string(),
                        }),
                        map.get("label").and_then(Json::as_str).map(str::to_string),
                    ),
                    _ => (Some(text.clone()), None),
                };
                match id.filter(|id| !id.is_empty()) {
                    Some(id) => Ok(Some(json!({"id": id, "label": shown.unwrap_or_else(|| id.clone())}))),
                    None => Err(format!("{label} is a record picked from its search")),
                }
            }
        }
    }
}

/// `firstName` of `First name`.
fn camel(text: &str) -> String {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .enumerate()
        .map(|(at, word)| {
            if at == 0 {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_lowercase().chain(chars).collect()
                })
            } else {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect()
                })
            }
        })
        .collect()
}

/// A question as the clients get it.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
    pub multi: bool,
    pub fields: Vec<Field>,
}

impl Question {
    /// The question of a call's arguments.
    ///
    /// # Errors
    ///
    /// Returns an error the model reads: no question, one option or more than four, too many fields.
    pub fn of(args: &Json) -> Result<Self, ToolError> {
        let text = args
            .get("question")
            .and_then(Json::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if text.is_empty() {
            return Err(ToolError::new(
                "no_question",
                "Call ask with the question and 2 to 4 short options, the one you recommend first.",
            ));
        }
        let list = |key: &str| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for item in args.get(key).and_then(Json::as_array).into_iter().flatten() {
                let item = match item {
                    Json::String(text) => text.trim().to_string(),
                    Json::Object(map) => map
                        .get("label")
                        .or_else(|| map.get("name"))
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    other => other.to_string(),
                };
                if !item.is_empty() && !out.iter().any(|known| known.eq_ignore_ascii_case(&item)) {
                    out.push(item);
                }
            }
            out
        };
        let options: Vec<String> = list("options")
            .into_iter()
            .map(|option| super::format::cut(&option, OPTION_CAP))
            .collect();
        let mut fields: Vec<Field> = Vec::new();
        for item in args.get("fields").and_then(Json::as_array).into_iter().flatten() {
            let field = match item {
                Json::String(text) if !text.trim().is_empty() => Some(Field::of(text)),
                Json::Object(_) => Field::of_json(item),
                _ => None,
            };
            if let Some(field) = field.filter(|field| !field.name.is_empty()) {
                if !fields.iter().any(|known| known.name == field.name) {
                    fields.push(field);
                }
            }
        }
        if fields.is_empty() && !(2..=MAX_OPTIONS).contains(&options.len()) {
            return Err(ToolError::new(
                "options",
                format!(
                    "ask takes 2 to 4 options, and this call has {}. Call ask again with 2 to 4 short options, the one you recommend first, or with fields.",
                    options.len()
                ),
            ));
        }
        if fields.len() > MAX_FIELDS {
            return Err(ToolError::new(
                "fields",
                format!("ask takes at most {MAX_FIELDS} fields. Ask for fewer, or open the screen that has them."),
            ));
        }
        Ok(Self {
            text,
            options,
            multi: args.get("multi").and_then(Json::as_bool).unwrap_or(false),
            fields,
        })
    }

    /// The event's data: the question, its options (the first recommended) and its fields.
    pub fn to_json(&self) -> Json {
        json!({
            "text": self.text,
            "options": self.options.iter().enumerate().map(|(at, label)| json!({"label": label, "recommended": at == 0}))
                .collect::<Vec<_>>(),
            "multi": self.multi,
            "fields": self.fields.iter().map(Field::to_json).collect::<Vec<_>>(),
            "freeText": true,
        })
    }

    /// What the model reads of an answer: the options chosen (by label, by number, or by data `choice` or `choices`),
    /// the fields filled (data `values`, or the data object itself), else the text the user typed.
    pub fn answer_text(&self, input: &ToolInput) -> (String, Json) {
        let data = input.data.clone().unwrap_or(Json::Null);
        let mut picked: Vec<String> = Vec::new();
        let named = |text: &str| -> Option<String> {
            let text = text.trim();
            if let Ok(number) = text.parse::<usize>() {
                if (1..=self.options.len()).contains(&number) {
                    return Some(self.options[number - 1].clone());
                }
            }
            self.options
                .iter()
                .find(|option| option.eq_ignore_ascii_case(text))
                .cloned()
        };
        for key in ["choice", "choices"] {
            match data.get(key) {
                Some(Json::String(text)) => picked.extend(named(text)),
                Some(Json::Array(items)) => picked.extend(items.iter().filter_map(Json::as_str).filter_map(named)),
                Some(Json::Number(number)) => picked.extend(named(&number.to_string())),
                _ => {}
            }
        }
        if picked.is_empty() && !self.options.is_empty() {
            let parts: Vec<&str> = if self.multi {
                input.text.split(',').collect()
            } else {
                vec![input.text.as_str()]
            };
            let all: Vec<Option<String>> = parts.iter().map(|part| named(part)).collect();
            if !all.is_empty() && all.iter().all(Option::is_some) {
                picked = all.into_iter().flatten().collect();
            }
        }
        if !picked.is_empty() {
            let chosen: Vec<String> = picked.iter().map(|label| format!("\"{label}\"")).collect();
            return (
                format!("The user chose {}.", chosen.join(", ")),
                json!({"choices": picked}),
            );
        }
        if !self.fields.is_empty() {
            let values = data.get("values").cloned().unwrap_or_else(|| data.clone());
            if let Some(values) = values.as_object().filter(|values| !values.is_empty()) {
                let checked = match self.check_values(values) {
                    Ok(checked) => checked,
                    Err(problems) => {
                        return (
                            format!(
                                "The user's answer does not fit the form ({}); ask again or go on with what you have.",
                                problems.join("; ")
                            ),
                            json!({"values": values, "problems": problems}),
                        )
                    }
                };
                let filled: Vec<String> = self
                    .fields
                    .iter()
                    .map(|field| {
                        let value = match checked.get(&field.name) {
                            None | Some(Json::Null) => String::new(),
                            Some(Json::String(text)) => text.clone(),
                            Some(Json::Object(record)) => {
                                let id = record.get("id").and_then(Json::as_str).unwrap_or_default();
                                let label = record.get("label").and_then(Json::as_str).unwrap_or(id);
                                if label == id {
                                    id.to_string()
                                } else {
                                    format!("{label} (id {id})")
                                }
                            }
                            Some(other) => other.to_string(),
                        };
                        format!("{}: {value}", field.name)
                    })
                    .collect();
                return (
                    format!("The user filled: {}.", filled.join(", ")),
                    json!({"values": checked}),
                );
            }
        }
        let typed = input.text.trim();
        if typed.is_empty() {
            return ("The user answered nothing.".to_string(), json!({"text": ""}));
        }
        (format!("The user answered: {typed}"), json!({"text": typed}))
    }
}

impl Question {
    /// The values of a form's answer, each normalized by its field ([`Field::check`]); a name the form does not have is
    /// left out.
    ///
    /// # Errors
    ///
    /// What does not fit: a required field left empty, a value of the wrong type, a choice not offered.
    pub fn check_values(&self, values: &serde_json::Map<String, Json>) -> Result<serde_json::Map<String, Json>, Vec<String>> {
        let mut checked = serde_json::Map::new();
        let mut problems = Vec::new();
        for field in &self.fields {
            match field.check(values.get(&field.name).unwrap_or(&Json::Null)) {
                Ok(Some(value)) => {
                    checked.insert(field.name.clone(), value);
                }
                Ok(None) if field.required => problems.push(format!("{} is required", field.label)),
                Ok(None) => {}
                Err(problem) => problems.push(problem),
            }
        }
        if problems.is_empty() {
            Ok(checked)
        } else {
            Err(problems)
        }
    }

    /// The question stored as `AgentQuestion` (its text, `choicesJson`, `schemaJson`).
    fn of_row(row: &crate::entity::EntityValue) -> Self {
        let json_of = |field: &str| {
            crate::llm::text_of(row, field)
                .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                .unwrap_or(Json::Null)
        };
        Self {
            text: crate::llm::text_of(row, "questionText").unwrap_or_default(),
            options: json_of("choicesJson")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|option| option.get("label").and_then(Json::as_str).map(str::to_string))
                .collect(),
            multi: false,
            fields: json_of("schemaJson")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Field::of_json)
                .collect(),
        }
    }
}

/// Check an answer to the question `question_id` before the run takes it: a form's values (`data.values`, or the data
/// object) must fit its fields. A choice, a typed answer or a question with no form is taken as it is.
///
/// # Errors
///
/// What the user must fix, for the drawer to show.
pub fn check_answer(
    facade: &crate::entity::EntityFacade,
    question_id: &str,
    data: Option<&Json>,
) -> Result<(), String> {
    let Some(data) = data.filter(|data| data.is_object()) else {
        return Ok(());
    };
    if data.get("choice").is_some() || data.get("choices").is_some() {
        return Ok(());
    }
    let row = facade
        .find(QUESTION)
        .and_then(|find| find.eq("questionId", Value::string(question_id)).disable_authz().one())
        .map_err(|error| format!("the question could not be read: {error}"))?;
    let Some(row) = row else {
        return Ok(());
    };
    let question = Question::of_row(&row);
    if question.fields.is_empty() {
        return Ok(());
    }
    let values = data.get("values").unwrap_or(data);
    let Some(values) = values.as_object() else {
        return Err("a form's answer is its values by field name".to_string());
    };
    question
        .check_values(values)
        .map(|_| ())
        .map_err(|problems| problems.join("; "))
}

impl AgentTool for Ask {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let question = match Question::of(&args) {
            Ok(question) => question,
            Err(error) => return error.outcome(),
        };
        let Some(run) = ctx.run else {
            return ToolError::new(
                "no_run",
                "There is no one to ask here. Go on with what you have, and say what you assumed.",
            )
            .outcome();
        };
        let Some(facade) = super::apps::factory().ok().and_then(|factory| factory.entity()) else {
            return ToolError::new("unavailable", "The question could not be asked now.").outcome();
        };
        let data = question.to_json();
        // the question row is run bookkeeping the runtime writes, as the budget question is, so the fence lifts for it
        // under the run's lease, in one transaction: a node that lost the run leaves no open question behind
        let node = ctx.events.node().to_string();
        let stored = {
            let _lifted = fence::lift();
            crate::llm::run::quietly(|| {
                crate::llm::run::in_tx(&facade, || {
                    crate::llm::run::held(&facade, &run.run_id, &node, crate::llm::run::Hold::Lease)?;
                    let mut row = facade.make_value(QUESTION)?;
                    row.set_sequenced_id_primary(&facade)?;
                    row.set("runId", Value::string(&run.run_id));
                    row.set("toolCallId", Value::string(&ctx.call.tool_call_id));
                    row.set("userId", Value::string(&ctx.user.user_id));
                    row.set("questionText", Value::string(&question.text));
                    row.set("choicesJson", Value::string(data["options"].to_string()));
                    if !question.fields.is_empty() {
                        row.set("schemaJson", Value::string(data["fields"].to_string()));
                    }
                    row.set("statusId", Value::string("AgqOpen"));
                    row.set("createdDate", crate::llm::run::stamp(crate::llm::run::now_millis()));
                    row.create(&facade)?;
                    Ok(crate::llm::text_of(&row, "questionId").unwrap_or_default())
                })
            })
        };
        let question_id = match stored {
            Ok(id) => id,
            Err(error) => {
                tracing::error!("the question of agent run {} could not be stored: {error}", run.run_id);
                return ToolError::new(
                    "unavailable",
                    "The question could not be asked now. Go on, and say what you assumed.",
                )
                .outcome();
            }
        };
        let mut payload = data;
        payload["questionId"] = json!(question_id);
        payload["toolCallId"] = json!(ctx.call.tool_call_id);
        payload["surface"] = json!(ctx.surface.as_str());
        ToolOutcome::Pause(Pause {
            kind: PauseKind::Question,
            reference: question_id,
            expires: None,
            event: Some(("question.asked".to_string(), payload)),
        })
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, args: Json, input: &ToolInput) -> ToolOutcome {
        let question = match Question::of(&args) {
            Ok(question) => question,
            Err(error) => return error.outcome(),
        };
        let (text, answer) = question.answer_text(input);
        if let (Some(run), Some(facade)) = (
            ctx.run,
            super::apps::factory().ok().and_then(|factory| factory.entity()),
        ) {
            let _lifted = fence::lift();
            let node = ctx.events.node().to_string();
            let answered = crate::llm::run::quietly(|| {
                crate::llm::run::in_tx(&facade, || {
                    crate::llm::run::held(&facade, &run.run_id, &node, crate::llm::run::Hold::Lease)?;
                    facade
                        .find(QUESTION)?
                        .eq("runId", Value::string(&run.run_id))
                        .eq("toolCallId", Value::string(&ctx.call.tool_call_id))
                        .eq("statusId", Value::string("AgqOpen"))
                        .update_all([
                            ("statusId".to_string(), Value::string("AgqAnswered")),
                            ("answerJson".to_string(), Value::string(answer.to_string())),
                            (
                                "answeredDate".to_string(),
                                crate::llm::run::stamp(crate::llm::run::now_millis()),
                            ),
                        ])
                })
            });
            if let Err(error) = answered {
                tracing::error!("the answer of agent run {} could not be stored: {error}", run.run_id);
            }
        }
        ctx.events.emit(
            "question.answered",
            json!({"toolCallId": ctx.call.tool_call_id, "answer": answer}),
        );
        ToolOutcome::text(text)
    }
}

crate::submit_tool!(Ask);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_takes_two_to_four_options_or_fields() {
        let question =
            Question::of(&json!({"question": "Which one?", "options": ["Acme Corp", "Acme Inc", "acme corp"]}))
                .unwrap();
        assert_eq!(question.options, ["Acme Corp", "Acme Inc"]);
        assert_eq!(
            question.to_json()["options"][0],
            json!({"label": "Acme Corp", "recommended": true})
        );
        let one = Question::of(&json!({"question": "Sure?", "options": ["Yes"]}))
            .unwrap_err()
            .to_text();
        assert!(
            one.starts_with("{\"error\":\"options\"") && one.contains("has 1"),
            "{one}"
        );
        assert!(Question::of(&json!({"options": ["a", "b"]})).is_err());
        let fields =
            Question::of(&json!({"question": "Who?", "fields": ["First name", "email: Email address"]})).unwrap();
        assert_eq!(
            fields.fields,
            [
                Field {
                    name: "firstName".to_string(),
                    label: "First name".to_string(),
                    ..Field::default()
                },
                Field {
                    name: "email".to_string(),
                    label: "Email address".to_string(),
                    ..Field::default()
                },
            ]
        );
    }

    #[test]
    fn a_form_has_typed_fields_and_its_answer_is_checked_and_normalized() {
        let form = Question::of(&json!({"question": "The new order?", "fields": [
            {"name": "customer", "label": "Customer", "type": "record", "required": true, "search": "people"},
            {"label": "Quantity", "type": "int", "required": true},
            {"name": "shipBy", "label": "Ship by", "type": "date"},
            {"name": "rush", "label": "Rush", "type": "boolean"},
            {"name": "carrier", "label": "Carrier", "options": ["UPS", "FedEx"]},
            "note: Note"]}))
        .unwrap();
        let kinds: Vec<FieldType> = form.fields.iter().map(|field| field.kind).collect();
        assert_eq!(
            kinds,
            [
                FieldType::Record,
                FieldType::Integer,
                FieldType::Date,
                FieldType::Boolean,
                FieldType::Choice,
                FieldType::Text
            ]
        );
        assert_eq!(form.fields[1].name, "quantity", "a name made from the label");
        assert_eq!(
            form.to_json()["fields"][0],
            json!({"name": "customer", "label": "Customer", "type": "record", "required": true, "search": "people"})
        );
        let values = |value: Json| value.as_object().unwrap().clone();
        let problems = form
            .check_values(&values(json!({"quantity": "many", "shipBy": "next week", "carrier": "DHL"})))
            .unwrap_err();
        assert_eq!(
            problems,
            [
                "Customer is required",
                "Quantity is a whole number",
                "Ship by is a date as YYYY-MM-DD",
                "Carrier is one of UPS, FedEx"
            ]
        );
        let checked = form
            .check_values(&values(json!({"customer": {"id": "100050", "label": "Acme Corp"}, "quantity": "3",
                "shipBy": "2026-11-02", "rush": "Y", "carrier": "fedex", "extra": "dropped"})))
            .unwrap();
        assert_eq!(
            Json::Object(checked),
            json!({"customer": {"id": "100050", "label": "Acme Corp"}, "quantity": 3, "shipBy": "2026-11-02",
                "rush": true, "carrier": "FedEx"})
        );
        let input = ToolInput {
            kind: "answer".to_string(),
            text: String::new(),
            data: Some(json!({"values": {"customer": {"id": "100050", "label": "Acme Corp"}, "quantity": 3}})),
        };
        assert_eq!(
            form.answer_text(&input).0,
            "The user filled: customer: Acme Corp (id 100050), quantity: 3, shipBy: , rush: , carrier: , note: ."
        );
    }

    #[test]
    fn an_answer_is_an_option_by_label_number_or_data_else_the_text() {
        let question = Question::of(&json!({"question": "Which?", "options": ["Acme Corp", "Acme Inc"]})).unwrap();
        let input = |text: &str, data: Option<Json>| ToolInput {
            kind: "answer".to_string(),
            text: text.to_string(),
            data,
        };
        assert_eq!(
            question.answer_text(&input("acme inc", None)).0,
            "The user chose \"Acme Inc\"."
        );
        assert_eq!(
            question.answer_text(&input("1", None)).0,
            "The user chose \"Acme Corp\"."
        );
        assert_eq!(
            question.answer_text(&input("", Some(json!({"choice": "Acme Inc"})))).0,
            "The user chose \"Acme Inc\"."
        );
        assert_eq!(
            question.answer_text(&input("Neither, use Beta LLC", None)).0,
            "The user answered: Neither, use Beta LLC"
        );
        let multi = Question::of(&json!({"question": "Which?", "options": ["A", "B", "C"], "multi": true})).unwrap();
        assert_eq!(
            multi.answer_text(&input("a, c", None)).0,
            "The user chose \"A\", \"C\"."
        );
        let form = Question::of(&json!({"question": "Who?", "fields": ["First name"]})).unwrap();
        assert_eq!(
            form.answer_text(&input("", Some(json!({"values": {"firstName": "Jo"}}))))
                .0,
            "The user filled: firstName: Jo."
        );
    }
}
