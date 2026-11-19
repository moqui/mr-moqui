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


use moqui_core::sso::{FlowConfig, FlowKind, OauthConfig, OidcConfig};
use serde_json::json;

use super::{profile, provider};

/// Admin P10 review: a flow that may not use the private network sends nothing to an internal address, written out
/// or by name, before any connection.
#[test]
fn a_flow_denied_the_private_network_sends_nothing_to_an_internal_address() {
    let transport = super::ReqwestTransport::new(true).unwrap();
    for url in [
        "https://10.1.2.3/.well-known/openid-configuration",
        "https://localhost:9/x",
    ] {
        let request = http::Request::get(url).body(bytes::Bytes::new()).unwrap();
        let transport = transport.clone();
        let err = crate::edge::block_on(async move { transport.send(request).await })
            .unwrap()
            .unwrap_err();
        assert!(err.contains("internal address"), "{url}: {err}");
    }
}

fn username(kind: FlowKind, client_type: &str, claims: serde_json::Value) -> Option<String> {
    let flow = FlowConfig {
        auth_flow_id: "F".to_string(),
        kind,
        deny_internal: false,
    };
    let serde_json::Value::Object(claims) = claims else {
        unreachable!()
    };
    profile(&flow, claims, provider(client_type).as_ref(), None).username
}

#[test]
fn an_email_is_a_username_only_when_verified_or_the_provider_names_it() {
    let oidc = || FlowKind::Oidc(OidcConfig::default());
    let claims =
        |verified: serde_json::Value| json!({"sub": "s1", "email": "a@example.com", "email_verified": verified});
    assert_eq!(
        username(oidc(), "", claims(json!(true))).as_deref(),
        Some("a@example.com")
    );
    assert_eq!(username(oidc(), "", claims(json!(false))), None);
    assert_eq!(
        username(oidc(), "", json!({"sub": "s1", "email": "a@example.com"})),
        None
    );
    assert_eq!(
        username(
            oidc(),
            "",
            json!({"sub": "s1", "preferred_username": "ann", "email": "a@example.com"})
        )
        .as_deref(),
        Some("ann")
    );
    let oauth = |client_type: &str| {
        FlowKind::Oauth(OauthConfig {
            client_type: client_type.to_string(),
            ..OauthConfig::default()
        })
    };
    // Facebook's username is its email, and it sends no email_verified.
    let facebook = json!({"id": "1", "email": "a@example.com"});
    assert_eq!(
        username(oauth("OctFacebook"), "OctFacebook", facebook).as_deref(),
        Some("a@example.com")
    );
    let unverified = json!({"id": "1", "email": "a@example.com", "email_verified": false});
    assert_eq!(username(oauth("OctGoogle"), "OctGoogle", unverified), None);
    assert_eq!(
        username(
            oauth("OctGitHub"),
            "OctGitHub",
            json!({"id": 1, "email": "a@example.com"})
        ),
        None
    );
}
