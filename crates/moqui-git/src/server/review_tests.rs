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

//! Push to review and forks against the git command line (plans/Dogfood.md DG4: F4, F5, N-REACH), on the in-memory
//! database with the framework's services and gitECA rules: a reader pushes `refs/for/main` and gets a merge
//! request, pushes again and updates it, and can push nothing else; the head is fetched by name, not advertised, and
//! hidden when the application says so; the caps; a fork of each kind, cloned and pushed to, a merge request from it
//! merged into its parent; a fork's commit that its parent does not answer for; a collection that keeps what a fork
//! reaches; a fork deleted and a parent that is not. Each test is skipped when git is not installed.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::commit_tests::internal;
use super::hosting_tests::{call, entry, framework_rules, get, grant_entities, list, ok, s, text_of, Work};
use super::tests::{
    define, fixture_in, fixture_with, git_available, git_in, git_ok, head_of, http_server, settled, url_with, Fixture,
    READER, WRITER,
};
use moqui_core::entity::EntityValue;

const GIT: &str = "org.moqui.git.GitServices";
const REVIEW: &str = "org.moqui.git.GitReviewServices";
const MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";

fn git(name: &str) -> String {
    format!("{GIT}.{name}")
}

fn review(name: &str) -> String {
    format!("{REVIEW}.{name}")
}

/// The merge requests of a repository, by number.
fn merge_requests(fixture: &Fixture, id: &str) -> Vec<EntityValue> {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find(MERGE_REQUEST)
        .unwrap()
        .eq("repositoryId", Value::string(id))
        .order_by("mergeRequestNum", false)
        .list()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(text_of).unwrap_or_default()
}

/// Text that does not compress, so a pack of it is about as large.
fn noise(len: usize) -> String {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(b'!' + (state % 90) as u8)
        })
        .collect()
}

/// Whether `user` reads the commit `id` of a repository by its id.
fn reads_commit(fixture: &Fixture, user: &str, repository: &str, id: &str) -> bool {
    let (out, errors) = call(
        fixture,
        user,
        &git("get#GitCommit"),
        &[
            ("repositoryId", s(repository)),
            ("commitId", s(id)),
            ("stats", s("false")),
        ],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    out.contains_key("commit")
}

#[test]
fn a_reader_pushes_for_review_and_gets_a_merge_request() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_with(
        "review-push",
        moqui_core::context::framework_dir(),
        r#"<git-server review-max-push-bytes="6000" review-max-account-bytes="6000" merge-request-url="/{repositoryPath}/-/merge-requests/{mergeRequestNum}"/>"#,
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "proj", "db");
    let base = http_server(fixture.context.factory());
    let writer = Work::clone(&fixture, &base, "proj", WRITER);
    writer.commit(&[("a.txt", "one\ntwo\n")], "First");
    writer.ok(&["push", "origin", "main"]);
    let main = head_of(&fixture, "proj", "refs/heads/main");

    // a reader may not push a branch, and is told the way that is open
    let reader = Work::clone(&fixture, &base, "proj", READER);
    let first = reader.commit(
        &[("a.txt", "one\ntwo\nthree\n")],
        "Add a <third> line\n\nWhy it is needed.",
    );
    let (pushed, out) = reader.run(&["push", "origin", "HEAD:refs/heads/main"]);
    assert!(
        !pushed && out.contains("permission denied") && out.contains("refs/for/"),
        "{out}"
    );
    let (pushed, out) = reader.run(&["push", "origin", "HEAD:refs/heads/mine"]);
    assert!(!pushed && out.contains("permission denied"), "{out}");
    let (pushed, out) = reader.run(&["push", "origin", "HEAD:refs/merge-requests/1/head"]);
    assert!(!pushed && out.contains("funny refname"), "{out}");
    let (pushed, out) = reader.run(&["push", "origin", "HEAD:refs/for/nosuch"]);
    assert!(!pushed && out.contains("GitNoSuchRef"), "{out}");

    // refs/for/main: a merge request, its head kept under its own ref, nothing else moved
    let out = reader.ok(&["push", "origin", "HEAD:refs/for/main"]);
    assert!(out.contains("merge request !1 opened for main"), "{out}");
    assert!(out.contains("/proj/-/merge-requests/1"), "{out}");
    let found = merge_requests(&fixture, "proj");
    assert_eq!(found.len(), 1);
    let request = &found[0];
    assert_eq!(field(request, "pushedForReview"), "Y");
    assert_eq!(field(request, "sourceRef"), "refs/merge-requests/1/head");
    assert_eq!(field(request, "authorUserId"), READER);
    assert_eq!(field(request, "title"), "Add a third line");
    assert_eq!(field(request, "description"), "Why it is needed.");
    assert_eq!(field(request, "headCommitId"), first);
    assert_eq!(field(request, "statusId"), "GmrOpen");
    assert_eq!(field(request, "mergeableEnumId"), "GmaMergeable");
    assert_eq!(
        head_of(&fixture, "proj", "refs/merge-requests/1/head").as_deref(),
        Some(first.as_str())
    );
    assert_eq!(head_of(&fixture, "proj", "refs/heads/main"), main);
    assert_eq!(head_of(&fixture, "proj", "refs/for/main"), None);
    let request_id = field(request, "mergeRequestId");

    // the same author and target again: the same merge request, its new head
    let second = reader.commit(&[("b.txt", "more\n")], "More");
    let out = reader.ok(&["push", "origin", "HEAD:refs/for/main"]);
    assert!(out.contains("merge request !1 updated for main"), "{out}");
    let found = merge_requests(&fixture, "proj");
    assert_eq!(found.len(), 1);
    assert_eq!(field(&found[0], "headCommitId"), second);
    // a topic is another merge request, titled by the push
    let out = reader.ok(&[
        "push",
        "-o",
        "topic=docs",
        "-o",
        "title=Docs first",
        "-o",
        "draft",
        "origin",
        "HEAD~1:refs/for/main",
    ]);
    assert!(out.contains("merge request !2 opened for main"), "{out}");
    let found = merge_requests(&fixture, "proj");
    assert_eq!(found.len(), 2);
    assert_eq!(field(&found[1], "reviewTopic"), "docs");
    assert_eq!(field(&found[1], "title"), "Docs first");
    assert_eq!(field(&found[1], "statusId"), "GmrDraft");
    assert_eq!(field(&found[1], "headCommitId"), first);
    // nothing new to review is refused
    let (pushed, out) = reader.run(&["push", "origin", "origin/main:refs/for/main"]);
    assert!(!pushed && out.contains("no new commits"), "{out}");

    // the push log has the pushes, with their size, counted to the reader
    settled(&fixture);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let pushes = fixture
            .entity
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .eq("userId", s(READER))
            .eq("reviewPush", s("Y"))
            .list()
            .unwrap();
        assert_eq!(pushes.len(), 3, "the three pushes for review that were taken");
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let used = provider.review_bytes_of(READER, 60_000);
        assert!(used > 0 && used < 6000, "{used}");
        // the account's cap is over all its pushes for review of the day
        moqui_core::user::internal_login_user(READER).unwrap();
        assert_eq!(provider.review_cap(1), None);
        let over = provider.review_cap(6000 - used + 1).unwrap_or_default();
        assert!(over.starts_with("GitReviewQuota"), "{over}");
        moqui_core::user::logout().unwrap();
        // who pushed the commits
        let pusher = fixture
            .entity
            .find("moqui.resource.GitCommitPush")
            .unwrap()
            .eq("repositoryId", s("proj"))
            .eq("commitId", s(&first))
            .one()
            .unwrap()
            .expect("the pusher of the first commit pushed for review");
        assert_eq!(field(&pusher, "userId"), READER);
    }

    // the heads are not advertised; one is fetched by its name, by a reader
    let listed = git_ok(
        &fixture.dir,
        &writer.home,
        &["ls-remote", &url_with(&base, WRITER, "proj")],
    );
    assert!(
        listed.contains("refs/heads/main") && !listed.contains("merge-requests"),
        "{listed}"
    );
    writer.ok(&["fetch", "origin", "refs/merge-requests/1/head"]);
    assert_eq!(writer.ok(&["rev-parse", "FETCH_HEAD"]).trim(), second);
    assert!(reads_commit(&fixture, WRITER, "proj", &second));

    // hidden by the application: fetched by nobody, by name or by id, and not read by id
    ok(
        &fixture,
        WRITER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id))],
    );
    let other = fixture.dir.join("other");
    git_ok(
        &fixture.dir,
        &writer.home,
        &["clone", &url_with(&base, WRITER, "proj"), &other.to_string_lossy()],
    );
    let (fetched, out) = git_in(&other, &writer.home, &["fetch", "origin", "refs/merge-requests/1/head"]);
    assert!(!fetched, "{out}");
    let (fetched, out) = git_in(&other, &writer.home, &["fetch", "origin", &second]);
    assert!(!fetched && out.contains("not our ref"), "{out}");
    assert!(!reads_commit(&fixture, WRITER, "proj", &second));
    let (_, errors) = call(
        &fixture,
        WRITER,
        &review("get#MergeRequest"),
        &[("mergeRequestId", s(&request_id))],
    );
    assert!(errors.contains("No merge request"), "{errors}");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&request_id))],
    );
    assert!(errors.contains("hidden"), "{errors}");
    // the other merge request's head, the first commit, is still read
    assert!(reads_commit(&fixture, WRITER, "proj", &first));
    // the application shows a hidden one to whom it decides, asking with authorization disabled
    let (out, errors) = internal(
        &fixture,
        READER,
        &git("get#GitCommit"),
        &[
            ("repositoryId", s("proj")),
            ("commitId", s(&second)),
            ("stats", s("false")),
            ("includeHidden", s("true")),
        ],
    );
    assert!(errors.trim().is_empty() && out.contains_key("commit"), "{errors}");
    let (out, errors) = call(
        &fixture,
        READER,
        &git("get#GitCommit"),
        &[
            ("repositoryId", s("proj")),
            ("commitId", s(&second)),
            ("includeHidden", s("true")),
        ],
    );
    assert!(errors.trim().is_empty() && !out.contains_key("commit"), "{errors}");
    let (out, errors) = internal(
        &fixture,
        READER,
        &review("get#MergeRequestDiff"),
        &[("mergeRequestId", s(&request_id)), ("withDiffs", s("false"))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(list(&out, "commitList").len(), 2);
    let (_, errors) = call(
        &fixture,
        READER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id)), ("hidden", s("false"))],
    );
    assert!(
        !errors.trim().is_empty(),
        "a reader does not show a hidden merge request"
    );
    ok(
        &fixture,
        WRITER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id)), ("hidden", s("false"))],
    );
    git_ok(&other, &writer.home, &["fetch", "origin", "refs/merge-requests/1/head"]);
    assert!(reads_commit(&fixture, WRITER, "proj", &second));

    // a push over the cap for a reader is refused whole; the writer is not capped
    reader.commit(&[("big.txt", &noise(20_000))], "Big");
    let (pushed, out) = reader.run(&["push", "origin", "HEAD:refs/for/main"]);
    assert!(!pushed && out.contains("GitReviewPushTooLarge"), "{out}");
    assert_eq!(field(&merge_requests(&fixture, "proj")[0], "headCommitId"), second);

    // merged by a user who may: main has the reader's commits
    let merged = ok(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&request_id))],
    );
    assert!(!get(&merged, "commitId").is_empty());
    writer.ok(&["pull", "origin", "main"]);
    assert!(writer.dir.join("b.txt").is_file());
    let found = merge_requests(&fixture, "proj");
    assert_eq!(field(&found[0], "statusId"), "GmrMerged");
    settled(&fixture);
}

/// A fork of `kind`: cloned, pushed to, a merge request from it merged into its parent; what each answers by id.
fn fork_round(kind: &str) {
    let fixture = fixture_in(&format!("fork-{kind}"), moqui_core::context::framework_dir());
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "parent", kind);
    let base = http_server(fixture.context.factory());
    let writer = Work::clone(&fixture, &base, "parent", WRITER);
    let first = writer.commit(&[("a.txt", "one\ntwo\n")], "First");
    writer.ok(&["push", "origin", "main"]);
    writer.ok(&["tag", "v1"]);
    writer.ok(&["push", "origin", "v1"]);
    settled(&fixture);

    // a reader of the parent forks it: its own refs, the parent's objects
    let (_, errors) = call(
        &fixture,
        READER,
        &git("fork#GitRepository"),
        &[("sourceRepositoryId", s("nosuch")), ("repositoryId", s("x"))],
    );
    assert!(!errors.trim().is_empty());
    let fork_dir = fixture.dir.join("fork.git").to_string_lossy().into_owned();
    let asked = [
        ("sourceRepositoryId", s("parent")),
        ("repositoryId", s("fork")),
        ("localPath", s(&fork_dir)),
    ];
    // where a fork's directory goes is not a caller's to say
    let (_, errors) = call(&fixture, READER, &git("fork#GitRepository"), &asked);
    assert!(errors.contains("authorization disabled"), "{errors}");
    let (forked, errors) = internal(&fixture, READER, &git("fork#GitRepository"), &asked);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(get(&forked, "forkNetworkId"), "parent");
    assert_eq!(text_of(forked.get("refCount").unwrap()), "2");
    assert_eq!(
        head_of(&fixture, "fork", "refs/heads/main").as_deref(),
        Some(first.as_str())
    );
    assert_eq!(
        head_of(&fixture, "fork", "refs/tags/v1").as_deref(),
        Some(first.as_str())
    );
    let (_, errors) = call(
        &fixture,
        READER,
        &git("fork#GitRepository"),
        &[("sourceRepositoryId", s("parent")), ("repositoryId", s("fork"))],
    );
    assert!(errors.contains("GitRepositoryExists"), "{errors}");

    // the fork is cloned and pushed to (the fixture's writer may push to every repository)
    let forker = Work::clone(&fixture, &base, "fork", WRITER);
    assert_eq!(forker.head(), first);
    forker.ok(&["checkout", "-b", "feature"]);
    let work = forker.commit(&[("f.txt", "feature\n")], "Feature");
    forker.ok(&["push", "origin", "feature"]);
    settled(&fixture);
    // the fork's commit is the fork's: the parent does not answer for it by id, the fork does
    assert!(reads_commit(&fixture, READER, "fork", &work));
    assert!(!reads_commit(&fixture, READER, "parent", &work));
    let (fetched, out) = writer.run(&["fetch", "origin", &work]);
    assert!(!fetched, "{out}");
    // nor does a push to the parent build on it without sending it
    let (pushed, out) = writer.run(&["push", "origin", &format!("{first}:refs/heads/probe")]);
    assert!(pushed, "{out}");
    assert_eq!(head_of(&fixture, "parent", "refs/heads/feature"), None);

    // compared across the two, then a merge request from the fork into the parent
    let compared = ok(
        &fixture,
        READER,
        &git("get#GitCompare"),
        &[
            ("repositoryId", s("parent")),
            ("base", s("main")),
            ("head", s("feature")),
            ("headRepositoryId", s("fork")),
        ],
    );
    assert_eq!(get(&compared, "headCommitId"), work);
    assert_eq!(text_of(compared.get("aheadBy").unwrap()), "1");
    let opened = ok(
        &fixture,
        READER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("parent")),
            ("sourceRepositoryId", s("fork")),
            ("sourceRef", s("feature")),
            ("title", s("Feature from a fork")),
        ],
    );
    let request_id = get(&opened, "mergeRequestId").to_string();
    assert_eq!(text_of(opened.get("mergeRequestNum").unwrap()), "1");
    assert_eq!(
        head_of(&fixture, "parent", "refs/merge-requests/1/head").as_deref(),
        Some(work.as_str())
    );
    // its head is the parent's to read now, and to fetch
    assert!(reads_commit(&fixture, READER, "parent", &work));
    writer.ok(&["fetch", "origin", "refs/merge-requests/1/head"]);
    assert_eq!(writer.ok(&["rev-parse", "FETCH_HEAD"]).trim(), work);

    // a push to the fork's branch moves the merge request
    let more = forker.commit(&[("f.txt", "feature\nmore\n")], "More feature");
    forker.ok(&["push", "origin", "feature"]);
    settled(&fixture);
    let request = ok(
        &fixture,
        READER,
        &review("get#MergeRequest"),
        &[("mergeRequestId", s(&request_id))],
    );
    assert_eq!(
        text_of(entry(request.get("mergeRequest").unwrap(), "headCommitId")),
        more
    );
    assert_eq!(
        head_of(&fixture, "parent", "refs/merge-requests/1/head").as_deref(),
        Some(more.as_str())
    );
    let diff = ok(
        &fixture,
        READER,
        &review("get#MergeRequestDiff"),
        &[("mergeRequestId", s(&request_id))],
    );
    assert_eq!(list(&diff, "commitList").len(), 2);

    // merged into the parent only: the fork's branch stays, as the reader asked for nothing else
    let merged = ok(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(&request_id))],
    );
    assert!(!get(&merged, "commitId").is_empty());
    writer.ok(&["pull", "origin", "main"]);
    assert_eq!(
        std::fs::read_to_string(writer.dir.join("f.txt")).unwrap(),
        "feature\nmore\n"
    );
    assert_eq!(
        head_of(&fixture, "fork", "refs/heads/feature").as_deref(),
        Some(more.as_str())
    );
    assert_eq!(
        head_of(&fixture, "fork", "refs/heads/main").as_deref(),
        Some(first.as_str())
    );
    settled(&fixture);

    // the fork has commits its parent does not reach; a collection of the network keeps them
    forker.ok(&["checkout", "-b", "private"]);
    let private = forker.commit(&[("p.txt", "private\n")], "Private");
    forker.ok(&["push", "origin", "private"]);
    settled(&fixture);
    assert!(!reads_commit(&fixture, READER, "parent", &private));
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        if kind == "db" {
            let store = crate::db::DbStore::new(fixture.entity.clone(), "parent", crate::db::object_cache());
            store.stamp_all(jiff::Timestamp::now().as_millisecond() - 30 * 24 * 60 * 60 * 1000);
        }
        fixture
            .entity
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .update_all([("ecaStatus".to_string(), Value::string("done"))])
            .unwrap();
        let mut options = crate::gc::GcOptions::from_conf(false, std::time::Duration::from_secs(120));
        options.reflog_millis = 0;
        options.prune_millis = 1;
        for (id, outcome) in provider.gc(None, &options).unwrap() {
            let report = outcome.unwrap_or_else(|err| panic!("gc of {id}: {err}"));
            assert_eq!(
                report.deleted, 0,
                "{id}: every object is reached by the parent or the fork: {report:?}"
            );
        }
    }
    let again = fixture.dir.join("fork-again");
    git_ok(
        &fixture.dir,
        &forker.home,
        &["clone", &url_with(&base, WRITER, "fork"), &again.to_string_lossy()],
    );
    assert_eq!(
        git_ok(&again, &forker.home, &["rev-parse", "origin/private"]).trim(),
        private
    );

    // usage: the fork's is what it added, not what it shares
    let usage: BTreeMap<String, i64> = list(
        &ok(
            &fixture,
            WRITER,
            &git("get#GitRepositoryUsage"),
            &[("repositoryIdList", Value::List(vec![s("parent"), s("fork")]))],
        ),
        "usageList",
    )
    .iter()
    .map(|usage| {
        (
            text_of(entry(usage, "repositoryId")),
            text_of(entry(usage, "storedBytes")).parse().unwrap_or(-1),
        )
    })
    .collect();
    assert!(usage["parent"] > 0 && usage["fork"] > 0, "{usage:?}");
    if kind == "db" {
        let count = |id: &str| -> i64 {
            let out = ok(
                &fixture,
                WRITER,
                &git("get#GitRepositoryUsage"),
                &[("repositoryId", s(id))],
            );
            text_of(entry(&list(&out, "usageList")[0], "objectCount"))
                .parse()
                .unwrap()
        };
        // First and the merge commit are the parent's; the two feature commits and the private one the fork's
        assert_eq!(count("fork"), 9, "three commits, each with its tree and blob");
        assert!(count("parent") >= 4);
    }

    // a repository with forks stays; its fork goes alone, and the parent is whole after it
    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("delete#GitRepository"),
        &[("repositoryId", s("parent"))],
    );
    assert!(errors.contains("GitHasForks"), "{errors}");
    ok(
        &fixture,
        WRITER,
        &git("delete#GitRepository"),
        &[("repositoryId", s("fork"))],
    );
    let (cloned, out) = git_in(
        &fixture.dir,
        &forker.home,
        &["ls-remote", &url_with(&base, WRITER, "fork")],
    );
    assert!(!cloned, "{out}");
    let whole = fixture.dir.join("parent-again");
    git_ok(
        &fixture.dir,
        &writer.home,
        &["clone", &url_with(&base, WRITER, "parent"), &whole.to_string_lossy()],
    );
    assert_eq!(std::fs::read_to_string(whole.join("f.txt")).unwrap(), "feature\nmore\n");
    git_ok(&whole, &writer.home, &["fsck", "--full"]);
    ok(
        &fixture,
        WRITER,
        &git("delete#GitRepository"),
        &[("repositoryId", s("parent"))],
    );
}

#[test]
fn a_db_fork_shares_its_parents_objects_and_keeps_its_own_refs() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    fork_round("db");
}

#[test]
fn a_local_fork_reads_its_parents_directory_and_keeps_its_own_objects() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    fork_round("local");
}

/// With `review-start-hidden` a proposal is hidden from the moment it is there, until the application shows it.
#[test]
fn a_proposal_starts_hidden_when_the_conf_says_so() {
    let fixture = fixture_with(
        "review-hidden",
        moqui_core::context::framework_dir(),
        r#"<git-server review-start-hidden="true"/>"#,
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "held", "db");
    let files = |text: &str| Value::Map([("a.txt".to_string(), s(text))].into_iter().collect());
    ok(
        &fixture,
        WRITER,
        &git("commit#GitFiles"),
        &[
            ("repositoryId", s("held")),
            ("message", s("First")),
            ("fileMap", files("one\n")),
        ],
    );
    let proposed = ok(
        &fixture,
        READER,
        &git("commit#GitFiles"),
        &[
            ("repositoryId", s("held")),
            ("asProposal", s("true")),
            ("message", s("Second")),
            ("fileMap", files("two\n")),
        ],
    );
    let (commit, request_id) = (get(&proposed, "commitId"), get(&proposed, "mergeRequestId"));
    assert!(!commit.is_empty() && !request_id.is_empty());
    let found = merge_requests(&fixture, "held");
    assert!(!field(&found[0], "hiddenDate").is_empty());
    assert_eq!(field(&found[0], "headCommitId"), commit);
    assert_eq!(field(&found[0], "mergeableEnumId"), "GmaMergeable");
    assert!(!reads_commit(&fixture, WRITER, "held", commit));
    let (_, errors) = call(
        &fixture,
        READER,
        &review("get#MergeRequest"),
        &[("mergeRequestId", s(request_id))],
    );
    assert!(errors.contains("No merge request"), "{errors}");
    let (shown, errors) = internal(
        &fixture,
        READER,
        &review("get#MergeRequest"),
        &[("mergeRequestId", s(request_id))],
    );
    assert!(
        errors.trim().is_empty() && shown.contains_key("mergeRequest"),
        "{errors}"
    );
    // shown by the application: read as any other
    let (_, errors) = internal(
        &fixture,
        READER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(request_id)), ("hidden", s("false"))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    assert!(reads_commit(&fixture, WRITER, "held", commit));
    ok(
        &fixture,
        READER,
        &review("get#MergeRequest"),
        &[("mergeRequestId", s(request_id))],
    );
    settled(&fixture);
}
