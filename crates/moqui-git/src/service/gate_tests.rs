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

//! What gates a branch's move through the services (docs/interfaces/Git-Hosting.md, gitECA rules and Merge
//! requests): a `pre-ref-update` rule that refuses by author address stops a commit through `commit#GitFiles`, a
//! merge by `merge#GitRefs` and by `merge#MergeRequest` (a merge commit and a squash) and a push, and its message is
//! the caller's error; a merge request merges the head its checks were counted for, by id, and only the head the
//! caller expected; a fork's source branch is deleted only by who may delete there; and the webhook queue services
//! ask the update right.

use moqui_lib::object::Value;

use super::commit::tests::{developer, internal, DEVELOPER};
use crate::server::hosting_tests::{call, framework_rules, framework_rules_with, get, grant_entities, ok, s, Work};
use crate::server::tests::{define, fixture_in, git_available, head_of, http_server, settled, Fixture, READER, WRITER};

fn git(name: &str) -> String {
    format!("org.moqui.git.GitServices.{name}")
}

fn review(name: &str) -> String {
    format!("org.moqui.git.GitReviewServices.{name}")
}

fn file(path: &str, text: &str) -> Value {
    Value::Map([(path.to_string(), Value::string(text))].into_iter().collect())
}

/// Commit one file to `repository`: to `branch`, or to the new branch `new_branch` made from main. The commit's id.
fn commit(fixture: &Fixture, user: &str, repository: &str, new_branch: Option<&str>, path: &str) -> String {
    let mut parameters = vec![
        ("repositoryId", s(repository)),
        ("message", s(&format!("Add {path}"))),
        ("fileMap", file(path, "text\n")),
    ];
    if let Some(new_branch) = new_branch {
        parameters.push(("branch", s("main")));
        parameters.push(("newBranch", s(new_branch)));
    }
    let out = ok(fixture, user, &git("commit#GitFiles"), &parameters);
    get(&out, "commitId").to_string()
}

fn open_request(fixture: &Fixture, user: &str, source_repository: Option<&str>, source: &str) -> String {
    let mut parameters = vec![
        ("repositoryId", s("site")),
        ("sourceRef", s(source)),
        ("targetRef", s("main")),
        ("title", s(&format!("Merge {source}"))),
    ];
    if let Some(other) = source_repository {
        parameters.push(("sourceRepositoryId", s(other)));
    }
    let out = ok(fixture, user, &review("create#MergeRequest"), &parameters);
    get(&out, "mergeRequestId").to_string()
}

fn main_head(fixture: &Fixture) -> String {
    head_of(fixture, "site", "refs/heads/main").unwrap_or_default()
}

/// A hosting application's rule: no commit enters main by an author who has not signed. It names every source, so
/// it holds for a push and for every service that moves the branch, and says what it was told of the change.
const AGREEMENT: &str = r#"<giteca rule-name="Agreement" event="pre-ref-update" source="push,fetch,import,write"
        ref-pattern="refs/heads/main">
    <actions><script><![CDATA[
        const unsigned = ['unsigned@example.com', 'git.developer@example.com']
        const bad = Array.from(newCommits).map((c) => c.authorEmail).filter((email) => unsigned.includes(email))
        if (newCommitsTruncated || bad.length) {
            ec.message.addError('no contributor agreement from ' + bad.join(',') + ' [merge=' + isMerge + ' method=' + mergeMethodEnumId
                + ' request=' + mergeRequestId + ' user=' + userId + ' commits=' + newCommitIds.length + ']')
        }
    ]]></script></actions>
</giteca>"#;

#[test]
fn a_pre_rule_by_author_stops_every_way_main_moves() {
    let fixture = fixture_in("gate-agreement", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    framework_rules_with(&fixture, AGREEMENT);
    developer(&fixture);
    define(&fixture, "site", "db");
    let first = commit(&fixture, WRITER, "site", None, "a.txt");
    assert_eq!(main_head(&fixture), first);
    // each change's events are done before the next: the in-memory test database commits whole tables
    settled(&fixture);

    // commit#GitFiles to the branch, by an author the application gives
    let unsigned = |extra: &[(&str, Value)]| {
        let mut parameters = vec![
            ("repositoryId", s("site")),
            ("message", s("Unsigned")),
            ("fileMap", file("u.txt", "u\n")),
            ("authorName", s("Una Unsigned")),
            ("authorEmail", s("unsigned@example.com")),
        ];
        parameters.extend(extra.iter().cloned());
        internal(&fixture, WRITER, &git("commit#GitFiles"), &parameters)
    };
    let (out, errors) = unsigned(&[]);
    assert!(
        errors.contains("no contributor agreement from unsigned@example.com")
            && errors.contains("merge=false")
            && errors.contains(&format!("user={WRITER}"))
            && errors.contains("commits=1"),
        "{errors}"
    );
    assert!(get(&out, "commitId").is_empty());
    assert_eq!(main_head(&fixture), first);
    // another branch is not the rule's: the commit waits there
    let (out, errors) = unsigned(&[("branch", s("main")), ("newBranch", s("feat"))]);
    assert!(errors.trim().is_empty(), "{errors}");
    let feat = get(&out, "commitId").to_string();
    settled(&fixture);

    // merge#GitRefs: the rule sees the source's commit and the merge commit
    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("merge#GitRefs"),
        &[
            ("repositoryId", s("site")),
            ("source", s("feat")),
            ("target", s("main")),
        ],
    );
    assert!(
        errors.contains("no contributor agreement from unsigned@example.com")
            && errors.contains("merge=true method=GmmMerge request=null")
            && errors.contains("commits=2"),
        "{errors}"
    );
    assert_eq!(main_head(&fixture), first);

    // merge#MergeRequest by a merge commit: the same, as that merge request's
    let request = open_request(&fixture, WRITER, None, "feat");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&request)), ("expectedHeadCommitId", s(&feat))],
    );
    assert!(
        errors.contains("no contributor agreement from unsigned@example.com")
            && errors.contains(&format!("merge=true method=GmmMerge request={request} "))
            && errors.contains("commits=2"),
        "{errors}"
    );
    assert_eq!(main_head(&fixture), first);

    // a squash: one new commit, by the merge request's author, whom the rule has not signed either
    commit(&fixture, WRITER, "site", Some("feat2"), "b.txt");
    settled(&fixture);
    let squashed = open_request(&fixture, DEVELOPER, None, "feat2");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&squashed)), ("mergeMethodEnumId", s("GmmSquash"))],
    );
    assert!(
        errors.contains("no contributor agreement from git.developer@example.com")
            && errors.contains(&format!("method=GmmSquash request={squashed} "))
            && errors.contains("commits=1"),
        "{errors}"
    );
    assert_eq!(main_head(&fixture), first);
    // neither merge request was merged
    for id in [&request, &squashed] {
        let found = ok(
            &fixture,
            WRITER,
            &review("get#MergeRequest"),
            &[("mergeRequestId", s(id))],
        );
        let status = crate::server::hosting_tests::entry(found.get("mergeRequest").unwrap(), "statusId");
        assert_eq!(crate::server::hosting_tests::text_of(status), "GmrOpen");
    }
    // what the rule passes merges: the writer's commit, by a merge commit of the writer's
    let merged = ok(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&squashed)), ("mergeMethodEnumId", s("GmmMerge"))],
    );
    assert_eq!(main_head(&fixture), get(&merged, "commitId"));
    settled(&fixture);

    // a push, when git is here
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "site", WRITER);
    let before = main_head(&fixture);
    work.ok(&[
        "commit",
        "--allow-empty",
        "--author=Una Unsigned <unsigned@example.com>",
        "-m",
        "Pushed",
    ]);
    let (pushed, out) = work.run(&["push", "origin", "main"]);
    assert!(
        !pushed && out.contains("no contributor agreement from unsigned@example.com") && out.contains("merge=false"),
        "{out}"
    );
    assert_eq!(main_head(&fixture), before);
    settled(&fixture);
}

#[test]
fn a_merge_request_merges_the_head_it_counted_and_a_forks_branch_goes_only_by_right() {
    let fixture = fixture_in("gate-merge", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    // no gitECA rule: a merge that deletes its fork's branch changes two repositories at once, and the in-memory
    // test database commits whole tables, so the event runs of two repositories must not overlap
    developer(&fixture);
    define(&fixture, "site", "db");
    let first = commit(&fixture, WRITER, "site", None, "a.txt");

    // a branch named as the head's id, at another commit: the merge is of the head, by its id
    let head = commit(&fixture, WRITER, "site", Some("feat"), "f.txt");
    let other = commit(&fixture, WRITER, "site", Some("other"), "o.txt");
    ok(
        &fixture,
        WRITER,
        &git("create#GitRef"),
        &[
            ("repositoryId", s("site")),
            ("refName", s(&head)),
            ("revision", s("other")),
        ],
    );
    assert_eq!(
        head_of(&fixture, "site", &format!("refs/heads/{head}")).as_deref(),
        Some(other.as_str())
    );
    let request = open_request(&fixture, WRITER, None, "feat");
    // the head the caller read is not the head now: nothing is merged
    let (_, errors) = call(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&request)), ("expectedHeadCommitId", s(&other))],
    );
    assert!(errors.contains("GitStaleRef"), "{errors}");
    assert_eq!(main_head(&fixture), first);
    ok(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[
            ("mergeRequestId", s(&request)),
            ("mergeMethodEnumId", s("GmmFastForward")),
            ("expectedHeadCommitId", s(&head)),
        ],
    );
    assert_eq!(main_head(&fixture), head);

    // a fork, and merge requests from its branches by the developer, who may push there and not delete
    let (_, errors) = internal(
        &fixture,
        DEVELOPER,
        &git("fork#GitRepository"),
        &[("sourceRepositoryId", s("site")), ("repositoryId", s("fork"))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    let branch_there = |name: &str| head_of(&fixture, "fork", &format!("refs/heads/{name}")).is_some();
    let merge_from_fork = |name: &str, user: &str, disabled: bool, extra: &[(&str, Value)]| {
        commit(&fixture, DEVELOPER, "fork", Some(name), &format!("{name}.txt"));
        let request = open_request(&fixture, DEVELOPER, Some("fork"), name);
        let mut parameters = vec![("mergeRequestId", s(&request)), ("deleteSourceBranch", s("true"))];
        parameters.extend(extra.iter().cloned());
        let before = main_head(&fixture);
        let (_, errors) = if disabled {
            internal(&fixture, user, &review("merge#MergeRequest"), &parameters)
        } else {
            call(&fixture, user, &review("merge#MergeRequest"), &parameters)
        };
        assert!(errors.trim().is_empty(), "{name}: {errors}");
        assert_ne!(main_head(&fixture), before, "{name} was merged");
        branch_there(name)
    };
    let force = [("forceDeleteSourceBranch", s("true"))];
    // authorization disabled is the application's word for the target, not the right to delete in the fork
    assert!(
        merge_from_fork("f1", DEVELOPER, true, &[]),
        "kept: no delete right in the fork"
    );
    assert!(
        merge_from_fork("f2", DEVELOPER, false, &force),
        "kept: force is the application's to say"
    );
    assert!(
        !merge_from_fork("f3", DEVELOPER, true, &force),
        "deleted: the application said so"
    );
    assert!(
        !merge_from_fork("f4", WRITER, false, &[]),
        "deleted: the writer may delete in the fork"
    );
}

#[test]
fn a_webhook_event_is_queued_only_by_who_may_update() {
    let fixture = fixture_in("gate-webhook", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    framework_rules(&fixture);
    developer(&fixture);
    define(&fixture, "site", "db");
    commit(&fixture, WRITER, "site", None, "a.txt");
    settled(&fixture);
    commit(&fixture, WRITER, "site", Some("feat"), "f.txt");
    settled(&fixture);
    let request = open_request(&fixture, WRITER, None, "feat");
    let event = [
        ("repositoryId", s("site")),
        ("event", s("merge_request")),
        ("action", s("merged")),
    ];
    let of_request = [("mergeRequestId", s(&request)), ("action", s("merged"))];
    // a reader queues nothing, by either service; who may push does, and so does an application
    let (out, errors) = call(&fixture, READER, &git("queue#GitWebhookEvent"), &event);
    assert!(
        errors.contains("may not update") && !out.contains_key("queuedCount"),
        "{errors}"
    );
    let (_, errors) = call(&fixture, READER, &review("queue#MergeRequestEvent"), &of_request);
    assert!(errors.contains("may not update"), "{errors}");
    let out = ok(&fixture, DEVELOPER, &git("queue#GitWebhookEvent"), &event);
    assert!(out.contains_key("queuedCount"));
    ok(&fixture, DEVELOPER, &review("queue#MergeRequestEvent"), &of_request);
    let (out, errors) = internal(&fixture, READER, &git("queue#GitWebhookEvent"), &event);
    assert!(errors.trim().is_empty() && out.contains_key("queuedCount"), "{errors}");
    let (_, errors) = internal(&fixture, READER, &review("queue#MergeRequestEvent"), &of_request);
    assert!(errors.trim().is_empty(), "{errors}");
    // the merge request services queue their events for a user who may only view: a review's
    let (_, errors) = call(
        &fixture,
        READER,
        &review("submit#MergeRequestReview"),
        &[("mergeRequestId", s(&request)), ("decision", s("comment"))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    settled(&fixture);
}

/// The ways a branch moves with no service of the hosting interface: a write through a `git://` location, a
/// mirror's fetch and an import. Each runs the pre events with its own source, and a refusal is its error.
#[test]
fn a_pre_rule_stops_a_git_location_write_a_fetch_and_an_import() {
    if !git_available() {
        eprintln!("skipped: a file:// fetch runs git-upload-pack, and git is not on the PATH");
        return;
    }
    let fixture = fixture_in("gate-sources", moqui_core::context::framework_dir());
    moqui_core::service::git_eca::install(
        fixture.context.factory(),
        r#"<gitecas>
            <giteca rule-name="NoBlockedWrites" event="pre-ref-update" source="write" repository-pattern="src">
                <condition><expression>newCommits.some(c => c.summary == 'Create blocked.md')</expression></condition>
                <actions><message error="true">blocked.md is not written</message></actions>
            </giteca>
            <giteca rule-name="NoSecrets" event="pre-receive" source="fetch,import">
                <condition><expression>newCommitsTruncated || newCommits.some(c => c.summary == 'Create secret.md')</expression></condition>
                <actions><message error="true">secret.md is not taken</message></actions>
            </giteca>
        </gitecas>"#,
    );
    define(&fixture, "src", "local");
    define(&fixture, "dest", "db");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let mut row = moqui_core::entity::EntityValue::make(&fixture.entity, crate::GIT_REPOSITORY).unwrap();
        row.set("repositoryId", s("mirror"));
        row.set("repositoryKind", s("remote"));
        row.set("localPath", s(&fixture.dir.join("mirror.git").to_string_lossy()));
        row.set(
            "remoteUrl",
            s(&format!("file://{}", fixture.dir.join("src.git").display())),
        );
        row.create(&fixture.entity).unwrap();
    }
    let head = |id: &str| head_of(&fixture, id, "refs/heads/main");
    let as_writer = |work: &dyn Fn() -> Result<(), String>| {
        let _entered = fixture.context.enter();
        moqui_core::user::internal_login_user(WRITER).unwrap();
        let outcome = work();
        moqui_core::user::logout().unwrap();
        outcome
    };
    let write = |path: &str| {
        as_writer(&|| {
            let resources = fixture.context.factory().resource();
            resources
                .write_bytes(&format!("git://src/{path}"), b"text\n")
                .map_err(|err| err.to_string())
        })
    };
    let fetch = || {
        as_writer(&|| {
            let provider = crate::provider(fixture.context.factory()).unwrap();
            provider.fetch("mirror").map(|_| ()).map_err(|err| err.to_string())
        })
    };

    // a write through git://
    write("a.md").unwrap();
    let first = head("src");
    assert!(first.is_some());
    let refused = write("blocked.md").unwrap_err();
    assert!(refused.contains("blocked.md is not written"), "{refused}");
    assert_eq!(head("src"), first);

    // a mirror's fetch: what the rule passes moves the mirror, what it refuses does not, and nothing is left staged
    fetch().unwrap();
    assert_eq!(head("mirror"), first);
    write("secret.md").unwrap();
    let second = head("src");
    assert_ne!(second, first);
    let refused = fetch().unwrap_err();
    assert!(refused.contains("secret.md is not taken"), "{refused}");
    assert_eq!(head("mirror"), first);
    {
        let _entered = fixture.context.enter();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let refs = provider.repo("mirror").unwrap().store.list_refs("refs/").unwrap();
        assert!(refs.iter().all(|(name, _)| name == "refs/heads/main"), "{refs:?}");
    }

    // an import
    let refused = as_writer(&|| {
        let provider = crate::provider(fixture.context.factory()).unwrap();
        provider
            .import("dest", Some("git://src"))
            .map(|_| ())
            .map_err(|err| err.to_string())
    })
    .unwrap_err();
    assert!(refused.contains("secret.md is not taken"), "{refused}");
    assert_eq!(head("dest"), None);
    settled(&fixture);
}
