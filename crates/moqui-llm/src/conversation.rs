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

//! Per-conversation operation services (plans/agent-code/01-Data-Mapping.md section 11). A conversation whose
//! `startServiceName` is empty or names `start#AgentRun` uses the native run engine; another name (an Agent Code
//! harness engine) owns its runs, and the native engine refuses to start or take one there.

use moqui_core::entity::EntityValue;
use moqui_db::sql::Condition;
use moqui_lib::object::Value;

/// The native start service, the entity default of `AgentConversation.startServiceName`.
pub const NATIVE_START: &str = "org.moqui.agent.AgentServices.start#AgentRun";

/// The condition that selects conversations of the native engine: an old empty value or the native default.
pub(crate) fn native_condition() -> Condition {
    Condition::or(vec![
        Condition::is_null("startServiceName"),
        Condition::equals("startServiceName", Value::string("")),
        Condition::equals("startServiceName", Value::string(NATIVE_START)),
    ])
}

/// Whether the conversation row uses the native run engine.
pub fn uses_native(row: &EntityValue) -> bool {
    crate::text_of(row, "startServiceName").is_none_or(|name| name.is_empty() || name == NATIVE_START)
}
