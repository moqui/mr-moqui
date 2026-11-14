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

//! `org.moqui.admin.OperationServices.get#WebProtection` and `lift#WebBan` (plans/Edge-Node2.md §4.3): what this
//! server's web protection is limiting, denying and banning, and lifting a ban, through the host's
//! [`crate::web::protection_view`]. Each server has its own; a ban made or lifted on one is told to the others. No Java
//! counterpart.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::service::util::param_text;

/// A read needs one of these.
const VIEW_PERMISSIONS: [&str; 2] = ["ADMIN_OPERATION_VIEW", "ADMIN_OPERATION_MANAGE"];
/// Lifting a ban needs this.
const LIFT_PERMISSION: &str = "ADMIN_OPERATION_MANAGE";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_web_protection)?;
    rust_service!(map, lift_web_ban)?;
    Ok(())
}

fn require_any(permissions: &[&str]) -> Result<(), LibError> {
    if permissions
        .iter()
        .any(|permission| crate::user::has_permission(permission).unwrap_or(false))
    {
        Ok(())
    } else {
        Err(LibError::new(format!("{} is required", permissions[0])))
    }
}

fn view() -> Option<std::sync::Arc<dyn crate::web::protection_view::ProtectionView>> {
    ExecutionContext::with_current(|ec| crate::web::protection_view::view(ec.factory())).flatten()
}

/// `get#WebProtection`: `started` false on a server whose listeners did not start (there is nothing to show), else
/// `enabled`, `countMap`, `ruleList` and `banList`.
fn get_web_protection(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    require_any(&VIEW_PERMISSIONS)?;
    let Some(view) = view() else {
        return Ok(BTreeMap::from([("started".to_string(), Value::Bool(false))]));
    };
    let report = view.report();
    let part = |name: &str| moqui_lib::json::from_json(report.get(name).unwrap_or(&serde_json::Value::Null));
    Ok(BTreeMap::from([
        ("started".to_string(), Value::Bool(true)),
        (
            "enabled".to_string(),
            Value::Bool(
                report
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            ),
        ),
        ("countMap".to_string(), part("counts")),
        ("ruleList".to_string(), part("rules")),
        ("banList".to_string(), part("bans")),
    ]))
}

/// `lift#WebBan`: lift every ban of `address` on this server, and tell the other members of its cluster and the nodes
/// at the other end of its edge links. `lifted` is false when it was not banned here.
fn lift_web_ban(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    require_any(&[LIFT_PERMISSION])?;
    let address = param_text(parameters, "address").trim().to_string();
    if address.is_empty() {
        return Err(LibError::new("address is required"));
    }
    let view = view().ok_or_else(|| LibError::new("this server's web listeners have not started"))?;
    let lifted = view.lift(&address).map_err(LibError::new)?;
    if lifted {
        tracing::info!("web protection: the ban of {address} was lifted by an administrator");
    }
    Ok(BTreeMap::from([("lifted".to_string(), Value::Bool(lifted))]))
}
