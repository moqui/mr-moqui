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

//! `services.d.ts`: the input and output map of each defined service. An input key is required when the validation
//! rejects the call without it: `required="true"` and no `default` or `default-value`. `auto-parameters` expand against
//! the entity model as the validation expands them. The implicit entity-auto verbs are mapped types over the entity map
//! in `moqui-host.d.ts`, so they are not listed here.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use moqui_db::model::EntityModel;

use super::ts::{doc, input_type, quote, server_type};
use crate::service::define::{ParamDef, Required, ServiceDef, ServiceKind};

struct Key {
    ts_type: &'static str,
    required: bool,
}

/// The declared parameters with `auto-parameters` expanded. An entity that is not loaded leaves a note instead.
fn expand(
    def: &ServiceDef,
    params: &[ParamDef],
    model: Option<&EntityModel>,
    notes: &mut Vec<String>,
) -> Vec<ParamDef> {
    let mut out = Vec::new();
    for param in params {
        let Some(auto) = param.auto.as_ref().filter(|_| param.name.is_empty()) else {
            crate::service::merge_param(&mut out, param.clone());
            continue;
        };
        let expanded = crate::service::auto_entity_name(def, auto).and_then(|name| {
            let model = model.ok_or_else(|| crate::context::Error::new("no entity model is loaded"))?;
            let entity = model.entity(&name).map_err(crate::context::Error::from)?;
            Ok(crate::service::auto_fields(auto, entity, model))
        });
        match expanded {
            Ok(params) => {
                for param in params {
                    crate::service::merge_param(&mut out, param);
                }
            }
            Err(err) => notes.push(format!("{}: auto-parameters: {err}", def.name)),
        }
    }
    out
}

/// Keys by name. A name declared twice is required when either declaration makes it required.
fn keys(params: &[ParamDef], ts_type: fn(&str) -> &'static str, inputs: bool) -> BTreeMap<String, Key> {
    let mut out: BTreeMap<String, Key> = BTreeMap::new();
    for param in params {
        if param.name.is_empty() || param.required == Required::Disabled {
            continue;
        }
        let defaulted = param.default_value.is_some() || param.default_expr.is_some();
        let required = inputs && param.required == Required::True && !defaulted;
        out.entry(param.name.clone())
            .and_modify(|key| key.required |= required)
            .or_insert(Key {
                ts_type: ts_type(&param.java_type),
                required,
            });
    }
    out
}

fn write_map(out: &mut String, keys: &BTreeMap<String, Key>, open: bool) {
    if keys.is_empty() {
        out.push_str(if open { "MoquiMap" } else { "Record<string, never>" });
        return;
    }
    out.push_str("{\n");
    for (name, key) in keys {
        if key.required {
            let _ = writeln!(out, "        {}: {};", quote(name), key.ts_type);
        } else {
            let _ = writeln!(out, "        {}?: {} | null;", quote(name), key.ts_type);
        }
    }
    out.push_str("    }");
    if open {
        out.push_str(" & MoquiMap");
    }
}

fn kind_name(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Inline => "inline",
        ServiceKind::EntityAuto => "entity-auto",
        ServiceKind::Script => "script",
        ServiceKind::Interface => "interface",
        ServiceKind::RemoteRest => "remote-rest",
        ServiceKind::Rust => "rust",
        ServiceKind::Native => "native",
        ServiceKind::Unsupported => "script (not run by this port)",
    }
}

/// The text of `services.d.ts`, and a note for each `auto-parameters` that could not be expanded.
pub(super) fn emit(services: &[ServiceDef], model: Option<&EntityModel>) -> (String, Vec<String>) {
    let mut notes = Vec::new();
    let mut sorted: Vec<&ServiceDef> = services.iter().collect();
    sorted.sort_by(|left, right| left.name.cmp(&right.name));
    let mut inputs = String::new();
    let mut outputs = String::new();
    for def in sorted {
        let in_params = expand(def, &def.in_params, model, &mut notes);
        let mut in_keys = keys(&in_params, input_type, true);
        // An entity-auto service keeps any field of its entity, declared or not.
        if def.kind == ServiceKind::EntityAuto {
            if let Some(entity) = model.and_then(|model| model.entity(&def.noun).ok()) {
                for field in entity.field_names() {
                    in_keys.entry(field.to_string()).or_insert(Key {
                        ts_type: input_type(entity.java_type_of(field).unwrap_or_default()),
                        required: false,
                    });
                }
            }
        }
        let out_params = expand(def, &def.out_params, model, &mut notes);
        let out_keys = keys(&out_params, server_type, false);
        let validation = if def.validate_input {
            ""
        } else {
            " validate=\"false\": other keys pass through."
        };
        let _ = write!(
            inputs,
            "    /** {} in-parameters.{validation} */\n    {}: ",
            kind_name(def.kind),
            quote(&def.name)
        );
        write_map(&mut inputs, &in_keys, !def.validate_input);
        inputs.push_str(";\n");
        let _ = write!(
            outputs,
            "    /** {} out-parameters. */\n    {}: ",
            doc(kind_name(def.kind)),
            quote(&def.name)
        );
        // A service that declares outputs returns only those; one that declares none returns what it made.
        write_map(&mut outputs, &out_keys, out_keys.is_empty());
        outputs.push_str(";\n");
    }
    let mut out = String::from(super::HEADER);
    out.push_str("// Service definitions. Implicit entity-auto names (`create#`, `store#`, `update#`, `delete#` and an entity name)\n");
    out.push_str("// are typed from the entity map: see MoquiEntityAutoIn in moqui-host.d.ts.\n");
    for note in &notes {
        let _ = writeln!(out, "// not expanded: {}", note.replace('\n', " "));
    }
    out.push_str("\n/** Input maps by service name, as the validation reads them: a required key has no default. */\n");
    out.push_str("interface MoquiServiceIn {\n");
    out.push_str(&inputs);
    out.push_str("}\n\n/** Output maps by service name. Nothing checks an output, so every key is optional. */\n");
    out.push_str("interface MoquiServiceOut {\n");
    out.push_str(&outputs);
    out.push_str("}\n");
    (out, notes)
}
