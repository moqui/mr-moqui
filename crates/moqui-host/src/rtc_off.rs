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

//! Call media when this build has no `rtc` feature: nothing starts, and a set `rtc_media_port` is an error in the log.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

pub(crate) struct RtcNode;

pub(crate) fn start(_factory: &Arc<ExecutionContextFactory>) -> Option<RtcNode> {
    if !moqui_lib::env::get_prop_or_env("rtc_media_port").trim().is_empty() {
        tracing::error!("rtc_media_port is set but this build has no call media (the rtc feature)");
    }
    None
}
