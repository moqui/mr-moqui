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

//! Mail authentication with fixed DNS records and no queries ([`Records::offline`]): a message this module signed
//! verifies, SPF and DMARC pass for an aligned sender, a changed body or an unlisted client fails DMARC and the domain's
//! policy decides; the head rewrite drops forged results and spam fields.

use std::sync::{Arc, OnceLock};

use super::*;

/// One key for the tests of this process (made once: RSA-2048 takes a moment).
fn test_key() -> &'static DkimKey {
    static KEY: OnceLock<DkimKey> = OnceLock::new();
    KEY.get_or_init(|| DkimKey::parse(&DkimKey::generate().unwrap()).unwrap())
}

const MESSAGE: &str = "From: Ann <ann@example.com>\r\nTo: bob@example.org\r\nSubject: Hello\r\nDate: Mon, 19 Oct 2026 \
                       12:00:00 +0000\r\nMessage-ID: <m1@example.com>\r\n\r\nHello Bob.\r\n";

/// example.com's records: the key, SPF for 192.0.2.1 only, and DMARC with `policy`.
fn records(policy: &str) -> Arc<Records> {
    let records = Records::offline();
    records
        .fix_txt("moqui._domainkey.example.com", &test_key().record())
        .unwrap();
    records.fix_txt("example.com", "v=spf1 ip4:192.0.2.1 -all").unwrap();
    records.fix_txt("mx.example.com", "v=spf1 ip4:192.0.2.1 -all").unwrap();
    records
        .fix_txt("_dmarc.example.com", &format!("v=DMARC1; p={policy}"))
        .unwrap();
    Arc::new(records)
}

fn session(ip: &str) -> Session {
    Session {
        client_ip: Some(ip.parse().unwrap()),
        helo: "mx.example.com".to_string(),
        mail_from: "ann@example.com".to_string(),
        hostname: "edge.example.net".to_string(),
    }
}

fn check(raw: &[u8], ip: &str, policy: &str) -> Verdict {
    dns::run(verify(
        raw.to_vec(),
        session(ip),
        "edge.example.net".to_string(),
        Some(records(policy)),
    ))
    .unwrap()
}

#[test]
fn a_signed_message_passes_dkim_spf_and_dmarc() {
    let signed = test_key().sign(MESSAGE.as_bytes(), "example.com", "moqui").unwrap();
    let text = String::from_utf8(signed.clone()).unwrap();
    assert!(text.starts_with("DKIM-Signature: v=1; a=rsa-sha256;"), "{text}");
    assert!(text.contains("d=example.com") && text.contains("s=moqui"), "{text}");
    assert!(text.ends_with(MESSAGE));
    let verdict = check(&signed, "192.0.2.1", "reject");
    assert_eq!(verdict.summary, "spf=pass dkim=pass dmarc=pass arc=none", "{verdict:?}");
    assert_eq!(verdict.dmarc, DmarcAction::Accept);
    assert!(
        verdict.field.starts_with("Authentication-Results: edge.example.net;"),
        "{}",
        verdict.field
    );
    assert!(verdict.field.contains("dkim=pass"), "{}", verdict.field);
    assert!(verdict.field.ends_with("\r\n"));
    // DKIM alone aligns: from an address SPF does not list, DMARC still passes
    let verdict = check(&signed, "198.51.100.7", "reject");
    assert_eq!(verdict.summary, "spf=fail dkim=pass dmarc=pass arc=none", "{verdict:?}");
    assert_eq!(verdict.dmarc, DmarcAction::Accept);
}

#[test]
fn a_changed_message_from_elsewhere_fails_dmarc_and_its_policy_decides() {
    let signed = test_key().sign(MESSAGE.as_bytes(), "example.com", "moqui").unwrap();
    let changed = String::from_utf8(signed).unwrap().replace("Hello Bob.", "Pay me.");
    let verdict = check(changed.as_bytes(), "198.51.100.7", "reject");
    // a body that no longer hashes as signed is neutral (RFC 6376 section 6.3), so nothing aligns
    assert_eq!(
        verdict.summary, "spf=fail dkim=neutral dmarc=fail arc=none",
        "{verdict:?}"
    );
    assert_eq!(verdict.dmarc, DmarcAction::Reject);
    assert_eq!(verdict.domain, "example.com");
    assert_eq!(
        check(changed.as_bytes(), "198.51.100.7", "quarantine").dmarc,
        DmarcAction::Quarantine
    );
    assert_eq!(
        check(changed.as_bytes(), "198.51.100.7", "none").dmarc,
        DmarcAction::Accept
    );
    // unsigned, but sent from the domain's listed address: SPF aligns
    let verdict = check(MESSAGE.as_bytes(), "192.0.2.1", "reject");
    assert_eq!(verdict.summary, "spf=pass dkim=none dmarc=pass arc=none", "{verdict:?}");
}

#[test]
fn the_record_and_the_signing_domain() {
    let record = test_key().record();
    assert!(record.starts_with("v=DKIM1; k=rsa; p=MII"), "{record}");
    // a PEM key reads as the base64 one
    let base64 = DkimKey::generate().unwrap();
    let lines: Vec<&str> = base64
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    );
    assert_eq!(
        DkimKey::parse(&pem).unwrap().record(),
        DkimKey::parse(&base64).unwrap().record()
    );
    assert!(DkimKey::parse("not a key").is_err());
    assert_eq!(from_domain(MESSAGE.as_bytes()).as_deref(), Some("example.com"));
    assert_eq!(
        from_domain(b"From: a@Sub.Example.COM\r\n\r\nx").as_deref(),
        Some("sub.example.com")
    );
    assert_eq!(from_domain(b"Subject: x\r\n\r\nx"), None);
    // with no mail_dkim_domains, a hosted account's domain
    assert_eq!(
        signing_domain(MESSAGE.as_bytes(), |domain| domain == "example.com").as_deref(),
        Some("example.com")
    );
    assert_eq!(signing_domain(MESSAGE.as_bytes(), |_| false), None);
}

#[test]
fn the_head_rewrite_drops_forged_results_and_the_spam_fields() {
    let raw =
        "Authentication-Results: edge.example.net; dkim=pass\r\nAuthentication-Results: other.example; spf=fail\r\n\
               X-Spam-Flag: NO\r\nSubject: Hi\r\n\r\nbody\r\n";
    let out = rewrite_head(
        raw.as_bytes(),
        "Edge.Example.NET",
        &["x-spam-flag".to_string()],
        &[
            "Authentication-Results: edge.example.net; spf=none\r\n".to_string(),
            "X-Spam-Flag: YES\r\n".to_string(),
        ],
    );
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "Authentication-Results: edge.example.net; spf=none\r\nX-Spam-Flag: YES\r\nAuthentication-Results: \
         other.example; spf=fail\r\nSubject: Hi\r\n\r\nbody\r\n"
    );
}

#[test]
fn an_internal_signature_passes_only_with_this_nodes_key() {
    let signed = test_key().sign(MESSAGE.as_bytes(), "example.com", "moqui").unwrap();
    let ours = || vec!["example.com".to_string()];
    let field = verify_with_key(&signed, "mx.example.com", test_key(), "moqui", ours()).unwrap();
    assert!(field.starts_with("Authentication-Results: mx.example.com;"), "{field}");
    assert!(
        field.contains("dkim=pass header.d=example.com header.s=moqui"),
        "{field}"
    );
    // another node's key under the same selector: nothing from here
    let other = DkimKey::parse(&DkimKey::generate().unwrap()).unwrap();
    assert_eq!(
        verify_with_key(&signed, "mx.example.com", &other, "moqui", ours()),
        None
    );
    // From, Subject, To and Date are oversigned: one added after the signature breaks it
    let added = format!("Subject: Pay me\r\n{}", String::from_utf8(signed).unwrap());
    assert_eq!(
        verify_with_key(added.as_bytes(), "mx.example.com", test_key(), "moqui", ours()),
        None
    );
}

#[test]
fn the_dkim_domains_are_by_role_and_an_all_in_one_nodes_are_both_sides() {
    use moqui_core::edge::replica::ReplicaValue;
    use moqui_core::edge::NodeRole;
    let copy = |id: &str, address: &str, kind: &str| ReplicaValue {
        paired_node_id: "node-a".to_string(),
        set_name: super::super::edge_mail::RECIPIENT_SET.to_string(),
        entity_name: super::super::sync::ACCOUNT.to_string(),
        pk: serde_json::json!({"emailAccountId": id}),
        fields: serde_json::json!({"emailAddress": address, "accountKindEnumId": kind}),
    };
    let copies = vec![
        copy("1", "ann@Desk.Example.", "EakHosted"),
        copy("2", "team@shared.example", "EakShared"),
        copy("3", "me@gmail.example", "EakExternal"),
    ];
    let own = || Ok(vec!["sales@server.example".to_string(), "x@shared.example".to_string()]);
    let replicated = || Ok(replicated_addresses(&copies));
    let domains = |role| {
        role_domains(role, own, replicated)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(domains(NodeRole::Normal), ["server.example", "shared.example"]);
    assert_eq!(
        domains(NodeRole::Edge),
        ["desk.example", "shared.example"],
        "an external account is not signed for"
    );
    assert_eq!(
        domains(NodeRole::AllInOne),
        ["desk.example", "server.example", "shared.example"]
    );
    // a normal node does not read the copies, nor an edge node its own accounts
    let fails = || Err(moqui_lib::error::Error::new("not read"));
    assert!(role_domains(NodeRole::Normal, own, fails).is_ok());
    assert!(role_domains(NodeRole::Edge, fails, replicated).is_ok());
    assert!(role_domains(NodeRole::AllInOne, own, fails).is_err());
}

#[test]
fn an_all_in_one_nodes_own_sends_are_signed_for_the_servers_domains_only() {
    use moqui_core::edge::NodeRole;
    assert_eq!(sending_role(NodeRole::AllInOne), NodeRole::Normal);
    assert_eq!(sending_role(NodeRole::Normal), NodeRole::Normal);
    assert_eq!(sending_role(NodeRole::Edge), NodeRole::Edge);
}

#[test]
fn the_from_address_is_the_first_address_of_the_from_field() {
    assert_eq!(
        from_address(b"From: Ann <Ann@Desk.Example>\r\nTo: x@y\r\n\r\nbody").as_deref(),
        Some("Ann@Desk.Example")
    );
    assert_eq!(
        from_address(b"From: ann@desk.example\r\n\r\nbody").as_deref(),
        Some("ann@desk.example")
    );
    assert_eq!(from_address(b"From: nobody\r\n\r\nbody"), None);
    assert_eq!(from_domain(b"From: Ann <ann@Desk.Example.>\r\n\r\nx").as_deref(), Some("desk.example"));
}
