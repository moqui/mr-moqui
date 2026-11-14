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

//! Forks against the git command line (plans/Dogfood.md DG4: F4, N-REACH), on the in-memory database with the
//! framework's services and gitECA rules, for a `db` and a `local` repository: a fork cloned and pushed to, a merge
//! request from it merged into its parent, a fork's commit that its parent does not answer for, a collection that
//! keeps what a fork reaches, a fork deleted and a parent that is not. Skipped when git is not installed.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use crate::server::hosting_tests::{call, entry, framework_rules, get, grant_entities, list, ok, s, text_of, Work};
use crate::server::review::tests::{git, reads_commit, review};
use crate::server::tests::{
    define, fixture_in, git_available, git_in, git_ok, head_of, http_server, settled, url_with, READER, WRITER,
};
use crate::service::commit::tests::internal;

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
    // a branch the fork will build on, and the parent will delete
    writer.ok(&["checkout", "-b", "side"]);
    writer.commit(&[("s.txt", "side\n")], "Side");
    writer.ok(&["push", "origin", "side"]);
    writer.ok(&["checkout", "main"]);
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
    assert_eq!(text_of(forked.get("refCount").unwrap()), "3");
    if kind == "local" {
        // the parent's objects by a path relative to the fork's own, so the two directories move together
        let alternates = std::fs::read_to_string(fixture.dir.join("fork.git").join("objects/info/alternates")).unwrap();
        assert!(alternates.starts_with("../"), "{alternates}");
    }
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
    // only who may write to the fork's branch offers it: it is followed from then on
    let from_fork = [
        ("repositoryId", s("parent")),
        ("sourceRepositoryId", s("fork")),
        ("sourceRef", s("feature")),
        ("title", s("Feature from a fork")),
    ];
    let (_, errors) = call(&fixture, READER, &review("create#MergeRequest"), &from_fork);
    assert!(errors.contains("may not push to fork"), "{errors}");
    let opened = ok(&fixture, WRITER, &review("create#MergeRequest"), &from_fork);
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

    // a merge request detached from its source keeps its head: the fork's later pushes do not reach the parent
    forker.ok(&["checkout", "-b", "loose", "main"]);
    let kept = forker.commit(&[("l.txt", "one\n")], "Loose one");
    forker.ok(&["push", "origin", "loose"]);
    let loose = ok(
        &fixture,
        WRITER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("parent")),
            ("sourceRepositoryId", s("fork")),
            ("sourceRef", s("loose")),
            ("title", s("Loose")),
        ],
    );
    let loose_id = get(&loose, "mergeRequestId").to_string();
    let (_, errors) = call(
        &fixture,
        READER,
        &review("detach#MergeRequestSource"),
        &[("mergeRequestId", s(&loose_id))],
    );
    assert!(!errors.trim().is_empty());
    let detached = ok(
        &fixture,
        WRITER,
        &review("detach#MergeRequestSource"),
        &[("mergeRequestId", s(&loose_id))],
    );
    assert!(detached.contains_key("sourceDetachedDate"));
    let later = forker.commit(&[("l.txt", "one\ntwo\n")], "Loose two");
    forker.ok(&["push", "origin", "loose"]);
    settled(&fixture);
    assert_eq!(
        head_of(&fixture, "parent", "refs/merge-requests/2/head").as_deref(),
        Some(kept.as_str())
    );
    assert!(!reads_commit(&fixture, READER, "parent", &later));
    ok(
        &fixture,
        WRITER,
        &review("close#MergeRequest"),
        &[("mergeRequestId", s(&loose_id))],
    );

    // the fork builds on a branch of the parent, and the parent deletes the branch: the fork's commit is in its own
    // store, and its parent commit is now reached through the fork alone
    forker.ok(&["checkout", "-b", "built", "origin/side"]);
    forker.commit(&[("s.txt", "side\nbuilt\n")], "Built on side");
    forker.ok(&["push", "origin", "built"]);
    forker.ok(&["push", "origin", ":side"]);
    writer.ok(&["push", "origin", ":side"]);
    settled(&fixture);

    // the fork has commits its parent does not reach; a collection of the network keeps them
    forker.ok(&["checkout", "-b", "private"]);
    let private = forker.commit(&[("p.txt", "private\n")], "Private");
    forker.ok(&["push", "origin", "private"]);
    settled(&fixture);
    assert!(!reads_commit(&fixture, READER, "parent", &private));
    {
        // nor is a file restored in the parent from a commit that is only the fork's
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let resources = fixture.context.factory().resource();
        let restored = resources.restore_version("git://parent/a.txt", &private, None);
        assert!(restored.is_err(), "{restored:?}");
        assert!(resources.restore_version("git://fork/a.txt", &first, None).is_ok());
    }
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
        // First, Side and the merge commit are the parent's; what the fork's pushes brought is the fork's
        let (of_fork, of_parent) = (count("fork"), count("parent"));
        assert!(of_fork >= 15, "six commits with their trees and new blobs: {of_fork}");
        assert!(of_parent >= 7, "{of_parent}");
        // a deleted fork's objects are not charged to the root while they wait for a collection
        let (extra, errors) = internal(
            &fixture,
            WRITER,
            &git("fork#GitRepository"),
            &[("sourceRepositoryId", s("parent")), ("repositoryId", s("short"))],
        );
        assert!(errors.trim().is_empty() && !extra.is_empty(), "{errors}");
        let short = Work::clone(&fixture, &base, "short", WRITER);
        short.commit(&[("x.txt", "x\n")], "Short lived");
        short.ok(&["push", "origin", "main"]);
        settled(&fixture);
        assert_eq!(count("short"), 3);
        ok(
            &fixture,
            WRITER,
            &git("delete#GitRepository"),
            &[("repositoryId", s("short"))],
        );
        assert_eq!(count("parent"), of_parent);
    }

    // a fork of the fork reads through both; a repository with forks stays
    let again_dir = fixture.dir.join("fork2.git").to_string_lossy().into_owned();
    let (_, errors) = internal(
        &fixture,
        WRITER,
        &git("fork#GitRepository"),
        &[
            ("sourceRepositoryId", s("fork")),
            ("repositoryId", s("fork2")),
            ("localPath", s(&again_dir)),
        ],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    let second = Work::clone(&fixture, &base, "fork2", WRITER);
    assert_eq!(second.ok(&["rev-parse", "origin/private"]).trim(), private);
    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("delete#GitRepository"),
        &[("repositoryId", s("fork"))],
    );
    assert!(errors.contains("GitHasForks"), "{errors}");
    ok(
        &fixture,
        WRITER,
        &git("delete#GitRepository"),
        &[("repositoryId", s("fork2"))],
    );

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

/// A file kept in LFS that a merge request from a fork adds becomes the target's LFS object too.
#[test]
fn lfs_objects_follow_a_merge_request_from_a_fork() {
    let fixture = fixture_in("fork-lfs", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "parent", "db");
    let commit = |repository: &str, message: &str, path: &str, content: Value| {
        ok(
            &fixture,
            WRITER,
            &git("commit#GitFiles"),
            &[
                ("repositoryId", s(repository)),
                ("message", s(message)),
                (
                    "fileMap",
                    Value::Map([(path.to_string(), content)].into_iter().collect()),
                ),
            ],
        )
    };
    commit("parent", "First", "a.txt", s("one\n"));
    ok(
        &fixture,
        WRITER,
        &git("fork#GitRepository"),
        &[("sourceRepositoryId", s("parent")), ("repositoryId", s("fork"))],
    );
    commit(
        "fork",
        "Track",
        ".gitattributes",
        s("*.bin filter=lfs diff=lfs merge=lfs -text\n"),
    );
    commit("fork", "Art", "art.bin", Value::Bytes(vec![7; 2000]));
    let lfs_rows = |repository: &str| {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.resource.GitLfsObject")
            .unwrap()
            .eq("repositoryId", s(repository))
            .count()
            .unwrap()
    };
    assert_eq!((lfs_rows("fork"), lfs_rows("parent")), (1, 0));
    let opened = ok(
        &fixture,
        WRITER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("parent")),
            ("sourceRepositoryId", s("fork")),
            ("sourceRef", s("main")),
            ("title", s("Art")),
        ],
    );
    assert_eq!(
        lfs_rows("parent"),
        1,
        "the pointer the merge request adds has its content in the target"
    );
    ok(
        &fixture,
        WRITER,
        &review("merge#MergeRequest"),
        &[("mergeRequestId", s(get(&opened, "mergeRequestId")))],
    );
    let art = ok(
        &fixture,
        READER,
        &git("get#GitBlob"),
        &[("repositoryId", s("parent")), ("path", s("art.bin"))],
    );
    assert_eq!(
        text_of(art.get("size").unwrap()),
        "2000",
        "the content, not the pointer"
    );
    settled(&fixture);
}
