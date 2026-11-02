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

use super::*;

#[test]
fn a_location_is_under_its_root_only_with_a_clean_rest() {
    let root = crate::edge::queue::CONTENT_IN;
    assert!(under(&format!("{root}node-1/item-2.r3"), root));
    assert!(!under(root, root));
    assert!(!under(&format!("{root}node-1/../other"), root));
    assert!(!under(&format!("{root}node-1//item"), root));
    assert!(!under("dbresource://moqui/edge/content/out/item", root));
    assert!(!under("file:/etc/passwd", root));
}

#[test]
fn an_ice_server_signs_its_turn_urls_with_its_secret() {
    use base64::Engine as _;
    use hmac::{KeyInit, Mac};
    let urls = vec![
        "turn:edge.test:3478?transport=udp".to_string(),
        " turns:edge.test:5349?transport=tcp ".to_string(),
        "stun:edge.test:3478".to_string(),
    ];
    let Some(Value::Map(server)) = ice_server(&urls, "100:leg-1", "edge-secret").unwrap() else {
        panic!("a server")
    };
    let listed = match &server["urls"] {
        Value::List(list) => list.iter().map(to_plain_string).collect::<Vec<_>>(),
        other => panic!("urls {other:?}"),
    };
    assert_eq!(
        listed,
        [
            "turn:edge.test:3478?transport=udp",
            "turns:edge.test:5349?transport=tcp"
        ]
    );
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(b"edge-secret").unwrap();
    mac.update(b"100:leg-1");
    let expected = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    assert_eq!(to_plain_string(&server["credential"]), expected);
    // no URLs, or no secret: no server
    assert!(ice_server(&["stun:x".to_string()], "u", "s").unwrap().is_none());
    assert!(ice_server(&urls, "u", "").unwrap().is_none());
    assert!(ice_server(&urls, "u", "CHANGEME").unwrap().is_none());
}
