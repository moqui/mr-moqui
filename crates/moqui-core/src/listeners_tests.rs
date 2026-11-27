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
