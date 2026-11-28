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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_core::seal::{Keeper, SealConf};
use moqui_lib::sync::lock;

use super::*;

/// A keeper of a runtime no server of this process runs: `db/seal.json` under a scratch directory, not made.
fn keeper(label: &str) -> (Keeper, PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-operator-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("db")).unwrap();
    let keeper = Keeper::open(&dir.join("db").join("seal.json"), SealConf::with_purpose("dev")).unwrap();
    (keeper, dir)
}

#[test]
fn a_listener_request_takes_its_fields_and_is_kept_unless_it_says_not() {
    let request = Request::parse(
        r#"{"command":"listener","name":"SSH","enabled":false,"reason":"incident 12","now":true,"port":"2223",
            "host":"0.0.0.0","access":"key","confirm_dependents":true}"#,
    )
    .unwrap();
    assert_eq!(request.name.as_deref(), Some("SSH"));
    assert_eq!(request.enabled, Some(false));
    assert_eq!(request.reason.as_deref(), Some("incident 12"));
    assert_eq!(
        (
            request.port.as_deref(),
            request.host.as_deref(),
            request.ssh_access.as_deref()
        ),
        (Some("2223"), Some("0.0.0.0"), Some("key"))
    );
    assert!(request.now && request.confirm_dependents && !request.confirm_public_http);
    assert!(request.persist, "kept in runtime/.env unless it says not");
    let unkept = Request::parse(r#"{"command":"listener","name":"HTTP","enabled":true,"persist":false}"#).unwrap();
    assert!(!unkept.persist && unkept.enabled == Some(true));
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
            metadata.target() == moqui_core::live_settings::AUDIT
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

/// `listeners` and `listener` are the operator's (root and the operator group), and act on the server this runtime
/// runs once it serves; a refused `listener` is on the audit log with the peer's uid.
#[test]
fn the_listener_commands_are_the_operators_and_need_a_serving_server() {
    let (keeper, dir) = keeper("listeners");
    let (_, lines) = audited(|| {
        for line in [
            r#"{"command":"listeners"}"#,
            r#"{"command":"listener","name":"HTTP","enabled":true,"reason":"setup"}"#,
        ] {
            let own_user = run(&keeper, Access::Status, 1000, line);
            assert_eq!(own_user["ok"], false);
            assert!(
                own_user["error"].as_str().unwrap().contains("only status"),
                "{own_user}"
            );
            let operator = run(&keeper, Access::All, 0, line);
            assert_eq!(operator["ok"], false);
            assert!(
                operator["error"].as_str().unwrap().contains("unsealed, as the conf"),
                "{operator}"
            );
        }
        let unnamed = run(&keeper, Access::All, 0, r#"{"command":"listener","enabled":false}"#);
        assert_eq!(unnamed["ok"], false);
    });
    assert_eq!(
        lines,
        [
            "Listener \"HTTP\" change to on by operator socket (uid 1000) refused (statusOnly); reason: \"setup\"",
            "Listener \"HTTP\" change to on by operator socket (uid 0) refused (notServing); reason: \"setup\"",
            "Listener \"\" change to off by operator socket (uid 0) refused (notServing); reason: \"\"",
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
