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

//! The comms listeners' switches as the code that serves them reads them (plans/Runtime-Config.md §3.2, §8.2): XMPP serves
//! this node's domain only while `xmpp-server@enabled` is true, so a domain alone no longer turns the server on, and every
//! reader of "this node's XMPP domain" takes it from here.

use crate::context::conf_env;
use crate::context::ExecutionContextFactory;

/// The XMPP server's switch.
pub const XMPP_ENABLED: &str = "xmpp-server@enabled";
/// The domain of its users' JIDs.
pub const XMPP_DOMAIN: &str = "xmpp-server@domain";

/// This node's XMPP domain, trimmed: `xmpp-server@domain` while `xmpp-server@enabled` is true and the domain is set, read
/// through `factory` ([`conf_env::setting_on`]); `None` otherwise.
#[must_use]
pub fn xmpp_domain(factory: &ExecutionContextFactory) -> Option<String> {
    xmpp_domain_by(&|selector| conf_env::setting_on(factory, selector))
}

/// [`xmpp_domain`] by the settings `setting` gives for a selector: the merged conf before the factory is built (the
/// port check), or the process's ([`conf_env::setting`]) for code with no factory at hand.
#[must_use]
pub fn xmpp_domain_by(setting: &dyn Fn(&str) -> String) -> Option<String> {
    if !conf_env::switch_on(&setting(XMPP_ENABLED)) {
        return None;
    }
    let domain = setting(XMPP_DOMAIN).trim().to_string();
    (!domain.is_empty()).then_some(domain)
}

#[cfg(test)]
#[path = "comms_tests.rs"]
mod tests;
