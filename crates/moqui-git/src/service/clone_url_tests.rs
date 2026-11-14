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

use super::SshAddress;

fn address(public_host: &str, public_port: &str, edge: Option<(&str, u16)>, port: &str) -> SshAddress {
    SshAddress {
        enabled: true,
        public_host: public_host.to_string(),
        public_port: public_port.to_string(),
        edge: edge.map(|(host, port)| (host.to_string(), port)),
        http_host: "local.example.com".to_string(),
        host: String::new(),
        port: port.to_string(),
    }
}

fn pair(host: &str, port: &str) -> Option<(String, String)> {
    Some((host.to_string(), port.to_string()))
}

/// Clone URLs behind an edge node name its SSH tunnel's host and port when neither public setting is set; the public
/// settings win; with no edge node the server's own port and webapp_http_host, as before.
#[test]
fn an_ssh_clone_url_names_the_edge_nodes_tunnel_behind_one() {
    assert_eq!(
        address("", "", Some(("edge.example.com", 2222)), "2200").host_and_port(),
        pair("edge.example.com", "2222")
    );
    // the local server listens on an ephemeral port behind the tunnel
    assert_eq!(
        address("", "", Some(("edge.example.com", 22)), "0").host_and_port(),
        pair("edge.example.com", "22")
    );
    // no local SSH server behind it: no URL
    assert_eq!(
        address("", "", Some(("edge.example.com", 22)), "").host_and_port(),
        None
    );
    // with no edge node, the server's own
    assert_eq!(
        address("", "", None, "2200").host_and_port(),
        pair("local.example.com", "2200")
    );
    assert_eq!(address("", "", None, "0").host_and_port(), None);
    // the public settings (the caller then passes no edge address)
    assert_eq!(
        address("git.example.com", "22", None, "2200").host_and_port(),
        pair("git.example.com", "22")
    );
    assert_eq!(
        address("", "2022", None, "2200").host_and_port(),
        pair("local.example.com", "2022")
    );
    let off = SshAddress {
        enabled: false,
        ..address("", "", Some(("edge.example.com", 22)), "2200")
    };
    assert_eq!(off.host_and_port(), None);
}
