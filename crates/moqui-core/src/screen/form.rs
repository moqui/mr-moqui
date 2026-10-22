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

//! Forms as the macros see them: the form element with `extends`, `widget-template-include`,
//! `auto-fields-entity`, and `screen-extend` forms merged in, as `ScreenForm.initForm` does.
//!
//! `auto-fields-service` and `auto-widget-service` build fields from a service's parameters, and a form whose
//! `transition` calls one service gets `validate-service` (or `validate-entity`) on its fields, as `setSubFieldValidateAttrs`.
//! A form is assembled once per definition and kept with it; a `dynamic="true"` form, and a form that a `DbForm` of some
//! user group modifies (`DbFormLookup`), is assembled for each render, with the user's `DbForm` fields merged in.
//! `extends="DbForm#formId"` extends a form made of `DbForm` records.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use moqui_db::model::{EntityDefinition, EntityModel};
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::cache::ScreenFacade;
use super::definition::{Actions, ScreenDef};
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::script;

const MAX_EXTENDS_DEPTH: usize = 8;

/// An assembled form. `xml` is the form element the macros walk.
#[derive(Debug)]
pub struct FormInstance {
    pub name: String,
    pub list: bool,
    pub xml: Arc<str>,
    /// `form-list` only: the list name, and `list-entry` when given.
    pub list_name: String,
    pub list_entry: String,
    /// The form's `entity-find` as actions that fill `list_name`.
    pub entity_find: Option<Actions>,
    pub row_actions: Option<Actions>,
    /// Fields with a `from` expression, evaluated for each row after the row actions.
    pub field_from: Vec<(String, String)>,
    /// `form-list` only: the `aggregate` and `show-total` of its fields, when any has one.
    pub(crate) aggregation: Option<super::aggregate::Aggregation>,
}

impl FormInstance {
    /// Parse the assembled form on this thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored XML does not parse.
    pub fn node(&self) -> Result<MNode, Error> {
        MNode::parse_text("assembled-form", &self.xml).map_err(Error::from)
    }

    /// A form instance of an assembled node that a render changed, such as a form-list with the user's columns.
    ///
    /// # Errors
    ///
    /// Returns an error when an expression of the form does not parse.
    pub(crate) fn from_node(def: &ScreenDef, node: &MNode) -> Result<Self, Error> {
        Self::of(def, node)
    }

    fn of(def: &ScreenDef, node: &MNode) -> Result<Self, Error> {
        let name = node.attribute("name").unwrap_or_default();
        let list = node.has_name("form-list");
        let label = format!("{} form {name}", def.location);
        let list_name = node.attribute("list").unwrap_or_default();
        let entity_find = match node.first_named("entity-find") {
            Some(find) if list => {
                let copy = find.deep_copy();
                copy.set_attribute("list", list_name.clone());
                copy.set_attribute("form-select-fields", form_select_fields(node).join(","));
                let actions = MNode::new("actions");
                actions.append_node(copy);
                Some(Actions::from_xml(
                    actions.write_string(),
                    &format!("{label} entity-find"),
                )?)
            }
            _ => None,
        };
        let row_actions = match node.first_named("row-actions") {
            Some(actions) if list => Some(Actions::from_node(&actions, &format!("{label} row-actions"))?),
            _ => None,
        };
        let mut field_from = Vec::new();
        if list {
            for field in node.children_named("field") {
                if let Some(from) = field.attribute("from").filter(|text| !text.is_empty()) {
                    crate::script::parse_js(&from)
                        .map_err(|err| Error::new(format!("{label}: field from `{from}`: {}", err.message())))?;
                    field_from.push((field.attribute("name").unwrap_or_default(), from));
                }
            }
        }
        let aggregation = if list {
            super::aggregate::Aggregation::of(node).map_err(|err| Error::new(format!("{label}: {err}")))?
        } else {
            None
        };
        Ok(Self {
            aggregation,
            name,
            list,
            xml: Arc::from(node.write_string()),
            list_name,
            list_entry: node.attribute("list-entry").unwrap_or_default(),
            entity_find,
            row_actions,
            field_from,
        })
    }
}

/// The fields a form-list's `entity-find` selects, as Java's `FormListRenderInfo`: the fields of the
/// columns (the `form-list-column` or `columns` field refs, otherwise each field whose widget is not
/// hidden), then the hidden fields that have a `default-field`, then the fields the `view-board`,
/// `view-calendar` and `view-tree` name (not in Java). A user's saved column selection comes through the node: as in Java,
/// `render_user_columns_form` rebuilds the form with those columns.
fn form_select_fields(form: &MNode) -> Vec<String> {
    let fields = form.children_named("field");
    let mut names: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    };
    let column_refs: Vec<String> = form
        .children()
        .into_iter()
        .filter(|child| child.has_name("form-list-column") || child.has_name("columns"))
        .flat_map(|child| descendants_named(&child, "field-ref"))
        .filter_map(|field_ref| field_ref.attribute("name"))
        .collect();
    if column_refs.is_empty() {
        for field in &fields {
            if !list_field_hidden_widget(field) && !field.attribute_is("hide", "true") {
                push(field.attribute("name").unwrap_or_default());
            }
        }
    } else {
        column_refs.into_iter().for_each(&mut push);
    }
    for field in &fields {
        if list_field_hidden_widget(field) && field.first_named("default-field").is_some() {
            push(field.attribute("name").unwrap_or_default());
        }
    }
    // the fields the board and calendar views read (a start, an all-day flag, a sum), which need not be columns
    for (view, attributes) in [
        (
            "view-board",
            &[
                "group-field",
                "group-fields",
                "card-title-field",
                "badge-field",
                "sum-field",
                "card-fields",
                "key-fields",
            ][..],
        ),
        (
            "view-calendar",
            &[
                "start-field",
                "end-field",
                "title-field",
                "color-field",
                "all-day-field",
                "resource-field",
                "card-fields",
                "key-fields",
            ][..],
        ),
        (
            "view-tree",
            &["parent-field", "rollup-fields", "child-count-field", "key-fields"][..],
        ),
    ] {
        if let Some(node) = form.first_named(view) {
            for attribute in attributes {
                for name in node.attribute(attribute).unwrap_or_default().split(',') {
                    push(name.trim().to_string());
                }
            }
        }
    }
    names
}

/// The fields a form-list shows, with each one's `hide` expression, as the macros' `formListColumns` picks them: the
/// `columns` of the `ui_type` (`_uiType`) when the form has them, else the untyped `columns`, else the
/// `form-list-column`s, else every field. A field whose widget is hidden is not shown. For Java's `includeFields`.
pub(crate) fn list_column_fields(form: &MNode, ui_type: Option<&str>) -> Vec<(String, Option<String>)> {
    let fields = form.children_named("field");
    let shown = |field: &MNode| {
        (!list_field_hidden_widget(field)).then(|| {
            (
                field.attribute("name").unwrap_or_default(),
                field.attribute("hide").filter(|hide| !hide.is_empty()),
            )
        })
    };
    let columns = form.children_named("columns");
    let typed = ui_type.filter(|kind| !kind.is_empty()).and_then(|kind| {
        columns
            .iter()
            .find(|node| node.attribute("type").as_deref() == Some(kind))
    });
    let untyped = || {
        columns
            .iter()
            .find(|node| node.attribute("type").is_none_or(|kind| kind.is_empty()))
    };
    let column_nodes = match typed.or_else(untyped) {
        Some(node) => node.children_named("column"),
        None => form.children_named("form-list-column"),
    };
    if column_nodes.is_empty() {
        return fields.iter().filter_map(shown).collect();
    }
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for field_ref in column_nodes
        .iter()
        .flat_map(|column| column.children_named("field-ref"))
    {
        let name = field_ref.attribute("name").unwrap_or_default();
        let field = fields
            .iter()
            .find(|field| field.attribute("name").as_deref() == Some(name.as_str()));
        if let Some(entry) = field.and_then(shown) {
            if !out.iter().any(|(known, _)| *known == entry.0) {
                out.push(entry);
            }
        }
    }
    out
}

/// The hidden fields that have a `default-field`, which an `entity-find` list also selects (Java's `getListObject`).
pub(crate) fn hidden_list_fields_with_default(form: &MNode) -> Vec<String> {
    form.children_named("field")
        .iter()
        .filter(|field| list_field_hidden_widget(field) && field.first_named("default-field").is_some())
        .filter_map(|field| field.attribute("name"))
        .collect()
}

/// Java's `isListFieldHiddenWidget`: the default field and each conditional field are hidden or ignored.
fn list_field_hidden_widget(field: &MNode) -> bool {
    let shown = |sub: &MNode| sub.first_named("hidden").is_none() && sub.first_named("ignored").is_none();
    if field.first_named("default-field").is_some_and(|sub| shown(&sub)) {
        return false;
    }
    !field.children_named("conditional-field").iter().any(shown)
}

/// The assembled form `name` of `def`, from the definition's cache when it was assembled before.
///
/// # Errors
///
/// Returns an error when the form is not in the screen, an `extends` target is missing, or an
/// expression in the form is not JavaScript.
pub fn assemble(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &ScreenDef,
    name: &str,
) -> Result<Arc<FormInstance>, Error> {
    if let Some(found) = def.generated.form(name) {
        return Ok(found);
    }
    let node = assemble_node(facade, factory, def, name, 0, false)?;
    let instance = Arc::new(FormInstance::of(def, &node)?);
    def.generated.put_form(name, Arc::clone(&instance));
    Ok(instance)
}

/// A `dynamic="true"` form assembled from the context as it is now: `${}` in `extends` and in the
/// `entity-name` of `auto-fields-entity` is expanded. Nothing is cached, since the next render may differ.
///
/// # Errors
///
/// Same as [`assemble`].
pub fn assemble_dynamic(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &ScreenDef,
    name: &str,
) -> Result<Arc<FormInstance>, Error> {
    let node = assemble_node(facade, factory, def, name, 0, true)?;
    for db_form in user_db_forms(&def.location, name)? {
        merge_form_nodes(&node, &db_form, false, true);
    }
    Ok(Arc::new(FormInstance::of(def, &node)?))
}

/// Whether a `DbForm` of any user group modifies the form `name` of the screen at `location` (Java's `hasDbExtensions`).
/// Such a form is assembled for each render, like a dynamic one.
pub(crate) fn has_db_extensions(location: &str, name: &str) -> bool {
    db_form_groups(location, name).is_ok_and(|forms| forms.iter().any(|(_, groups)| !groups.is_empty()))
}

/// The `DbForm` ids that modify the form, each with the user groups it is for: `DbFormLookup`, read as its two entities.
fn db_form_groups(location: &str, name: &str) -> Result<Vec<(String, Vec<String>)>, Error> {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(Vec::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let forms = facade
        .find("moqui.screen.form.DbForm")?
        .eq("modifyXmlScreenForm", Value::string(format!("{location}#{name}")))
        .use_cache(true)
        .list()?;
    let mut out = Vec::new();
    for form in forms.iter() {
        let form_id = db_text(form, "formId");
        let groups = facade
            .find("moqui.screen.form.DbFormUserGroup")?
            .eq("formId", Value::string(form_id.clone()))
            .use_cache(true)
            .list()?;
        out.push((
            form_id,
            groups.iter().map(|group| db_text(group, "userGroupId")).collect(),
        ));
    }
    Ok(out)
}

/// The `DbForm` nodes that modify this form for the current user's groups, as `getDbFormNodeList`.
fn user_db_forms(location: &str, name: &str) -> Result<Vec<MNode>, Error> {
    let forms = db_form_groups(location, name)?;
    if forms.is_empty() {
        return Ok(Vec::new());
    }
    let user_groups = crate::user::current_user_groups()?;
    forms
        .iter()
        .filter(|(_, groups)| groups.iter().any(|group| user_groups.contains(group)))
        .map(|(id, _)| db_form_node(id))
        .collect()
}

/// Field attributes and sub-field attributes of a `DbFormFieldAttribute`; any other name goes on the widget.
const DB_FIELD_ATTRIBUTES: &[&str] = &["name", "from", "entry-name", "hide"];
const DB_SUB_FIELD_ATTRIBUTES: &[&str] = &[
    "title",
    "tooltip",
    "red-when",
    "validate-service",
    "validate-parameter",
    "validate-entity",
    "validate-field",
];

fn db_text(value: &crate::entity::EntityValue, field: &str) -> String {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn db_number(value: &crate::entity::EntityValue, field: &str) -> i64 {
    db_text(value, field).parse().unwrap_or(0)
}

/// `getDbFormNode`: a form-single or form-list built from a `DbForm` and its `DbFormField`, attribute and option records.
fn db_form_node(form_id: &str) -> Result<MNode, Error> {
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let id = Value::string(form_id);
    let form = facade
        .find("moqui.screen.form.DbForm")?
        .eq("formId", id.clone())
        .use_cache(true)
        .one()?
        .ok_or_else(|| Error::new(format!("Could not find DbForm record with ID [{form_id}]")))?;
    let node = MNode::new(if db_text(&form, "isListForm") == "Y" {
        "form-list"
    } else {
        "form-single"
    });
    let fields = facade
        .find("moqui.screen.form.DbFormField")?
        .eq("formId", id.clone())
        .order_by("layoutSequenceNum", false)
        .use_cache(true)
        .list()?;
    for db_field in fields.iter() {
        let field_name = db_text(db_field, "fieldName");
        let field = MNode::new("field");
        field.set_attribute("name", field_name.clone());
        let entry = db_text(db_field, "entryName");
        if !entry.is_empty() {
            field.set_attribute("from", entry);
        }
        let condition = db_text(db_field, "condition");
        let sub = if condition.is_empty() {
            field.append("default-field")
        } else {
            let sub = field.append("conditional-field");
            sub.set_attribute("condition", condition);
            sub
        };
        for attribute in ["title", "tooltip"] {
            let value = db_text(db_field, attribute);
            if !value.is_empty() {
                sub.set_attribute(attribute, value);
            }
        }
        let field_type = db_text(db_field, "fieldTypeEnumId");
        let Some(widget_name) = field_type.get(6..).filter(|name| !name.is_empty()) else {
            return Err(Error::new(format!(
                "DbFormField record with formId [{form_id}] and fieldName [{field_name}] has no fieldTypeEnumId"
            )));
        };
        let widget = sub.append(widget_name);
        let of_field = |entity: &str| -> Result<crate::entity::EntityList, Error> {
            Ok(facade
                .find(entity)?
                .eq("formId", id.clone())
                .eq("fieldName", Value::string(field_name.clone()))
                .use_cache(true)
                .list()?)
        };
        for attribute in of_field("moqui.screen.form.DbFormFieldAttribute")?.iter() {
            let name = db_text(attribute, "attributeName");
            let value = db_text(attribute, "value");
            if DB_FIELD_ATTRIBUTES.contains(&name.as_str()) {
                field.set_attribute(name, value);
            } else if DB_SUB_FIELD_ATTRIBUTES.contains(&name.as_str()) {
                sub.set_attribute(name, value);
            } else {
                widget.set_attribute(name, value);
            }
        }
        let options = of_field("moqui.screen.form.DbFormFieldOption")?;
        let entity_options = of_field("moqui.screen.form.DbFormFieldEntOpts")?;
        let mut combined: Vec<(i64, bool, &crate::entity::EntityValue)> = options
            .iter()
            .map(|value| (db_number(value, "sequenceNum"), false, value))
            .chain(
                entity_options
                    .iter()
                    .map(|value| (db_number(value, "sequenceNum"), true, value)),
            )
            .collect();
        combined.sort_by_key(|(sequence, _, _)| *sequence);
        for (sequence_num, from_entity, option) in combined {
            if !from_entity {
                let node = widget.append("option");
                node.set_attribute("key", db_text(option, "keyValue"));
                node.set_attribute("text", db_text(option, "text"));
                continue;
            }
            let options_node = widget.append("entity-options");
            let option_text = db_text(option, "text");
            options_node.set_attribute(
                "text",
                if option_text.is_empty() {
                    "${description}".to_string()
                } else {
                    option_text
                },
            );
            let find = options_node.append("entity-find");
            find.set_attribute("entity-name", db_text(option, "entityName"));
            let of_sequence = |entity: &str| -> Result<Vec<crate::entity::EntityValue>, Error> {
                Ok(of_field(entity)?
                    .into_values()
                    .into_iter()
                    .filter(|value| db_number(value, "sequenceNum") == sequence_num)
                    .collect())
            };
            for cond in of_sequence("moqui.screen.form.DbFormFieldEntOptsCond")? {
                let node = find.append("econdition");
                node.set_attribute("field-name", db_text(&cond, "entityFieldName"));
                node.set_attribute("value", db_text(&cond, "value"));
            }
            let mut orders = of_sequence("moqui.screen.form.DbFormFieldEntOptsOrder")?;
            orders.sort_by_key(|order| db_number(order, "orderSequenceNum"));
            for order in orders {
                find.append("order-by")
                    .set_attribute("field-name", db_text(&order, "entityFieldName"));
            }
        }
        let layout = db_text(db_field, "layoutSequenceNum");
        if !layout.is_empty() {
            field.set_attribute("layoutSequenceNum", layout);
        }
        merge_field_node(&node, &field, false);
    }
    Ok(node)
}

fn assemble_node(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &ScreenDef,
    name: &str,
    depth: usize,
    dynamic: bool,
) -> Result<MNode, Error> {
    if depth > MAX_EXTENDS_DEPTH {
        return Err(Error::new(format!(
            "form extends are nested too deeply at {}#{name}",
            def.location
        )));
    }
    let form = def
        .forms
        .get(name)
        .ok_or_else(|| Error::new(format!("Could not find form {name} in screen {}", def.location)))?;
    let base = form.node.node()?;
    if form.dynamic && !dynamic {
        return Err(Error::new(format!(
            "form {name} in {} is dynamic (dynamic=\"true\"), and dynamic forms are not rendered yet",
            def.location
        )));
    }
    let extensions: Vec<MNode> = form.extensions.iter().map(|ext| ext.node()).collect::<Result<_, _>>()?;
    let assembled = MNode::new(base.name());
    let mut column_info: Vec<(String, Vec<String>)> = Vec::new();

    if let Some(mut extends) = base.attribute("extends").filter(|text| !text.is_empty()) {
        if dynamic {
            extends = script::expand(&extends)?;
        }
        let found = extends_node(facade, factory, def, &extends, depth, dynamic)?;
        merge_form_nodes(&assembled, &found, true, true);
    }
    let is_list = base.has_name("form-list");
    // Attributes are copied onto `assembled` after the auto fields, so the form name is read from the source node.
    let form_name = base.attribute("name").unwrap_or_default();
    for sub in base.children() {
        match sub.name().as_str() {
            "field" => {
                let copy = sub.deep_copy();
                expand_field_node(factory, &assembled, &copy, &form_name)?;
                merge_field_node(&assembled, &copy, false);
            }
            "auto-fields-entity" => {
                let mut entity_name = sub.attribute("entity-name").unwrap_or_default();
                if dynamic {
                    entity_name = script::expand(&entity_name)?;
                    if entity_name.is_empty() || entity_name == "null" {
                        entity_name = script::context_value("formLocationExtension")
                            .map(|value| moqui_lib::object::to_plain_string(&value))
                            .unwrap_or_default();
                    }
                }
                let shape = entity_shape(&entity_name)?;
                let excludes: Vec<String> = sub
                    .children_named("exclude")
                    .iter()
                    .filter_map(|exclude| exclude.attribute("field-name"))
                    .collect();
                let include = sub
                    .attribute("include")
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "all".to_string());
                let field_type = sub
                    .attribute("field-type")
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "find-display".to_string());
                let displayed = add_entity_fields(&shape, &include, &field_type, &excludes, &assembled, &form_name);
                if is_list && !sub.attribute_is("auto-columns", "false") {
                    column_info.push((entity_name, displayed));
                }
            }
            "auto-fields-service" => {
                let mut service_name = sub.attribute("service-name").unwrap_or_default();
                if dynamic {
                    service_name = script::expand(&service_name)?;
                    if service_name.is_empty() || service_name == "null" {
                        service_name = script::context_value("formLocationExtension")
                            .map(|value| moqui_lib::object::to_plain_string(&value))
                            .unwrap_or_default();
                    }
                }
                let excludes: Vec<String> = sub
                    .children_named("exclude")
                    .iter()
                    .filter_map(|exclude| exclude.attribute("parameter-name"))
                    .collect();
                let field_type = sub
                    .attribute("field-type")
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "edit".to_string());
                if let Some(service) = crate::service::call::service_form_info(&service_name) {
                    let include = sub
                        .attribute("include")
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "in".to_string());
                    add_service_fields(&service, &include, &field_type, &excludes, &assembled, &form_name);
                } else if let Some(entity_name) = crate::service::call::entity_auto_noun(&service_name) {
                    let shape = entity_shape(&entity_name)?;
                    let displayed = add_entity_fields(&shape, "all", &field_type, &[], &assembled, &form_name);
                    if is_list {
                        column_info.push((entity_name, displayed));
                    }
                } else {
                    return Err(Error::new(format!(
                        "Cound not find service [{service_name}] or entity noun referred to in auto-fields-service of form [{name}] of screen [{}]",
                        def.location
                    )));
                }
            }
            _ => {}
        }
    }
    merge_form_nodes(&assembled, &base, false, false);
    for extension in &extensions {
        merge_form_nodes(&assembled, extension, true, true);
    }
    for (attribute, sub_name) in [
        ("transition", "default-field"),
        ("transition", "conditional-field"),
        ("transition-first-row", "first-row-field"),
        ("transition-second-row", "second-row-field"),
        ("transition-last-row", "last-row-field"),
    ] {
        set_sub_field_validate_attrs(def, &assembled, attribute, sub_name);
    }

    // A field-layout with fields it does not reference still needs the hidden ones, so they submit.
    if let Some(layout) = assembled.first_named("field-layout") {
        if !has_descendant(&layout, "fields-not-referenced") {
            for field in assembled.children_named("field") {
                let field_name = field.attribute("name").unwrap_or_default();
                let referenced = descendants_named(&layout, "field-ref")
                    .iter()
                    .any(|reference| reference.attribute_is("name", &field_name));
                if !referenced && has_descendant(&field, "hidden") {
                    add_field_to_layout(&assembled, &field);
                }
            }
        }
    }
    for (entity_name, displayed) in &column_info {
        let shape = entity_shape(entity_name)?;
        add_auto_entity_columns(&assembled, &base, &shape, displayed);
    }
    Ok(assembled)
}

fn extends_node(
    facade: &ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &ScreenDef,
    extends: &str,
    depth: usize,
    dynamic: bool,
) -> Result<MNode, Error> {
    let (location, form_name) = match extends.split_once('#') {
        Some((location, form_name)) => (location, form_name),
        None => (def.location.as_str(), extends),
    };
    if location == def.location {
        return assemble_node(facade, factory, def, form_name, depth + 1, dynamic);
    }
    if location == "moqui.screen.form.DbForm" || location == "DbForm" {
        return db_form_node(form_name);
    }
    let other = facade.screen_def(factory, location)?;
    // The assembled form is kept with this definition, so a change to the other screen makes this one stale.
    def.generated.add_dependencies(&other.sources);
    assemble_node(facade, factory, &other, form_name, depth + 1, dynamic)
}

fn has_descendant(node: &MNode, name: &str) -> bool {
    !descendants_named(node, name).is_empty()
}

fn descendants_named(node: &MNode, name: &str) -> Vec<MNode> {
    let mut out = Vec::new();
    for child in node.children() {
        if child.name() == name {
            out.push(child.clone());
        }
        out.extend(descendants_named(&child, name));
    }
    out
}

/// `ScreenForm.mergeFormNodes`.
fn merge_form_nodes(base: &MNode, over: &MNode, deep_copy: bool, copy_fields: bool) {
    base.copy_attributes_from(over);
    if let Some(find) = over.first_named("entity-find") {
        match base
            .first_named("entity-find")
            .and_then(|existing| base.index_of(&existing))
        {
            Some(index) => {
                let _ = base.replace_child(index, find.deep_copy());
            }
            None => base.insert_node(0, find.deep_copy()),
        }
    }
    if let Some(actions) = over.first_named("row-actions") {
        let target = base
            .first_named("row-actions")
            .unwrap_or_else(|| base.append("row-actions"));
        for action in actions.children() {
            target.append_node(action.deep_copy());
        }
    }
    if let Some(selection) = over.first_named("row-selection") {
        let target = base
            .first_named("row-selection")
            .unwrap_or_else(|| base.append("row-selection"));
        target.copy_attributes_from(&selection);
        for action in selection.children() {
            target.append_node(action.deep_copy());
        }
    }
    if let Some(hidden) = over.first_named("hidden-parameters") {
        base.remove_where(|child| child.has_name("hidden-parameters"));
        base.append_node(hidden.deep_copy());
    }
    if copy_fields {
        for field in over.children_named("field") {
            merge_field_node(base, &field, deep_copy);
        }
    }
    if let Some(layout) = over.first_named("field-layout") {
        base.remove_where(|child| child.has_name("field-layout"));
        base.append_node(layout.deep_copy());
    }
    if over.first_named("form-list-column").is_some() {
        base.remove_where(|child| child.has_name("form-list-column"));
        for column in over.children_named("form-list-column") {
            base.append_node(column.deep_copy());
        }
    }
    for columns in over.children_named("columns") {
        let kind = columns.attribute("type");
        base.remove_where(|child| child.has_name("columns") && child.attribute("type") == kind);
        base.append_node(columns.deep_copy());
    }
    // the views of a form-list: the extending form's view element takes the place of the one it extends
    for view in ["view-board", "view-calendar", "view-tree"] {
        if let Some(found) = over.first_named(view) {
            base.remove_where(|child| child.name() == view);
            base.append_node(found.deep_copy());
        }
    }
}

/// `ScreenForm.mergeFieldNode`: a field of the same name is merged in place, else appended.
fn merge_field_node(base: &MNode, over: &MNode, deep_copy: bool) {
    let name = over.attribute("name");
    let existing = base
        .children()
        .into_iter()
        .find(|child| child.has_name("field") && child.attribute("name") == name);
    match existing {
        Some(field) => {
            field.copy_attributes_from(over);
            for single in ["header-field", "first-row-field", "second-row-field"] {
                field.merge_single_child(over, single);
            }
            field.merge_children_by_keys(over, "conditional-field", &["condition"], None);
            for single in ["default-field", "last-row-field"] {
                field.merge_single_child(over, single);
            }
        }
        None => {
            base.append_node(if deep_copy { over.deep_copy() } else { over.clone() });
            if base.first_named("field-layout").is_some() {
                add_field_to_layout(base, over);
            }
        }
    }
}

fn add_field_to_layout(form: &MNode, field: &MNode) {
    let Some(layout) = form.first_named("field-layout") else {
        return;
    };
    let reference = layout.append("field-ref");
    reference.set_attribute("name", field.attribute("name").unwrap_or_default());
}

/// Replace `widget-template-include` and `auto-widget-entity` in the sub-nodes of a field.
fn expand_field_node(
    factory: &ExecutionContextFactory,
    form: &MNode,
    field: &MNode,
    form_name: &str,
) -> Result<(), Error> {
    for sub in field.children() {
        if !matches!(
            sub.name().as_str(),
            "header-field"
                | "first-row-field"
                | "second-row-field"
                | "conditional-field"
                | "default-field"
                | "last-row-field"
        ) {
            continue;
        }
        expand_field_sub_node(factory, form, field, &sub, form_name)?;
    }
    Ok(())
}

fn expand_field_sub_node(
    factory: &ExecutionContextFactory,
    form: &MNode,
    field: &MNode,
    sub: &MNode,
    form_name: &str,
) -> Result<(), Error> {
    let Some(widget) = sub.first_child() else {
        return Ok(());
    };
    match widget.name().as_str() {
        "widget-template-include" => {
            let location = widget.attribute("location").unwrap_or_default();
            let Some((file, template_name)) = location.split_once('#') else {
                return Err(Error::new(format!(
                    "widget-template-include.@location must contain a hash/pound sign to separate the file location and widget-template.@name: [{location}]"
                )));
            };
            let text = factory.resource().get(file)?.read_text()?;
            let templates = MNode::parse_text(file, &text)?;
            let template = templates
                .children()
                .into_iter()
                .find(|node| node.attribute_is("name", template_name))
                .ok_or_else(|| Error::new(format!("Could not find widget-template [{template_name}] in [{file}]")))?;
            let others: Vec<MNode> = sub.children().into_iter().skip(1).collect();
            let sets = widget.children_named("set");
            sub.clear_children();
            for node in template.children() {
                sub.append_node(node.deep_copy());
            }
            for node in others {
                sub.append_node(node.deep_copy());
            }
            for set in sets {
                sub.append_node(set.deep_copy());
            }
        }
        "auto-widget-entity" => {
            let entity_name = widget.attribute("entity-name").unwrap_or_default();
            let shape = entity_shape(&entity_name)?;
            let field_name = widget
                .attribute("field-name")
                .filter(|text| !text.is_empty())
                .or_else(|| field.attribute("name"))
                .unwrap_or_default();
            let field_type = widget
                .attribute("field-type")
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "find-display".to_string());
            sub.clear_children();
            add_auto_entity_field(&shape, &field_name, &field_type, field, Some(sub), form, form_name);
        }
        "auto-widget-service" => {
            let service_name = widget.attribute("service-name").unwrap_or_default();
            let parameter_name = widget
                .attribute("parameter-name")
                .filter(|text| !text.is_empty())
                .or_else(|| field.attribute("name"))
                .unwrap_or_default();
            let field_type = widget
                .attribute("field-type")
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "edit".to_string());
            sub.clear_children();
            if let Some(service) = crate::service::call::service_form_info(&service_name) {
                let parameter = service
                    .in_params
                    .iter()
                    .find(|parameter| parameter.name == parameter_name)
                    .ok_or_else(|| {
                        Error::new(format!(
                            "Cound not find parameter [{parameter_name}] in service [{service_name}] referred to in auto-widget-service of form [{}]",
                            form_name
                        ))
                    })?;
                let noun = entity_shape(&service.noun).ok();
                add_auto_service_field(noun.as_deref(), parameter, &field_type, field, sub, form, form_name);
            } else if let Some(entity_name) = crate::service::call::entity_auto_noun(&service_name) {
                let shape = entity_shape(&entity_name)?;
                add_auto_entity_field(&shape, &parameter_name, &field_type, field, Some(sub), form, form_name);
            } else {
                return Err(Error::new(format!(
                    "Cound not find service [{service_name}] or entity noun referred to in auto-widget-service of form [{}]",
                    form_name
                )));
            }
        }
        _ => {}
    }
    Ok(())
}

/// A type-`one` relationship with a single key that is not an auto-reverse. The first one for a field wins.
pub(super) struct OneRelation {
    /// The field on this entity.
    field: String,
    /// The related entity's full name.
    related: String,
    /// The key field on the related entity.
    related_key: String,
    /// The relationship `title`. Enumeration and StatusItem drop-downs filter by it.
    title: Option<String>,
}

/// What the form builder needs of an entity definition.
pub(super) struct EntityShape {
    pub full_name: String,
    /// The entity name without its package, which `getPrettyName` splits.
    pub entity_name: String,
    /// Name, type, and whether it is a primary key, in definition order.
    pub fields: Vec<(String, String, bool)>,
    pub pk_fields: Vec<String>,
    /// Related `one` entities reached by a single key field.
    pub one_relations: Vec<OneRelation>,
    pub description_field: String,
    /// A view alias's `default-display`, by alias name.
    pub default_display: HashMap<String, bool>,
    /// A view alias's `function`, by alias name. Empty for an entity that is not a view.
    pub alias_function: HashMap<String, String>,
    /// The view's `pq-expression` aliases with `default-display="true"`, which the auto fields show after the others.
    pub pq_display: Vec<String>,
}

/// The shapes made so far, by entity name, for the model they were read from. A form-list row and an option item ask
/// for the same entity over and over, and the model does not change for the life of an entity facade.
#[derive(Default)]
struct EntityShapes {
    by_name: RwLock<(usize, HashMap<String, Arc<EntityShape>>)>,
}

impl EntityShapes {
    fn get(&self, model: &EntityModel, entity_name: &str) -> Result<Arc<EntityShape>, moqui_lib::error::Error> {
        // The model's address tells a replaced entity facade apart, so its shapes are not reused.
        let model_key = std::ptr::from_ref(model) as usize;
        {
            let guard = self.by_name.read().unwrap_or_else(PoisonError::into_inner);
            if guard.0 == model_key {
                if let Some(shape) = guard.1.get(entity_name) {
                    return Ok(Arc::clone(shape));
                }
            }
        }
        let shape = Arc::new(read_entity_shape(model, entity_name)?);
        let mut guard = self.by_name.write().unwrap_or_else(PoisonError::into_inner);
        if guard.0 != model_key {
            *guard = (model_key, HashMap::new());
        }
        guard.1.insert(entity_name.to_string(), Arc::clone(&shape));
        Ok(shape)
    }
}

/// The shape of `entity_name` in the current factory's entity model, shared from a cache.
pub(super) fn entity_shape(entity_name: &str) -> Result<Arc<EntityShape>, Error> {
    let (shapes, facade) = ExecutionContext::with_current(|ec| {
        let factory = ec.factory();
        factory
            .entity()
            .map(|facade| (factory.extension(|_| EntityShapes::default()), facade))
    })
    .flatten()
    .ok_or_else(|| Error::new("no entity facade is installed"))?;
    // A definition built at runtime (a DB view entity, `DataDocument.<id>`) changes with its rows, so its shape is read
    // each time rather than kept with the model's.
    if facade.with_model(|model| model.entity(entity_name).is_err()) {
        return facade
            .with_entity(entity_name, |entity, _, _| Ok(Arc::new(entity_shape_of(entity))))
            .map_err(Error::from);
    }
    facade
        .with_model(|model| shapes.get(model, entity_name))
        .map_err(Error::from)
}

fn read_entity_shape(model: &EntityModel, entity_name: &str) -> Result<EntityShape, moqui_lib::error::Error> {
    Ok(entity_shape_of(model.entity(entity_name)?))
}

fn entity_shape_of(entity: &EntityDefinition) -> EntityShape {
    // A view's fields are its aliases, as Java's `getFieldNames`; an alias with no type is `text-long` to the forms.
    let fields: Vec<(String, String, bool)> = if entity.is_view {
        entity
            .aliases
            .iter()
            .map(|alias| {
                let field_type = alias.field_type.clone().unwrap_or_else(|| "text-long".to_string());
                (alias.name.clone(), field_type, alias.is_pk)
            })
            .collect()
    } else {
        entity
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.field_type.clone(), field.is_pk))
            .collect()
    };
    let mut one_relations = Vec::new();
    for relationship in &entity.relationships {
        // The expanded keys, so a relationship with no key-map (the related entity's primary key of the
        // same name) counts, as in Java.
        if relationship.relationship_type != "one" || relationship.is_auto_reverse || relationship.key_pairs.len() != 1
        {
            continue;
        }
        let (field, related_key) = relationship.key_pairs[0].clone();
        one_relations.push(OneRelation {
            field,
            related: relationship.related.clone(),
            related_key,
            title: relationship.title.clone(),
        });
    }
    let description_field = fields
        .iter()
        .find(|(name, _, is_pk)| !*is_pk && name.ends_with("Name"))
        .or_else(|| fields.iter().find(|(name, _, _)| name == "description"))
        .map(|(name, _, _)| name.clone())
        .unwrap_or_default();
    let default_display = entity
        .aliases
        .iter()
        .filter_map(|alias| alias.default_display.map(|display| (alias.name.clone(), display)))
        .collect();
    let alias_function = entity
        .aliases
        .iter()
        .filter_map(|alias| alias.function.clone().map(|function| (alias.name.clone(), function)))
        .collect();
    let pq_display = entity
        .pq_aliases
        .iter()
        .filter(|alias| alias.default_display)
        .map(|alias| alias.name.clone())
        .collect();
    EntityShape {
        full_name: entity.full_name.clone(),
        entity_name: entity.entity_name.clone(),
        pk_fields: fields
            .iter()
            .filter(|(_, _, is_pk)| *is_pk)
            .map(|(name, _, _)| name.clone())
            .collect(),
        fields,
        one_relations,
        description_field,
        default_display,
        alias_function,
        pq_display,
    }
}

fn add_entity_fields(
    shape: &EntityShape,
    include: &str,
    field_type: &str,
    excludes: &[String],
    form: &MNode,
    form_name: &str,
) -> Vec<String> {
    let mut displayed = Vec::new();
    let want_pk = include == "all" || include == "pk";
    let want_non_pk = include == "all" || include == "nonpk";
    for (name, entity_type, is_pk) in &shape.fields {
        if (*is_pk && !want_pk) || (!*is_pk && !want_non_pk) {
            continue;
        }
        if excludes.contains(name) || name == "lastUpdatedStamp" {
            continue;
        }
        let mut make_default = true;
        if form.has_name("form-list") {
            // As Java: an alias's `default-display` decides; without it a form-list leaves out the long text types but
            // `text-long`.
            let display = shape.default_display.get(name).copied();
            if display.is_none() && matches!(entity_type.as_str(), "text-very-long" | "text-fts" | "binary-very-long") {
                continue;
            }
            make_default = display.unwrap_or(true);
        }
        displayed.push(name.clone());
        let field = MNode::new("field");
        field.set_attribute("name", name.clone());
        let sub = make_default.then(|| {
            let sub = field.append("default-field");
            sub.set_attribute("validate-entity", shape.full_name.clone());
            sub.set_attribute("validate-field", name.clone());
            sub
        });
        add_auto_entity_field(shape, name, field_type, &field, sub.as_ref(), form, form_name);
        merge_field_node(form, &field, false);
    }
    // As Java, a `pq-expression` alias with `default-display="true"` is a display field whatever `include`, the
    // excludes and the field type, and is not among the displayed names.
    for name in &shape.pq_display {
        let field = MNode::new("field");
        field.set_attribute("name", name.clone());
        let sub = field.append("default-field");
        sub.set_attribute("validate-entity", shape.full_name.clone());
        sub.set_attribute("validate-field", name.clone());
        add_auto_entity_field(shape, name, "display", &field, Some(&sub), form, form_name);
        merge_field_node(form, &field, false);
    }
    displayed
}

fn add_auto_entity_field(
    shape: &EntityShape,
    field_name: &str,
    field_type: &str,
    field: &MNode,
    sub: Option<&MNode>,
    form: &MNode,
    form_name: &str,
) {
    let entity_type = shape
        .fields
        .iter()
        .find(|(name, _, _)| name == field_name)
        .map_or("text-long", |(_, field_type, _)| field_type.as_str());
    let relation = shape.one_relations.iter().find(|relation| relation.field == field_name);
    let is_list = form.has_name("form-list");
    let header = |order: &str| {
        if is_list && field.first_named("header-field").is_none() {
            let header = field.append("header-field");
            header.set_attribute("show-order-by", order);
        }
    };
    let display_entity = |target: &MNode, relation: &OneRelation| {
        // The text is over the related entity's value, so it is that entity's description field (Java's
        // `getDefaultDescriptionField` of the related definition), not the form entity's.
        let description = entity_shape(&relation.related)
            .map(|related| related.description_field.clone())
            .unwrap_or_default();
        let text = if description.is_empty() {
            format!("[${{{}}}]", relation.related_key)
        } else {
            format!("${{{description} || ''}} [${{{}}}]", relation.related_key)
        };
        let node = target.append("display-entity");
        node.set_attribute("entity-name", relation.related.clone());
        node.set_attribute("text", text);
    };
    match field_type {
        "edit" => {
            // lastUpdatedStamp stays hidden. The Edit link below still applies when it is also a relationship key.
            if field_name == "lastUpdatedStamp" {
                if let Some(sub) = sub {
                    sub.append("hidden");
                }
            } else {
                header("true");
                if let Some(sub) = sub {
                    if entity_type.starts_with("date") || entity_type.starts_with("time") {
                        let node = sub.append("date-time");
                        node.set_attribute("type", entity_type);
                        if field_name == "fromDate" {
                            node.set_attribute(
                                "default-value",
                                "${ec.l10n.format(ec.user.nowTimestamp, 'yyyy-MM-dd HH:mm')}",
                            );
                        }
                    } else if matches!(entity_type, "text-long" | "text-very-long" | "text-fts") {
                        sub.append("text-area");
                    } else if entity_type == "text-indicator" {
                        let drop = sub.append("drop-down");
                        drop.set_attribute("allow-empty", "true");
                        drop.append("option").set_attribute("key", "Y");
                        drop.append("option").set_attribute("key", "N");
                    } else if entity_type == "binary-very-long" {
                        sub.append("display");
                    } else if let Some(relation) = relation {
                        add_entity_field_drop_down(relation, sub);
                    } else {
                        let size = if entity_type.starts_with("number-") || entity_type.starts_with("currency-") {
                            "10"
                        } else {
                            "30"
                        };
                        sub.append("text-line").set_attribute("size", size);
                    }
                }
            }
        }
        "find" => {
            header("case-insensitive");
            if let Some(sub) = sub {
                if entity_type.starts_with("date") || entity_type.starts_with("time") {
                    sub.append("date-find").set_attribute("type", entity_type);
                } else if entity_type.starts_with("number-") || entity_type.starts_with("currency-") {
                    sub.append("range-find");
                } else if let Some(relation) = relation {
                    add_entity_field_drop_down(relation, sub);
                } else {
                    sub.append("text-find");
                }
            }
        }
        "display" => {
            header("case-insensitive");
            if let Some(sub) = sub {
                match relation {
                    Some(relation) => display_entity(sub, relation),
                    None => display_with_format(sub, entity_type),
                }
            }
        }
        "find-display" => {
            header("case-insensitive");
            let header_field = field
                .first_named("header-field")
                .unwrap_or_else(|| field.append("header-field"));
            if entity_type == "date" || entity_type == "time" {
                header_field.append("date-find").set_attribute("type", entity_type);
            } else if entity_type == "date-time" {
                header_field.append("date-period").set_attribute("time", "true");
            } else if entity_type.starts_with("number-") || entity_type.starts_with("currency-") {
                header_field.append("range-find").set_attribute("size", "10");
                field.set_attribute("align", "right");
                // A view alias's `function` of min, max, or avg is the total. Anything else, and an
                // entity that is not a view, totals with sum.
                let total = shape
                    .alias_function
                    .get(field_name)
                    .map(|function| function.to_ascii_lowercase())
                    .filter(|function| matches!(function.as_str(), "min" | "max" | "avg"))
                    .unwrap_or_else(|| "sum".to_string());
                field.set_attribute("show-total", total);
            } else if let Some(relation) = relation {
                add_entity_field_drop_down(relation, &header_field);
            } else {
                let find = header_field.append("text-find");
                find.set_attribute("size", "30");
                find.set_attribute("default-operator", "begins");
                find.set_attribute("ignore-case", "false");
            }
            if let Some(sub) = sub {
                match relation {
                    Some(relation) => display_entity(sub, relation),
                    None => display_with_format(sub, entity_type),
                }
            }
        }
        "hidden" => {
            if let Some(sub) = sub {
                sub.append("hidden");
            }
        }
        _ => {}
    }
    if form_name == "UpdateMasterEntityValue" {
        if let (Some(relation), Some(sub)) = (relation, sub) {
            add_master_edit_link(relation, sub);
        }
    }
}

/// `addEntityFieldDropDown`: a related entity with 1 to 200 rows is a drop-down, and any other count is a text line
/// with no size. Enumeration and StatusItem are counted only for the relationship title.
fn add_entity_field_drop_down(relation: &OneRelation, sub: &MNode) {
    let Ok(related) = entity_shape(&relation.related) else {
        sub.append("text-line");
        return;
    };
    let condition_field = match related.full_name.as_str() {
        "moqui.basic.Enumeration" => Some("enumTypeId"),
        "moqui.basic.StatusItem" => Some("statusTypeId"),
        _ => None,
    };
    let count = related_record_count(&related.full_name, condition_field, relation.title.as_deref());
    let count = match count {
        Ok(count) => count,
        Err(err) => {
            tracing::error!(
                entity = %related.full_name,
                field = %relation.field,
                "auto field drop-down count failed, using a text line: {err}"
            );
            sub.append("text-line");
            return;
        }
    };
    if !(1..=200).contains(&count) {
        sub.append("text-line");
        return;
    }
    let drop = sub.append("drop-down");
    drop.set_attribute("allow-empty", "true");
    drop.set_attribute("style", "");
    let options = drop.append("entity-options");
    let find = options.append("entity-find");
    find.set_attribute("entity-name", related.full_name.clone());
    find.set_attribute("offset", "0");
    find.set_attribute("limit", "200");
    if let Some(field) = condition_field {
        let condition = find.append("econdition");
        condition.set_attribute("field-name", field);
        condition.set_attribute("value", relation.title.clone().unwrap_or_default());
    }
    if !related.description_field.is_empty() {
        // Java writes Groovy `?:`. Option text is expanded as JavaScript, so this is `||`, the same
        // substitution `display-entity` already uses.
        options.set_attribute(
            "text",
            format!(
                "${{{} || ''}} [${{{}}}]",
                related.description_field, relation.related_key
            ),
        );
        let order = find.append("order-by");
        order.set_attribute("field-name", related.description_field.clone());
    }
}

/// Count rows with authorization off and without forcing the cache, so the entity's own cache flag applies.
fn related_record_count(entity_name: &str, field: Option<&str>, title: Option<&str>) -> Result<i64, Error> {
    ExecutionContext::with_current(|ec| {
        let facade = ec
            .factory()
            .entity()
            .ok_or_else(|| Error::new("no entity facade is installed"))?;
        let mut find = facade.find(entity_name).map_err(Error::from)?.disable_authz();
        if let Some(field) = field {
            let value = match title {
                Some(title) => Value::string(title),
                None => Value::Null,
            };
            find = find.eq(field, value);
        }
        find.count().map_err(Error::from)
    })
    .unwrap_or_else(|| Err(Error::new("no execution context is entered")))
}

/// The Edit link Java adds on `UpdateMasterEntityValue` for a field that points at one related entity.
fn add_master_edit_link(relation: &OneRelation, sub: &MNode) {
    let Ok(related) = entity_shape(&relation.related) else {
        return;
    };
    let pretty = crate::entity::info::pretty_name(&related.entity_name, None, None);
    let link = sub.append("link");
    link.set_attribute("url", "edit");
    link.set_attribute("text", format!("Edit {pretty} [${{fieldValues.{}}}]", relation.field));
    link.set_attribute("condition", relation.field.clone());
    link.set_attribute("link-type", "anchor");
    let entity = link.append("parameter");
    entity.set_attribute("name", "aen");
    entity.set_attribute("value", relation.related.clone());
    let key = link.append("parameter");
    key.set_attribute("name", relation.related_key.clone());
    key.set_attribute("from", format!("fieldValues.{}", relation.field));
}

/// `setSubFieldValidateAttrs`: when the form's transition calls one service, each field that is one of its in-parameters
/// gets `validate-service` on its sub-fields, or `validate-entity` for an implicit entity-auto service. A sub-field that
/// already names one keeps it.
fn set_sub_field_validate_attrs(def: &ScreenDef, form: &MNode, attribute: &str, sub_name: &str) {
    let Some(transition_name) = form.attribute(attribute).filter(|text| !text.is_empty()) else {
        return;
    };
    let Some(service) = def
        .transition(&transition_name, None)
        .and_then(|transition| transition.single_service.clone())
    else {
        return;
    };
    let (names, key, value) = if let Some(names) = crate::service::call::service_in_parameter_names(&service) {
        (names, "validate-service", service)
    } else if let Some(entity_name) = crate::service::call::entity_auto_noun(&service) {
        let Ok(shape) = entity_shape(&entity_name) else {
            return;
        };
        let names = shape.fields.iter().map(|(name, _, _)| name.clone()).collect();
        (names, "validate-entity", entity_name)
    } else {
        return;
    };
    for field in form.children_named("field") {
        if !names.contains(&field.attribute("name").unwrap_or_default()) {
            continue;
        }
        for sub in field.children_named(sub_name) {
            if sub.attribute(key).is_none_or(|text| text.is_empty()) {
                sub.set_attribute(key, value.clone());
            }
        }
    }
}

/// `addServiceFields`: a field for each parameter of the service, with `validate-service` on its default field.
fn add_service_fields(
    service: &crate::service::call::FormService,
    include: &str,
    field_type: &str,
    excludes: &[String],
    form: &MNode,
    form_name: &str,
) {
    let noun = entity_shape(&service.noun).ok();
    let mut parameters: Vec<&crate::service::define::ParamDef> = Vec::new();
    if include == "in" || include == "all" {
        parameters.extend(&service.in_params);
    }
    if include == "out" || include == "all" {
        parameters.extend(&service.out_params);
    }
    for parameter in parameters {
        if excludes.contains(&parameter.name) || parameter.name == "lastUpdatedStamp" || parameter.name.is_empty() {
            continue;
        }
        let field = MNode::new("field");
        field.set_attribute("name", parameter.name.clone());
        let sub = field.append("default-field");
        sub.set_attribute("validate-service", service.name.clone());
        sub.set_attribute("validate-parameter", parameter.name.clone());
        add_auto_service_field(noun.as_deref(), parameter, field_type, &field, &sub, form, form_name);
        merge_field_node(form, &field, false);
    }
}

/// `addAutoServiceField`: the widget of an entity field when the parameter is one, else one chosen by the parameter's type.
fn add_auto_service_field(
    noun: Option<&EntityShape>,
    parameter: &crate::service::define::ParamDef,
    field_type: &str,
    field: &MNode,
    sub: &MNode,
    form: &MNode,
    form_name: &str,
) {
    let own = parameter
        .entity_name
        .as_deref()
        .and_then(|name| entity_shape(name).ok());
    let shape = own.as_deref().or(noun);
    let field_name = parameter.field_name.clone().unwrap_or_else(|| parameter.name.clone());
    if let Some(shape) = shape {
        if shape.fields.iter().any(|(name, _, _)| *name == field_name) {
            add_auto_entity_field(shape, &field_name, field_type, field, Some(sub), form, form_name);
            return;
        }
    }
    let java = parameter.java_type.as_str();
    let entity_type = shape.and_then(|shape| {
        shape
            .fields
            .iter()
            .find(|(name, _, _)| *name == parameter.name)
            .map(|(_, field_type, _)| field_type.clone())
    });
    let date_kind = if java.ends_with("Date") && java != "java.util.Date" {
        Some("date")
    } else if java.ends_with("Time") {
        Some("time")
    } else if java.ends_with("Timestamp") || java == "java.util.Date" {
        Some("date-time")
    } else {
        None
    };
    let numeric = [
        "BigDecimal",
        "BigInteger",
        "Long",
        "Integer",
        "Double",
        "Float",
        "Number",
    ]
    .iter()
    .any(|suffix| java.ends_with(suffix));
    let with_format = |node: &MNode| {
        if let Some(format) = &parameter.format {
            node.set_attribute("format", format.clone());
        }
    };
    let find_widget = |target: &MNode| {
        if let Some(kind) = date_kind {
            let node = target.append("date-find");
            node.set_attribute("type", kind);
            with_format(&node);
        } else if numeric {
            target.append("range-find");
        } else {
            target.append("text-find");
        }
    };
    match field_type {
        "edit" => {
            if parameter.name == "lastUpdatedStamp" {
                sub.append("hidden");
            } else if let Some(kind) = date_kind {
                let node = sub.append("date-time");
                node.set_attribute("type", kind);
                with_format(&node);
            } else if matches!(
                entity_type.as_deref(),
                Some("text-long" | "text-very-long" | "text-fts")
            ) {
                sub.append("text-area");
            } else {
                let node = sub.append("text-line");
                if let Some(default) = &parameter.default_value {
                    node.set_attribute("default-value", default.clone());
                }
            }
        }
        "find" => find_widget(sub),
        "display" => with_format(&sub.append("display")),
        "find-display" => {
            find_widget(&field.append("header-field"));
            with_format(&sub.append("display"));
        }
        "hidden" => {
            sub.append("hidden");
        }
        _ => {}
    }
}

fn display_with_format(sub: &MNode, entity_type: &str) {
    let node = sub.append("display");
    match entity_type {
        "currency-amount" => node.set_attribute("format", "#,##0.00"),
        "currency-precise" => node.set_attribute("format", "#,##0.000"),
        _ => {}
    }
}

/// `addAutoEntityColumns`: the displayed fields in columns, id and date fields first, then
/// numbers, short text, and long text, about six to a column.
fn add_auto_entity_columns(form: &MNode, initial: &MNode, shape: &EntityShape, displayed: &[String]) {
    if displayed.is_empty() {
        return;
    }
    let per_column = {
        let count = displayed.len();
        // BigDecimal ROUND_HALF_UP of count / 6.0
        (count * 2 + 6) / 12
    };
    let kind = |name: &str| {
        shape
            .fields
            .iter()
            .find(|(field, _, _)| field == name)
            .map_or("text-long", |(_, field_type, _)| field_type.as_str())
    };
    let mut groups: [Vec<&String>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for name in displayed {
        let field_type = kind(name);
        let group = if field_type.starts_with("id") || field_type.starts_with("date") || field_type == "time" {
            0
        } else if field_type.starts_with("number")
            || field_type.starts_with("currency")
            || field_type == "text-indicator"
        {
            1
        } else if field_type == "text-short" || field_type == "text-medium" {
            2
        } else {
            3
        };
        groups[group].push(name);
    }
    let sorted: Vec<&String> = groups.into_iter().flatten().collect();
    let columns = match form.first_named("columns") {
        Some(columns) => columns,
        None => {
            let columns = form.append("columns");
            for field in initial.children_named("field") {
                let name = field.attribute("name").unwrap_or_default();
                let mut subs = field.children_named("default-field");
                subs.extend(field.children_named("conditional-field"));
                let mut used = false;
                for sub in subs {
                    if used {
                        continue;
                    }
                    if sub.children().len() == 1
                        && (sub.first_named("hidden").is_some() || sub.first_named("ignored").is_some())
                    {
                        continue;
                    }
                    columns
                        .append("column")
                        .append("field-ref")
                        .set_attribute("name", name.clone());
                    used = true;
                }
            }
            columns
        }
    };
    let mut current: Vec<&String> = Vec::new();
    for (index, name) in sorted.iter().enumerate() {
        current.push(name);
        if current.len() == per_column || index + 1 == sorted.len() {
            let column = columns.append("column");
            for field in current.drain(..) {
                column.append("field-ref").set_attribute("name", field.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use moqui_lib::node::MNode;

    use moqui_db::model::EntityModel;

    use super::{add_entity_fields, merge_field_node, merge_form_nodes, read_entity_shape};

    fn parse(xml: &str) -> MNode {
        MNode::parse_text("test", xml).unwrap()
    }

    #[test]
    fn an_override_form_replaces_attributes_the_entity_find_and_the_layout() {
        let base = parse(
            r#"<form-list name="A" list="one"><entity-find entity-name="X"/><row-actions><set field="a"/></row-actions>
               <field-layout><field-ref name="f"/></field-layout><field name="f"><default-field title="F"><display/></default-field></field></form-list>"#,
        );
        let over = parse(
            r#"<form-list name="A" list="two"><entity-find entity-name="Y"/><row-actions><set field="b"/></row-actions>
               <field-layout><field-ref name="g"/></field-layout>
               <field name="f"><default-field title="Changed"/></field><field name="g"><default-field><text-line/></default-field></field></form-list>"#,
        );
        merge_form_nodes(&base, &over, true, true);
        assert_eq!(base.attribute("list").as_deref(), Some("two"));
        let finds = base.children_named("entity-find");
        assert_eq!(finds.len(), 1);
        assert_eq!(finds[0].attribute("entity-name").as_deref(), Some("Y"));
        // row-actions of both run
        let actions: Vec<String> = base
            .first_named("row-actions")
            .unwrap()
            .children()
            .iter()
            .filter_map(|node| node.attribute("field"))
            .collect();
        assert_eq!(actions, ["a", "b"]);
        // the layout is the override's, with no field-ref added for a field it already names
        let layout = base.first_named("field-layout").unwrap();
        assert_eq!(
            layout
                .children()
                .iter()
                .filter_map(|node| node.attribute("name"))
                .collect::<Vec<_>>(),
            ["g"]
        );
        // the field of the same name is merged in place, the new one is appended
        let fields = base.children_named("field");
        assert_eq!(fields.len(), 2);
        let default = fields[0].first_named("default-field").unwrap();
        assert_eq!(default.attribute("title").as_deref(), Some("Changed"));
        assert!(
            default.first_named("display").is_some(),
            "a default-field without children keeps the base widgets"
        );
    }

    #[test]
    fn conditional_fields_merge_by_condition() {
        let base = parse(
            r#"<form-single name="A"><field name="f"><conditional-field condition="a"><display/></conditional-field>
               <default-field><display/></default-field></field></form-single>"#,
        );
        let over = parse(
            r#"<field name="f"><conditional-field condition="a" title="T"><text-line/></conditional-field>
               <conditional-field condition="b"><hidden/></conditional-field></field>"#,
        );
        merge_field_node(&base, &over, false);
        let field = base.first_named("field").unwrap();
        let conditions: Vec<String> = field
            .children_named("conditional-field")
            .iter()
            .filter_map(|node| node.attribute("condition"))
            .collect();
        assert_eq!(conditions, ["a", "b"]);
        let first = &field.children_named("conditional-field")[0];
        assert_eq!(first.attribute("title").as_deref(), Some("T"));
        assert!(first.first_named("text-line").is_some() && first.first_named("display").is_none());
    }

    /// As Java's `addEntityFields`: a view's auto fields are its aliases, and in a form-list an alias's
    /// `default-display` decides whether it gets a field to show, over the default that leaves out the long text types.
    #[test]
    fn a_view_alias_default_display_decides_a_form_list_auto_field() {
        let conf = parse(
            r#"<database-list>
                <dictionary-type type="id" java-type="java.lang.String" default-sql-type="VARCHAR(40)"/>
                <dictionary-type type="text-very-long" java-type="java.lang.String" default-sql-type="TEXT"/>
            </database-list>"#,
        );
        let entities = parse(
            r#"<entities>
                <entity entity-name="Item" package="t" no-update-stamp="true">
                    <field name="itemId" type="id" is-pk="true"/><field name="note" type="text-very-long"/>
                    <field name="code" type="id"/><field name="body" type="text-very-long"/></entity>
                <view-entity entity-name="ItemView" package="t">
                    <member-entity entity-alias="I" entity-name="t.Item"/>
                    <alias name="itemId" entity-alias="I"/>
                    <alias name="note" entity-alias="I" default-display="true"/>
                    <alias name="code" entity-alias="I" default-display="false"/>
                    <alias name="body" entity-alias="I"/>
                </view-entity></entities>"#,
        );
        let model = EntityModel::load(&conf, &[entities]).unwrap();
        let shape = read_entity_shape(&model, "t.ItemView").unwrap();
        assert_eq!(shape.pk_fields, ["itemId"]);
        let form = parse(r#"<form-list name="L" list="items"/>"#);
        let shown = add_entity_fields(&shape, "all", "display", &[], &form, "L");
        assert_eq!(shown, ["itemId", "note", "code"]);
        let field = |name: &str| {
            form.children_named("field")
                .into_iter()
                .find(|field| field.attribute_is("name", name))
                .unwrap()
        };
        assert!(field("note").first_named("default-field").is_some());
        assert!(field("code").first_named("default-field").is_none());
    }

    /// A find-display number totals with the view alias function when that is min, max, or avg.
    /// An alias with no function, and a field of an entity that is not a view, stay sum.
    #[test]
    fn a_view_alias_function_sets_the_find_display_total() {
        let conf = parse(
            r#"<database-list>
                <dictionary-type type="id" java-type="java.lang.String" default-sql-type="VARCHAR(40)"/>
                <dictionary-type type="number-decimal" java-type="java.math.BigDecimal" default-sql-type="NUMERIC(26,6)"/>
            </database-list>"#,
        );
        let entities = parse(
            r#"<entities>
                <entity entity-name="Line" package="t" no-update-stamp="true">
                    <field name="lineId" type="id" is-pk="true"/>
                    <field name="qty" type="number-decimal"/>
                    <field name="low" type="number-decimal"/>
                    <field name="counted" type="number-decimal"/>
                </entity>
                <view-entity entity-name="LineView" package="t">
                    <member-entity entity-alias="L" entity-name="t.Line"/>
                    <alias name="lineId" entity-alias="L"/>
                    <alias name="qty" entity-alias="L"/>
                    <alias name="low" entity-alias="L" function="min"/>
                    <alias name="counted" entity-alias="L" function="count"/>
                </view-entity></entities>"#,
        );
        let model = EntityModel::load(&conf, &[entities]).unwrap();
        let total = |form: &MNode, name: &str| {
            form.children_named("field")
                .into_iter()
                .find(|field| field.attribute_is("name", name))
                .and_then(|field| field.attribute("show-total"))
        };
        let view = read_entity_shape(&model, "t.LineView").unwrap();
        let form = parse(r#"<form-list name="L"/>"#);
        add_entity_fields(&view, "all", "find-display", &[], &form, "L");
        assert_eq!(total(&form, "low").as_deref(), Some("min"));
        assert_eq!(total(&form, "qty").as_deref(), Some("sum"));
        assert_eq!(total(&form, "counted").as_deref(), Some("sum"));
        let entity = read_entity_shape(&model, "t.Line").unwrap();
        assert!(entity.alias_function.is_empty());
        let plain = parse(r#"<form-list name="L"/>"#);
        add_entity_fields(&entity, "nonpk", "find-display", &[], &plain, "L");
        assert_eq!(total(&plain, "qty").as_deref(), Some("sum"));
        assert_eq!(total(&plain, "low").as_deref(), Some("sum"));
    }

    /// As Java's `addEntityFields`: a view's `pq-expression` alias with `default-display="true"` is a display field
    /// whatever the form, `include`, field type and excludes, and is not one of the names shown; one without it gets no
    /// field.
    #[test]
    fn a_pq_expression_alias_with_default_display_is_an_auto_display_field() {
        let conf = parse(
            r#"<database-list>
                <dictionary-type type="id" java-type="java.lang.String" default-sql-type="VARCHAR(40)"/>
            </database-list>"#,
        );
        let entities = parse(
            r#"<entities>
                <entity entity-name="Item" package="t" no-update-stamp="true">
                    <field name="itemId" type="id" is-pk="true"/><field name="code" type="id"/></entity>
                <view-entity entity-name="ItemView" package="t">
                    <member-entity entity-alias="I" entity-name="t.Item"/>
                    <alias-all entity-alias="I"/>
                    <alias name="label" pq-expression="itemId + ':' + code" default-display="true"/>
                    <alias name="quiet" pq-expression="code"/>
                </view-entity></entities>"#,
        );
        let model = EntityModel::load(&conf, &[entities]).unwrap();
        let shape = read_entity_shape(&model, "t.ItemView").unwrap();
        assert_eq!(shape.pq_display, ["label"]);
        let form = parse(r#"<form-single name="S"/>"#);
        let shown = add_entity_fields(&shape, "pk", "edit", &["label".to_string()], &form, "S");
        assert_eq!(shown, ["itemId"]);
        let names: Vec<String> = form
            .children_named("field")
            .iter()
            .filter_map(|field| field.attribute("name"))
            .collect();
        assert_eq!(names, ["itemId", "label"]);
        let label = form.children_named("field")[1].first_named("default-field").unwrap();
        assert_eq!(label.attribute("validate-entity").as_deref(), Some("t.ItemView"));
        assert_eq!(label.attribute("validate-field").as_deref(), Some("label"));
        assert!(label.first_named("display").is_some());
        assert!(label.first_named("text-area").is_none() && label.first_named("text-line").is_none());
    }

    /// Java's `addEntityFields` of a `DataDocument.<id>` name: the document's view, its fields' `defaultDisplay` and
    /// an expression field with `defaultDisplay` `Y` a display field; its shape is read again after its rows change.
    #[test]
    fn a_data_document_name_gives_the_auto_fields_of_its_view() {
        use moqui_lib::object::Value;
        let facade = crate::testing::open();
        let context = crate::context::ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let create = |entity: &str, pairs: &[(&str, &str)]| {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in pairs {
                row.set(*name, Value::string(*value));
            }
            row.create(&facade).unwrap();
        };
        create(
            "moqui.entity.document.DataDocument",
            &[
                ("dataDocumentId", "FormDoc"),
                ("primaryEntityName", "moqui.basic.Enumeration"),
            ],
        );
        let field = |seq: &str, path: &str, extra: &[(&str, &str)]| {
            let mut pairs = vec![("dataDocumentId", "FormDoc"), ("fieldSeqId", seq), ("fieldPath", path)];
            pairs.extend_from_slice(extra);
            create("moqui.entity.document.DataDocumentField", &pairs);
        };
        field("01", "enumId", &[]);
        field("02", "description", &[("defaultDisplay", "N")]);
        field(
            "03",
            "(enumId + '!')",
            &[("fieldNameAlias", "shout"), ("defaultDisplay", "Y")],
        );
        let shape = super::entity_shape("DataDocument.FormDoc").unwrap();
        assert_eq!(shape.full_name, "DataDocument.FormDoc");
        assert_eq!(shape.pq_display, ["shout"]);
        let form = parse(r#"<form-list name="L"/>"#);
        let shown = add_entity_fields(&shape, "all", "display", &[], &form, "L");
        assert_eq!(shown, ["enumId", "description"]);
        let fields = form.children_named("field");
        let names: Vec<String> = fields.iter().filter_map(|field| field.attribute("name")).collect();
        assert_eq!(names, ["enumId", "description", "shout"]);
        assert!(fields[1].first_named("default-field").is_none(), "defaultDisplay N");
        assert!(fields[2]
            .first_named("default-field")
            .unwrap()
            .first_named("display")
            .is_some());

        field("04", "enumTypeId", &[]);
        let shape = super::entity_shape("DataDocument.FormDoc").unwrap();
        assert!(shape.fields.iter().any(|(name, _, _)| name == "enumTypeId"));
    }

    /// A `display-entity` of a status or an enumeration shows its `description`: the first non-key field ending in
    /// `Name` would come first (StatusItem once gained a `colorName`, and every status in a form showed empty).
    #[test]
    fn a_status_and_an_enumeration_are_described_by_their_description() {
        let framework = crate::testing::framework_dir();
        let conf_text = std::fs::read_to_string(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
        let conf = parse(&conf_text);
        let model = EntityModel::load_directory(&conf.first_named("database-list").unwrap(), &framework.join("entity"))
            .unwrap();
        for entity_name in ["moqui.basic.StatusItem", "moqui.basic.Enumeration"] {
            let shape = read_entity_shape(&model, entity_name).unwrap();
            assert_eq!(shape.description_field, "description", "{entity_name}");
        }
    }
}
