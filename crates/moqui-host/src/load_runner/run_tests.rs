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

fn uses(kinds: &[&str]) -> Vec<String> {
    kinds.iter().map(|kind| (*kind).to_string()).collect()
}

#[test]
fn a_plan_uses_what_its_target_supplies() {
    assert_eq!(check_uses(&uses(&["service"]), TargetKind::Service), Ok(()));
    assert_eq!(check_uses(&uses(&["http"]), TargetKind::This), Ok(()));
    assert_eq!(check_uses(&uses(&["http"]), TargetKind::Url), Ok(()));
    let err = check_uses(&uses(&["service"]), TargetKind::This).unwrap_err();
    assert!(err.contains("service target"), "{err}");
    assert!(check_uses(&uses(&["service"]), TargetKind::Url).is_err());
    let err = check_uses(&uses(&["http"]), TargetKind::Service).unwrap_err();
    assert!(err.contains("HTTP"), "{err}");
    assert!(check_uses(&uses(&["service", "http"]), TargetKind::Service).is_err());
    assert!(check_uses(&uses(&["ssh"]), TargetKind::This).is_err());
}

#[test]
fn a_thread_is_bound_to_its_own_run_only() {
    let bound = Bound::new();
    let load = Load::new(&[]);
    let _ = bound.load.set(Arc::clone(&load));
    let in_server = RunInServer {
        policy: TargetPolicy::default(),
        uses: OnceLock::new(),
        bound,
    };
    let _ = in_server.uses.set(vec!["service".to_string(), "http".to_string()]);
    // another run's load is refused whatever thread asks
    let err = in_server
        .check_call("thread.begin", &json!({ "load": load.id() + 1000 }))
        .unwrap_err();
    assert!(err.contains("not this thread's"), "{err}");
    // this run's load on a thread that is not one of its users' is refused too
    assert!(in_server
        .check_call("thread.begin", &json!({ "load": load.id() }))
        .is_err());
    assert!(in_server.check_call("thread.end", &json!({})).is_err());
    // and such a thread neither sends nor calls
    let err = in_server.allow("http://localhost/").unwrap_err();
    assert!(err.contains("top level"), "{err}");
    assert!(in_server.check_call("session.new", &json!({})).is_ok());
    // the waits and the recorder's calls only in a user's scenario: no wait with no recorder to see a stop
    for name in [
        "load.ramp",
        "load.pause",
        "load.timed",
        "load.iterated",
        "load.error",
        "sleep",
        "step.begin",
    ] {
        let err = in_server.check_call(name, &json!({ "until": 1e15 })).unwrap_err();
        assert!(err.contains("only for a load user's scenario"), "{name}: {err}");
    }
    // a thread.begin needs the prelude's one-time token, which no thread but a user's has
    assert!(take_token(Some("guessed")).is_err());
    assert!(take_token(None).is_err());
    assert_eq!(new_token().len(), 32);
    assert_ne!(new_token(), new_token());
}

#[test]
fn an_unknown_run_is_not_running_here() {
    assert!(find("20261110-100000-1").is_none());
    assert!(stop("20261110-100000-1", true).is_none());
    assert!(wait_for_end("20261110-100000-1", Duration::from_millis(1)));
}
