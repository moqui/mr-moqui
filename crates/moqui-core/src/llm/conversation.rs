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

//! Per-conversation operation services. Old rows with no service names keep the native run engine.

use moqui_lib::object::Value;

use crate::entity::{Condition, EntityValue};

const START: &str = "org.moqui.agent.AgentServices.start#AgentRun";

/// Both an old NULL/empty value and the explicit entity default select the native run engine.
pub(crate) fn native_condition() -> Condition {
    Condition::or(vec![
        Condition::is_null("startServiceName"),
        Condition::equals("startServiceName", Value::string("")),
        Condition::equals("startServiceName", Value::string(START)),
    ])
}

pub(crate) fn uses_native(row: &EntityValue) -> bool {
    super::text_of(row, "startServiceName").is_none_or(|name| name.is_empty() || name == START)
}
