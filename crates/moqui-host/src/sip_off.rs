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

//! Phone calls when this build has no `sip` feature: nothing starts, a set `sip_port` or `sip_tls_port` is an error in
//! the log, and a prompt has no leg to play to. The voice services report the feature missing
//! (`get#VoiceCapabilities` supported false).

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

pub(crate) struct SipNode;

pub(crate) fn start(_factory: &Arc<ExecutionContextFactory>) -> Option<SipNode> {
    for name in ["sip_port", "sip_tls_port"] {
        if !moqui_lib::env::get_prop_or_env(name).trim().is_empty() {
            tracing::error!("{name} is set but this build has no phone calls (the sip feature)");
        }
    }
    None
}

/// Audio played to a phone leg (`sip/leg.rs` has the API).
#[allow(dead_code, reason = "the recording notice (W5) calls it")]
#[derive(Debug, Clone)]
pub(crate) enum Prompt {
    Tone { hz: f64, millis: u32 },
    Pcm(Arc<[i16]>),
}

/// No phone legs without SIP.
#[allow(dead_code, reason = "the recording notice (W5) calls it")]
pub(crate) fn play_prompt(_leg: &str, _prompt: Prompt) -> Result<(), &'static str> {
    Err("noSip")
}
