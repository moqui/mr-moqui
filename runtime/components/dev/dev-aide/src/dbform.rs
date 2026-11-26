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

//! "Convert to site extension" (plans/design-dev-aide.md 7.2, question 16: one way only). A `DbForm` that modifies a
//! screen form (`modifyXmlScreenForm`, rows in the database) becomes the form merge of the site component's
//! `screen-extend` of that screen: the form of the base form's kind and name with the DbForm's fields, written as the
//! screen builder's extension mode writes a form merge. The fields are the ones the framework builds from the rows
//! (`moqui_core::screen::db_form_xml`, as of now with no drafts), so each `DbFormField` (title, tooltip, condition,
//! entry name, required), `DbFormFieldAttribute` (on the field, the sub-field or the widget, as the render puts it),
//! option, entity option with its conditions and order, and dependency (`visible-when`, `require-when`) is in the merge
//! as the render merged it. A DbForm's new field has no `after`, so it goes last, as the DbForm put it.
//!
//! Review and accept are one step each: `get#DbFormConversion` shows the file as it would be (its diff), the rows that
//! go and what is not carried; `convert#DbForm` with that preview's digest deletes the rows and writes the file
//! through the save pipeline (a conflict refuses the whole), in one transaction, so the DbForm is gone only when the
//! extension is written.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::EntityFacade;
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::xml_edit::{Locator, Op, OutlineNode, Position};
use serde_json::{json, Value as Json};

use crate::audit::{self, Write};
use crate::tier::Tier;
use crate::workspace::{json_value, sha256, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_db_form_changes), get_db_form_changes)?;
    registry.service_function(
        moqui_core::service_location!(get_db_form_conversion),
        get_db_form_conversion,
    )?;
    registry.service_function(moqui_core::service_location!(convert_db_form), convert_db_form)
}

/// A new `screen-extend` file, as the screen builder starts one.
const EMPTY_EXTEND: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<screen-extend xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/mr-xml-screen.xsd\">\n</screen-extend>\n";

/// The rows of a DbForm the conversion deletes, children first: the field's attributes, options and entity options,
/// the dependencies, the fields, the user groups and the form.
const ROWS: [&str; 9] = [
    "moqui.screen.form.DbFormFieldAttribute",
    "moqui.screen.form.DbFormFieldOption",
    "moqui.screen.form.DbFormFieldEntOptsCond",
    "moqui.screen.form.DbFormFieldEntOptsOrder",
    "moqui.screen.form.DbFormFieldEntOpts",
    "moqui.screen.form.DbFormFieldDependency",
    "moqui.screen.form.DbFormField",
    "moqui.screen.form.DbFormUserGroup",
    "moqui.screen.form.DbForm",
];

/// The answers to a DbForm, which keep it: a DbForm with responses is a survey, not a change of a screen form.
const RESPONSES: [&str; 2] = ["moqui.screen.form.FormResponse", "moqui.screen.form.FormResponseAnswer"];

/// The print settings of a DbForm and its fields, which a screen form has no place for.
const PRINT_FIELDS: [&str; 4] = [
    "printTemplateLocation",
    "acroFormLocation",
    "printFontSize",
    "printFontFamily",
];

/// The fields of a form a DbForm makes (`db_form_xml`), each as the form merge writes it: its XML with the
/// `layoutSequenceNum` the build keeps for its own order left out (the order is the fields' order here).
///
/// # Errors
///
/// When the text is not XML.
pub fn merge_fields(form_xml: &str) -> Result<Vec<String>, LibError> {
    let node = MNode::parse_text("dbform", form_xml)?;
    Ok(node
        .children_named("field")
        .iter()
        .map(|field| {
            field.remove_attribute("layoutSequenceNum");
            field.write_string()
        })
        .collect())
}

/// The operations that put `fields` into the form merge `kind` (`form-single` or `form-list`) named `form` of the
/// `screen-extend` whose outline is `outline`: into the merge of that name when the file has one, else into a new one
/// at its end, as the screen builder's extension mode adds a field (`ops.extendFormField`).
pub fn extension_ops(outline: &OutlineNode, kind: &str, form: &str, fields: &[String]) -> Vec<Op> {
    let existing = outline
        .children
        .iter()
        .find(|child| child.name == kind && child.attr("name") == Some(form));
    match existing {
        Some(merge) => fields
            .iter()
            .map(|field| Op::Insert {
                at: Locator::Path(merge.path.clone()),
                position: Position::Last,
                xml: field.clone(),
            })
            .collect(),
        None => vec![Op::Insert {
            at: Locator::Path(Vec::new()),
            position: Position::Last,
            xml: format!(
                "<{kind} name=\"{}\">\n{}\n</{kind}>",
                crate::workspace::xml_attr(form),
                fields.join("\n")
            ),
        }],
    }
}

fn row_text(row: &moqui_core::entity::EntityValue, field: &str) -> String {
    row.get(field)
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn of_form(
    facade: &EntityFacade,
    entity: &str,
    form_id: &str,
) -> Result<Vec<moqui_core::entity::EntityValue>, LibError> {
    Ok(facade
        .find(entity)?
        .disable_authz()
        .eq("formId", Value::string(form_id))
        .list()?
        .into_values())
}

/// `get#DbFormChanges`: the DbForms that modify a screen form, each with its screen and form, its user groups and how
/// many fields it has: the rows Customizations offers Convert on.
fn get_db_form_changes(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let forms = context
        .facade
        .find("moqui.screen.form.DbForm")?
        .disable_authz()
        .order_by("formId", false)
        .list()?;
    let mut rows = Vec::new();
    for form in forms.iter() {
        let modifies = row_text(form, "modifyXmlScreenForm");
        let Some((screen, form_name)) = modifies.split_once('#') else {
            continue;
        };
        let form_id = row_text(form, "formId");
        let groups: Vec<String> = of_form(&context.facade, "moqui.screen.form.DbFormUserGroup", &form_id)?
            .iter()
            .map(|row| row_text(row, "userGroupId"))
            .collect();
        let fields = of_form(&context.facade, "moqui.screen.form.DbFormField", &form_id)?.len();
        rows.push(json!({
            "formId": form_id, "description": row_text(form, "description"), "screenLocation": screen,
            "formName": form_name, "userGroups": groups.join(", "), "fieldCount": fields,
        }));
    }
    let mut out = Parameters::new();
    out.insert("dbForms".into(), json_value(&rows));
    Ok(out)
}

/// What converting the DbForm `form_id` would do, as both services compute it.
struct Conversion {
    form_id: String,
    root_id: String,
    path: String,
    component: String,
    exists: bool,
    base_hash: String,
    current: String,
    proposed: String,
    /// Each entity of [`ROWS`] with the rows of it that go.
    rows: Vec<(&'static str, usize)>,
    notes: Vec<String>,
    digest: String,
}

fn conversion(context: &Context, form_id: &str) -> Result<Conversion, LibError> {
    let facade = &context.facade;
    let form = facade
        .find("moqui.screen.form.DbForm")?
        .disable_authz()
        .eq("formId", Value::string(form_id))
        .one()?
        .ok_or_else(|| LibError::new(format!("no DbForm {form_id}")))?;
    let modifies = row_text(&form, "modifyXmlScreenForm");
    let (screen, form_name) = modifies.split_once('#').ok_or_else(|| {
        LibError::new(format!(
            "DbForm {form_id} modifies no screen form (modifyXmlScreenForm): a form that extends it stays a DbForm"
        ))
    })?;
    for entity in RESPONSES {
        if !of_form(facade, entity, form_id)?.is_empty() {
            return Err(LibError::new(format!(
                "DbForm {form_id} has responses ({entity}), which keep it: it stays a DbForm"
            )));
        }
    }
    // the kind of the base form: a merge of the other kind would not apply
    let definition = moqui_core::screen::ScreenFacade::of(&context.factory).screen_def(&context.factory, screen)?;
    let base = definition
        .forms
        .get(form_name)
        .ok_or_else(|| LibError::new(format!("{screen} has no form {form_name}")))?;
    let kind = if base.list { "form-list" } else { "form-single" };
    let after = screen.strip_prefix("component://").unwrap_or(screen);
    let target = crate::site::site_path(context, &format!("screen-extend/{after}"))
        .ok_or_else(|| LibError::new("there is no site component to write the extension in: Home creates one"))?;
    let root = context.root(&target.root_id)?;
    let resolved = context.resolve(root, &target.path)?;
    let (current, base_hash) = match std::fs::read(&resolved.path) {
        Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), sha256(&bytes)),
        Err(_) => (EMPTY_EXTEND.to_string(), String::new()),
    };
    let form_xml = moqui_core::screen::db_form_xml(form_id, moqui_core::screen::DbFormAsOf::of(None, None))?;
    let fields = merge_fields(&form_xml)?;
    if fields.is_empty() {
        return Err(LibError::new(format!(
            "DbForm {form_id} has no fields in effect to convert"
        )));
    }
    let outline = moqui_lib::xml_edit::outline(&current)?;
    let proposed = moqui_lib::xml_edit::apply(&current, &extension_ops(&outline, kind, form_name, &fields))?.text;
    let mut rows = Vec::new();
    for entity in ROWS {
        rows.push((entity, of_form(facade, entity, form_id)?.len()));
    }
    let groups: Vec<String> = of_form(facade, "moqui.screen.form.DbFormUserGroup", form_id)?
        .iter()
        .map(|row| row_text(row, "userGroupId"))
        .collect();
    let mut notes = Vec::new();
    if !groups.iter().any(|group| group == "ALL_USERS") {
        notes.push(format!(
            "The DbForm changed the form for the users of {}; the extension changes it for every user.",
            if groups.is_empty() {
                "no group".to_string()
            } else {
                groups.join(", ")
            }
        ));
    }
    if row_text(&form, "isDated") == "Y" {
        notes.push(
            "The DbForm is dated: the fields in effect now are converted; its drafts and its history go with its rows."
                .to_string(),
        );
    }
    let fields_rows = of_form(facade, "moqui.screen.form.DbFormField", form_id)?;
    if PRINT_FIELDS.iter().any(|field| !row_text(&form, field).is_empty())
        || fields_rows.iter().any(|row| {
            ["printTop", "printLeft", "printWidth", "printPageNumber"]
                .iter()
                .any(|f| !row_text(row, f).is_empty())
        })
    {
        notes.push("The DbForm's print layout is not carried: a screen form has no place for it.".to_string());
    }
    let digest = sha256(
        format!(
            "{form_id}\n{base_hash}\n{}\n{proposed}",
            rows.iter()
                .map(|(entity, count)| format!("{entity}={count}"))
                .collect::<Vec<_>>()
                .join(",")
        )
        .as_bytes(),
    );
    Ok(Conversion {
        form_id: form_id.to_string(),
        root_id: target.root_id,
        path: target.path,
        component: target.component,
        exists: target.exists,
        base_hash,
        current,
        proposed,
        rows,
        notes,
        digest,
    })
}

/// `get#DbFormConversion`: the conversion of a DbForm to review: the site file it writes (`rootId`, `path`, whether it
/// exists), its text now and as it would be with the `diff`, the rows that go, what is not carried (`notes`), and the
/// `digest` that `convert#DbForm` takes.
fn get_db_form_conversion(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let found = conversion(&context, &text(parameters, "formId"))?;
    let rows: Vec<Json> = found
        .rows
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(entity, count)| json!({ "entityName": entity, "count": count }))
        .collect();
    let mut out = Parameters::new();
    out.insert("rootId".into(), Value::string(&found.root_id));
    out.insert("path".into(), Value::string(&found.path));
    out.insert("component".into(), Value::string(&found.component));
    out.insert("exists".into(), Value::Bool(found.exists));
    out.insert("currentText".into(), Value::string(&found.current));
    out.insert("proposedText".into(), Value::string(&found.proposed));
    out.insert(
        "diff".into(),
        Value::string(moqui_core::resource::textdiff::unified(
            if found.exists { &found.current } else { "" },
            &found.proposed,
            &format!("{} (now)", found.path),
            &format!("{} (converted)", found.path),
        )),
    );
    out.insert("rows".into(), json_value(&rows));
    out.insert("notes".into(), json_value(&found.notes));
    out.insert("digest".into(), Value::string(&found.digest));
    Ok(out)
}

/// `convert#DbForm`: the accepted conversion. The preview's `digest` must still be the conversion's (the rows and the
/// file as they were reviewed); the DbForm's rows are deleted and the file written through the save pipeline in the
/// service's transaction, so a write that fails or conflicts keeps the rows.
fn convert_db_form(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let form_id = text(parameters, "formId");
    let found = conversion(&context, &form_id)?;
    if found.digest != text(parameters, "digest") {
        return Err(LibError::new(
            "the DbForm or the site's file changed since the review: review the conversion again",
        ));
    }
    let root = context.root(&found.root_id)?.clone();
    let resolved = context.resolve(&root, &found.path)?;
    // the rows first, in this transaction: a write refused below rolls them back
    let mut deleted = serde_json::Map::new();
    for entity in ROWS {
        let count = context
            .facade
            .find(entity)?
            .disable_authz()
            .eq("formId", Value::string(&found.form_id))
            .delete_all()?;
        deleted.insert(entity.to_string(), json!(count));
    }
    let detail = json!({ "dbForm": found.form_id, "deleted": deleted });
    let saved = crate::workspace::save_text(
        &context,
        &root,
        &resolved,
        &found.proposed,
        &found.base_hash,
        Some(detail.clone()),
    )?;
    if saved
        .get("conflict")
        .is_some_and(|value| matches!(value, Value::Bool(true)))
    {
        return Err(LibError::new(format!(
            "{} changed on disk since the review: review the conversion again",
            found.path
        )));
    }
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_CONVERT",
            root: &root.id,
            path: &found.path,
            detail: Some(detail),
            ..Write::default()
        },
    );
    let mut out = saved;
    out.insert("rootId".into(), Value::string(&found.root_id));
    out.insert("path".into(), Value::string(&found.path));
    out.insert("deleted".into(), json_value(&deleted));
    Ok(out)
}

#[cfg(test)]
#[path = "dbform_tests.rs"]
mod tests;
