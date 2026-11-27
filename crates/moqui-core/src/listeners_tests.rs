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

/// A control that answers one listener and keeps the requests it was given.
#[derive(Default)]
struct Stub {
    requests: Mutex<Vec<(ReconcileRequest, Duration)>>,
}

impl ListenerControl for Stub {
    fn listeners(&self) -> Vec<ListenerInfo> {
        vec![info()]
    }

    fn reconcile(&self, request: ReconcileRequest, wait: Duration) -> Result<Vec<Outcome>, String> {
        let name = request.end_now.first().cloned().unwrap_or_default();
        moqui_lib::sync::lock(&self.requests).push((request, wait));
        Ok(vec![Outcome {
            name,
            action: Action::Stopped,
            detail: String::new(),
        }])
    }

    fn reconcile_later(&self, request: ReconcileRequest, delay: Duration) {
        moqui_lib::sync::lock(&self.requests).push((request, delay));
    }
}

fn info() -> ListenerInfo {
    ListenerInfo {
        name: "SSH".to_string(),
        state: State::On("tcp 127.0.0.1:2222".to_string()),
        since: Some(jiff::Timestamp::UNIX_EPOCH),
        cause: Some("set ssh-server@port".to_string()),
        address: Some("127.0.0.1:2222".to_string()),
        live: true,
        pending: None,
        last_error: None,
        connections: Some(2),
        dependents: vec!["edge SSH tunnel".to_string()],
        selectors: vec!["ssh-server@port".to_string(), "ssh-server@host".to_string()],
    }
}

/// Each factory has its own control, so two servers in one process (the tests') do not share one.
#[test]
fn the_control_is_per_factory_and_none_until_installed() {
    let first = crate::context::ExecutionContextFactory::test_context();
    let second = crate::context::ExecutionContextFactory::test_context();
    assert!(control(first.factory()).is_none());
    let stub = Arc::new(Stub::default());
    install(first.factory(), Arc::clone(&stub) as Arc<dyn ListenerControl>);
    assert!(control(second.factory()).is_none());
    let found = control(first.factory()).expect("installed on the first factory");
    assert_eq!(found.listeners(), vec![info()]);
    let request = ReconcileRequest {
        cause: Cause::Request {
            user: "admin".to_string(),
            reason: "incident".to_string(),
        },
        end_now: vec!["SSH".to_string()],
    };
    let outcomes = found.reconcile(request.clone(), Duration::from_secs(1)).unwrap();
    assert_eq!(outcomes[0].name, "SSH");
    found.reconcile_later(ReconcileRequest::new(Cause::ConfReload), Duration::from_secs(1));
    let requests = moqui_lib::sync::lock(&stub.requests);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, request);
    assert_eq!(requests[1].0.cause, Cause::ConfReload);
    assert!(requests[1].0.end_now.is_empty());
}

#[test]
fn a_cause_says_what_ran_the_reconcile() {
    let texts: Vec<String> = [
        Cause::Boot,
        Cause::Shutdown,
        Cause::ConfReload,
        Cause::SetValue("ssh-server@port".to_string()),
        Cause::Signal("ACME"),
        Cause::Request {
            user: "admin".to_string(),
            reason: "incident 12".to_string(),
        },
        Cause::Request {
            user: "admin".to_string(),
            reason: " ".to_string(),
        },
    ]
    .iter()
    .map(Cause::to_string)
    .collect();
    assert_eq!(
        texts,
        [
            "boot",
            "shutdown",
            "conf reload",
            "set ssh-server@port",
            "signal ACME",
            "request by admin: incident 12",
            "request by admin",
        ]
    );
}

#[test]
fn an_action_is_one_word() {
    let words: Vec<String> = [
        Action::Same,
        Action::Started,
        Action::Stopped,
        Action::Rebound,
        Action::Applied,
        Action::Held,
        Action::Failed,
    ]
    .iter()
    .map(Action::to_string)
    .collect();
    assert_eq!(
        words,
        ["same", "started", "stopped", "rebound", "applied", "held", "failed"]
    );
}

#[test]
fn listener_info_answers_as_json_for_the_services() {
    assert_eq!(
        info().to_json(),
        serde_json::json!({
            "name": "SSH",
            "state": "on",
            "detail": "tcp 127.0.0.1:2222",
            "since": "1970-01-01T00:00:00Z",
            "cause": "set ssh-server@port",
            "address": "127.0.0.1:2222",
            "live": true,
            "pending": null,
            "lastError": null,
            "connections": 2,
            "dependents": ["edge SSH tunnel"],
            "selectors": ["ssh-server@port", "ssh-server@host"],
        })
    );
    let not_built = ListenerInfo {
        state: State::NotBuilt,
        since: None,
        ..info()
    };
    let json = not_built.to_json();
    assert!(json["state"].is_null(), "{json}");
    assert!(json["since"].is_null(), "{json}");
    assert_eq!(json["detail"], "");
}

/// What one environment name did, as the boot's merge reports it.
fn set_by(selector: &str, source: crate::context::conf_env::Source) -> Override {
    Override {
        name: crate::context::conf_env::env_name_of(selector),
        source,
        selector: Some(selector.to_string()),
        secret: false,
        outcome: crate::context::conf_env::Outcome::Set,
    }
}

fn merged(text: &str) -> MNode {
    MNode::parse_text("conf", &format!("<moqui-conf>{text}</moqui-conf>")).unwrap()
}

/// The default conf's own file: never read for the check, so it need not exist.
fn default_file() -> ConfFile {
    ConfFile {
        path: "framework/conf/MoquiDefaultConf.xml".into(),
        role: ConfRole::Default,
    }
}

/// The upgrade warning (plans/Runtime-Config.md §8.2): a port the deployment set by `.env` or a property, or in a conf
/// file of its own, with the switch still off warns naming the setting, where, and the switch to set; one only the
/// default conf writes, or beside a switch the deployment set false, does not.
#[test]
fn a_port_set_to_turn_a_listener_on_with_its_switch_off_warns_once_by_its_setting() {
    use crate::context::conf_env::Source;
    let conf = merged(
        r#"<smtp-server enabled="false" listen-port="25"/><ssh-server enabled="false" port="2222"/>
        <xmpp-server enabled="false" domain="chat.example" port="5222"/><sip-server enabled="false" port="5060"/>"#,
    );
    let base = std::path::Path::new("/srv");
    let by_env = upgrade_warnings(
        &conf,
        &[set_by("smtp-server@listen-port", Source::DotEnv)],
        &[default_file()],
        base,
    );
    assert_eq!(by_env.len(), 1, "{by_env:?}");
    assert_eq!(by_env[0].name, "SMTP");
    assert_eq!(
        by_env[0].message,
        "smtp-server@listen-port 25 is set (runtime/.env) but smtp-server@enabled is false: SMTP is off; set \
         moqui_conf__smtp_server__enabled=true, or smtp-server@enabled false to keep it off"
    );
    let by_property = upgrade_warnings(&conf, &[set_by("xmpp-server@domain", Source::Property)], &[], base);
    assert_eq!(by_property.len(), 1);
    assert!(
        by_property[0]
            .message
            .starts_with("xmpp-server@domain chat.example is set (-D)"),
        "{by_property:?}"
    );
    // the default conf alone writes them all: nothing to tell
    assert!(upgrade_warnings(&conf, &[], &[default_file()], base).is_empty());
    // the deployment set the switch, false, beside the port
    let both = [
        set_by("smtp-server@listen-port", Source::DotEnv),
        set_by("smtp-server@enabled", Source::Environment),
    ];
    assert!(upgrade_warnings(&conf, &both, &[], base).is_empty());
    // a switch that is on, by the conf or a name, never warns
    let on = merged(r#"<smtp-server enabled="true" listen-port="25"/>"#);
    assert!(upgrade_warnings(&on, &[set_by("smtp-server@listen-port", Source::DotEnv)], &[], base).is_empty());

    // in a conf file of the deployment's: the last file merged that writes the port is the one in effect, and an empty
    // value there is no port though an earlier file set one; a file that writes the switch, false here, excuses a port a
    // file writes, but not one `.env` sets (an edge node that turned mail on by `.env` before the shipped conf wrote
    // the switch)
    let dir = std::env::temp_dir().join(format!("moqui-upgrade-warning-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("runtime/conf")).unwrap();
    let file = |name: &str, text: &str, role: ConfRole| {
        let path = dir.join(name);
        std::fs::write(&path, format!("<moqui-conf>{text}</moqui-conf>")).unwrap();
        ConfFile { path, role }
    };
    let files = [
        default_file(),
        file(
            "component.xml",
            r#"<ssh-server port="2200"/><sip-server port="5060"/>"#,
            ConfRole::Component("comp".to_string()),
        ),
        file(
            "runtime/conf/MoquiProductionConf.xml",
            r#"<ssh-server port="2222"/><sip-server port=""/><smtp-server enabled="false" listen-port="25"/>"#,
            ConfRole::Runtime,
        ),
    ];
    let in_files = upgrade_warnings(&conf, &[], &files, &dir);
    assert_eq!(in_files.len(), 1, "{in_files:?}");
    assert_eq!(in_files[0].name, "SSH");
    assert!(
        in_files[0].message.starts_with(
            "ssh-server@port 2222 is set (runtime/conf/MoquiProductionConf.xml) but ssh-server@enabled is false"
        ),
        "{in_files:?}"
    );
    let by_env_beside_file_switch = upgrade_warnings(
        &conf,
        &[set_by("smtp-server@listen-port", Source::DotEnv)],
        &files,
        &dir,
    );
    let smtp: Vec<&UpgradeWarning> = by_env_beside_file_switch
        .iter()
        .filter(|warning| warning.name == "SMTP")
        .collect();
    assert_eq!(smtp.len(), 1, "{by_env_beside_file_switch:?}");
    assert!(
        smtp[0]
            .message
            .starts_with("smtp-server@listen-port 25 is set (runtime/.env)"),
        "{smtp:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The deployment's acting on a switch live ends that listener's boot warning, in any spelling of the switch; another
/// setting, or another listener's switch, leaves it.
#[test]
fn a_switch_set_live_ends_its_listeners_upgrade_warning() {
    let ctx = ExecutionContextFactory::test_context();
    let factory = ctx.factory();
    let warning = |name: &'static str| UpgradeWarning {
        name,
        message: format!("{name} warning"),
    };
    keep_upgrade_warnings(factory, vec![warning("SMTP"), warning("HTTPS")]);
    switch_set_live(factory, "smtp-server@listen-port");
    switch_set_live(factory, "ssh-server@enabled");
    assert!(upgrade_warning(factory, "SMTP").is_some());
    switch_set_live(factory, "smtp-server@enabled");
    assert_eq!(upgrade_warning(factory, "SMTP"), None);
    switch_set_live(factory, "webapp-list/webapp[WEBROOT]@https_listen_enabled");
    assert_eq!(upgrade_warning(factory, "HTTPS"), None);
}

/// A listener's Off row says its upgrade warning: in place of the reason that says only its switch is false, after any
/// other reason; a factory with none keeps the reason.
#[test]
fn an_off_row_says_the_upgrade_warning() {
    let ctx = ExecutionContextFactory::test_context();
    let factory = ctx.factory();
    let reason = "ssh-server@enabled is false".to_string();
    assert_eq!(off_reason(factory, "SSH", reason.clone()), reason);
    let warning = "ssh-server@port 2200 is set (runtime/.env) but ssh-server@enabled is false".to_string();
    keep_upgrade_warnings(
        factory,
        vec![UpgradeWarning {
            name: "SSH",
            message: warning.clone(),
        }],
    );
    assert_eq!(off_reason(factory, "SSH", reason), warning);
    assert_eq!(
        off_reason(factory, "SSH", "not started yet".to_string()),
        format!("not started yet; {warning}")
    );
    assert_eq!(
        off_reason(factory, "SMTP", "smtp-server@enabled is false".to_string()),
        "smtp-server@enabled is false"
    );
    crate::startup::listener(factory, "SSH", State::off("ssh-server@enabled is false"));
    let row = crate::startup::report(factory)
        .entries()
        .into_iter()
        .find(|row| row.name == "SSH")
        .expect("the row just recorded");
    assert_eq!(row.state, State::Off(warning));
}

/// Every switch's selectors are conf selectors, each listener once.
#[test]
fn every_switch_names_conf_selectors_once() {
    let mut names = std::collections::BTreeSet::new();
    for entry in &SWITCHES {
        assert!(names.insert(entry.name), "{}", entry.name);
        for selector in std::iter::once(entry.enabled).chain(entry.ports.iter().copied()) {
            assert!(
                moqui_lib::node::path::AttrPath::parse(selector).is_ok_and(|path| path.env_name().is_some()),
                "{selector}"
            );
        }
        assert_eq!(switch(entry.name), Some(entry));
    }
}
