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

//! A screen's doc: its title and settings, the comment that heads its file, its parameters, subscreens, transitions and
//! forms, and what mounts, extends and links to it.

use moqui_core::screen::{Authentication, Implicit, ResponseKind, ScreenDef, ScreenFacade};
use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use super::Run;
use crate::autodoc::plan::DocEntry;

/// The screen definition of a screen doc (its name is its location).
pub(super) fn definition(run: &Run<'_>, location: &str) -> Result<std::sync::Arc<ScreenDef>, Error> {
    ScreenFacade::of(run.factory)
        .screen_def(run.factory, location)
        .map_err(|err| Error::new(err.to_string()))
}

pub(super) fn data(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let def = definition(run, &doc.name)?;
    let description = run.descriptions.lead(&doc.file);
    let mut map = run.header(doc, description);

    let parameters: Vec<Json> = def
        .parameters
        .iter()
        .map(|parameter| json!({"name": parameter.name, "required": parameter.required, "from": parameter.from}))
        .collect();
    let subscreens: Vec<Json> = def
        .subscreens
        .items
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "title": item.menu_title,
                "index": item.menu_index,
                "include": item.menu_include,
                "includeWhen": item.menu_include_when,
                "location": item.location,
                "default": def.subscreens.default_item.as_deref() == Some(item.name.as_str()),
                "href": run.link_id(doc, &format!("screen:{}", item.location)),
            })
        })
        .collect();
    let transitions: Vec<Json> = def
        .transitions
        .iter()
        .filter(|transition| transition.implicit == Implicit::None)
        .map(|transition| {
            let response = &transition.default_response;
            let response_text = match response.kind {
                ResponseKind::None => "none".to_string(),
                ResponseKind::Url => response.url.clone(),
                ResponseKind::ScreenLast => "screen-last".to_string(),
                ResponseKind::ScreenLastNoParam => "screen-last-noparam".to_string(),
            };
            json!({
                "name": transition.name,
                "method": transition.method,
                "service": transition.single_service,
                "serviceHref": transition.single_service.as_ref().and_then(|service| run.link_id(doc, &format!("service:{service}"))),
                "actions": transition.actions.is_some() || transition.service_actions.is_some(),
                "response": response_text,
                "readOnly": transition.read_only,
            })
        })
        .collect();
    let mut forms: Vec<Json> = Vec::new();
    for (name, form) in &def.forms {
        let form_id = format!("form:{}#{name}", def.location);
        let fields: Vec<String> = run
            .source
            .children(&form_id)?
            .into_iter()
            .filter(|row| row.kind == "form-field")
            .map(|row| {
                row.id
                    .rsplit_once('@')
                    .map_or(row.short_name.clone(), |(_, field)| field.to_string())
            })
            .collect();
        forms.push(json!({
            "name": name,
            "type": if form.list { "form-list" } else { "form-single" },
            "extends": form.extends,
            "fields": fields.join(", "),
            "fieldCount": fields.len(),
        }));
    }
    let (used_by, uses, usage) = run.usage_data(doc, &[&doc.id])?;
    map.insert(
        "screen".into(),
        json!({
            "title": def.default_menu_title,
            "renderModes": def.render_modes.join(", "),
            "serverStatic": def.server_static.join(", "),
            "standalone": def.standalone,
            "authentication": match def.authentication {
                Authentication::Required => "true",
                Authentication::Optional => "false",
                Authentication::AnonymousView => "anonymous-view",
                Authentication::AnonymousAll => "anonymous-all",
            },
            "allowExtraPath": def.allow_extra_path,
            "defaultItem": def.subscreens.default_item,
            "parameters": parameters,
            "subscreens": subscreens,
            "transitions": transitions,
            "forms": forms,
        }),
    );
    map.insert("usedBy".into(), used_by);
    map.insert("uses".into(), uses);
    map.insert("diagrams".into(), json!({"usage": usage}));
    Ok(Json::Object(map))
}
