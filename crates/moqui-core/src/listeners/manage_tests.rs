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

use std::sync::{Arc, Mutex};

use moqui_lib::sync::lock;

use super::*;
use crate::listeners::{install, Action, Outcome};

/// HTTP off on every address with no proxy, HTTPS on, SSH on at 127.0.0.1: the dev conf's listeners.
const CONF: &str = r#"<moqui-conf>
    <webapp-list><webapp name="webroot" http-listen-enabled="false" http-listen-port="8080" https-listen-enabled="true"
        https-listen-port="8443" grpc-listen-enabled="false" grpc-listen-port="4317" http-bind="" trusted-proxies=""/>
    </webapp-list>
    <ssh-server enabled="true" port="2222" host="127.0.0.1" access="open"/>
</moqui-conf>"#;

/// A registry that answers the rows it is given, holds the ports `busy`, and keeps what it was asked.
#[derive(Default)]
struct Stub {
    rows: Mutex<Vec<ListenerInfo>>,
    busy: Vec<u16>,
    asked: Mutex<Vec<String>>,
}

/// A request as the stub keeps it: its cause, the settings it set and the listeners it ends now.
fn asked(request: &ReconcileRequest) -> String {
    let set = match &request.cause {
        Cause::Request { selectors, .. } => selectors.join(" "),
        _ => String::new(),
    };
    format!("{} [{set}] end now {:?}", request.cause, request.end_now)
}

impl ListenerControl for Stub {
    fn listeners(&self) -> Vec<ListenerInfo> {
        lock(&self.rows).clone()
    }

    fn reconcile(&self, request: ReconcileRequest, _wait: Duration) -> Result<Vec<Outcome>, String> {
        lock(&self.asked).push(format!("reconcile {}", asked(&request)));
        Ok(Vec::new())
    }

    fn reconcile_later(&self, request: ReconcileRequest, delay: Duration) {
        lock(&self.asked).push(format!("later {}s {}", delay.as_secs(), asked(&request)));
    }

    fn check_port(&self, name: &str, host: Option<&str>, port: u16) -> Result<(), String> {
        lock(&self.asked).push(format!("check {name} {} {port}", host.unwrap_or("none")));
        if self.busy.contains(&port) {
            Err(format!("{port} on {}: address in use", host.unwrap_or("its address")))
        } else {
            Ok(())
        }
    }
}

/// A live listener's row: on at `address`, or off.
fn row_at(name: &str, address: Option<&str>) -> ListenerInfo {
    ListenerInfo {
        name: name.to_string(),
        state: match address {
            Some(address) => State::On(format!("tcp {address}")),
            None => State::Off("its switch is false".to_string()),
        },
        since: None,
        cause: None,
        address: address.map(str::to_string),
        live: true,
        pending: None,
        last_error: None,
        connections: None,
        dependents: Vec::new(),
        selectors: Vec::new(),
    }
}

/// The rows of the dev conf's listeners running: HTTPS on every address, SSH on loopback, HTTP off.
fn dev_rows() -> Vec<ListenerInfo> {
    vec![
        row_at("HTTP", None),
        row_at("HTTPS", Some("0.0.0.0:8443")),
        row_at("SSH", Some("127.0.0.1:2222")),
    ]
}

/// A change kept as a process property only, so no test writes `runtime/.env` (a test factory's runtime is `.`).
fn change_of(name: &str, enabled: bool) -> Change {
    Change {
        persist: false,
        ..Change::new(name, enabled)
    }
}

fn off(name: &str) -> Change {
    Change {
        reason: "incident".to_string(),
        ..change_of(name, false)
    }
}

fn user(origin: Option<&str>) -> Actor {
    Actor::User {
        username: "admin".to_string(),
        user_id: None,
        origin: origin.map(str::to_string),
    }
}

fn refused(change: Change) -> String {
    validate(&change).map(|_| ()).unwrap_err().message
}

fn access(value: &str) -> Change {
    Change {
        settings: BTreeMap::from([("access".to_string(), value.to_string())]),
        ..change_of("SSH", true)
    }
}

/// The audit log's lines while `run` runs on this thread.
fn audited<T>(run: impl FnOnce() -> T) -> (T, Vec<String>) {
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<String>>>);
    struct Message(String);
    impl tracing::field::Visit for Message {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = format!("{value:?}");
            }
        }
    }
    impl tracing::Subscriber for Capture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.target() == crate::live_settings::AUDIT
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut message = Message(String::new());
            event.record(&mut message);
            lock(&self.0).push(message.0);
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }
    let capture = Capture::default();
    let result = tracing::subscriber::with_default(capture.clone(), run);
    let lines = lock(&capture.0).clone();
    (result, lines)
}

#[test]
fn a_change_names_a_listener_its_values_and_a_reason_to_turn_it_off() {
    assert!(refused(change_of("FTP", true)).contains("No listener is named \"FTP\": one of HTTP, HTTPS, gRPC, SSH"));
    assert_eq!(refused(change_of("ssh", false)), "A reason is required to turn SSH off");
    for port in ["0", "65536", "+22", "off", "22a"] {
        let message = refused(Change {
            port: Some(port.to_string()),
            ..change_of("SSH", true)
        });
        assert!(
            message.contains("is not a port: a number from 1 to 65535"),
            "{port}: {message}"
        );
    }
    let host = |name: &str, host: &str| Change {
        host: Some(host.to_string()),
        ..change_of(name, true)
    };
    assert!(refused(host("HTTP", "example.com")).contains("is not an IP address"));
    assert!(refused(host("SSH", "bad host")).contains("is not a host name or an IP address"));
    assert!(refused(host("XMPP", "0.0.0.0")).contains("XMPP has no address setting"));
    assert!(refused(access("password")).contains("\"password\" is not a SSH access: one of open, login, key"));
    assert!(refused(Change {
        settings: BTreeMap::from([("access".to_string(), "key".to_string())]),
        ..change_of("HTTPS", true)
    })
    .contains("access is not a setting of HTTPS"));

    // the switch first, then the port, the host (in lower case, as the listeners read it) and the own settings
    let plan = validate(&Change {
        name: "ssh".to_string(),
        port: Some(" 2223 ".to_string()),
        host: Some("SSH.Example.com".to_string()),
        ..access("key")
    })
    .unwrap();
    assert_eq!(
        plan.settings,
        [
            ("ssh-server@enabled", "true".to_string()),
            ("ssh-server@port", "2223".to_string()),
            ("ssh-server@host", "ssh.example.com".to_string()),
            ("ssh-server@access", "key".to_string()),
        ]
    );
    assert_eq!(
        validate(&host("HTTP", "LOCALHOST")).unwrap().value(HTTP_BIND),
        Some("localhost")
    );
    // an empty address clears it: every address, SSH's 127.0.0.1, gRPC following http-bind
    assert_eq!(validate(&host("gRPC", " ")).unwrap().value(GRPC_BIND), Some(""));
    // a reason is one line of text, at most 500 characters
    for reason in [
        "incident\n12",
        "\u{1b}[31mred",
        "\u{1b}]8;;https://example.com\u{7}link",
        "\u{9b}2J",
    ] {
        let message = refused(Change {
            reason: reason.to_string(),
            ..change_of("SSH", false)
        });
        assert!(message.contains("no control characters"), "{reason:?}: {message}");
    }
    assert!(refused(Change {
        reason: "x".repeat(501),
        ..change_of("SSH", false)
    })
    .contains("at most 500 characters"));
    assert!(validate(&Change {
        reason: "é".repeat(500),
        ..change_of("SSH", false)
    })
    .is_ok());
    // XMPP's port is its port, not its domain
    let xmpp = validate(&Change {
        port: Some("5299".to_string()),
        ..change_of("xmpp", true)
    })
    .unwrap();
    assert_eq!(xmpp.value("xmpp-server@port"), Some("5299"));
}

/// Only what changes is written: a setting written at its value would be kept in `.env`, outranking later edits of the
/// conf files. The listeners a change affects are each that reads one of its settings.
#[test]
fn only_the_settings_that_change_are_written_and_each_listener_reading_one_is_affected() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let plan = validate(&access("key")).unwrap().changed(ec.factory(), false, &[]);
    assert_eq!(plan.settings, [("ssh-server@access", "key".to_string())]);
    assert!(!plan.switches());
    assert_eq!(affected(ec.factory(), &plan, None), ["SSH"]);
    let same = validate(&Change {
        port: Some("2222".to_string()),
        ..access("open")
    })
    .unwrap()
    .changed(ec.factory(), false, &[]);
    assert!(same.settings.is_empty());
    let bind = validate(&Change {
        host: Some("127.0.0.1".to_string()),
        ..change_of("HTTPS", true)
    })
    .unwrap()
    .changed(ec.factory(), false, &[]);
    assert_eq!(bind.settings, [(HTTP_BIND, "127.0.0.1".to_string())]);
    // gRPC binds http-bind while its own address is empty (the conf has none here)
    assert_eq!(affected(ec.factory(), &bind, None), ["HTTP", "HTTPS", "gRPC"]);
    let own = ExecutionContextFactory::test_context_with_conf(
        &CONF.replace(r#"http-bind="""#, r#"http-bind="" grpc-listen-bind="10.0.0.5""#),
    );
    let bind = validate(&Change {
        host: Some("127.0.0.1".to_string()),
        ..change_of("HTTPS", true)
    })
    .unwrap()
    .changed(own.factory(), false, &[]);
    assert_eq!(affected(own.factory(), &bind, None), ["HTTP", "HTTPS"]);
    // a row's own settings count too
    let mut grpc = row_at("gRPC", None);
    grpc.selectors = vec![HTTP_BIND.to_string()];
    assert_eq!(affected(own.factory(), &bind, Some(&[grpc])), ["HTTP", "HTTPS", "gRPC"]);
}

/// A setting at the value the running conf has is still written when a server started on an address holds it there,
/// so the hold is released, and, kept, when a live process property holds it (set without keeping it), so it goes to
/// `runtime/.env` and the property is cleared.
#[test]
fn a_setting_at_its_running_value_is_written_when_held_or_kept_elsewhere() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let _put_back = PutBack(ec.factory(), SIP);
    assert!(validate(&off("HTTP"))
        .unwrap()
        .changed(ec.factory(), false, &[])
        .settings
        .is_empty());
    let held = [HTTP_SWITCH.enabled.to_string()];
    assert_eq!(
        validate(&off("HTTP"))
            .unwrap()
            .changed(ec.factory(), false, &held)
            .settings
            .len(),
        1
    );
    // SIP on as a process property, not kept: asked again kept, it is written; asked again unkept, it is not
    conf_reload::set_many(ec.factory(), &[(SIP, Some("true"))], false).unwrap();
    let on = || validate(&change_of("SIP", true)).unwrap();
    assert_eq!(
        on().changed(ec.factory(), true, &[]).settings,
        [(SIP, "true".to_string())]
    );
    assert!(on().changed(ec.factory(), false, &[]).settings.is_empty());
}

fn way(name: &'static str, on: bool, host: &str) -> Way {
    Way {
        name,
        on,
        host: host.to_string(),
    }
}

#[test]
fn a_way_in_counts_only_where_clients_reach_it() {
    assert!(reachable(&way("SSH", true, "0.0.0.0"), false));
    assert!(!reachable(&way("SSH", true, "127.0.0.1"), false));
    assert!(
        !reachable(&way("SSH", true, "127.0.0.1"), true),
        "a proxy carries no SSH"
    );
    assert!(reachable(&way("HTTPS", true, ""), false));
    assert!(!reachable(&way("HTTPS", true, "::1"), false));
    assert!(
        reachable(&way("HTTPS", true, "127.0.0.1"), true),
        "behind a trusted proxy on this machine"
    );
    assert!(!reachable(&way("HTTP", true, "localhost"), false));
    assert!(!reachable(&way("HTTP", false, "0.0.0.0"), false));
    assert!(!reachable(&way("HTTPS", true, "::ffff:127.0.0.1"), false));
    assert!(
        reachable(&way("SSH", true, "ssh.example.com"), false),
        "a name is not looked up"
    );
    // only a proxy on this machine reaches a listener on loopback
    for (proxies, local) in [
        ("127.0.0.1", true),
        ("10.0.0.0/8, 127.0.0.0/8", true),
        ("::1", true),
        ("::ffff:127.0.0.0/104", true),
        ("10.0.0.0/8", false),
        ("fd00::/8", false),
        ("", false),
        ("127.0.0.1/x", false),
        ("not-an-address", false),
    ] {
        assert_eq!(proxy_on_loopback(proxies), local, "{proxies:?}");
    }

    let before = [
        way("HTTPS", true, ""),
        way("HTTP", false, ""),
        way("SSH", true, "127.0.0.1"),
    ];
    // HTTPS off with SSH left on loopback: no way in
    let turned_off = [
        way("HTTPS", false, ""),
        way("HTTP", false, ""),
        way("SSH", true, "127.0.0.1"),
    ];
    assert!(locks_out(&before, &turned_off, &["HTTPS"], false, None));
    // HTTPS moved to loopback: no way in, unless a proxy is believed in front
    let moved = [
        way("HTTPS", true, "127.0.0.1"),
        way("HTTP", false, "127.0.0.1"),
        way("SSH", true, "127.0.0.1"),
    ];
    assert!(locks_out(&before, &moved, &["HTTP", "HTTPS"], false, None));
    assert!(!locks_out(&before, &moved, &["HTTP", "HTTPS"], true, None));
    // SSH on every address is a way left
    let ssh_open = [
        way("HTTPS", false, ""),
        way("HTTP", false, ""),
        way("SSH", true, "0.0.0.0"),
    ];
    assert!(!locks_out(&before, &ssh_open, &["HTTPS"], false, None));
    // nothing reachable before, and the change touches no way in that carried the request: nothing more is lost
    let none = [
        way("HTTPS", false, ""),
        way("HTTP", false, ""),
        way("SSH", true, "127.0.0.1"),
    ];
    assert!(!locks_out(&none, &none, &["SSH"], false, None));
    assert!(locks_out(&none, &none, &["SSH"], false, Some("SSH")));
}

#[test]
fn the_ways_back_follow_where_the_change_is_kept_and_name_the_operator_socket_first() {
    let names = ["moqui_conf__ssh_server__enabled".to_string()];
    let file = ways_back("SSH", &names, Some(Kept::DotEnv));
    assert!(
        file.starts_with("The ways back: `moqui operator listener SSH on` over the operator socket"),
        "{file}"
    );
    assert!(
        file.contains("edit moqui_conf__ssh_server__enabled in runtime/.env")
            && file.contains("outranks the conf files")
    );
    assert!(file.contains("systemctl reload moqui"));
    let property = ways_back("SSH", &names, Some(Kept::Property));
    assert!(
        property.contains("or a restart: this change is held as a process property"),
        "{property}"
    );
    assert!(
        !property.contains("systemctl reload"),
        "a reload does not undo a process property"
    );
    let environment = ways_back("SSH", &names, Some(Kept::UntilRestart));
    assert!(
        environment.contains("the environment names moqui_conf__ssh_server__enabled"),
        "{environment}"
    );
}

#[test]
fn turning_off_the_last_reachable_way_in_is_refused_unless_confirmed_and_not_for_the_operator_socket() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let rows = dev_rows();
    let now = Now {
        factory: ec.factory(),
        rows: Some(&rows),
    };
    let check = |change: &Change, actor: &Actor| {
        let plan = validate(change).unwrap().changed(ec.factory(), false, &[]);
        let affected = affected(ec.factory(), &plan, now.rows);
        guard(&now, &plan, change, &affected, actor)
    };
    // SSH is on, but on loopback: HTTPS is the last way in
    let refusal = check(&off("HTTPS"), &user(None)).unwrap_err();
    assert_eq!(
        (refusal.kind.code(), refusal.kind.confirmation()),
        ("lockout", Some("confirmLockout"))
    );
    assert!(
        refusal.message.contains("`moqui operator listener HTTPS on`"),
        "{}",
        refusal.message
    );
    assert!(
        refusal.message.contains("process property"),
        "kept as asked: {}",
        refusal.message
    );
    // moving it to loopback with no trusted proxy is one too
    let moved = Change {
        host: Some("127.0.0.1".to_string()),
        ..change_of("HTTPS", true)
    };
    assert_eq!(check(&moved, &user(None)).unwrap_err().kind, RefusalKind::Lockout);
    let confirmed = Change {
        confirm_lockout: true,
        ..off("HTTPS")
    };
    assert_eq!(check(&confirmed, &user(None)), Ok(()));
    assert_eq!(
        check(&off("HTTPS"), &Actor::Operator { uid: 7 }),
        Ok(()),
        "the socket is a way back"
    );
    // SSH on every address is a way left
    let mut open = dev_rows();
    open[2] = row_at("SSH", Some("0.0.0.0:2222"));
    let now = Now {
        factory: ec.factory(),
        rows: Some(&open),
    };
    let plan = validate(&off("HTTPS")).unwrap().changed(ec.factory(), false, &[]);
    assert_eq!(guard(&now, &plan, &off("HTTPS"), &["HTTPS"], &user(None)), Ok(()));
}

#[test]
fn a_listener_turned_off_counts_as_off_before_its_pass_runs() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let rows = vec![row_at("HTTP", Some("0.0.0.0:8080"))];
    let now = Now {
        factory: ec.factory(),
        rows: Some(&rows),
    };
    assert!(!now.on("HTTP"), "its row runs, its switch is off");
    assert!(!now.on("HTTPS"), "its switch is on, no row runs");
    assert!(Now {
        factory: ec.factory(),
        rows: None
    }
    .on("HTTPS"));
}

#[test]
fn ssh_off_while_something_depends_on_it_needs_its_own_confirmation() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let mut rows = dev_rows();
    rows[2] = row_at("SSH", Some("0.0.0.0:2222"));
    rows[2].dependents = vec!["the edge node's SSH tunnel to this node".to_string()];
    let now = Now {
        factory: ec.factory(),
        rows: Some(&rows),
    };
    let check = |change: &Change| {
        let plan = validate(change).unwrap().changed(ec.factory(), false, &[]);
        guard(&now, &plan, change, &["SSH"], &user(None))
    };
    let refusal = check(&off("SSH")).unwrap_err();
    assert_eq!(
        (refusal.kind.code(), refusal.kind.confirmation()),
        ("dependents", Some("confirmDependents"))
    );
    assert!(
        refusal.message.contains("the edge node's SSH tunnel"),
        "{}",
        refusal.message
    );
    let confirmed = Change {
        confirm_dependents: true,
        ..off("SSH")
    };
    assert_eq!(check(&confirmed), Ok(()));
}

#[test]
fn plain_http_at_a_public_address_with_no_proxy_needs_a_confirmation() {
    let binds = [
        ("", true),
        ("0.0.0.0", true),
        ("::", true),
        ("192.0.2.10", true),
        ("127.0.0.1", false),
        ("::1", false),
        ("localhost", false),
    ];
    for (bind, public) in binds {
        for proxies in ["", "10.0.0.0/8"] {
            let asks = public_http(true, true, false, bind, proxies).is_some();
            assert_eq!(asks, public && proxies.is_empty(), "bind {bind:?}, proxies {proxies:?}");
        }
    }
    assert_eq!(public_http(false, false, true, "", ""), None, "HTTP left off");
    assert_eq!(
        public_http(true, false, false, "", ""),
        None,
        "on already, where it was"
    );
    assert!(
        public_http(true, false, true, "0.0.0.0", "").is_some(),
        "moved to every address"
    );

    // the conf's HTTP is off on every address with no proxy
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let now = Now {
        factory: ec.factory(),
        rows: None,
    };
    let check = |change: &Change| {
        let plan = validate(change).unwrap().changed(ec.factory(), false, &[]);
        let affected = affected(ec.factory(), &plan, None);
        guard(&now, &plan, change, &affected, &user(None))
    };
    let on = change_of("HTTP", true);
    let refusal = check(&on).unwrap_err();
    assert_eq!(refusal.kind, RefusalKind::PublicHttp);
    assert_eq!(refusal.kind.confirmation(), Some("confirmPublicHttp"));
    assert!(refusal.message.contains("every address"), "{}", refusal.message);
    let confirmed = Change {
        confirm_public_http: true,
        ..on.clone()
    };
    assert_eq!(check(&confirmed), Ok(()));
    // on loopback it is not public, but HTTPS shares http-bind and moves there too: no way in is left
    let local = Change {
        host: Some("127.0.0.1".to_string()),
        ..on
    };
    assert_eq!(check(&local).unwrap_err().kind, RefusalKind::Lockout);
    let local = Change {
        confirm_lockout: true,
        ..local
    };
    assert_eq!(check(&local), Ok(()));
    // keyed on HTTP's switch: on in the conf, though its row failed, it is not turned on again
    let switched = ExecutionContextFactory::test_context_with_conf(
        &CONF.replace(r#"http-listen-enabled="false""#, r#"http-listen-enabled="true""#),
    );
    let failed = vec![ListenerInfo {
        state: State::Failed("8080 in use".to_string()),
        ..row_at("HTTP", None)
    }];
    let now = Now {
        factory: switched.factory(),
        rows: Some(&failed),
    };
    let on = change_of("HTTP", true);
    let plan = validate(&on).unwrap().changed(switched.factory(), false, &[]);
    assert_eq!(guard(&now, &plan, &on, &[], &user(None)), Ok(()));
}

#[test]
fn each_affected_listener_that_binds_something_new_has_its_port_checked() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let stub = Stub {
        busy: vec![2222],
        ..Stub::default()
    };
    let rows = dev_rows();
    let now = Now {
        factory: ec.factory(),
        rows: Some(&rows),
    };
    let check = |change: &Change| {
        let plan = validate(change).unwrap().changed(ec.factory(), false, &[]);
        let affected = affected(ec.factory(), &plan, now.rows);
        check_ports(&stub, &now, &plan, change, &affected)
    };
    // a host move on its unchanged port, somewhere that does not collide with where it is bound
    let moved = Change {
        host: Some("192.0.2.10".to_string()),
        ..change_of("SSH", true)
    };
    let refusal = check(&moved).unwrap_err();
    assert_eq!(refusal.kind, RefusalKind::PortBusy);
    assert!(
        refusal.message.starts_with("SSH cannot bind 2222"),
        "{}",
        refusal.message
    );
    // to every address: the registry closes the old bind first
    let every = Change {
        host: Some("0.0.0.0".to_string()),
        ..change_of("SSH", true)
    };
    assert_eq!(check(&every), Ok(()));
    // a new port; turning off checks none; HTTPS where it is bound binds nothing new
    let port = Change {
        port: Some("2223".to_string()),
        ..change_of("SSH", true)
    };
    assert_eq!(check(&port), Ok(()));
    assert_eq!(check(&off("SSH")), Ok(()));
    assert_eq!(check(&change_of("HTTPS", true)), Ok(()));
    assert_eq!(*lock(&stub.asked), ["check SSH 192.0.2.10 2222", "check SSH none 2223"]);
}

#[test]
fn a_refused_change_keeps_nothing_and_is_audited() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let stub = Arc::new(Stub {
        busy: vec![2223],
        ..Stub::default()
    });
    *lock(&stub.rows) = dev_rows();
    install(ec.factory(), Arc::clone(&stub) as Arc<dyn ListenerControl>);
    let generation = ec.factory().conf_generation();
    let (refusals, lines) = audited(|| {
        let busy = change(
            ec.factory(),
            &Change {
                port: Some("2223".to_string()),
                ..change_of("SSH", true)
            },
            &user(Some("HTTPS")),
        );
        let lockout = change(ec.factory(), &off("HTTPS"), &user(Some("HTTPS")));
        [busy.unwrap_err().kind, lockout.unwrap_err().kind]
    });
    assert_eq!(refusals, [RefusalKind::PortBusy, RefusalKind::Lockout]);
    assert_eq!(ec.factory().conf_generation(), generation, "nothing kept");
    assert_eq!(*lock(&stub.asked), ["check SSH none 2223"], "no pass asked for");
    assert_eq!(
        lines,
        [
            "Listener \"SSH\" change to on by admin refused (portBusy); reason: \"\"",
            "Listener \"HTTPS\" change to off by admin refused (lockout); reason: \"incident\"",
        ]
    );
}

/// TURN's and SIP's switches, each changed by one test only: no other test here reads them, as neither is live yet.
const TURN: &str = "media-server/turn-server@enabled";
const SIP: &str = "sip-server@enabled";

/// A switch put back as the conf files have it, and its process property cleared, however the test ends.
struct PutBack<'a>(&'a ExecutionContextFactory, &'static str);

impl Drop for PutBack<'_> {
    fn drop(&mut self) {
        let _ = conf_reload::set_many(self.0, &[(self.1, None)], false);
    }
}

/// A change kept and applied: audited with who, the listener, from, to and the reason, then one pass with who, why
/// and what was set; deferred when it affects the listener the request came over, whose connections `endNow` spares.
/// TURN, not live yet, so its switch is read by no other test here.
#[test]
fn a_change_kept_is_audited_then_asks_for_one_pass_or_defers_it() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let _put_back = PutBack(ec.factory(), TURN);
    let stub = Arc::new(Stub::default());
    install(ec.factory(), Arc::clone(&stub) as Arc<dyn ListenerControl>);
    let ((on, deferred, again), lines) = audited(|| {
        let on = change(ec.factory(), &change_of("TURN", true), &Actor::Operator { uid: 7 }).unwrap();
        let ended = Change {
            reason: "incident 7".to_string(),
            end_now: true,
            ..change_of("TURN", false)
        };
        let deferred = change(ec.factory(), &ended, &user(Some("TURN"))).unwrap();
        let again = change(ec.factory(), &ended, &user(None)).unwrap();
        (on, deferred, again)
    });
    assert_eq!(on.applied, Applied::Now(Vec::new()));
    assert_eq!(on.settings[0].outcome.kept, Kept::Property);
    assert_eq!(on.message(), "TURN set on; the listeners did not change");
    assert_eq!(deferred.applied, Applied::Later);
    assert!(deferred
        .notes
        .iter()
        .any(|note| note.contains("endNow does not end the connections of TURN")));
    assert!(again.settings.is_empty() && again.notes[0].starts_with("Nothing changed in the conf"));
    assert_eq!(
        ec.factory().conf_value(TURN).map(|found| found.value).as_deref(),
        Some("false")
    );
    assert_eq!(
        *lock(&stub.asked),
        [
            "reconcile request by operator socket (uid 7) [media-server/turn-server@enabled] end now []",
            "later 1s request by admin: incident 7 [media-server/turn-server@enabled] end now []",
            "reconcile request by admin: incident 7 [] end now [\"TURN\"]",
        ]
    );
    assert_eq!(
        lines,
        [
            "Listener TURN changed by operator socket (uid 7): off to on; reason: \"\"; end now: false; persist: false",
            "Listener TURN changed by admin: off to off; reason: \"incident 7\"; end now: true; persist: false",
            "Listener TURN unchanged (pass asked) by admin: asked off; reason: \"incident 7\"; end now: true; persist: false",
        ]
    );
    // the two that changed a setting are the recent changes, the newest first; the one that changed nothing is not
    let recent = recent(ec.factory());
    assert_eq!(recent.len(), 2, "{recent:?}");
    assert_eq!(
        (
            &recent[0]["name"],
            &recent[0]["who"],
            &recent[0]["reason"],
            &recent[0]["applied"]
        ),
        (
            &json!("TURN"),
            &json!("admin"),
            &json!("incident 7"),
            &json!("deferred")
        )
    );
    assert_eq!((&recent[0]["to"], &recent[0]["endNow"]), (&json!("off"), &json!(true)));
    assert!(recent[0]["outcome"]
        .as_str()
        .unwrap()
        .starts_with("it applies in a second"));
    assert_eq!(
        (
            &recent[1]["who"],
            &recent[1]["from"],
            &recent[1]["to"],
            &recent[1]["applied"]
        ),
        (
            &json!("operator socket (uid 7)"),
            &json!("off"),
            &json!("on"),
            &json!("now")
        )
    );
    assert_eq!(recent[1]["outcome"], "the listeners did not change");
    assert!(recent[1]["time"].as_str().unwrap().parse::<jiff::Timestamp>().is_ok());
}

#[test]
fn the_recent_changes_keep_the_last_ones() {
    let ec = ExecutionContextFactory::test_context();
    assert!(recent(ec.factory()).is_empty());
    for number in 0..RECENT_KEPT + 3 {
        remember(
            ec.factory(),
            RecentChange {
                time: jiff::Timestamp::now(),
                name: "SSH",
                who: "admin".to_string(),
                from: "on".to_string(),
                to: "off".to_string(),
                reason: format!("change {number}"),
                applied: "now",
                outcome: "stopped".to_string(),
                end_now: false,
                persist: true,
            },
        );
    }
    let kept = recent(ec.factory());
    assert_eq!(kept.len(), RECENT_KEPT);
    assert_eq!(kept[0]["reason"], format!("change {}", RECENT_KEPT + 2));
    assert_eq!(kept[RECENT_KEPT - 1]["reason"], "change 3");
}

fn outcome(selector: &str, kept: Kept, class: Class, previous: Option<&str>) -> SetOutcome {
    SetOutcome {
        selector: selector.to_string(),
        env_name: conf_env::env_name_of(selector),
        secret: false,
        previous: previous.map(str::to_string),
        before_kept: None,
        kept,
        outranked_by: None,
        class,
        problems: Vec::new(),
        queued: false,
    }
}

#[test]
fn the_answer_says_where_each_setting_is_kept_and_when_it_applies() {
    let listeners = Class::Apply(Group::Listeners);
    let answer = Answer {
        name: "SSH",
        enabled: false,
        settings: vec![
            SettingSet {
                value: "false".to_string(),
                outcome: outcome("ssh-server@enabled", Kept::DotEnv, listeners, Some("true")),
            },
            SettingSet {
                value: "2223".to_string(),
                outcome: SetOutcome {
                    outranked_by: Some(Source::Property),
                    ..outcome("ssh-server@port", Kept::UntilRestart, listeners, Some("2222"))
                },
            },
            SettingSet {
                value: "2525".to_string(),
                outcome: outcome(
                    "smtp-server@listen-port",
                    Kept::DotEnv,
                    Class::Restart("a listener"),
                    None,
                ),
            },
        ],
        applied: Applied::Now(vec![Outcome {
            name: "SSH".to_string(),
            action: Action::Stopped,
            detail: String::new(),
        }]),
        notes: Vec::new(),
    };
    let json = answer.to_json();
    assert_eq!(json["applied"], "now");
    assert_eq!(json["message"], "SSH set off; stopped");
    let settings = &json["settingList"];
    assert_eq!(settings[0]["kept"], "runtime/.env");
    assert_eq!(settings[0]["value"], "false");
    assert_eq!(settings[0]["previous"], "true");
    assert_eq!(settings[0]["applies"], "it applies now");
    assert_eq!(settings[0]["note"], "kept in runtime/.env; it applies now");
    assert_eq!(settings[1]["kept"], "this process until a restart");
    assert_eq!(settings[1]["outrankedBy"], "-D");
    let note = settings[1]["note"].as_str().unwrap();
    assert!(note.contains("until it restarts") && note.contains("a -D of"), "{note}");
    assert_eq!(settings[2]["applies"], "it applies at the next start (a listener)");
    assert_eq!(json["outcomeList"][0]["action"], "stopped");

    let later = Answer {
        applied: Applied::Later,
        ..answer.clone()
    }
    .to_json();
    assert_eq!(later["applied"], "deferred");
    assert_eq!(
        later["settingList"][0]["applies"],
        "it applies in a second, after this answer"
    );
    assert!(later["message"]
        .as_str()
        .unwrap()
        .contains("closes the connection this request came over"));
    assert_eq!(later["outcomeList"], json!([]));
    let none = Answer {
        applied: Applied::NoListeners,
        ..answer
    }
    .to_json();
    assert_eq!(none["applied"], "no-listeners");
    assert!(none["settingList"][1]["applies"]
        .as_str()
        .unwrap()
        .starts_with("no listeners run in this process"));
}

#[test]
fn the_notes_name_a_shared_address_the_ways_back_and_what_waits_for_a_restart() {
    let plan = validate(&Change {
        host: Some("127.0.0.1".to_string()),
        ..change_of("HTTPS", true)
    })
    .unwrap();
    let mut https = row_at("HTTPS", Some("0.0.0.0:8443"));
    https.state = State::On("tcp 0.0.0.0:8443 TLS; edge links are taken after a restart".to_string());
    let found = notes(&plan, None, Some("The ways back: ...".to_string()), Some(&https));
    assert!(found[0].contains("HTTP and HTTPS both bind it"), "{found:?}");
    assert_eq!(found[1], "No way in over the network is left. The ways back: ...");
    assert_eq!(found[2], "HTTPS: edge links are taken after a restart");
}

#[test]
fn a_change_that_affects_the_callers_own_listener_applies_after_its_answer() {
    assert!(defers(Some("HTTPS"), &["HTTPS"]));
    assert!(
        defers(Some("HTTPS"), &["HTTP", "HTTPS"]),
        "the shared http-bind moves both"
    );
    assert!(!defers(Some("SSH"), &["HTTPS"]), "another listener");
    assert!(!defers(None, &["HTTPS"]), "the operator socket");
}

#[test]
fn the_rows_come_from_the_conf_without_listeners_and_from_the_registry_with_them() {
    let ec = ExecutionContextFactory::test_context_with_conf(CONF);
    let found = rows(ec.factory(), None);
    assert_eq!(found.len(), SWITCHES.len());
    let ssh = found.iter().find(|row| row["name"] == "SSH").unwrap();
    assert_eq!(ssh["state"], "not-running");
    assert_eq!(ssh["running"], false);
    assert_eq!(ssh["enabled"], true);
    assert_eq!(ssh["wayIn"], true);
    assert_eq!(ssh["ownRequest"], false);
    let selectors: Vec<&str> = ssh["settingList"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|setting| setting["selector"].as_str())
        .collect();
    assert_eq!(
        selectors,
        [
            "ssh-server@enabled",
            "ssh-server@port",
            "ssh-server@host",
            "ssh-server@access"
        ]
    );
    assert_eq!(ssh["settingList"][1]["value"], "2222");
    assert_eq!(ssh["settingList"][1]["envName"], "moqui_conf__ssh_server__port");
    assert!(ssh["settingList"][1]["source"].is_string());
    // what a form needs to change it: its port, its address and its own settings with their values
    assert_eq!(
        (&ssh["portSelector"], &ssh["port"]),
        (&json!("ssh-server@port"), &json!("2222"))
    );
    assert_eq!(
        (&ssh["hostSelector"], &ssh["host"]),
        (&json!("ssh-server@host"), &json!("127.0.0.1"))
    );
    assert_eq!(
        ssh["ownSettingList"],
        json!([{"name": "access", "selector": "ssh-server@access", "value": "open", "values": ["open", "login", "key"]}])
    );
    // its services, each with its switch, on when the conf leaves it at its default
    let services = ssh["subServiceList"].as_array().unwrap();
    assert_eq!(services.len(), 4);
    assert_eq!(services[1]["selector"], "ssh-server@sftp-enabled");
    assert_eq!(services[1]["title"], "SFTP");
    assert!(services[1]["source"].is_string());
    let http = found.iter().find(|row| row["name"] == "HTTP").unwrap();
    assert_eq!(http["enabled"], false);
    assert_eq!(http["hostSelector"], "webapp-list/webapp[webroot]@http-bind");
    // where the ways in reach, as the lockout guard counts them: SSH on loopback does not, HTTPS on every address does
    assert_eq!(
        (&ssh["reachable"], &http["reachable"], &http["plainPublic"]),
        (&json!(false), &json!(false), &json!(false))
    );
    let https = found.iter().find(|row| row["name"] == "HTTPS").unwrap();
    assert_eq!(https["reachable"], true);
    assert_eq!(https.get("plainPublic"), None, "HTTP's alone");
    assert_eq!(
        (&http["ownSettingList"], &http["subServiceList"]),
        (&json!([]), &json!([]))
    );
    let turn = found.iter().find(|row| row["name"] == "TURN").unwrap();
    assert_eq!(
        (&turn["portSelector"], &turn["hostSelector"]),
        (&json!("media-server/turn-server@port"), &json!(null))
    );

    let stub = Arc::new(Stub::default());
    let mut cluster = row_at("Cluster", Some("0.0.0.0:8444"));
    cluster.live = false;
    cluster.selectors = vec!["cluster-member@https-port".to_string()];
    *lock(&stub.rows) = vec![row_at("HTTPS", Some("0.0.0.0:8443")), cluster];
    install(ec.factory(), Arc::clone(&stub) as Arc<dyn ListenerControl>);
    // TURN rides on call media: the registry has no entry of it, its startup report row says what it is doing
    crate::startup::listener(ec.factory(), "TURN", State::off("call media is not running"));
    let found = rows(ec.factory(), Some("HTTPS"));
    assert_eq!(found.len(), SWITCHES.len() + 1);
    let https = found.iter().find(|row| row["name"] == "HTTPS").unwrap();
    assert_eq!(
        (&https["state"], &https["ownRequest"], &https["running"]),
        (&json!("on"), &json!(true), &json!(true))
    );
    let ssh = found.iter().find(|row| row["name"] == "SSH").unwrap();
    assert_eq!(ssh["state"], "not-running", "not in this registry");
    assert_eq!(ssh["reachable"], false, "it does not run here");
    let turn = found.iter().find(|row| row["name"] == "TURN").unwrap();
    assert_eq!(
        (&turn["state"], &turn["detail"], &turn["live"]),
        (&json!("off"), &json!("call media is not running"), &json!(false))
    );
    assert!(turn["since"].is_string());
    let https = found.iter().find(|row| row["name"] == "HTTPS").unwrap();
    assert_eq!(https["reachable"], true, "running on every address");
    let last = found.last().unwrap();
    assert_eq!((&last["name"], &last["switchable"]), (&json!("Cluster"), &json!(false)));
    assert_eq!(last["settingList"][0]["selector"], "cluster-member@https-port");
}

#[test]
fn the_origin_is_the_requests_listener_else_the_one_its_context_was_marked_with() {
    let ec = ExecutionContextFactory::test_context();
    assert_eq!(origin(&ec), None);
    ec.set_listener_origin(Some("SSH".to_string()));
    assert_eq!(origin(&ec).as_deref(), Some("SSH"));
    let _web = crate::web::install(BTreeMap::new());
    let mut request = crate::web::WebRequest::new("GET", "/apps");
    request.listener = Some("HTTPS".to_string());
    crate::web::set_request(Arc::new(request), bytes::Bytes::new());
    assert_eq!(origin(&ec).as_deref(), Some("HTTPS"));
}
