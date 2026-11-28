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

fn settings(enabled: &str, port: &str, bind: &str) -> [String; 3] {
    [enabled.to_string(), port.to_string(), bind.to_string()]
}

fn on(bind: &str) -> Wanted<SocketAddr, ()> {
    Wanted::On {
        bind: bind.parse().unwrap(),
        in_place: (),
    }
}

/// HTTP's wants: off for a node reached in process only, or by its switch, which the row names; on its written port
/// and `http-bind`; a setting that does not parse is the row's reason (plans/Runtime-Config.md §3.2).
#[test]
fn http_follows_its_switch_port_and_address() {
    let none = [false; 3];
    assert_eq!(
        http_wanted(&ListenBind::None, none, &settings("true", "8080", "")),
        Wanted::Off(IN_PROCESS.to_string())
    );
    assert_eq!(
        http_wanted(&ListenBind::Property, none, &settings("false", "8080", "")),
        Wanted::Off("webapp-list/webapp[webroot]@http-listen-enabled is false".to_string())
    );
    assert_eq!(
        http_wanted(&ListenBind::Property, none, &settings("true", "8081", "")),
        on("0.0.0.0:8081")
    );
    assert_eq!(
        http_wanted(&ListenBind::Property, none, &settings("true", "8081", "127.0.0.1")),
        on("127.0.0.1:8081")
    );
    let Wanted::Off(reason) = http_wanted(&ListenBind::Property, none, &settings("true", "8081", "nowhere")) else {
        panic!("an address that is not one is off");
    };
    assert!(reason.contains("http-bind"), "{reason}");
    let Wanted::Off(reason) = http_wanted(&ListenBind::Property, none, &settings("true", "off", "")) else {
        panic!("a port that is not one is off");
    };
    assert!(reason.contains("http-listen-port"), "{reason}");
}

/// A server started on an address listens there, on, whatever the conf said at the start; each of the switch, the port
/// and the address is the conf's once it is set, to any value, or a reload changes it ([`crate::http`]'s
/// `AddressHold`).
#[test]
fn an_address_bind_holds_each_setting_until_it_is_set_or_reloaded() {
    let address = ListenBind::Address("127.0.0.1:9".parse().unwrap());
    let hold = crate::http::AddressHold::default();
    let boot = settings("false", "8080", "");
    hold.note_boot(&boot);
    assert_eq!(hold.held(), [true; 3]);
    assert_eq!(http_wanted(&address, hold.held(), &boot), on("127.0.0.1:9"));
    // a reload that changed the port: the address's host stays, the port is the conf's
    let moved = settings("false", "9090", "");
    hold.release_changed(&moved);
    assert_eq!(hold.held(), [true, false, true]);
    assert_eq!(http_wanted(&address, hold.held(), &moved), on("127.0.0.1:9090"));
    // the switch set to the value it had at the start is the conf's all the same: off, as the conf says
    hold.release(0);
    assert_eq!(hold.held(), [false, false, true]);
    assert_eq!(
        http_wanted(&address, hold.held(), &boot),
        Wanted::Off("webapp-list/webapp[webroot]@http-listen-enabled is false".to_string())
    );
    // a later boot note changes nothing
    hold.note_boot(&settings("true", "1", "127.0.0.2"));
    hold.release_changed(&boot);
    assert_eq!(hold.held(), [false, false, true]);
}

/// A set names its settings however it spelled them, and its cause lists them as the conf hook joins them.
#[test]
fn a_set_names_the_held_settings_however_spelled() {
    assert!(same_selector(
        "webapp-list/webapp[webroot]@http-listen-enabled",
        HTTP_ENABLED
    ));
    assert!(same_selector(
        "webapp_list/webapp[WEBROOT]@http_listen_enabled",
        HTTP_ENABLED
    ));
    assert!(!same_selector(
        "webapp-list/webapp[webroot]@http-listen-port",
        HTTP_ENABLED
    ));
    assert!(!same_selector("not a selector", HTTP_ENABLED));
    let joined = format!("{HTTP_PORT}, {HTTP_BIND}");
    assert_eq!(set_selectors(&joined).collect::<Vec<_>>(), [HTTP_PORT, HTTP_BIND]);
}

/// HTTPS serves while a certificate loads, answers ACME alone while it wants the listener with none, and is off with
/// neither, or by its switch, or with no TLS settings, or for a node reached in process only.
#[test]
fn https_follows_its_switch_the_certificates_and_acme() {
    let listen = |enabled: &str| crate::tls::Listen {
        enabled: moqui_core::context::conf_env::switch_on(enabled),
        port: "8443".to_string(),
    };
    let ip = || Ok(Some("0.0.0.0".parse().unwrap()));
    let serving = |mode| Wanted::On {
        bind: "0.0.0.0:8443".parse().unwrap(),
        in_place: mode,
    };
    for acme in [false, true] {
        assert_eq!(
            https_wanted(ip(), &listen("true"), Some(true), acme),
            serving(HttpsMode::Serving),
            "a certificate serves, ACME or not"
        );
        assert_eq!(
            https_wanted(ip(), &listen("false"), Some(true), acme),
            Wanted::Off("webapp-list/webapp[webroot]@https-listen-enabled is false".to_string())
        );
        assert_eq!(
            https_wanted(ip(), &listen("false"), Some(false), acme),
            Wanted::Off("webapp-list/webapp[webroot]@https-listen-enabled is false".to_string())
        );
        assert_eq!(
            https_wanted(Ok(None), &listen("true"), Some(true), acme),
            Wanted::Off(IN_PROCESS.to_string())
        );
    }
    assert_eq!(
        https_wanted(ip(), &listen("true"), Some(false), true),
        serving(HttpsMode::AcmeOnly)
    );
    assert_eq!(
        https_wanted(ip(), &listen("true"), Some(false), false),
        Wanted::Off(crate::http::STANDBY.to_string())
    );
    let Wanted::Off(reason) = https_wanted(ip(), &listen("true"), None, true) else {
        panic!("no TLS settings is off");
    };
    assert!(reason.contains("TLS settings"), "{reason}");
}

/// Two binds collide on the same port when their addresses are the same or either is every address; a port of the
/// kernel's choosing never does.
#[test]
fn web_binds_overlap_on_one_port_by_address() {
    let at = |text: &str| -> SocketAddr { text.parse().unwrap() };
    assert!(web_overlaps(at("127.0.0.1:8080"), at("127.0.0.1:8080")));
    assert!(web_overlaps(at("0.0.0.0:8080"), at("127.0.0.1:8080")));
    assert!(web_overlaps(at("127.0.0.1:8080"), at("0.0.0.0:8080")));
    assert!(web_overlaps(at("[::]:8080"), at("127.0.0.1:8080")));
    assert!(!web_overlaps(at("127.0.0.1:8080"), at("127.0.0.2:8080")));
    assert!(!web_overlaps(at("127.0.0.1:8080"), at("127.0.0.1:8081")));
    assert!(!web_overlaps(at("127.0.0.1:0"), at("127.0.0.1:0")));
}

/// HTTPS tells ACME what it does from what the registry knows, not from its row's words, which may carry a warning:
/// bound, or waiting for ACME with no certificate, answers a challenge; off or failed does not.
#[test]
fn https_tells_acme_whether_it_answers_a_challenge() {
    use crate::acme::{Acme, HttpsState};
    use moqui_core::listeners::Cause;
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    let factory = std::sync::Arc::clone(context.factory());
    let draining = std::sync::Arc::default();
    let boot = super::super::BootInputs {
        bind: ListenBind::None,
        runtime_dir: std::path::PathBuf::from("runtime-of-the-test"),
        cluster_posts: std::sync::Arc::default(),
    };
    let cause = Cause::ConfReload;
    let ctx = Ctx {
        factory: &factory,
        conf: factory.conf_snapshot(),
        draining: &draining,
        boot: &boot,
        web: None,
        cause: &cause,
    };
    let told = |now: Now<'_, HttpsMode>| {
        Https.published(&ctx, now);
        Acme::of(&factory).https()
    };
    assert_eq!(told(Now::NotYet), HttpsState::Off);
    assert_eq!(told(Now::Off(crate::http::STANDBY)), HttpsState::StandingBy);
    assert_eq!(told(Now::Running(&HttpsMode::AcmeOnly)), HttpsState::AcmeOnly);
    assert_eq!(told(Now::Running(&HttpsMode::Serving)), HttpsState::Serving);
    assert_eq!(told(Now::Failed), HttpsState::Failed);
    assert_eq!(
        told(Now::Off("webapp-list/webapp[webroot]@https-listen-enabled is false")),
        HttpsState::Off
    );
}
