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

//! Saved finds and the columns a user picks for a form-list, a port of the `ScreenForm` statics and of the parts of
//! `ScreenRenderImpl` and `ScreenDefinition` that use them:
//!
//! - at the start of a render, the user's default saved find for the target screen, `formListFindId=_clear`, and the
//!   saved find's parameters when the request has none of its own; and the `screen.user.page.size` preference;
//! - the implicit transitions `formSaveFind` (save, update, delete, make default, clear default, schedule),
//!   `formSelectColumns` (the `FormConfig` of the user's column picker) and `screenDoc` (a `ScreenDocument` by index);
//! - the columns of a form-list from the active saved find's `FormConfig`, else the user's, else a group's.
//!
//! A form location is Java's: the screen location, `.form_list$` or `.form_single$`, and the form name, so saved finds
//! and column configurations written by the Java framework still apply. A dynamic form's saved finds also take `#`
//! and the entity or service. A posted `formLocation` that is only a form name (what the column dialog sends) is
//! taken as a form of the screen the transition is on.

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};
use crate::script;

fn facade() -> Result<EntityFacade, Error> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))
}

fn text(value: &EntityValue, field: &str) -> String {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn context_text(name: &str) -> String {
    script::context_value(name)
        .filter(|value| !value.is_null())
        .map(|value| moqui_lib::object::to_plain_string(&value))
        .unwrap_or_default()
}

fn context_has(name: &str) -> bool {
    script::context_value(name).is_some_and(|value| !value.is_null())
}

fn add_error(message: &str) {
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

fn user_id() -> Option<String> {
    crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

/// Template source that expands to `text` itself: `script::expand` keeps `\${` as the literal `${`, and a `\` before
/// anything else is already literal. User text that goes into a template, such as a saved find's description in a
/// scheduled email subject, goes through this so it is never run.
pub(crate) fn template_literal(text: &str) -> String {
    text.replace("${", "\\${")
}

/// The rows of `entity` with these field values.
fn rows(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) -> Result<Vec<EntityValue>, Error> {
    let mut find = facade.find(entity)?;
    for (name, value) in fields {
        find = find.eq(name, Value::string(*value));
    }
    Ok(find.list()?.into_values())
}

fn delete_all(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) -> Result<(), Error> {
    for row in rows(facade, entity, fields)? {
        row.delete(facade)?;
    }
    Ok(())
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) -> Result<EntityValue, Error> {
    let mut value = facade.make_value(entity)?;
    for (name, field) in fields {
        value.set(*name, field.clone());
    }
    value.create(facade)?;
    Ok(value)
}

/// Java's `ScreenForm.location`: `{screen}.form_list${name}` or `.form_single$`.
pub(crate) fn form_location(screen_location: &str, list: bool, form_name: &str) -> String {
    format!(
        "{screen_location}.{}${form_name}",
        if list { "form_list" } else { "form_single" }
    )
}

/// The location a dynamic form's saved finds are kept under, as `getSavedFindFullLocation`.
///
/// `form` is the source form: assembly consumes `auto-fields-service` and `auto-fields-entity`. The extension is the
/// context's `formLocationExtension`, else the first auto service name, else the first auto entity name. That is
/// Java's order. The assembler prefers the expanded name and falls back to the context; this key does not. Blank and
/// the literal `null` are missing, as the assembler treats them.
pub(crate) fn saved_find_location(location: &str, dynamic: bool, form: &MNode) -> String {
    if !dynamic {
        return location.to_string();
    }
    let extension = form_location_extension(form);
    if extension.is_empty() {
        location.to_string()
    } else {
        format!("{location}#{extension}")
    }
}

/// The `#` suffix of a dynamic saved find. Empty when none of the three sources has one.
fn form_location_extension(form: &MNode) -> String {
    if let Some(text) = present_extension(context_text("formLocationExtension")) {
        return text;
    }
    for (element, attribute) in [
        ("auto-fields-service", "service-name"),
        ("auto-fields-entity", "entity-name"),
    ] {
        let Some(node) = form.first_named(element) else {
            continue;
        };
        let raw = node.attribute(attribute).unwrap_or_default();
        let expanded = match script::expand(&raw) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!(error = %err, "saved find location extension did not expand");
                String::new()
            }
        };
        if let Some(text) = present_extension(expanded) {
            return text;
        }
    }
    String::new()
}

fn present_extension(text: String) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text == "null" {
        None
    } else {
        Some(text.to_string())
    }
}

/// `makeFormListFindParameters`: the request parameters a saved find stands for, `formListFindId` first.
pub(crate) fn find_parameters(form_list_find_id: &str) -> Result<IndexMap<String, String>, Error> {
    let facade = facade()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut out = IndexMap::new();
    out.insert("formListFindId".to_string(), form_list_find_id.to_string());
    for field in rows(
        &facade,
        "moqui.screen.form.FormListFindField",
        &[("formListFindId", form_list_find_id)],
    )? {
        let name = text(&field, "fieldName");
        let value = text(&field, "fieldValue");
        if !value.is_empty() {
            out.insert(name.clone(), value);
            let operator = text(&field, "fieldOperator");
            if !operator.is_empty() && operator != "equals" {
                out.insert(format!("{name}_op"), operator);
            }
            if text(&field, "fieldNot") == "Y" {
                out.insert(format!("{name}_not"), "Y".to_string());
            }
            if text(&field, "fieldIgnoreCase") == "Y" {
                out.insert(format!("{name}_ic"), "Y".to_string());
            }
        } else if !text(&field, "fieldPeriod").is_empty() {
            out.insert(format!("{name}_period"), text(&field, "fieldPeriod"));
            out.insert(format!("{name}_poffset"), text(&field, "fieldPerOffset"));
        } else {
            for (suffix, column) in [("_from", "fieldFrom"), ("_thru", "fieldThru")] {
                let value = text(&field, column);
                if !value.is_empty() {
                    out.insert(format!("{name}{suffix}"), value);
                }
            }
        }
    }
    Ok(out)
}

/// At the start of a render of `target_location`: the user's default saved find when the request names none, `_clear`,
/// and the saved find's parameters when the request has none of its own (`target_parameters`, the screen's own
/// parameters, do not count). Then `pageSize`: a request value is saved as the `screen.user.page.size` preference, and
/// with none the preference is the page size.
pub(crate) fn apply_on_render(
    target_location: &str,
    target_parameters: &[String],
    with_request: bool,
) -> Result<(), Error> {
    let Ok(facade) = facade() else {
        return Ok(());
    };
    let user = user_id();
    let mut find_id = context_text("formListFindId");
    if find_id.is_empty() {
        if let Some(user) = &user {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            let default = rows(
                &facade,
                "moqui.screen.form.FormListFindUserDefault",
                &[("userId", user), ("screenLocation", target_location)],
            )?;
            if let Some(found) = default.first() {
                find_id = text(found, "formListFindId");
                script::set_context_name("formListFindId", Value::string(find_id.clone()))?;
            }
        }
    }
    if find_id == "_clear" {
        find_id.clear();
        script::set_context_name("formListFindId", Value::Null)?;
    }
    if !find_id.is_empty() {
        let parameters = find_parameters(&find_id)?;
        let has_own = parameters.keys().any(|name| {
            name != "formListFindId"
                && !target_parameters.contains(name)
                && script::context_value(name).is_some_and(|value| !moqui_lib::object::is_empty(&value))
        });
        if !has_own {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            let find = rows(
                &facade,
                "moqui.screen.form.FormListFind",
                &[("formListFindId", &find_id)],
            )?;
            if let Some(find) = find.first() {
                let order = text(find, "orderByField");
                if !order.is_empty() && context_text("orderByField").is_empty() {
                    script::set_context_name("orderByField", Value::string(order))?;
                }
            }
            for (name, value) in parameters {
                script::set_context_name(&name, Value::string(value))?;
            }
        }
    }
    if with_request && user.is_some() {
        let page_size = crate::web::parameters()
            .get("pageSize")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        let saved = crate::user::get_preference("screen.user.page.size")?;
        if page_size.parse::<i64>().is_ok() {
            // a fenced render (an agent's) reads as the user and changes none of the user's preferences
            if saved.as_deref() != Some(page_size.as_str()) && !crate::entity::fence::active() {
                crate::user::set_preference("screen.user.page.size", Some(&page_size))?;
            }
        } else if let Some(saved) = saved.filter(|saved| saved.parse::<i64>().is_ok()) {
            script::set_context_name("pageSize", Value::string(saved))?;
        }
    }
    Ok(())
}

/// `getUserFormListFinds`: the saved finds of the user and the user's groups for a form, by description, each a map of
/// `description`, `formListFind`, `findParameters` (with `orderByField`) and `isByUserId`.
pub(crate) fn user_form_list_finds(saved_location: &str) -> Result<Vec<Value>, Error> {
    let Some(user) = user_id() else {
        return Ok(Vec::new());
    };
    let facade = facade()?;
    let groups = crate::user::current_user_groups()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let finds = rows(
        &facade,
        "moqui.screen.form.FormListFind",
        &[("formLocation", saved_location)],
    )?;
    let mut out: Vec<(String, Value)> = Vec::new();
    for find in finds {
        let id = text(&find, "formListFindId");
        let by_user = !rows(
            &facade,
            "moqui.screen.form.FormListFindUser",
            &[("formListFindId", &id), ("userId", &user)],
        )?
        .is_empty();
        let by_group = rows(
            &facade,
            "moqui.screen.form.FormListFindUserGroup",
            &[("formListFindId", &id)],
        )?
        .iter()
        .any(|row| groups.contains(&text(row, "userGroupId")));
        if !by_user && !by_group {
            continue;
        }
        let mut parameters: IndexMap<String, Value> = find_parameters(&id)?
            .into_iter()
            .map(|(name, value)| (name, Value::string(value)))
            .collect();
        let order = text(&find, "orderByField");
        if !order.is_empty() {
            parameters.insert("orderByField".to_string(), Value::string(order));
        }
        let description = text(&find, "description");
        let mut info = IndexMap::new();
        info.insert("description".to_string(), Value::string(description.clone()));
        info.insert(
            "formListFind".to_string(),
            Value::Map(
                find.fields()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone()))
                    .collect(),
            ),
        );
        info.insert("findParameters".to_string(), Value::Map(parameters));
        info.insert(
            "isByUserId".to_string(),
            Value::string(if by_user { "true" } else { "false" }),
        );
        out.push((description, Value::Map(info)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out.into_iter().map(|(_, info)| info).collect())
}

/// `getUserDefaultFormListFindId` of a screen.
pub(crate) fn user_default_find_id(screen_location: &str) -> Result<Option<String>, Error> {
    let Some(user) = user_id() else {
        return Ok(None);
    };
    let facade = facade()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    Ok(rows(
        &facade,
        "moqui.screen.form.FormListFindUserDefault",
        &[("userId", &user), ("screenLocation", screen_location)],
    )?
    .first()
    .map(|row| text(row, "formListFindId")))
}

/// The newest `ScreenScheduled` of a saved find for the user, else for one of the user's groups, as
/// `ScreenForm.getFormListFindScreenScheduled`: `{renderMode, cronExpression, cronDescription}`, or None.
///
/// # Errors
///
/// Returns an error when the records cannot be read.
pub(crate) fn form_list_find_screen_scheduled(find_id: &str) -> Result<Option<Value>, Error> {
    let Some(user) = user_id().filter(|_| !find_id.is_empty()) else {
        return Ok(None);
    };
    let facade = facade()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let all = facade
        .find("moqui.screen.ScreenScheduled")?
        .eq("formListFindId", Value::string(find_id))
        .order_by("screenScheduledId", true)
        .list()?
        .into_values();
    let mut scheduled = all.iter().find(|row| text(row, "userId") == user).cloned();
    if scheduled.is_none() {
        let groups = crate::user::current_user_groups()?;
        scheduled = all.into_iter().find(|row| groups.contains(&text(row, "userGroupId")));
    }
    Ok(scheduled.map(|row| {
        let cron = text(&row, "cronExpression");
        // A cron the parser does not read is shown as it is.
        let description = crate::service::info::cron_description(&cron).unwrap_or_else(|_| cron.clone());
        let mut map = IndexMap::new();
        map.insert("renderMode".to_string(), Value::string(text(&row, "renderMode")));
        map.insert("cronExpression".to_string(), Value::string(cron));
        map.insert("cronDescription".to_string(), Value::string(description));
        Value::Map(map)
    }))
}

/// The form a posted `formLocation` names: a full location, else the name of a form of the screen at `screen_location`.
/// Returns the full location, the screen location, the form name, and the location extension.
fn split_form_location(posted: &str, screen_location: &str) -> Option<(String, String, String, String)> {
    let (base, extension) = match posted.rfind('#') {
        Some(at) if at > 0 => (&posted[..at], posted[at + 1..].to_string()),
        _ => (posted, String::new()),
    };
    if let Some(at) = base.rfind('$') {
        let screen = base[..at].rsplit_once('.').map(|(screen, _)| screen)?.to_string();
        return Some((posted.to_string(), screen, base[at + 1..].to_string(), extension));
    }
    if base.is_empty() || base.contains('/') {
        return None;
    }
    let full = form_location(screen_location, true, base);
    let full = if extension.is_empty() {
        full
    } else {
        format!("{full}#{extension}")
    };
    Some((full, screen_location.to_string(), base.to_string(), extension))
}

/// The fields of a form-list a saved find records, as `makeFormListFindFields`: each field (not the header submit, nor
/// a `skip-fields` one) with a value, an operator, a period, or a from or thru value in the context.
fn find_fields(form: &moqui_lib::node::MNode) -> Vec<IndexMap<String, Value>> {
    let skip: Vec<String> = form
        .first_named("entity-find")
        .and_then(|find| find.first_named("search-form-inputs"))
        .and_then(|inputs| inputs.attribute("skip-fields"))
        .map(|text| text.split(',').map(|name| name.trim().to_string()).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for field in form.children_named("field") {
        let name = field.attribute("name").unwrap_or_default();
        let header_submit = field
            .first_named("header-field")
            .is_some_and(|header| header.first_named("submit").is_some());
        if name.is_empty() || header_submit || skip.contains(&name) {
            continue;
        }
        let mut row = IndexMap::new();
        row.insert("fieldName".to_string(), Value::string(name.clone()));
        let flag = |suffix: &str| matches!(context_text(&format!("{name}{suffix}")).as_str(), "Y" | "true");
        if context_has(&name) || context_has(&format!("{name}_op")) {
            let value = script::context_value(&name).filter(|value| !moqui_lib::object::is_empty(value));
            let operator = Some(context_text(&format!("{name}_op")))
                .filter(|op| !op.is_empty())
                .unwrap_or_else(|| "equals".to_string());
            if value.is_none() && operator != "empty" {
                continue;
            }
            row.insert(
                "fieldValue".to_string(),
                value.map_or(Value::Null, |value| {
                    Value::string(moqui_lib::object::to_plain_string(&value))
                }),
            );
            row.insert("fieldOperator".to_string(), Value::string(operator));
            row.insert(
                "fieldNot".to_string(),
                Value::string(if flag("_not") { "Y" } else { "N" }),
            );
            row.insert(
                "fieldIgnoreCase".to_string(),
                Value::string(if flag("_ic") { "Y" } else { "N" }),
            );
        } else if !context_text(&format!("{name}_period")).is_empty() {
            row.insert(
                "fieldPeriod".to_string(),
                Value::string(context_text(&format!("{name}_period"))),
            );
            let offset = context_text(&format!("{name}_poffset")).parse::<i64>().unwrap_or(0);
            row.insert("fieldPerOffset".to_string(), Value::I64(offset));
        } else {
            let from = context_text(&format!("{name}_from"));
            let thru = context_text(&format!("{name}_thru"));
            if from.is_empty() && thru.is_empty() {
                continue;
            }
            row.insert("fieldFrom".to_string(), Value::string(from));
            row.insert("fieldThru".to_string(), Value::string(thru));
        }
        out.push(row);
    }
    out
}

/// The source form, which still has its `auto-fields-*` elements, and whether it is dynamic.
/// `None` when the screen has no form of that name.
fn source_form(screen_location: &str, form_name: &str) -> Result<Option<(bool, MNode)>, Error> {
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| Error::new("no execution context"))?;
    let screens = super::ScreenFacade::of(&factory);
    let def = screens.screen_def(&factory, screen_location)?;
    let Some(form) = def.forms.get(form_name) else {
        return Ok(None);
    };
    Ok(Some((form.dynamic, form.node.node()?)))
}

/// The assembled form a saved find is for, from the screen at `screen_location`.
fn assembled_form(screen_location: &str, form_name: &str) -> Result<MNode, Error> {
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| Error::new("no execution context"))?;
    let screens = super::ScreenFacade::of(&factory);
    let def = screens.screen_def(&factory, screen_location)?;
    if !def.forms.contains_key(form_name) {
        return Err(Error::new(format!(
            "Form {form_name} not found in screen at {screen_location}, cannot process saved find"
        )));
    }
    let form = if def.forms.get(form_name).is_some_and(|form| form.dynamic) {
        super::form::assemble_dynamic(&screens, &factory, &def, form_name)?
    } else {
        super::form::assemble(&screens, &factory, &def, form_name)?
    };
    form.node()
}

/// Whether the user saved the find (`FormListFindUser`).
fn find_by_user(facade: &EntityFacade, find_id: &str, user: &str) -> Result<bool, Error> {
    Ok(!rows(
        facade,
        "moqui.screen.form.FormListFindUser",
        &[("userId", user), ("formListFindId", find_id)],
    )?
    .is_empty())
}

/// Whether the find is shared with a group the user is in (`FormListFindUserGroup`).
fn find_by_group(facade: &EntityFacade, find_id: &str) -> Result<bool, Error> {
    let groups = crate::user::current_user_groups()?;
    Ok(rows(
        facade,
        "moqui.screen.form.FormListFindUserGroup",
        &[("formListFindId", find_id)],
    )?
    .iter()
    .any(|row| groups.contains(&text(row, "userGroupId"))))
}

/// The `formSaveFind` transition, as `processFormSavedFind`: returns the saved find to show next, if any. It needs a
/// user, and a find is scheduled or made a default only by a user it belongs to, directly or through a group.
pub(crate) fn process_saved_find(transition_screen: &str) -> Result<Option<String>, Error> {
    let facade = facade()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(user) = user_id() else {
        add_error("No user logged in, cannot process saved find");
        return Ok(None);
    };
    let find_id = context_text("formListFindId");
    let existing = if find_id.is_empty() {
        None
    } else {
        rows(
            &facade,
            "moqui.screen.form.FormListFind",
            &[("formListFindId", &find_id)],
        )?
        .into_iter()
        .next()
    };

    if context_has("DeleteFind") {
        return delete_find(&facade, &user, &find_id, existing.is_some());
    }
    if context_has("ScheduleFind") {
        return schedule_find(&facade, &user, find_id, existing.as_ref());
    }
    if context_has("ClearDefault") {
        return clear_default(&facade, &user, &find_id);
    }

    let posted = context_text("formLocation");
    if posted.is_empty() {
        add_error("No form location specified, cannot process saved find");
        return Ok(None);
    }
    let Some((mut location, screen_location, form_name, mut extension)) =
        split_form_location(&posted, transition_screen)
    else {
        add_error("Form location invalid, cannot process saved find");
        return Ok(None);
    };
    // A posted '#' is the key. A bare name, or a full location with no '#', still keys a dynamic form by its entity
    // or service, so a post that sends only the form name does not land in the shared list.
    if extension.is_empty() {
        if let Some((dynamic, node)) = source_form(&screen_location, &form_name)? {
            if dynamic {
                let derived = saved_find_location(&location, true, &node);
                if let Some((base, ext)) = derived.rsplit_once('#') {
                    if !base.is_empty() && !ext.is_empty() {
                        extension = ext.to_string();
                        location = derived;
                    }
                }
            }
        }
    }
    if !extension.is_empty() {
        script::set_context_name("formLocationExtension", Value::string(&extension))?;
    }

    if context_has("MakeDefault") {
        return make_default(&facade, &user, &find_id, existing.is_some(), &screen_location);
    }
    let form = SavedForm {
        location,
        screen_location,
        form_name,
    };
    save_find(&facade, &user, find_id, existing, &form)
}

/// `DeleteFind`: drop the user's link to the find and default, and the find itself once no user or group has it.
fn delete_find(facade: &EntityFacade, user: &str, find_id: &str, exists: bool) -> Result<Option<String>, Error> {
    if !exists {
        add_error(&format!("Saved find with ID {find_id} not found, not deleting"));
        return Ok(None);
    }
    delete_all(
        facade,
        "moqui.screen.form.FormListFindUserDefault",
        &[("userId", user), ("formListFindId", find_id)],
    )?;
    let mine = rows(
        facade,
        "moqui.screen.form.FormListFindUser",
        &[("userId", user), ("formListFindId", find_id)],
    )?;
    let Some(mine) = mine.into_iter().next() else {
        return Ok(None);
    };
    mine.delete(facade)?;
    let users = rows(
        facade,
        "moqui.screen.form.FormListFindUser",
        &[("formListFindId", find_id)],
    )?;
    let groups = rows(
        facade,
        "moqui.screen.form.FormListFindUserGroup",
        &[("formListFindId", find_id)],
    )?;
    if users.is_empty() && groups.is_empty() {
        delete_all(
            facade,
            "moqui.screen.form.FormListFindField",
            &[("formListFindId", find_id)],
        )?;
        delete_all(facade, "moqui.screen.form.FormListFind", &[("formListFindId", find_id)])?;
    }
    Ok(None)
}

/// The render modes a saved find may be scheduled in: a spreadsheet, a CSV file, or a PDF (`xsl-fo` is `html-print`).
const SCHEDULE_RENDER_MODES: &[&str] = &["xlsx", "csv", "xsl-fo"];

/// `ScheduleFind`: a `ScreenScheduled` that renders the find's screen and sends it by email on a cron schedule.
fn schedule_find(
    facade: &EntityFacade,
    user: &str,
    find_id: String,
    existing: Option<&EntityValue>,
) -> Result<Option<String>, Error> {
    let Some(find) = existing else {
        add_error(&format!("Saved find with ID {find_id} not found, not scheduling"));
        return Ok(None);
    };
    if !find_by_user(facade, &find_id, user)? && !find_by_group(facade, &find_id)? {
        add_error(&format!(
            "You are not associated with Saved Find {find_id}, not scheduling"
        ));
        return Ok(None);
    }
    let render_mode = Some(context_text("renderMode"))
        .filter(|mode| !mode.is_empty())
        .unwrap_or_else(|| "csv".into());
    // The modes the Saved Finds dialog offers, which the scheduled send attaches; Java stores any text.
    if !SCHEDULE_RENDER_MODES.contains(&render_mode.as_str()) {
        add_error(&format!(
            "Render mode {render_mode} cannot be scheduled, not scheduling saved find with ID {find_id}"
        ));
        return Ok(Some(find_id));
    }
    let screen_path = context_text("screenPath");
    let cron = context_text("cronSelected");
    if screen_path.is_empty() {
        add_error(&format!(
            "Screen Path not specified, not scheduling saved find with ID {find_id}"
        ));
        return Ok(Some(find_id));
    }
    if cron.is_empty() {
        add_error(&format!(
            "Cron Schedule not specified, not scheduling saved find with ID {find_id}"
        ));
        return Ok(Some(find_id));
    }
    let mut scheduled = facade.make_value("moqui.screen.ScreenScheduled")?;
    scheduled.set_sequenced_id_primary(facade)?;
    for (name, value) in [
        ("screenPath", screen_path),
        ("formListFindId", find_id.clone()),
        ("renderMode", render_mode),
        ("noResultsAbort", "Y".to_string()),
        ("cronExpression", cron),
        ("emailTemplateId", "SCREEN_RENDER".to_string()),
        (
            "emailSubject",
            format!(
                "{} ${{ec.l10n.format(ec.user.nowTimestamp, null)}}",
                template_literal(&text(find, "description"))
            ),
        ),
        ("userId", user.to_string()),
    ] {
        scheduled.set(name, Value::string(value));
    }
    scheduled.create(facade)?;
    ExecutionContext::with_current(|ec| ec.message().add_message("Saved find scheduled to send by email"));
    Ok(Some(find_id))
}

/// `ClearDefault`: the find is no longer the user's default.
fn clear_default(facade: &EntityFacade, user: &str, find_id: &str) -> Result<Option<String>, Error> {
    delete_all(
        facade,
        "moqui.screen.form.FormListFindUserDefault",
        &[("userId", user), ("formListFindId", find_id)],
    )?;
    Ok(None)
}

/// `MakeDefault`: the find becomes the user's default for the screen.
fn make_default(
    facade: &EntityFacade,
    user: &str,
    find_id: &str,
    exists: bool,
    screen_location: &str,
) -> Result<Option<String>, Error> {
    if !exists {
        add_error(&format!("Saved find with ID {find_id} not found, not making default"));
        return Ok(None);
    }
    if !find_by_user(facade, find_id, user)? && !find_by_group(facade, find_id)? {
        add_error(&format!(
            "You are not associated with Saved Find {find_id}, not making default"
        ));
        return Ok(None);
    }
    let current = rows(
        facade,
        "moqui.screen.form.FormListFindUserDefault",
        &[("userId", user), ("screenLocation", screen_location)],
    )?;
    match current.into_iter().next() {
        Some(mut row) => {
            row.set("formListFindId", Value::string(find_id));
            row.update(facade)?;
        }
        None => {
            create(
                facade,
                "moqui.screen.form.FormListFindUserDefault",
                &[
                    ("userId", Value::string(user)),
                    ("screenLocation", Value::string(screen_location)),
                    ("formListFindId", Value::string(find_id)),
                ],
            )?;
        }
    }
    Ok(None)
}

/// The form a find is saved for, from the posted `formLocation`.
struct SavedForm {
    location: String,
    screen_location: String,
    form_name: String,
}

/// Save the form's current find fields: update the user's find, or create one (also in place of a group's find, which
/// is copied for this user rather than changed for the group).
fn save_find(
    facade: &EntityFacade,
    user: &str,
    mut find_id: String,
    mut existing: Option<EntityValue>,
    saved: &SavedForm,
) -> Result<Option<String>, Error> {
    let location = &saved.location;
    let form = assembled_form(&saved.screen_location, &saved.form_name)?;
    let form_base_location = location.split('#').next().unwrap_or(location).to_string();
    let mut config_id = user_form_config_id(&form_base_location)?;
    if config_id.is_none() {
        config_id = existing
            .as_ref()
            .map(|find| text(find, "formConfigId"))
            .filter(|id| !id.is_empty());
    }
    let config_fields = match &config_id {
        Some(id) => rows(facade, "moqui.screen.form.FormConfigField", &[("formConfigId", id)])?,
        None => Vec::new(),
    };

    if let Some(find) = &existing {
        if text(find, "formLocation") != *location {
            add_error(&format!(
                "Specified form location did not match form on Saved Find {find_id}, not updating"
            ));
            return Ok(None);
        }
        if !find_by_user(facade, &find_id, user)? {
            if !find_by_group(facade, &find_id)? {
                add_error(&format!(
                    "You are not associated with Saved Find {find_id}, cannot update"
                ));
                return Ok(Some(find_id));
            }
            // A group's saved find is copied for this user rather than changed for the group.
            existing = None;
            find_id.clear();
        }
    }

    let description = context_text("_findDescription");
    let order = context_text("orderByField");
    if let Some(mut find) = existing {
        let mut find_config = text(&find, "formConfigId");
        if !config_fields.is_empty() {
            if find_config.is_empty() {
                let mut config = facade.make_value("moqui.screen.form.FormConfig")?;
                config.set("formLocation", Value::string(location.clone()));
                config.set_sequenced_id_primary(facade)?;
                config.create(facade)?;
                find_config = text(&config, "formConfigId");
                find.set("formConfigId", Value::string(find_config.clone()));
            } else {
                delete_all(
                    facade,
                    "moqui.screen.form.FormConfigField",
                    &[("formConfigId", &find_config)],
                )?;
            }
            for field in &config_fields {
                let mut copy = field.clone();
                copy.set("formConfigId", Value::string(find_config.clone()));
                copy.create(facade)?;
            }
        } else if !find_config.is_empty() {
            find.set("formConfigId", Value::Null);
            delete_all(
                facade,
                "moqui.screen.form.FormConfigField",
                &[("formConfigId", &find_config)],
            )?;
        }
        if !description.is_empty() {
            find.set("description", Value::string(description));
        }
        if !order.is_empty() {
            find.set("orderByField", Value::string(order));
        }
        find.update(facade)?;
        delete_all(
            facade,
            "moqui.screen.form.FormListFindField",
            &[("formListFindId", &find_id)],
        )?;
    } else {
        let mut config_id_new = None;
        if !config_fields.is_empty() {
            let mut config = facade.make_value("moqui.screen.form.FormConfig")?;
            config.set("formLocation", Value::string(location.clone()));
            config.set_sequenced_id_primary(facade)?;
            config.create(facade)?;
            let id = text(&config, "formConfigId");
            for field in &config_fields {
                let mut copy = field.clone();
                copy.set("formConfigId", Value::string(id.clone()));
                copy.create(facade)?;
            }
            config_id_new = Some(id);
        }
        let mut find = facade.make_value("moqui.screen.form.FormListFind")?;
        find.set("formLocation", Value::string(location.clone()));
        let description = if description.is_empty() {
            let username = crate::user::current_username().ok().flatten().unwrap_or_default();
            let now = jiff::Zoned::now().strftime("%Y-%m-%d %H:%M").to_string();
            format!("{username} - {now}")
        } else {
            description
        };
        find.set("description", Value::string(description));
        if !order.is_empty() {
            find.set("orderByField", Value::string(order));
        }
        if let Some(id) = config_id_new {
            find.set("formConfigId", Value::string(id));
        }
        find.set_sequenced_id_primary(facade)?;
        find.create(facade)?;
        find_id = text(&find, "formListFindId");
        create(
            facade,
            "moqui.screen.form.FormListFindUser",
            &[
                ("formListFindId", Value::string(find_id.clone())),
                ("userId", Value::string(user)),
            ],
        )?;
    }
    for mut field in find_fields(&form) {
        field.insert("formListFindId".to_string(), Value::string(find_id.clone()));
        let fields: Vec<(&str, Value)> = field
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        create(facade, "moqui.screen.form.FormListFindField", &fields)?;
    }
    Ok(Some(find_id))
}

/// The `FormConfigType` enumeration for the context's `_uiType`, if any.
fn config_type() -> Result<Option<String>, Error> {
    let explicit = context_text("configTypeEnumId");
    if !explicit.is_empty() {
        return Ok(Some(explicit));
    }
    let ui_type = context_text("_uiType");
    if ui_type.is_empty() {
        return Ok(None);
    }
    let facade = facade()?;
    Ok(rows(
        &facade,
        "moqui.basic.Enumeration",
        &[("enumTypeId", "FormConfigType"), ("enumCode", &ui_type)],
    )?
    .first()
    .map(|row| text(row, "enumId")))
}

/// `getUserActiveFormConfigId`: the user's `FormConfig` for a form, by `_uiType` when there is one, else the user's
/// plain one, else one of the user's groups.
pub(crate) fn user_form_config_id(location: &str) -> Result<Option<String>, Error> {
    let Some(user) = user_id() else {
        return Ok(None);
    };
    let facade = facade()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if !context_text("_uiType").is_empty() {
        let Some(kind) = config_type()? else {
            return Ok(None);
        };
        return Ok(rows(
            &facade,
            "moqui.screen.form.FormConfigUserType",
            &[
                ("formLocation", location),
                ("userId", &user),
                ("configTypeEnumId", &kind),
            ],
        )?
        .first()
        .map(|row| text(row, "formConfigId")));
    }
    if let Some(found) = rows(
        &facade,
        "moqui.screen.form.FormConfigUser",
        &[("formLocation", location), ("userId", &user)],
    )?
    .first()
    {
        return Ok(Some(text(found, "formConfigId")));
    }
    let groups = crate::user::current_user_groups()?;
    for config in rows(&facade, "moqui.screen.form.FormConfig", &[("formLocation", location)])? {
        let id = text(&config, "formConfigId");
        let shared = rows(
            &facade,
            "moqui.screen.form.FormConfigUserGroup",
            &[("formConfigId", &id)],
        )?
        .iter()
        .any(|row| groups.contains(&text(row, "userGroupId")));
        if shared {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// The columns of a form-list for this user, as field names: from the active saved find's `FormConfig` when it is a
/// find of this form, else the user's `FormConfig`. `None` keeps the form's own columns.
pub(crate) fn user_columns(location: &str, saved_location: &str) -> Result<Option<Vec<Vec<String>>>, Error> {
    let Ok(facade) = facade() else {
        return Ok(None);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut config_id = None;
    let find_id = context_text("formListFindId");
    if !find_id.is_empty() {
        if let Some(find) = rows(
            &facade,
            "moqui.screen.form.FormListFind",
            &[("formListFindId", &find_id)],
        )?
        .first()
        {
            if text(find, "formLocation") == saved_location {
                config_id = Some(text(find, "formConfigId")).filter(|id| !id.is_empty());
            }
        }
    }
    if config_id.is_none() {
        config_id = user_form_config_id(location)?;
    }
    let Some(config_id) = config_id else {
        return Ok(None);
    };
    let mut fields = rows(
        &facade,
        "moqui.screen.form.FormConfigField",
        &[("formConfigId", &config_id)],
    )?;
    if fields.is_empty() {
        return Ok(None);
    }
    let number = |row: &EntityValue, field: &str| text(row, field).parse::<i64>().unwrap_or(0);
    fields.sort_by_key(|row| (number(row, "positionIndex"), number(row, "positionSequence")));
    let mut columns: Vec<Vec<String>> = Vec::new();
    let mut current = i64::MIN;
    for row in fields {
        let index = number(&row, "positionIndex");
        if index != current || columns.is_empty() {
            columns.push(Vec::new());
            current = index;
        }
        if let Some(column) = columns.last_mut() {
            column.push(text(&row, "fieldName"));
        }
    }
    Ok(Some(columns))
}

/// The `formSelectColumns` transition, as `saveFormConfig`: the user's column choice for a form, kept in a `FormConfig`
/// of the user's own (a shared one is not changed), or removed with `ResetColumns`.
pub(crate) fn save_form_config(transition_screen: &str) -> Result<(), Error> {
    let facade = facade()?;
    let Some(user) = user_id() else {
        add_error("No user logged in, cannot save form configuration");
        return Ok(());
    };
    let posted = context_text("formLocation");
    if posted.is_empty() {
        add_error("No form location specified, cannot save form configuration");
        return Ok(());
    }
    let Some((location, ..)) = split_form_location(&posted, transition_screen) else {
        add_error("Form location invalid, cannot save form configuration");
        return Ok(());
    };
    let location = location.split('#').next().unwrap_or(&location).to_string();
    let kind = config_type()?;
    let mut config_id = Some(context_text("formConfigId")).filter(|id| !id.is_empty());
    if config_id.is_none() {
        config_id = match &kind {
            Some(kind) => rows(
                &facade,
                "moqui.screen.form.FormConfigUserType",
                &[
                    ("formLocation", &location),
                    ("userId", &user),
                    ("configTypeEnumId", kind),
                ],
            )?,
            None => rows(
                &facade,
                "moqui.screen.form.FormConfigUser",
                &[("formLocation", &location), ("userId", &user)],
            )?,
        }
        .first()
        .map(|row| text(row, "formConfigId"));
    }
    let users_current = config_id.clone();
    if let Some(id) = &config_id {
        let users = match &kind {
            Some(kind) => rows(
                &facade,
                "moqui.screen.form.FormConfigUserType",
                &[("formConfigId", id), ("configTypeEnumId", kind)],
            )?,
            None => rows(&facade, "moqui.screen.form.FormConfigUser", &[("formConfigId", id)])?,
        };
        let groups = rows(
            &facade,
            "moqui.screen.form.FormConfigUserGroup",
            &[("formConfigId", id)],
        )?;
        // A configuration that is shared, or is not the user's (a posted formConfigId), is not changed: the user
        // gets one of their own.
        let mine = users.iter().any(|row| text(row, "userId") == user);
        if users.len() > 1 || !groups.is_empty() || !mine {
            config_id = None;
        }
    }
    if let Some(id) = &config_id {
        delete_all(&facade, "moqui.screen.form.FormConfigField", &[("formConfigId", id)])?;
    }
    if context_has("ResetColumns") {
        if let Some(id) = &config_id {
            delete_all(&facade, "moqui.screen.form.FormConfigUser", &[("formConfigId", id)])?;
            delete_all(&facade, "moqui.screen.form.FormConfigUserType", &[("formConfigId", id)])?;
            delete_all(&facade, "moqui.screen.form.FormConfig", &[("formConfigId", id)])?;
        } else if let Some(id) = &users_current {
            delete_all(
                &facade,
                "moqui.screen.form.FormConfigUser",
                &[("formConfigId", id), ("userId", &user)],
            )?;
            delete_all(
                &facade,
                "moqui.screen.form.FormConfigUserType",
                &[("formConfigId", id), ("userId", &user)],
            )?;
        }
        return Ok(());
    }
    let config_id = match config_id {
        Some(id) => id,
        None => {
            let mut config = facade.make_value("moqui.screen.form.FormConfig")?;
            config.set("formLocation", Value::string(location.clone()));
            config.set_sequenced_id_primary(&facade)?;
            config.create(&facade)?;
            let id = text(&config, "formConfigId");
            match &kind {
                Some(kind) => create(
                    &facade,
                    "moqui.screen.form.FormConfigUserType",
                    &[
                        ("formConfigId", Value::string(id.clone())),
                        ("userId", Value::string(user.clone())),
                        ("formLocation", Value::string(location.clone())),
                        ("configTypeEnumId", Value::string(kind.clone())),
                    ],
                )?,
                None => create(
                    &facade,
                    "moqui.screen.form.FormConfigUser",
                    &[
                        ("formConfigId", Value::string(id.clone())),
                        ("userId", Value::string(user.clone())),
                        ("formLocation", Value::string(location.clone())),
                    ],
                )?,
            };
            id
        }
    };
    let tree = context_text("columnsTree");
    if tree.is_empty() {
        return Ok(());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&tree).map_err(|err| Error::new(format!("columnsTree is not JSON: {err}")))?;
    let order = |node: &serde_json::Value| node.get("order").and_then(serde_json::Value::as_i64).unwrap_or(0);
    let mut columns: Vec<&serde_json::Value> = parsed.as_array().map(|list| list.iter().collect()).unwrap_or_default();
    columns.sort_by_key(|column| order(column));
    let mut index = 0i64;
    for column in columns {
        if column.get("id").and_then(serde_json::Value::as_str) == Some("hidden") {
            continue;
        }
        let mut children: Vec<&serde_json::Value> = column
            .get("children")
            .and_then(serde_json::Value::as_array)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        children.sort_by_key(|child| order(child));
        for (sequence, child) in children.into_iter().enumerate() {
            let Some(name) = child.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            create(
                &facade,
                "moqui.screen.form.FormConfigField",
                &[
                    ("formConfigId", Value::string(config_id.clone())),
                    ("fieldName", Value::string(name)),
                    ("positionIndex", Value::I64(index)),
                    (
                        "positionSequence",
                        Value::I64(i64::try_from(sequence).unwrap_or(i64::MAX)),
                    ),
                ],
            )?;
        }
        index += 1;
    }
    Ok(())
}

/// `getScreenDocumentInfoList`: the title and index of each `ScreenDocument` of a screen in the user's locale, its
/// language, or none; a document with no title is named by its file.
pub(crate) fn screen_document_list(screen_location: &str) -> Result<Vec<Value>, Error> {
    let Ok(facade) = facade() else {
        return Ok(Vec::new());
    };
    let locale = crate::l10n::locale();
    let language = locale.split_once('_').map(|(language, _)| language.to_string());
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut documents = facade
        .find("moqui.screen.ScreenDocument")?
        .eq("screenLocation", Value::string(screen_location))
        .order_by("docIndex", false)
        .use_cache(true)
        .list()?
        .into_values();
    documents.sort_by_key(|row| text(row, "docIndex").parse::<i64>().unwrap_or(0));
    let mut out = Vec::new();
    for document in documents {
        let doc_locale = text(&document, "locale");
        if !doc_locale.is_empty() && doc_locale != locale && language.as_deref() != Some(doc_locale.as_str()) {
            continue;
        }
        let mut title = text(&document, "docTitle");
        if title.is_empty() {
            let location = text(&document, "docLocation");
            let file = location.rsplit('/').next().unwrap_or(&location);
            title = file.split('.').next().unwrap_or(file).to_string();
        }
        let mut item = IndexMap::new();
        item.insert("title".to_string(), Value::string(title));
        item.insert(
            "index".to_string(),
            Value::I64(text(&document, "docIndex").parse::<i64>().unwrap_or(0)),
        );
        out.push(Value::Map(item));
    }
    Ok(out)
}

/// What the `screenDoc` transition answers.
pub(crate) enum ScreenDocument {
    Text(String),
    /// There is no document: a 404 with this message.
    Missing(String),
}

/// The `screenDoc` transition: the `ScreenDocument` of a screen with the context's `docIndex`, rendered as a template.
/// Returns the rendered text, or the message of a 404 when there is none.
pub(crate) fn screen_document(screen_location: &str) -> Result<ScreenDocument, Error> {
    let index = context_text("docIndex");
    if index.is_empty() {
        return Ok(ScreenDocument::Missing("No docIndex specified".to_string()));
    }
    let facade = facade()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let found = facade
        .find("moqui.screen.ScreenDocument")?
        .eq("screenLocation", Value::string(screen_location))
        .eq("docIndex", Value::string(index.clone()))
        .use_cache(true)
        .one()?;
    let Some(document) = found else {
        return Ok(ScreenDocument::Missing(format!("No document found for index {index}")));
    };
    let location = text(&document, "docLocation");
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| Error::new("no execution context"))?;
    let rendered = crate::script::template::TemplateRegistry::of(&factory)
        .render_location_default(&factory, &location, None, "")?;
    Ok(ScreenDocument::Text(rendered))
}
