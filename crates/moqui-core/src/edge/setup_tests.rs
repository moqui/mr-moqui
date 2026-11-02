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
use crate::edge::EdgeSettings;

fn open() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    ctx.factory().extension(|_| EdgeSettings {
        kinds: BTreeMap::from([(
            ECHO_KIND.to_string(),
            "org.moqui.impl.EdgeServices.receive#EdgeEcho".to_string(),
        )]),
        ..EdgeSettings::default()
    });
    (ctx, facade)
}

fn row(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> Option<crate::entity::EntityValue> {
    facade
        .find(entity)
        .unwrap()
        .eq(field, Value::string(id))
        .disable_authz()
        .use_cache(false)
        .one()
        .unwrap()
}

fn create(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (name, text) in fields {
        value.set(*name, Value::string(*text));
    }
    value.create(facade).unwrap();
}

#[test]
fn domains_and_ids_are_checked_as_input() {
    assert_eq!(normalize_domain(" Example.COM. ").as_deref(), Some("example.com"));
    assert_eq!(normalize_domain("example"), None, "one label is not a mail domain");
    assert_eq!(normalize_domain("a..b"), None);
    assert_eq!(normalize_domain("-a.b"), None);
    assert_eq!(normalize_domain("a b.com"), None);
    assert!(valid_id("SITE_1.a-b"));
    assert!(!valid_id(""));
    assert!(!valid_id("a/b"));
    assert!(!valid_id(&"x".repeat(64)));
}

#[test]
fn a_configure_is_normalized_and_one_that_is_not_input_is_refused() {
    let host = |name: &str, mode: &str| EdgeHostInfo {
        host_name: name.to_string(),
        paired_node_id: "someone-else".to_string(),
        mode_enum_id: mode.to_string(),
    };
    let setup = EdgeSetup {
        hosts: vec![
            host("ERP.example.com.", HOST_PASSTHROUGH),
            host("erp.example.com", HOST_TERMINATE),
            host("not a name", HOST_TERMINATE),
        ],
        set_hosts: true,
        mail_domains: vec!["Example.com".to_string(), "example.com".to_string()],
        chat_bridge_ids: vec!["100".to_string()],
        pass_sites: vec!["SITE".to_string()],
        xmpp_domain: " Chat.Example.com. ".to_string(),
    };
    let checked = check_setup(&setup).unwrap();
    assert_eq!(checked.xmpp_domain, "chat.example.com");
    assert_eq!(
        checked.hosts.len(),
        2,
        "a repeated name is kept once; a bad one is kept for its own error"
    );
    assert_eq!(checked.hosts[0].host_name, "erp.example.com");
    assert!(
        checked.hosts.iter().all(|host| host.paired_node_id.is_empty()),
        "the node is the link's, never the request's"
    );
    assert_eq!(checked.mail_domains, vec!["example.com"]);
    let bad_mode = EdgeSetup {
        hosts: vec![host("a.example.com", "Other")],
        ..EdgeSetup::default()
    };
    assert!(check_setup(&bad_mode).is_err());
    let bad_domain = EdgeSetup {
        mail_domains: vec!["no_domain".to_string()],
        ..EdgeSetup::default()
    };
    assert!(check_setup(&bad_domain).is_err());
    let bad_id = EdgeSetup {
        chat_bridge_ids: vec!["../x".to_string()],
        ..EdgeSetup::default()
    };
    assert!(check_setup(&bad_id).is_err());
    let too_many = EdgeSetup {
        pass_sites: (0..=MAX_NAMES).map(|n| format!("S{n}")).collect(),
        ..EdgeSetup::default()
    };
    assert!(check_setup(&too_many).is_err());
    for domain in ["localhost", "10.0.0.1", "chat", "not a domain"] {
        let bad_xmpp = EdgeSetup {
            xmpp_domain: domain.to_string(),
            ..EdgeSetup::default()
        };
        let refused = check_setup(&bad_xmpp).unwrap_err();
        assert!(refused.message().contains("not an XMPP domain"), "{domain}: {refused}");
    }
    assert_eq!(
        check_setup(&EdgeSetup::default()).unwrap().xmpp_domain,
        "",
        "no XMPP domain is XMPP not chosen"
    );
    assert!(
        NOT_AVAILABLE.is_empty(),
        "calls, XMPP and phones have areas of their own"
    );
}

#[test]
fn mail_goes_through_the_edge_server_for_system_and_the_hosted_accounts_only() {
    let (_ctx, facade) = open();
    create(
        &facade,
        SERVER,
        &[("emailServerId", "SYSTEM"), ("smtpHost", "smtp.example.com")],
    );
    create(&facade, SERVER, &[("emailServerId", "OWN"), ("smtpHost", "localhost")]);
    create(
        &facade,
        SERVER,
        &[
            ("emailServerId", "GMAIL"),
            ("smtpHost", "smtp.gmail.com"),
            ("storeHost", "imap.gmail.com"),
        ],
    );
    let account = |id: &str, kind: &str, server: &str, owns: &str| {
        create(
            &facade,
            ACCOUNT,
            &[
                ("emailAccountId", id),
                ("emailAddress", &format!("{}@example.com", id.to_ascii_lowercase())),
                ("accountKindEnumId", kind),
                ("emailServerId", server),
                ("ownsEmailServer", owns),
            ],
        );
    };
    account("HOSTED", "EakHosted", "", "N");
    account("OWNS", "EakShared", "OWN", "Y");
    account("SYNCED", "EakShared", "GMAIL", "Y");
    account("EXTERNAL", "EakExternal", "", "N");
    let choices = LocalChoices {
        mail: Some(MailChoice {
            system: true,
            account_ids: ["HOSTED", "OWNS", "SYNCED", "EXTERNAL", "MISSING"]
                .map(str::to_string)
                .to_vec(),
        }),
        ..LocalChoices::default()
    };
    let env = std::env::temp_dir().join(format!("moqui-setup-{}.env", std::process::id()));
    let lines = apply_local(&facade, &env, &choices, &EdgeOffer::default());
    let host = |id: &str| text(&row(&facade, SERVER, "emailServerId", id).unwrap(), "smtpHost");
    assert_eq!(host(EDGE_SERVER_ID), EDGE_SMTP_HOST);
    assert_eq!(host("SYSTEM"), EDGE_SMTP_HOST);
    assert_eq!(
        host("OWN"),
        EDGE_SMTP_HOST,
        "an account's own server with no incoming store goes through the edge node"
    );
    assert_eq!(
        host("GMAIL"),
        "smtp.gmail.com",
        "an account that syncs from a provider is left"
    );
    let hosted = row(&facade, ACCOUNT, "emailAccountId", "HOSTED").unwrap();
    assert_eq!(text(&hosted, "emailServerId"), EDGE_SERVER_ID);
    let oks: Vec<bool> = lines.iter().map(|line| line.ok).collect();
    assert_eq!(oks, vec![true, true, true, false, false, false], "{lines:?}");
    assert!(lines[0].text.contains("not smtp.example.com"), "{lines:?}");
    assert!(lines[3].text.contains("imap.gmail.com"), "{lines:?}");
    assert!(!env.exists(), "mail writes no settings");
}

#[test]
fn pass_needs_sites_and_a_callback_with_a_path() {
    let offer = EdgeOffer::default();
    let env = std::env::temp_dir().join(format!("moqui-setup-pass-{}.env", std::process::id()));
    let pass = |sites: &[&str], callback: &str| {
        apply_pass(
            &env,
            &PassChoice {
                sites: sites.iter().map(|site| (*site).to_string()).collect(),
                callback: callback.to_string(),
            },
            &offer,
        )
    };
    assert!(!pass(&[], "https://forum.example.com/account/Sso").ok);
    assert!(!pass(&["a/b"], "https://forum.example.com/account/Sso").ok);
    assert!(!pass(&["SITE"], "https://forum.example.com").ok, "no path");
    assert!(!pass(&["SITE"], "ftp://forum.example.com/x").ok);
    assert!(!pass(&["SITE"], "https://forum.example.com/x#frag").ok);
    assert!(!env.exists(), "nothing written for a refused choice");
}

#[test]
fn a_local_node_keeps_the_edge_nodes_answer_and_refuses_anything_else() {
    let (ctx, facade) = open();
    let token = "11111111-2222-3333-4444-555555555555";
    let answer = serde_json::json!({"token": token, "back": true, "ok": true, "message": "took it"}).to_string();
    receive_echo(ctx.factory(), &facade, "local", &answer).unwrap();
    let got = wait_echo(token, Instant::now()).unwrap();
    assert!(got.ok);
    assert_eq!(got.message, "took it");
    assert!(wait_echo(token, Instant::now()).is_none(), "an answer is read once");
    let not_back = serde_json::json!({"token": token, "check": "link"}).to_string();
    assert!(receive_echo(ctx.factory(), &facade, "local", &not_back).is_err());
    assert!(receive_echo(ctx.factory(), &facade, "local", "not json").is_err());
}

#[test]
fn a_round_trip_is_queued_for_the_edge_node() {
    let (ctx, facade) = open();
    let token = start_echo(
        ctx.factory(),
        &facade,
        &EchoCheck {
            check: "mailIn".to_string(),
            address: "a@example.com".to_string(),
            bridge_ids: Vec::new(),
        },
    )
    .unwrap();
    let item = row(&facade, queue::QUEUE_ITEM, "idempotencyKey", &token).unwrap();
    assert_eq!(text(&item, "kind"), ECHO_KIND);
    assert_eq!(text(&item, "directionEnumId"), queue::TO_EDGE);
    assert!(text(&item, "payload").contains("a@example.com"));
}

#[test]
fn a_host_name_is_a_public_dns_name_and_not_the_edge_nodes_own() {
    assert_eq!(check_host_name("ERP.Example.com.").as_deref(), Ok("erp.example.com"));
    for bad in [
        "localhost",
        "app.localhost",
        "mail",
        "10.0.0.1",
        "[::1]",
        "2001:db8::1",
        "a.123",
        "not a name",
    ] {
        assert!(check_host_name(bad).is_err(), "{bad}");
    }
    let reserved = vec!["edge.example.net".to_string(), "Bounce.Example.net.".to_string()];
    assert!(is_reserved("edge.example.net", &reserved));
    assert!(is_reserved("admin.edge.example.net", &reserved), "a subdomain");
    assert!(is_reserved("bounce.example.net", &reserved));
    assert!(
        !is_reserved("xedge.example.net", &reserved),
        "not a subdomain, only a longer name"
    );
    assert!(!is_reserved("erp.example.com", &reserved));
}

#[test]
fn unpairing_puts_back_the_servers_and_accounts_apply_changed() {
    let (_ctx, facade) = open();
    create(
        &facade,
        SERVER,
        &[("emailServerId", "SYSTEM"), ("smtpHost", "smtp.example.com")],
    );
    create(&facade, SERVER, &[("emailServerId", "OWN")]);
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "OWNS"),
            ("emailAddress", "owns@example.com"),
            ("accountKindEnumId", "EakShared"),
            ("emailServerId", "OWN"),
            ("ownsEmailServer", "Y"),
        ],
    );
    create(
        &facade,
        ACCOUNT,
        &[
            ("emailAccountId", "HOSTED"),
            ("emailAddress", "hosted@example.com"),
            ("accountKindEnumId", "EakHosted"),
        ],
    );
    let choices = LocalChoices {
        mail: Some(MailChoice {
            system: true,
            account_ids: vec!["OWNS".to_string(), "HOSTED".to_string()],
        }),
        ..LocalChoices::default()
    };
    let env = std::env::temp_dir().join(format!("moqui-setup-restore-{}.env", std::process::id()));
    // applied twice: the second keeps the first's prior host, not edge:
    for _ in 0..2 {
        let lines = apply_local(&facade, &env, &choices, &EdgeOffer::default());
        assert!(lines.iter().all(|line| line.ok), "{lines:?}");
    }
    let host = |id: &str| text(&row(&facade, SERVER, "emailServerId", id).unwrap(), "smtpHost");
    assert_eq!(host("SYSTEM"), EDGE_SMTP_HOST);
    let lines = restore_local(&facade);
    assert!(lines.iter().all(|line| line.ok), "{lines:?}");
    assert_eq!(host("SYSTEM"), "smtp.example.com");
    assert_eq!(host("OWN"), "", "a server with no host before has none again");
    let hosted = row(&facade, ACCOUNT, "emailAccountId", "HOSTED").unwrap();
    assert_eq!(text(&hosted, "emailServerId"), "", "the account had no server before");
    assert!(restore_local(&facade).is_empty(), "nothing left to put back");
}
