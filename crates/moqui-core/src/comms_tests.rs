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

use super::xmpp_domain_by;

/// The domain counts only while the switch is on (plans/Runtime-Config.md §3.2): off, or on with none, there is no XMPP
/// domain.
#[test]
fn the_xmpp_domain_is_the_servers_only_while_its_switch_is_on() {
    let conf = |enabled: &'static str, domain: &'static str| {
        move |selector: &str| match selector {
            "xmpp-server@enabled" => enabled.to_string(),
            "xmpp-server@domain" => domain.to_string(),
            _ => String::new(),
        }
    };
    assert_eq!(
        xmpp_domain_by(&conf("true", " chat.example.com ")).as_deref(),
        Some("chat.example.com")
    );
    assert_eq!(
        xmpp_domain_by(&conf("TRUE", "chat.example.com")).as_deref(),
        Some("chat.example.com")
    );
    assert_eq!(xmpp_domain_by(&conf("false", "chat.example.com")), None);
    assert_eq!(xmpp_domain_by(&conf("", "chat.example.com")), None);
    assert_eq!(xmpp_domain_by(&conf("true", " ")), None);
}
