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

//! `entity.json`, `master.json`, and `service.swagger`. YAML and Swagger 2 stay unbuilt.

use super::dispatch::Call;
use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::entity::EntityFacade;
use crate::user;
use crate::web;

const SCHEMA_AUTH: &str = "Authentication required for REST schema";
const SCHEMA_FORBIDDEN: &str = "Not authorized for REST schema";
/// The permission the schema documents need: Java's `RestSchemaUtil.requireSchemaAccess` asks for
/// `REST_SCHEMA`, and the port's seed data grants it to `ADMIN`.
const SCHEMA_PERMISSION: &str = "REST_SCHEMA";

pub(crate) fn entity_json(call: &Call<'_>, master: bool) -> Result<bool, Error> {
    if !require_schema_access(call.login_error)? {
        return Ok(true);
    }
    if yaml(call.extra) {
        web::send_json_error(404, "YAML is not built");
        return Ok(true);
    }
    let facade = facade()?;
    match crate::entity::schema_document(&facade, &plain_names(call.extra), master).map_err(Error::from) {
        Ok(value) => send_document(&value, "application/schema+json"),
        Err(err) if err.kind() == ErrorKind::BadRequest => web::send_json_error(400, err.message()),
        Err(err) => return Err(err),
    }
    Ok(true)
}

/// A schema document as Java's `RestSchemaUtil` sends it, `sendTextResponse` with its type: the JSON alone, with no
/// `messages` merged in. It is shown inline; Java names an attachment (`<entity>.schema.json`).
fn send_document(document: &moqui_lib::object::Value, content_type: &str) {
    web::send_text_response_with(
        &crate::entity::query_rest::encode_json(document),
        Some(content_type),
        None,
    );
}

pub(crate) fn service_swagger(call: &Call<'_>) -> Result<bool, Error> {
    if !require_schema_access(call.login_error)? {
        return Ok(true);
    }
    if yaml(call.extra) {
        web::send_json_error(404, "YAML is not built");
        return Ok(true);
    }
    match crate::service::openapi_document(&plain_names(call.extra)) {
        Ok(value) => send_document(&value, "application/json"),
        Err(err) if matches!(err.kind(), ErrorKind::BadRequest | ErrorKind::NotFound) => {
            web::send_json_error(web::status_for(&err), err.message());
        }
        Err(err) => return Err(err),
    }
    Ok(true)
}

/// Java's `RestSchemaUtil.requireSchemaAccess`: a user, and the `REST_SCHEMA` permission.
/// `has_permission` asks even when artifact authorization is off, as Java always asks.
fn require_schema_access(login_error: Option<&str>) -> Result<bool, Error> {
    if let Some(error) = login_error {
        web::send_json_error(401, error);
        return Ok(false);
    }
    if user::current_username()?.is_none() {
        web::send_json_error(401, SCHEMA_AUTH);
        return Ok(false);
    }
    if !user::has_permission(SCHEMA_PERMISSION)? {
        web::send_json_error(403, SCHEMA_FORBIDDEN);
        return Ok(false);
    }
    Ok(true)
}

fn yaml(extra: &[String]) -> bool {
    extra.iter().any(|part| {
        let lower = part.to_ascii_lowercase();
        lower.ends_with(".yaml") || lower.ends_with(".yml")
    })
}

fn plain_names(extra: &[String]) -> Vec<String> {
    extra
        .iter()
        .map(|part| part.strip_suffix(".json").unwrap_or(part))
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn facade() -> Result<EntityFacade, Error> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))
}
