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

//! Phone calls when this build has no `sip` feature: nothing starts, and `sip-server@enabled` true is an error in the log. The voice services report the feature missing (`get#VoiceCapabilities` supported false).

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

pub(crate) struct SipNode;

/// Nothing to install: without the sip feature rustls has only aws-lc-rs.
pub(crate) fn install_crypto() {}

/// No SIP engine reads the pairings in this build.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn pairings_changed() {}

pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) -> Option<SipNode> {
    if moqui_core::context::conf_env::switch_on(&moqui_core::context::conf_env::setting_on(
        factory,
        "sip-server@enabled",
    )) {
        tracing::error!("sip-server@enabled is true but this build has no phone calls (the sip feature)");
    }
    moqui_core::startup::listener(factory, "SIP", moqui_core::startup::State::NotBuilt);
    None
}
