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

//! Push mirrors and `fetch#GitRefs` against this crate's own git server over loopback HTTP (smart HTTP, as GitHub
//! is reached), on the in-memory database with the framework's services: a mirror forced to match and its gone refs
//! deleted, a busy mirror, the back-off and the notice of a failure with the credential kept out, the outbound rules,
//! and refs fetched into a db repository with the local refs a fetch may not write refused.

use std::collections::BTreeMap;

use gix::ObjectId;
use moqui_lib::object::Value;

use super::{backoff_millis, mirror_maps, parse_mirror_url, MirrorUrl, GIT_REPOSITORY_MIRROR};
use crate::server::hosting_tests::{call, get, list, ok, s, text_of};
use crate::server::smart_http::{check_url, redact};
use crate::server::tests::{define, fixture_with, http_server, settled, Fixture, PASSWORD, WRITER};

const GIT: &str = "org.moqui.git.GitServices";

fn git(name: &str) -> String {
    format!("{GIT}.{name}")
}

#[test]
fn the_back_off_doubles_from_a_minute_to_six_hours() {
    assert_eq!(backoff_millis(1), 60_000);
    assert_eq!(backoff_millis(2), 120_000);
    assert_eq!(backoff_millis(4), 480_000);
    assert_eq!(backoff_millis(9), 256 * 60_000);
    assert_eq!(backoff_millis(10), 6 * 3_600_000);
    assert_eq!(backoff_millis(500), 6 * 3_600_000);
}

#[test]
fn mirror_urls_carry_no_credential_and_patterns_are_of_branches_and_tags() {
    assert_eq!(
        parse_mirror_url("https://github.com/moqui/mr-moqui.git", false),
        Ok(MirrorUrl::Http("https://github.com/moqui/mr-moqui.git".into()))
    );
    assert!(parse_mirror_url("https://user:token@github.com/o/r.git", false).is_err());
    assert!(parse_mirror_url("https://token@github.com/o/r.git", false).is_err());
    assert!(parse_mirror_url("http://ci.local/o/r.git", false).is_err());
    assert!(parse_mirror_url("http://ci.local/o/r.git", true).is_ok());
    assert!(parse_mirror_url("ftp://host/r.git", false).is_err());
    assert!(matches!(
        parse_mirror_url("git@github.com:moqui/mr-moqui.git", false),
        Ok(MirrorUrl::Ssh(_))
    ));
    assert!(parse_mirror_url("ssh://git:secret@github.com/o/r.git", false).is_err());
    assert!(check_url("https://github.com/o/r.git", false).is_ok());

    assert_eq!(mirror_maps("").unwrap().len(), 2);
    assert_eq!(mirror_maps("main, refs/tags/v*").unwrap().len(), 2);
    assert!(mirror_maps("refs/merge-requests/*").is_err());
    assert!(mirror_maps("refs/*").is_err());
    assert!(mirror_maps("main:refs/notes/x").is_err());

    assert_eq!(
        redact("HTTP 401 for s3cret-token at host (s3cret-token)", Some("s3cret-token")),
        "HTTP 401 for *** at host (***)"
    );
    assert_eq!(redact("nothing here", None), "nothing here");
}

fn fixture(name: &str, extra: &str) -> Fixture {
    let fixture = fixture_with(name, moqui_core::context::framework_dir(), extra);
    moqui_core::testing::seed_status_flows(&fixture.entity);
    fixture
}

fn write(fixture: &Fixture, repository: &str, text: &str) {
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(WRITER).unwrap();
    fixture
        .context
        .factory()
        .resource()
        .write_bytes(&format!("git://{repository}/a.md"), text.as_bytes())
        .unwrap();
    moqui_core::user::logout().unwrap();
}

fn head(fixture: &Fixture, repository: &str, name: &str) -> Option<ObjectId> {
    let _entered = fixture.context.enter();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    provider.repo(repository).unwrap().store.read_ref(name).unwrap()
}

/// Set a ref straight in a repository's store (a branch made or moved by hand, outside any push).
fn set_ref(fixture: &Fixture, repository: &str, name: &str, id: Option<ObjectId>) {
    let _entered = fixture.context.enter();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    let store = &provider.repo(repository).unwrap().store;
    let current = store.read_ref(name).unwrap();
    let edit = crate::store::RefEdit {
        name: name.to_string(),
        expected: current,
        new: id,
    };
    store.update_refs(&[edit], false).unwrap();
}

fn mirror(fixture: &Fixture, mirror_id: &str) -> moqui_core::entity::EntityValue {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find(GIT_REPOSITORY_MIRROR)
        .unwrap()
        .eq("mirrorId", s(mirror_id))
        .use_cache(false)
        .one()
        .unwrap()
        .unwrap()
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(text_of).unwrap_or_default()
}

fn run(fixture: &Fixture, mirror_id: &str) -> (BTreeMap<String, Value>, String) {
    call(
        fixture,
        WRITER,
        &git("run#GitRepositoryMirrors"),
        &[("mirrorId", s(mirror_id))],
    )
}

/// A mirror is forced to match: a branch moved back here is moved back there, a branch and a stray ref gone here are
/// deleted there with deleteRefs, and the row says it is current. The credential is never read back.
#[test]
fn a_mirror_is_forced_to_match_and_its_gone_refs_are_deleted() {
    if !crate::server::tests::git_available() {
        return;
    }
    let fixture = fixture("mirror-match", r#"<git-server webhook-private-network="true"/>"#);
    define(&fixture, "src", "db");
    define(&fixture, "dst", "db");
    write(&fixture, "src", "one\n");
    let first = head(&fixture, "src", "refs/heads/main").unwrap();
    write(&fixture, "src", "two\n");
    let second = head(&fixture, "src", "refs/heads/main").unwrap();
    set_ref(&fixture, "src", "refs/heads/topic", Some(first));
    settled(&fixture);
    let base = http_server(fixture.context.factory());

    let stored = ok(
        &fixture,
        WRITER,
        &git("store#GitRepositoryMirror"),
        &[
            ("repositoryId", s("src")),
            ("url", s(&format!("{base}/git/dst.git"))),
            ("username", s(WRITER)),
            ("credential", s(PASSWORD)),
            ("deleteRefs", s("Y")),
        ],
    );
    let mirror_id = get(&stored, "mirrorId").to_string();
    let found = ok(
        &fixture,
        WRITER,
        &git("find#GitRepositoryMirrors"),
        &[("repositoryId", s("src"))],
    );
    let listed = format!("{:?}", list(&found, "mirrorList"));
    assert!(
        listed.contains("hasCredential") && !listed.contains(PASSWORD),
        "{listed}"
    );
    // a new mirror is due at once
    assert!(!field(&mirror(&fixture, &mirror_id), "nextAttemptDate").is_empty());

    let (out, errors) = run(&fixture, &mirror_id);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(list(&out, "pushedIds").len(), 1, "{out:?}");
    settled(&fixture);
    assert_eq!(head(&fixture, "dst", "refs/heads/main"), Some(second));
    assert_eq!(head(&fixture, "dst", "refs/heads/topic"), Some(first));
    let row = mirror(&fixture, &mirror_id);
    assert_eq!(
        (
            field(&row, "lastError").as_str(),
            field(&row, "attemptCount").as_str(),
            field(&row, "nextAttemptDate").as_str()
        ),
        ("", "0", "")
    );
    assert!(
        field(&row, "lastResult").starts_with("pushed 2 refs"),
        "{}",
        field(&row, "lastResult")
    );

    // main moved back (not a fast-forward), topic deleted here, a stray branch there
    set_ref(&fixture, "src", "refs/heads/main", Some(first));
    set_ref(&fixture, "src", "refs/heads/topic", None);
    set_ref(&fixture, "dst", "refs/heads/stray", Some(second));
    let (_, errors) = run(&fixture, &mirror_id);
    assert!(errors.trim().is_empty(), "{errors}");
    settled(&fixture);
    assert_eq!(head(&fixture, "dst", "refs/heads/main"), Some(first));
    assert_eq!(head(&fixture, "dst", "refs/heads/topic"), None);
    assert_eq!(head(&fixture, "dst", "refs/heads/stray"), None);

    // without deleteRefs nothing is deleted there
    ok(
        &fixture,
        WRITER,
        &git("store#GitRepositoryMirror"),
        &[
            ("repositoryId", s("src")),
            ("mirrorId", s(&mirror_id)),
            ("deleteRefs", s("N")),
        ],
    );
    set_ref(&fixture, "dst", "refs/heads/stray", Some(second));
    let (_, errors) = run(&fixture, &mirror_id);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(head(&fixture, "dst", "refs/heads/stray"), Some(second));
}

/// A push of a mirror another member runs answers GitMirrorBusy; a failure counts, puts the next try off by the
/// back-off, keeps the credential out of the row and tells the user who stored the mirror once.
#[test]
fn a_busy_mirror_is_refused_and_a_failing_one_backs_off() {
    let fixture = fixture("mirror-backoff", r#"<git-server webhook-private-network="true"/>"#);
    define(&fixture, "src", "db");
    write(&fixture, "src", "one\n");
    let secret = "s3cret-mirror-token";
    let base = http_server(fixture.context.factory());
    let stored = ok(
        &fixture,
        WRITER,
        &git("store#GitRepositoryMirror"),
        &[
            ("repositoryId", s("src")),
            // a repository the server does not have: each push fails
            ("url", s(&format!("{base}/git/nowhere.git"))),
            ("username", s(WRITER)),
            ("credential", s(secret)),
        ],
    );
    let mirror_id = get(&stored, "mirrorId").to_string();

    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let entity = fixture.entity.clone();
    let name = format!("git-mirror:{mirror_id}");
    let holder = std::thread::spawn(move || {
        let lease = crate::lease::try_take(&entity, &name, crate::lease::TTL)
            .unwrap()
            .unwrap();
        held_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        drop(lease);
    });
    held_rx.recv().unwrap();
    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("push#GitRepositoryMirror"),
        &[("mirrorId", s(&mirror_id))],
    );
    assert!(errors.trim().starts_with("GitMirrorBusy: "), "{errors}");
    done_tx.send(()).unwrap();
    holder.join().unwrap();

    let before = moqui_core::user::now_millis();
    let (out, errors) = run(&fixture, &mirror_id);
    assert!(errors.trim().is_empty(), "the queue logs a failure: {errors}");
    assert_eq!(list(&out, "failedIds").len(), 1, "{out:?}");
    let row = mirror(&fixture, &mirror_id);
    let error = field(&row, "lastError");
    assert!(error.contains("nowhere.git") && !error.contains(secret), "{error}");
    assert_eq!(field(&row, "attemptCount"), "1");
    let next = |row: &moqui_core::entity::EntityValue| {
        let value = row.get("nextAttemptDate").cloned().unwrap();
        match moqui_lib::object::basic_convert(&value, "Timestamp").value {
            Value::Timestamp(at) => at.as_millisecond(),
            other => panic!("{other:?}"),
        }
    };
    let wait = next(&row) - before;
    assert!((55_000..=70_000).contains(&wait), "{wait}");
    // not due yet: the queue passes it over
    let (out, _) = call(&fixture, WRITER, &git("run#GitRepositoryMirrors"), &[]);
    assert!(list(&out, "failedIds").is_empty(), "{out:?}");
    let (_, _) = run(&fixture, &mirror_id);
    let row = mirror(&fixture, &mirror_id);
    assert_eq!(field(&row, "attemptCount"), "2");
    let wait = next(&row) - before;
    assert!((115_000..=130_000).contains(&wait), "{wait}");

    // told once, at the first failure, to who stored it, without the credential
    let notes: Vec<String> = {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", s(super::FAILURE_TOPIC))
            .use_cache(false)
            .list()
            .unwrap()
            .iter()
            .map(|row| format!("{:?}", row.get("messageJson")))
            .collect()
    };
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(!notes[0].contains(secret), "{}", notes[0]);
}

/// An edit keeps what it leaves out or sends null; `clearRefPattern` empties the patterns and `clearCredential` the
/// username and credential, as a null parameter is not told from an absent one.
#[test]
fn an_edit_clears_the_patterns_and_the_credential_only_by_their_flags() {
    let fixture = fixture("mirror-clear", "");
    define(&fixture, "src", "db");
    let store = |extra: &[(&str, Value)]| {
        let mut parameters = vec![("repositoryId", s("src"))];
        parameters.extend(extra.iter().cloned());
        ok(&fixture, WRITER, &git("store#GitRepositoryMirror"), &parameters)
    };
    let stored = store(&[
        ("url", s("https://github.com/o/r.git")),
        ("username", s("x-access-token")),
        ("credential", s("token-value")),
        ("refPattern", s("main")),
    ]);
    let id = get(&stored, "mirrorId").to_string();
    let state = || {
        let row = mirror(&fixture, &id);
        (
            field(&row, "refPattern"),
            field(&row, "username"),
            !field(&row, "credential").is_empty(),
        )
    };
    store(&[
        ("mirrorId", s(&id)),
        ("refPattern", Value::Null),
        ("credential", Value::Null),
    ]);
    assert_eq!(state(), ("main".to_string(), "x-access-token".to_string(), true));
    store(&[
        ("mirrorId", s(&id)),
        ("refPattern", Value::Null),
        ("clearRefPattern", Value::Bool(true)),
    ]);
    assert_eq!(state(), (String::new(), "x-access-token".to_string(), true));
    store(&[
        ("mirrorId", s(&id)),
        ("username", Value::Null),
        ("credential", Value::Null),
        ("clearCredential", Value::Bool(true)),
    ]);
    assert_eq!(state(), (String::new(), String::new(), false));
    let found = ok(
        &fixture,
        WRITER,
        &git("find#GitRepositoryMirrors"),
        &[("repositoryId", s("src"))],
    );
    assert!(format!("{:?}", list(&found, "mirrorList")).contains("\"hasCredential\": Bool(false)"));
}

const OUTBOUND_CHECK: &str = r#"<services>
    <service verb="check" noun="Outbound">
        <implements service="org.moqui.git.GitServices.check#GitOutbound"/>
        <actions><script><![CDATA[
            allowed = repositoryId !== 'held'
            if (!allowed) message = 'held by its owner (' + kind + ')'
            // as if another member took the mirror's lease over while this one pushed
            if (repositoryId === 'stolen') ec.entity.find('moqui.resource.GitLease').condition('leaseName', 'like', 'git-mirror:%')
                .disableAuthz().updateAll({holderId: 'another-member'})
        ]]></script></actions>
    </service>
</services>"#;

/// The outbound rules of webhooks hold for mirrors: no plain http and no internal address unless the conf allows
/// them, and the application's outbound check is asked before each push and each fetch#GitRefs. A push whose lease
/// another member took over meanwhile records nothing: the lease is checked right before the outcome is written.
#[test]
fn mirror_pushes_follow_the_outbound_rules() {
    let fixture = fixture(
        "mirror-outbound",
        r#"<git-server outbound-check-service="test.MirrorOutbound.check#Outbound"/>"#,
    );
    {
        let _entered = fixture.context.enter();
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/MirrorOutbound.xml");
        std::fs::write(&stub, OUTBOUND_CHECK).unwrap();
        let failures = moqui_core::service::lint(&[
            moqui_core::context::framework_dir().join("service/org/moqui/git/GitServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    for id in ["held", "open", "stolen"] {
        define(&fixture, id, "db");
        write(&fixture, id, "one\n");
    }
    let store = |id: &str, url: &str| {
        call(
            &fixture,
            WRITER,
            &git("store#GitRepositoryMirror"),
            &[
                ("repositoryId", s(id)),
                ("url", s(url)),
                ("credential", s("token-value")),
            ],
        )
    };
    let (_, errors) = store("open", "http://127.0.0.1:9/o/r.git");
    assert!(errors.contains("http URL is allowed only"), "{errors}");
    let (_, errors) = store("open", "ssh://git@github.com/o/r.git");
    assert!(errors.contains("hostKeyFingerprint"), "{errors}");
    let (out, errors) = store("open", "https://127.0.0.1:9/o/r.git");
    assert!(errors.trim().is_empty(), "{errors}");
    let open_id = get(&out, "mirrorId").to_string();
    let (out, _) = store("held", "https://127.0.0.1:9/o/r.git");
    let held_id = get(&out, "mirrorId").to_string();

    run(&fixture, &open_id);
    let error = field(&mirror(&fixture, &open_id), "lastError");
    assert!(error.contains("internal address"), "{error}");
    run(&fixture, &held_id);
    let error = field(&mirror(&fixture, &held_id), "lastError");
    assert!(
        error.starts_with("not pushed: held by its owner (push-mirror)"),
        "{error}"
    );

    let (out, _) = store("stolen", "https://127.0.0.1:9/o/r.git");
    let stolen_id = get(&out, "mirrorId").to_string();
    let (out, _) = run(&fixture, &stolen_id);
    assert_eq!(list(&out, "failedIds").len(), 1, "{out:?}");
    let row = mirror(&fixture, &stolen_id);
    assert_eq!(
        (field(&row, "lastError"), field(&row, "attemptCount")),
        (String::new(), "0".to_string()),
        "a holder that lost the lease writes nothing"
    );

    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("fetch#GitRefs"),
        &[
            ("repositoryId", s("held")),
            ("remoteUrl", s("https://127.0.0.1:9/o/r.git")),
            (
                "refMap",
                Value::Map(
                    [("refs/heads/main".to_string(), s("refs/heads/x"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        ],
    );
    assert!(errors.contains("not fetched: held by its owner (fetch)"), "{errors}");
}

/// fetch#GitRefs brings refs of another server into a db repository under the names given, refuses local refs
/// outside branches, tags and merge requests (the framework's own among them) and, with authorization on, a merge
/// request's head that is not the first of an imported one, names a remote ref that is not there, and keeps the
/// password out of a failure.
#[test]
fn fetch_refs_into_a_db_repository_and_refuse_other_local_refs() {
    if !crate::server::tests::git_available() {
        return;
    }
    let fixture = fixture("fetch-refs", r#"<git-server webhook-private-network="true"/>"#);
    define(&fixture, "upstream", "db");
    define(&fixture, "imp", "db");
    write(&fixture, "upstream", "one\n");
    let first = head(&fixture, "upstream", "refs/heads/main").unwrap();
    write(&fixture, "upstream", "two\n");
    let second = head(&fixture, "upstream", "refs/heads/main").unwrap();
    set_ref(&fixture, "upstream", "refs/heads/pr-1", Some(first));
    settled(&fixture);
    let base = http_server(fixture.context.factory());
    let ref_map = |pairs: &[(&str, &str)]| {
        Value::Map(
            pairs
                .iter()
                .map(|(remote, local)| ((*remote).to_string(), s(local)))
                .collect(),
        )
    };
    let fetch = |password: &str, pairs: &[(&str, &str)]| {
        call(
            &fixture,
            WRITER,
            &git("fetch#GitRefs"),
            &[
                ("repositoryId", s("imp")),
                ("remoteUrl", s(&format!("{base}/git/upstream.git"))),
                ("username", s(WRITER)),
                ("password", s(password)),
                ("refMap", ref_map(pairs)),
            ],
        )
    };
    let (out, errors) = fetch(
        PASSWORD,
        &[
            ("refs/heads/pr-1", "refs/merge-requests/1/head"),
            ("refs/heads/main", "refs/heads/imported"),
            ("refs/heads/gone", "refs/heads/gone-here"),
            ("refs/heads/x", "refs/moqui-hidden/x"),
            ("refs/heads/y", "refs/notes/y"),
        ],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    let fetched: Vec<String> = list(&out, "fetched").iter().map(text_of).collect();
    assert_eq!(fetched, ["refs/heads/imported"], "{out:?}");
    let problems = format!("{:?}", list(&out, "problems"));
    assert!(problems.contains("refs/merge-requests/1/head is refused"), "{problems}");
    assert!(problems.contains("refs/moqui-hidden/x is refused"), "{problems}");
    assert!(problems.contains("refs/notes/y is refused"), "{problems}");
    assert!(problems.contains("has no refs/heads/gone"), "{problems}");
    assert_eq!(head(&fixture, "imp", "refs/merge-requests/1/head"), None);
    assert_eq!(head(&fixture, "imp", "refs/heads/imported"), Some(second));
    {
        let _entered = fixture.context.enter();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let store = &provider.repo("imp").unwrap().store;
        assert!(store.find(&second).unwrap().is_some());
    }
    // again: nothing to bring, the refs already there
    let (out, errors) = fetch(PASSWORD, &[("refs/heads/main", "refs/heads/imported")]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(list(&out, "fetched").len(), 1);

    let wrong = "not-the-password-123";
    let (_, errors) = fetch(wrong, &[("refs/heads/main", "refs/heads/again")]);
    assert!(errors.contains("refused the credential"), "{errors}");
    assert!(!errors.contains(wrong), "{errors}");
}

/// Every basic authorization value is taken out of a message, as the credential is; an ssh mirror connects to an
/// address its name resolved to, after the one check that every address is public (no second resolution); and the
/// credential's destination is the scheme, host and port (and an ssh user), whatever the case or the default port.
#[test]
fn the_basic_value_is_redacted_and_an_ssh_mirror_connects_by_a_checked_address() {
    assert_eq!(
        redact(
            "sent Authorization: Basic dXNlcjpzM2NyZXQ= and basic Zm9vOmJhcg==.",
            None
        ),
        "sent Authorization: Basic *** and basic ***."
    );
    assert_eq!(
        redact("Basic dXNlcjp0b2tlbi12YWx1ZQ== for token-value", Some("token-value")),
        "Basic *** for ***"
    );

    use super::ssh_address;
    assert_eq!(
        ssh_address("8.8.8.8:2222", false).unwrap(),
        ("8.8.8.8".to_string(), 2222)
    );
    assert_eq!(ssh_address("8.8.4.4", false).unwrap(), ("8.8.4.4".to_string(), 22));
    assert_eq!(
        ssh_address("[2001:4860:4860::8888]:2200", false).unwrap(),
        ("2001:4860:4860::8888".to_string(), 2200)
    );
    assert!(ssh_address("127.0.0.1:22", false).is_err());
    assert!(ssh_address("[::1]:22", false).is_err());
    assert!(ssh_address("8.8.8.8:x", false).is_err());
    assert_eq!(
        ssh_address("git.internal:2200", true).unwrap(),
        ("git.internal".to_string(), 2200)
    );
}

/// A change of where a mirror's credential goes (any change of the URL, another user, another host key) needs the
/// credential again in the same call, or clearCredential: one who may edit the mirror does not send the stored
/// credential to a host of their own, nor push with it to another repository on the same host.
#[test]
fn a_new_destination_needs_the_credential_again() {
    let fixture = fixture("mirror-destination", "");
    define(&fixture, "src", "db");
    let store = |extra: &[(&str, Value)]| {
        let mut parameters = vec![("repositoryId", s("src"))];
        parameters.extend(extra.iter().cloned());
        call(&fixture, WRITER, &git("store#GitRepositoryMirror"), &parameters)
    };
    let (out, errors) = store(&[
        ("url", s("https://github.com/o/r.git")),
        ("username", s("x-access-token")),
        ("credential", s("admin-token")),
    ]);
    assert!(errors.trim().is_empty(), "{errors}");
    let id = get(&out, "mirrorId").to_string();
    let credential = || field(&mirror(&fixture, &id), "credential");
    let url = || field(&mirror(&fixture, &id), "url");
    for change in [
        vec![("url", s("https://attacker.example/o/r.git"))],
        vec![("url", s("https://github.com:8443/o/r.git"))],
        vec![("url", s("https://github.com/attacker/r.git"))],
        vec![("url", s("https://GitHub.com/o/other.git"))],
        vec![("username", s("someone-else"))],
    ] {
        let mut parameters = vec![("mirrorId", s(&id))];
        parameters.extend(change.clone());
        let (_, errors) = store(&parameters);
        assert!(
            errors.contains("goes only where it was given for"),
            "{change:?}: {errors}"
        );
        assert_eq!(
            (url(), credential()),
            ("https://github.com/o/r.git".to_string(), "admin-token".to_string())
        );
    }
    // the same URL, or none given: kept
    let (_, errors) = store(&[("mirrorId", s(&id)), ("url", s(" https://github.com/o/r.git "))]);
    assert!(errors.trim().is_empty(), "{errors}");
    let (_, errors) = store(&[("mirrorId", s(&id)), ("refPattern", s("main"))]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(credential(), "admin-token");
    // another path on the same host with the credential again
    let (_, errors) = store(&[
        ("mirrorId", s(&id)),
        ("url", s("https://github.com/o/other.git")),
        ("credential", s("other-token")),
    ]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(
        (url(), credential()),
        ("https://github.com/o/other.git".to_string(), "other-token".to_string())
    );
    // another host with a credential of its own, or with the stored one cleared
    let (_, errors) = store(&[
        ("mirrorId", s(&id)),
        ("url", s("https://gitlab.example/o/r.git")),
        ("credential", s("gitlab-token")),
    ]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(credential(), "gitlab-token");
    let (_, errors) = store(&[
        ("mirrorId", s(&id)),
        ("url", s("https://elsewhere.example/o/r.git")),
        ("clearCredential", Value::Bool(true)),
    ]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(credential(), "");

    // an ssh mirror's host key
    let (out, errors) = store(&[
        ("url", s("git@github.com:o/r.git")),
        ("hostKeyFingerprint", s("SHA256:first")),
        ("credential", s("ssh-password")),
    ]);
    assert!(errors.trim().is_empty(), "{errors}");
    let ssh_id = get(&out, "mirrorId").to_string();
    let (_, errors) = store(&[("mirrorId", s(&ssh_id)), ("hostKeyFingerprint", s("SHA256:other"))]);
    assert!(errors.contains("goes only where it was given for"), "{errors}");
    assert_eq!(field(&mirror(&fixture, &ssh_id), "hostKeyFingerprint"), "SHA256:first");
}

/// A user with the update right but not the delete right on a repository keeps no push mirror of it and fetches no
/// refs into it (the right of its webhooks and branch rules); a mirror whose credential was given by a user who has
/// since lost the delete right is paused at its next push, its credential not used.
#[test]
fn mirrors_and_fetches_are_the_administrators() {
    const UPDATER: &str = "git.updater";
    const ADMIN: &str = "git.admin";
    let fixture = fixture("mirror-rights", r#"<git-server webhook-private-network="true"/>"#);
    define(&fixture, "src", "db");
    write(&fixture, "src", "one\n");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let entity = &fixture.entity;
        crate::server::tests::user(entity, UPDATER);
        crate::server::tests::user(entity, ADMIN);
        crate::server::tests::create(entity, "moqui.security.UserGroup", &[("userGroupId", "GIT_UPDATERS")]);
        crate::server::tests::create(
            entity,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "GIT_UPDATERS"),
                ("userGroupId", "GIT_UPDATERS"),
                ("artifactGroupId", "GIT_ALL"),
                ("authzTypeEnumId", "AUTHZT_ALWAYS"),
                ("authzActionEnumId", "AUTHZA_UPDATE"),
            ],
        );
        for (member, group) in [
            (UPDATER, "GIT_UPDATERS"),
            (UPDATER, "GIT_READERS"),
            (ADMIN, "GIT_WRITERS"),
        ] {
            let mut row = moqui_core::entity::EntityValue::make(entity, "moqui.security.UserGroupMember").unwrap();
            row.set("userId", s(member));
            row.set("userGroupId", s(group));
            row.set(
                "fromDate",
                moqui_lib::object::basic_convert(&Value::I64(1_000), "Timestamp").value,
            );
            row.create(entity).unwrap();
        }
    }
    let url = "http://127.0.0.1:9/o/r.git";
    let ref_map = || {
        Value::Map(
            [("refs/heads/main".to_string(), s("refs/heads/x"))]
                .into_iter()
                .collect(),
        )
    };
    let (_, errors) = call(
        &fixture,
        UPDATER,
        &git("store#GitRepositoryMirror"),
        &[
            ("repositoryId", s("src")),
            ("url", s(url)),
            ("credential", s("token-value")),
        ],
    );
    assert!(errors.contains("you may not change the push mirrors"), "{errors}");
    let (_, errors) = call(
        &fixture,
        UPDATER,
        &git("find#GitRepositoryMirrors"),
        &[("repositoryId", s("src"))],
    );
    assert!(errors.contains("you may not change the push mirrors"), "{errors}");
    let (_, errors) = call(
        &fixture,
        UPDATER,
        &git("fetch#GitRefs"),
        &[("repositoryId", s("src")), ("remoteUrl", s(url)), ("refMap", ref_map())],
    );
    assert!(errors.contains("fetch the refs"), "{errors}");

    let stored = ok(
        &fixture,
        ADMIN,
        &git("store#GitRepositoryMirror"),
        &[
            ("repositoryId", s("src")),
            ("url", s(url)),
            ("credential", s("token-value")),
        ],
    );
    let mirror_id = get(&stored, "mirrorId").to_string();
    assert_eq!(field(&mirror(&fixture, &mirror_id), "createdByUserId"), ADMIN);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.security.UserGroupMember")
            .unwrap()
            .eq("userId", s(ADMIN))
            .delete_all()
            .unwrap();
    }
    let (out, _) = run(&fixture, &mirror_id);
    assert_eq!(list(&out, "failedIds").len(), 1, "{out:?}");
    let row = mirror(&fixture, &mirror_id);
    let error = field(&row, "lastError");
    assert!(error.contains("no longer administers repository src"), "{error}");
    assert_eq!(
        (
            field(&row, "enabled"),
            field(&row, "lastResult"),
            field(&row, "attemptCount")
        ),
        ("N".to_string(), "paused".to_string(), "0".to_string())
    );
}
