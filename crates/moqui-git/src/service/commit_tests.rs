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

//! `commit#GitFiles` and what an application asks of the hosting services (plans/Dogfood.md DG4: N-COMMIT, N-MRNUM,
//! N-CONTAINS, N-PUSHER, N-HOOKS), through the services alone on the in-memory database, for a `db` and a `local`
//! repository: a commit to a branch, to a new branch and as a proposal, the blob an edit began from, a rename and a
//! delete in one commit, bytes, the branch rules; a merge request with a given number; the refs that contain a
//! commit; who pushed one; the system's status contexts; and who merges into a protected branch.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use crate::server::hosting_tests::{call, entry, framework_rules_with, get, grant_entities, list, ok, s, text_of};
use crate::server::tests::{create, define, fixture_with, head_of, settled, user, Fixture, READER, WRITER};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityValue;

const GIT: &str = "org.moqui.git.GitServices";
const REVIEW: &str = "org.moqui.git.GitReviewServices";

/// A user who may view, create and update in every repository, and not delete.
const DEVELOPER: &str = "git.developer";

fn git(name: &str) -> String {
    format!("{GIT}.{name}")
}

fn review(name: &str) -> String {
    format!("{REVIEW}.{name}")
}

fn files(pairs: &[(&str, &str)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(path, text)| ((*path).to_string(), Value::string(*text)))
            .collect(),
    )
}

/// Call a service as `user` with authorization disabled, as an application's own service does: its result and the
/// errors it left.
pub(crate) fn internal(
    fixture: &Fixture,
    user: &str,
    name: &str,
    parameters: &[(&str, Value)],
) -> (BTreeMap<String, Value>, String) {
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let options = moqui_core::service::call::CallOptions {
        disable_authz: true,
        ..Default::default()
    };
    let result = moqui_core::service::call::sync_with(name, &parameters, &options);
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors_text().join("\n");
        ec.message().clear();
        text
    })
    .unwrap_or_default();
    moqui_core::user::logout().unwrap();
    match result {
        Ok(out) => (out, errors),
        Err(err) => (BTreeMap::new(), format!("{errors}\n{err}")),
    }
}

fn commit(fixture: &Fixture, user: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    let mut all = vec![("repositoryId", s("site"))];
    all.extend(parameters.iter().cloned());
    call(fixture, user, &git("commit#GitFiles"), &all)
}

fn committed(fixture: &Fixture, user: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let (out, errors) = commit(fixture, user, parameters);
    assert!(errors.trim().is_empty(), "commit#GitFiles: {errors}");
    out
}

/// The text of a file at a revision, when it is one there.
fn blob(fixture: &Fixture, revision: &str, path: &str) -> Option<String> {
    let out = ok(
        fixture,
        READER,
        &git("get#GitBlob"),
        &[
            ("repositoryId", s("site")),
            ("revision", s(revision)),
            ("path", s(path)),
        ],
    );
    out.get("text").map(text_of)
}

fn developer(fixture: &Fixture) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    user(&fixture.entity, DEVELOPER);
    create(
        &fixture.entity,
        "moqui.security.UserGroup",
        &[("userGroupId", "GIT_DEVELOPERS")],
    );
    for group in ["GIT_DEVELOPERS", "GIT_READERS"] {
        let mut row = EntityValue::make(&fixture.entity, "moqui.security.UserGroupMember").unwrap();
        row.set("userId", s(DEVELOPER));
        row.set("userGroupId", s(group));
        row.set(
            "fromDate",
            moqui_lib::object::basic_convert(&Value::I64(1_000), "Timestamp").value,
        );
        row.create(&fixture.entity).unwrap();
    }
    for action in ["AUTHZA_CREATE", "AUTHZA_UPDATE"] {
        create(
            &fixture.entity,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", &format!("DEV_{action}")),
                ("userGroupId", "GIT_DEVELOPERS"),
                ("artifactGroupId", "GIT_ALL"),
                ("authzTypeEnumId", "AUTHZT_ALWAYS"),
                ("authzActionEnumId", action),
            ],
        );
    }
}

fn commits_and_what_is_asked_of_them(kind: &str) {
    // a hosting application's settings: the system's status contexts, and a rule of its own for pushes for review
    let fixture = fixture_with(
        &format!("commit-{kind}"),
        moqui_core::context::framework_dir(),
        r#"<git-server reserved-status-contexts="agreement/,ci/"/>"#,
    );
    grant_entities(&fixture);
    framework_rules_with(
        &fixture,
        r#"<giteca rule-name="HeldForReview" event="pre-receive">
            <condition><expression>forReview &amp;&amp; newCommits.some(c => c.summary.startsWith('BLOCK'))</expression></condition>
            <actions><message error="true">not taken for review</message></actions>
        </giteca>"#,
    );
    developer(&fixture);
    define(&fixture, "site", kind);
    let head = |name: &str| head_of(&fixture, "site", &format!("refs/heads/{name}"));

    // the first commit of an empty repository makes its default branch; a reader commits to no branch
    let first = committed(
        &fixture,
        WRITER,
        &[
            ("message", s("First")),
            ("fileMap", files(&[("docs/a.md", "one\n"), ("b.txt", "b\n")])),
        ],
    );
    let c1 = get(&first, "commitId").to_string();
    assert_eq!(get(&first, "refName"), "refs/heads/main");
    assert_eq!(head("main").as_deref(), Some(c1.as_str()));
    let (_, errors) = commit(
        &fixture,
        READER,
        &[("message", s("No")), ("fileMap", files(&[("b.txt", "no\n")]))],
    );
    assert!(!errors.trim().is_empty());
    assert_eq!(head("main").as_deref(), Some(c1.as_str()));

    // to a named branch, by an author the application gives: the committer is the server
    ok(
        &fixture,
        WRITER,
        &git("create#GitRef"),
        &[("repositoryId", s("site")), ("refName", s("dev"))],
    );
    let by_ada = [
        ("repositoryId", s("site")),
        ("branch", s("dev")),
        ("message", s("On dev")),
        ("fileMap", files(&[("docs/a.md", "one\ndev\n")])),
        ("authorName", s("Ada Author")),
        ("authorEmail", s("ada@example.com")),
    ];
    // who a commit is by is the application's to say, not a caller's
    let (_, errors) = call(&fixture, WRITER, &git("commit#GitFiles"), &by_ada);
    assert!(errors.contains("authorization disabled"), "{errors}");
    assert_eq!(head("dev").as_deref(), Some(c1.as_str()));
    let (on_dev, errors) = internal(&fixture, WRITER, &git("commit#GitFiles"), &by_ada);
    assert!(errors.trim().is_empty(), "{errors}");
    let dev_commit = get(&on_dev, "commitId").to_string();
    assert_eq!(head("dev").as_deref(), Some(dev_commit.as_str()));
    assert_eq!(head("main").as_deref(), Some(c1.as_str()));
    let read = ok(
        &fixture,
        READER,
        &git("get#GitCommit"),
        &[("repositoryId", s("site")), ("commitId", s(&dev_commit))],
    );
    let details = read.get("commit").unwrap();
    assert_eq!(text_of(entry(details, "authorName")), "Ada Author");
    assert_eq!(text_of(entry(details, "authorEmail")), "ada@example.com");
    assert_eq!(text_of(entry(details, "committerName")), "Moqui");
    assert_eq!(text_of(entry(details, "moquiUserId")), WRITER);
    let (_, errors) = commit(
        &fixture,
        WRITER,
        &[
            ("branch", s("nosuch")),
            ("message", s("No")),
            ("fileMap", files(&[("b.txt", "no\n")])),
        ],
    );
    assert!(errors.contains("GitNoSuchRef"), "{errors}");

    // to a new branch made from one; a name that is taken is refused by its code
    let patch = [
        ("branch", s("main")),
        ("newBranch", s("patch-1")),
        ("message", s("Patch")),
        ("fileMap", files(&[("b.txt", "b2\n")])),
    ];
    let on_patch = committed(&fixture, WRITER, &patch);
    assert_eq!(get(&on_patch, "refName"), "refs/heads/patch-1");
    assert_eq!(head("patch-1").as_deref(), Some(get(&on_patch, "commitId")));
    assert_eq!(head("main").as_deref(), Some(c1.as_str()));
    let (_, errors) = commit(&fixture, WRITER, &patch);
    assert!(errors.contains("GitRefExists"), "{errors}");
    // a name git does not take makes no ref
    for name in ["bad..name", "ends.lock", "has space", "refs/heads/../x", "-dash/~1"] {
        let (_, errors) = commit(
            &fixture,
            WRITER,
            &[
                ("newBranch", s(name)),
                ("message", s("No")),
                ("fileMap", files(&[("b.txt", "no\n")])),
            ],
        );
        assert!(errors.contains("GitBadRefName"), "{name}: {errors}");
    }
    let branches = ok(
        &fixture,
        READER,
        &git("get#GitRefs"),
        &[("repositoryId", s("site")), ("refType", s("branch"))],
    );
    assert_eq!(list(&branches, "refList").len(), 3, "main, dev and patch-1");

    // a commit of a branch that is gone: in a repository alone in its store, read by id by who may push
    let gone = committed(
        &fixture,
        WRITER,
        &[
            ("newBranch", s("gone")),
            ("message", s("Gone")),
            ("fileMap", files(&[("g.txt", "g\n")])),
        ],
    );
    let gone = get(&gone, "commitId").to_string();
    ok(
        &fixture,
        WRITER,
        &git("delete#GitRef"),
        &[("repositoryId", s("site")), ("refName", s("gone"))],
    );
    let reads = |user: &str, commit: &str| {
        let out = ok(
            &fixture,
            user,
            &git("get#GitCommit"),
            &[
                ("repositoryId", s("site")),
                ("commitId", s(commit)),
                ("stats", s("false")),
            ],
        );
        out.contains_key("commit")
    };
    assert!(reads(WRITER, &gone) && reads(WRITER, &gone[..12]));
    assert!(!reads(READER, &gone) && !reads(READER, &gone[..12]));

    // the blob an edit began from: the same one commits, another is a conflict that says what is there now
    let tree = ok(
        &fixture,
        READER,
        &git("get#GitTree"),
        &[
            ("repositoryId", s("site")),
            ("revision", s("main")),
            ("path", s("docs")),
        ],
    );
    let began = text_of(entry(&list(&tree, "entryList")[0], "objectId"));
    let expected = || Value::Map([("docs/a.md".to_string(), s(&began))].into_iter().collect());
    let edited = committed(
        &fixture,
        WRITER,
        &[
            ("message", s("Edit a")),
            ("fileMap", files(&[("docs/a.md", "one\nmain\n")])),
            ("expectedBlobIds", expected()),
        ],
    );
    let c2 = get(&edited, "commitId").to_string();
    assert!(!c2.is_empty());
    let stale = committed(
        &fixture,
        WRITER,
        &[
            ("message", s("Stale edit")),
            ("fileMap", files(&[("docs/a.md", "stale\n")])),
            ("expectedBlobIds", expected()),
        ],
    );
    assert_eq!(text_of(stale.get("conflict").unwrap()), "true");
    assert_eq!(get(&stale, "errorCode"), "GitFileChanged");
    assert!(get(&stale, "commitId").is_empty());
    let conflict = &list(&stale, "conflictList")[0];
    assert_eq!(text_of(entry(conflict, "path")), "docs/a.md");
    assert_eq!(text_of(entry(conflict, "expectedBlobId")), began);
    let current = text_of(entry(conflict, "currentBlobId"));
    assert!(current.len() == 40 && current != began, "{current}");
    assert_eq!(head("main").as_deref(), Some(c2.as_str()));
    assert_eq!(blob(&fixture, "main", "docs/a.md").as_deref(), Some("one\nmain\n"));

    // a rename, an edit of the renamed file and a delete in one commit; then bytes
    committed(
        &fixture,
        WRITER,
        &[
            ("message", s("Move")),
            ("renameMap", files(&[("docs/a.md", "guide/a.md")])),
            ("removePaths", Value::List(vec![s("b.txt")])),
            ("fileMap", files(&[("guide/a.md", "moved\n")])),
        ],
    );
    assert_eq!(blob(&fixture, "main", "guide/a.md").as_deref(), Some("moved\n"));
    assert_eq!(blob(&fixture, "main", "docs/a.md"), None);
    assert_eq!(blob(&fixture, "main", "b.txt"), None);
    committed(
        &fixture,
        WRITER,
        &[
            ("message", s("Logo")),
            (
                "fileMap",
                Value::Map(
                    [("logo.bin".to_string(), Value::Bytes(vec![0, 159, 146, 150]))]
                        .into_iter()
                        .collect(),
                ),
            ),
        ],
    );
    let logo = ok(
        &fixture,
        READER,
        &git("get#GitBlob"),
        &[("repositoryId", s("site")), ("path", s("logo.bin"))],
    );
    assert_eq!(text_of(logo.get("binary").unwrap()), "true");
    assert_eq!(text_of(logo.get("size").unwrap()), "4");
    let main_before_rule = head("main");

    // the branch rules, as a push meets them
    ok(
        &fixture,
        WRITER,
        &git("store#GitBranchRule"),
        &[
            ("repositoryId", s("site")),
            ("refPattern", s("main")),
            ("requireMergeRequest", s("Y")),
        ],
    );
    let (_, errors) = commit(
        &fixture,
        WRITER,
        &[
            ("message", s("Direct")),
            ("fileMap", files(&[("guide/a.md", "direct\n")])),
        ],
    );
    assert!(errors.contains("GitProtectedBranch"), "{errors}");
    assert_eq!(head("main"), main_before_rule);
    // an application's own write passes them when it says so, with authorization disabled; a caller's does not
    let past = [
        ("repositoryId", s("site")),
        ("message", s("Release notes")),
        ("fileMap", files(&[("CHANGELOG.md", "1.0\n")])),
        ("bypassBranchRules", s("true")),
    ];
    let (_, errors) = call(&fixture, WRITER, &git("commit#GitFiles"), &past);
    assert!(errors.contains("GitProtectedBranch"), "{errors}");
    let (noted, errors) = internal(&fixture, WRITER, &git("commit#GitFiles"), &past);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(head("main").as_deref(), Some(get(&noted, "commitId")));
    let main_before_rule = head("main");

    // a proposal meets the rules a push for review meets
    let (_, errors) = commit(
        &fixture,
        READER,
        &[
            ("asProposal", s("true")),
            ("message", s("BLOCK this")),
            ("fileMap", files(&[("guide/a.md", "blocked\n")])),
        ],
    );
    assert!(errors.contains("not taken for review"), "{errors}");

    // a proposal by a reader: no branch, the commit the head of a merge request; again, the same one
    let proposed = committed(
        &fixture,
        READER,
        &[
            ("asProposal", s("true")),
            ("message", s("Fix a typo\n\nIt was wrong.")),
            ("fileMap", files(&[("guide/a.md", "moved!\n")])),
        ],
    );
    let proposal = get(&proposed, "commitId").to_string();
    assert_eq!(text_of(proposed.get("mergeRequestNum").unwrap()), "1");
    assert_eq!(text_of(proposed.get("mergeRequestCreated").unwrap()), "true");
    assert_eq!(get(&proposed, "refName"), "refs/merge-requests/1/head");
    assert_eq!(
        head_of(&fixture, "site", "refs/merge-requests/1/head").as_deref(),
        Some(proposal.as_str())
    );
    assert_eq!(head("main"), main_before_rule);
    let request = ok(
        &fixture,
        READER,
        &review("get#MergeRequest"),
        &[("mergeRequestId", s(get(&proposed, "mergeRequestId")))],
    );
    let request = request.get("mergeRequest").unwrap();
    assert_eq!(text_of(entry(request, "title")), "Fix a typo");
    assert_eq!(text_of(entry(request, "description")), "It was wrong.");
    assert_eq!(text_of(entry(request, "pushedForReview")), "Y");
    assert_eq!(text_of(entry(request, "authorUserId")), READER);
    assert_eq!(text_of(entry(request, "headCommitId")), proposal);
    let again = committed(
        &fixture,
        READER,
        &[
            ("asProposal", s("true")),
            ("message", s("Fix it better")),
            ("fileMap", files(&[("guide/a.md", "moved!!\n")])),
        ],
    );
    assert_eq!(text_of(again.get("mergeRequestNum").unwrap()), "1");
    assert_eq!(text_of(again.get("mergeRequestCreated").unwrap()), "false");
    assert_eq!(
        head_of(&fixture, "site", "refs/merge-requests/1/head").as_deref(),
        Some(get(&again, "commitId"))
    );
    let (_, errors) = commit(
        &fixture,
        READER,
        &[
            ("newBranch", s("readers")),
            ("message", s("No")),
            ("fileMap", files(&[("x.txt", "x\n")])),
        ],
    );
    assert!(!errors.trim().is_empty());
    assert_eq!(head("readers"), None);

    // the branches and tags that contain a commit
    let names = |commit: &str| -> Vec<String> {
        let out = ok(
            &fixture,
            READER,
            &git("get#GitCommitRefs"),
            &[("repositoryId", s("site")), ("commitId", s(commit))],
        );
        list(&out, "refList")
            .iter()
            .map(|found| text_of(entry(found, "shortName")))
            .collect()
    };
    assert_eq!(names(&c1), ["main", "dev", "patch-1"]);
    assert_eq!(names(&dev_commit), ["dev"]);
    assert_eq!(names(&c2), ["main"]);

    // who brought a commit: the user of the write
    settled(&fixture);
    let pushers = ok(
        &fixture,
        READER,
        &git("get#GitCommitPushers"),
        &[
            ("repositoryId", s("site")),
            (
                "commitIdList",
                // the proposal's first commit is its head no more: who pushed it is not told either
                Value::List(vec![
                    s(&c1),
                    s(&proposal),
                    s(get(&again, "commitId")),
                    s("0".repeat(40).as_str()),
                ]),
            ),
        ],
    );
    let by_commit: BTreeMap<String, String> = list(&pushers, "pusherList")
        .iter()
        .map(|found| (text_of(entry(found, "commitId")), text_of(entry(found, "userId"))))
        .collect();
    assert_eq!(by_commit.len(), 2, "{by_commit:?}");
    assert_eq!(by_commit[&c1], WRITER);
    assert_eq!(by_commit[get(&again, "commitId")], READER);

    // a merge request with a given number, for an import: only with authorization disabled, and only a free one
    let numbered = [
        ("repositoryId", s("site")),
        ("sourceRef", s("dev")),
        ("title", s("Dev")),
        ("mergeRequestNum", Value::I64(40)),
    ];
    let (_, errors) = call(&fixture, WRITER, &review("create#MergeRequest"), &numbered);
    assert!(errors.contains("authorization disabled"), "{errors}");
    let (made, errors) = internal(&fixture, WRITER, &review("create#MergeRequest"), &numbered);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(text_of(made.get("mergeRequestNum").unwrap()), "40");
    let dev_request = get(&made, "mergeRequestId").to_string();
    let (_, errors) = internal(
        &fixture,
        WRITER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("site")),
            ("sourceRef", s("patch-1")),
            ("title", s("Patch")),
            ("mergeRequestNum", Value::I64(40)),
        ],
    );
    assert!(errors.contains("GitNumberTaken"), "{errors}");
    let next = ok(
        &fixture,
        WRITER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("site")),
            ("sourceRef", s("patch-1")),
            ("targetRef", s("dev")),
            ("title", s("Patch into dev")),
        ],
    );
    assert_eq!(text_of(next.get("mergeRequestNum").unwrap()), "41");
    assert_eq!(
        head_of(&fixture, "site", "refs/merge-requests/41/head"),
        head("patch-1"),
        "a merge request of a branch has its head ref too"
    );

    // the system's status contexts are not a user's to post
    let status = |context: &str| {
        [
            ("repositoryId", s("site")),
            ("commitId", s(&c1)),
            ("state", s("success")),
            ("context", s(context)),
        ]
    };
    for context in ["ci/build", "agreement/authors"] {
        let (_, errors) = call(&fixture, WRITER, &git("create#GitCommitStatus"), &status(context));
        assert!(errors.contains("GitReservedContext"), "{context}: {errors}");
        let (_, errors) = internal(&fixture, WRITER, &git("create#GitCommitStatus"), &status(context));
        assert!(errors.trim().is_empty(), "{context}: {errors}");
    }
    ok(&fixture, WRITER, &git("create#GitCommitStatus"), &status("lint"));

    // merging needs the update right, into a branch with rules too; what merges is the head that was checked
    let into_dev = ok(
        &fixture,
        DEVELOPER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(get(&next, "mergeRequestId")))],
    );
    assert!(!get(&into_dev, "commitId").is_empty());
    let clean_commit = committed(
        &fixture,
        WRITER,
        &[
            ("newBranch", s("clean")),
            ("message", s("Clean")),
            ("fileMap", files(&[("c.txt", "c\n")])),
        ],
    );
    let clean = ok(
        &fixture,
        WRITER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("site")),
            ("sourceRef", s("clean")),
            ("title", s("Clean")),
        ],
    );
    let clean_request = get(&clean, "mergeRequestId").to_string();
    let asked = |user: &str| {
        let out = ok(
            &fixture,
            user,
            &review("get#MergeRequest"),
            &[("mergeRequestId", s(&clean_request))],
        );
        text_of(out.get("canMerge").unwrap())
    };
    assert_eq!(asked(DEVELOPER), "true");
    assert_eq!(asked(READER), "false");
    settled(&fixture);
    let merged = ok(
        &fixture,
        DEVELOPER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&clean_request))],
    );
    assert!(!get(&merged, "commitId").is_empty());
    assert_eq!(blob(&fixture, "main", "c.txt").as_deref(), Some("c\n"));
    // the merge request of dev, whose number was given, is still there
    assert!(!dev_request.is_empty());
    settled(&fixture);

    // a merge request from before the head refs were kept: its ref comes back from its recorded head when it is read
    let clean_num = text_of(clean.get("mergeRequestNum").unwrap());
    let head_ref = format!("refs/merge-requests/{clean_num}/head");
    {
        let _entered = fixture.context.enter();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let repo = provider.repo("site").unwrap();
        let current = repo.store.read_ref(&head_ref).unwrap();
        assert_eq!(
            current.map(|id| id.to_string()).as_deref(),
            Some(get(&clean_commit, "commitId"))
        );
        repo.store
            .update_refs(
                &[crate::store::RefEdit {
                    name: head_ref.clone(),
                    expected: current,
                    new: None,
                }],
                true,
            )
            .unwrap();
    }
    assert_eq!(head_of(&fixture, "site", &head_ref), None);
    let diff = ok(
        &fixture,
        READER,
        &review("get#MergeRequestDiff"),
        &[("mergeRequestId", s(&clean_request)), ("withDiffs", s("false"))],
    );
    assert_eq!(list(&diff, "commitList").len(), 1);
    assert_eq!(
        head_of(&fixture, "site", &head_ref).as_deref(),
        Some(get(&clean_commit, "commitId"))
    );
}

#[test]
fn files_are_committed_through_the_service_in_a_db_repository() {
    commits_and_what_is_asked_of_them("db");
}

#[test]
fn files_are_committed_through_the_service_in_a_local_repository() {
    commits_and_what_is_asked_of_them("local");
}
