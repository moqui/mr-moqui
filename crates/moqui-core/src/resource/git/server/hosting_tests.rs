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

//! Hosting through Moqui against the git command line, on the in-memory database with the framework's services and
//! gitECA rules: branch rules refusing pushes, an archived repository, the tree, refs, compare and the four merge
//! methods (a conflict refused and listed), merge requests through reviews, checks and a moved source, webhooks
//! signed and retried, and commit statuses over REST with an access token. Each test is skipped when git is not
//! installed.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use moqui_lib::object::{to_plain_string, Value};

use super::tests::{
    create, define, fixture_in, fixture_with, git_available, git_in, git_ok, head_of, http_server, settled, url_with,
    user, write_file, Fixture, READER, WRITER,
};
use crate::context::ExecutionContext;
use crate::entity::EntityValue;

/// Every entity for both test users, as a screen's inherited authorization gives them; the repository rights stay
/// AT_GIT_REPOSITORY's.
fn grant_entities(fixture: &Fixture) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    create(
        &fixture.entity,
        "moqui.security.ArtifactGroup",
        &[("artifactGroupId", "ENTITY_ALL")],
    );
    create(
        &fixture.entity,
        "moqui.security.ArtifactGroupMember",
        &[
            ("artifactGroupId", "ENTITY_ALL"),
            ("artifactName", ".*"),
            ("artifactTypeEnumId", "AT_ENTITY"),
            ("nameIsPattern", "Y"),
            ("inheritAuthz", "Y"),
        ],
    );
    for group in ["GIT_WRITERS", "GIT_READERS"] {
        create(
            &fixture.entity,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", &format!("ENT_{group}")),
                ("userGroupId", group),
                ("artifactGroupId", "ENTITY_ALL"),
                ("authzTypeEnumId", "AUTHZT_ALWAYS"),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
    }
}

/// The framework's own gitECA rules (`service/Git.gitecas.xml`), which a test factory without components does not
/// load: merge requests follow their branches, webhooks follow pushes.
fn framework_rules(fixture: &Fixture) {
    let xml = std::fs::read_to_string(crate::context::framework_dir().join("service/Git.gitecas.xml")).unwrap();
    crate::service::git_eca::install(fixture.context.factory(), &xml);
}

/// Call a service as `user`: its result and the errors it left.
fn call(fixture: &Fixture, user: &str, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    let _entered = fixture.context.enter();
    crate::user::internal_login_user(user).unwrap();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let result = crate::service::sync(name, &parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors_text().join("\n");
        ec.message().clear();
        text
    })
    .unwrap_or_default();
    crate::user::logout().unwrap();
    match result {
        Ok(out) => (out, errors),
        Err(err) => (BTreeMap::new(), format!("{errors}\n{err}")),
    }
}

fn ok(fixture: &Fixture, user: &str, name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let (out, errors) = call(fixture, user, name, parameters);
    assert!(errors.trim().is_empty(), "{name}: {errors}");
    out
}

/// A second user with every repository action, beside the fixture's writer and reader.
const APPROVER: &str = "git.approver";

/// Set a reviewer's row of a merge request straight in the database (an approval left from before F132, or of a user
/// who has since lost the push right).
fn set_review(fixture: &Fixture, mr_id: &str, user: &str, status: &str) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mr = fixture
        .entity
        .find("moqui.resource.GitMergeRequest")
        .unwrap()
        .eq("mergeRequestId", s(mr_id))
        .one()
        .unwrap()
        .unwrap();
    let mut row = fixture
        .entity
        .find("moqui.resource.GitMergeRequestReviewer")
        .unwrap()
        .eq("mergeRequestId", s(mr_id))
        .eq("userId", s(user))
        .one()
        .unwrap()
        .unwrap();
    row.set("statusId", s(status));
    row.set(
        "reviewedCommitId",
        mr.get("headCommitId").cloned().unwrap_or(Value::Null),
    );
    row.update(&fixture.entity).unwrap();
}

fn s(text: &str) -> Value {
    Value::string(text)
}

fn get<'a>(out: &'a BTreeMap<String, Value>, key: &str) -> &'a str {
    out.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn list<'a>(out: &'a BTreeMap<String, Value>, key: &str) -> &'a [Value] {
    match out.get(key) {
        Some(Value::List(items)) => items,
        _ => &[],
    }
}

fn entry<'a>(value: &'a Value, key: &str) -> &'a Value {
    match value {
        Value::Map(entries) => entries.get(key).unwrap_or(&Value::Null),
        _ => &Value::Null,
    }
}

fn text_of(value: &Value) -> String {
    to_plain_string(value)
}

fn row(fixture: &Fixture, entity: &str, field: &str, id: &str) -> EntityValue {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find(entity)
        .unwrap()
        .eq(field, Value::string(id))
        .one()
        .unwrap()
        .unwrap_or_else(|| panic!("no {entity} {id}"))
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

/// A clone of a repository with one commit on main, pushed.
struct Work {
    dir: std::path::PathBuf,
    home: std::path::PathBuf,
}

impl Work {
    fn clone(fixture: &Fixture, base: &str, id: &str, user: &str) -> Self {
        let home = fixture.dir.join(format!("home-{id}-{user}"));
        std::fs::create_dir_all(&home).unwrap();
        let dir = fixture.dir.join(format!("work-{id}-{user}"));
        git_ok(
            &fixture.dir,
            &home,
            &["clone", &url_with(base, user, id), &dir.to_string_lossy()],
        );
        Self { dir, home }
    }

    fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
        for (name, text) in files {
            write_file(&self.dir, name, text);
        }
        git_ok(&self.dir, &self.home, &["add", "."]);
        git_ok(&self.dir, &self.home, &["commit", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        git_ok(&self.dir, &self.home, &["rev-parse", "HEAD"]).trim().to_string()
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        git_in(&self.dir, &self.home, args)
    }

    fn ok(&self, args: &[&str]) -> String {
        git_ok(&self.dir, &self.home, args)
    }
}

const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";

#[test]
fn branch_rules_refuse_pushes_and_an_archived_repository_refuses_all() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("hosting-rules", crate::context::framework_dir());
    grant_entities(&fixture);
    define(&fixture, "guarded", "db");
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "guarded", WRITER);
    work.commit(&[("a.txt", BASE)], "First");
    work.ok(&["push", "origin", "main"]);

    // main changes only through a merge request; release branches are neither forced nor deleted
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.store#GitBranchRule",
        &[
            ("repositoryId", s("guarded")),
            ("refPattern", s("main")),
            ("requireMergeRequest", s("Y")),
        ],
    );
    let release = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.store#GitBranchRule",
        &[("repositoryId", s("guarded")), ("refPattern", s("release/*"))],
    );
    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.store#GitBranchRule",
        &[("repositoryId", s("guarded")), ("refPattern", s("*"))],
    );
    assert!(errors.contains("may not change the branch rules"), "{errors}");

    work.commit(&[("a.txt", &BASE.replace("one", "ONE"))], "Direct to main");
    let (pushed, out) = work.run(&["push", "origin", "main"]);
    assert!(
        !pushed && out.contains("protected branch: main changes through a merge request"),
        "{out}"
    );
    let main = head_of(&fixture, "guarded", "refs/heads/main");
    work.ok(&["push", "origin", "HEAD:refs/heads/topic"]);
    work.ok(&["push", "origin", "HEAD~1:refs/heads/release/1"]);
    work.ok(&["push", "origin", "HEAD:refs/heads/release/1"]);
    work.ok(&["commit", "--amend", "-m", "Rewritten"]);
    let (pushed, out) = work.run(&["push", "--force", "origin", "HEAD:refs/heads/release/1"]);
    assert!(!pushed && out.contains("may not be force-pushed"), "{out}");
    let (pushed, out) = work.run(&["push", "origin", "--delete", "release/1"]);
    assert!(!pushed && out.contains("may not be deleted"), "{out}");
    // the services keep the same rules
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.delete#GitRef",
        &[("repositoryId", s("guarded")), ("refName", s("release/1"))],
    );
    assert!(errors.contains("may not be deleted"), "{errors}");
    assert_eq!(head_of(&fixture, "guarded", "refs/heads/main"), main);

    // the release rule allows deletes now; the writers may push to main directly
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.store#GitBranchRule",
        &[
            ("branchRuleId", s(get(&release, "branchRuleId"))),
            ("repositoryId", s("guarded")),
            ("refPattern", s("release/*")),
            ("allowDelete", s("Y")),
        ],
    );
    work.ok(&["push", "origin", "--delete", "release/1"]);
    let rules = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.get#GitBranchRules",
        &[("repositoryId", s("guarded"))],
    );
    let main_rule = list(&rules, "ruleList")
        .iter()
        .find(|rule| text_of(entry(rule, "refPattern")) == "main")
        .map(|rule| text_of(entry(rule, "branchRuleId")))
        .unwrap();
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.store#GitBranchRule",
        &[
            ("branchRuleId", s(&main_rule)),
            ("repositoryId", s("guarded")),
            ("refPattern", s("main")),
            ("requireMergeRequest", s("Y")),
            ("pushUserGroupId", s("GIT_WRITERS")),
        ],
    );
    work.ok(&["push", "--force", "origin", "HEAD:refs/heads/topic"]);
    work.ok(&["pull", "--rebase", "origin", "main"]);
    let (pushed, out) = work.run(&["push", "origin", "HEAD:main"]);
    assert!(pushed, "a member of the push group pushes to main: {out}");

    // archived: no push, no ref write; open again, it takes both
    let archived = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.archive#GitRepository",
        &[("repositoryId", s("guarded"))],
    );
    assert_eq!(text_of(archived.get("closedMergeRequestCount").unwrap()), "0");
    work.commit(&[("b.txt", "b\n")], "After the archive");
    let (pushed, out) = work.run(&["push", "origin", "HEAD:refs/heads/topic"]);
    assert!(!pushed && out.contains("403"), "{out}");
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.create#GitRef",
        &[("repositoryId", s("guarded")), ("refName", s("late"))],
    );
    assert!(errors.contains("archived"), "{errors}");
    let listed = ok(&fixture, READER, "org.moqui.impl.GitServices.find#GitRepositories", &[]);
    assert!(
        list(&listed, "repositoryList").is_empty(),
        "an archived repository is left out"
    );
    let listed = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.find#GitRepositories",
        &[("includeArchived", Value::Bool(true))],
    );
    assert_eq!(list(&listed, "repositoryList").len(), 1);
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.unarchive#GitRepository",
        &[("repositoryId", s("guarded"))],
    );
    work.ok(&["push", "origin", "HEAD:refs/heads/topic"]);
    settled(&fixture);
}

#[test]
fn the_tree_refs_compare_and_merge_methods_through_services() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("hosting-merge", crate::context::framework_dir());
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "code", "db");
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "code", WRITER);
    work.commit(&[("a.txt", BASE), ("dir/b.txt", "b\n")], "First");
    let second = work.commit(&[("dir/c.txt", "c\n")], "Second");
    work.ok(&["push", "origin", "main"]);

    // the tree: directories first, each entry with its last commit
    let tree = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.get#GitTree",
        &[("repositoryId", s("code"))],
    );
    let names: Vec<String> = list(&tree, "entryList")
        .iter()
        .map(|e| text_of(entry(e, "name")))
        .collect();
    assert_eq!(names, ["dir", "a.txt"]);
    let dir_last = text_of(entry(entry(&list(&tree, "entryList")[0], "lastCommit"), "commitId"));
    assert_eq!(dir_last, second, "dir changed last in the second commit");
    let file_last = text_of(entry(entry(&list(&tree, "entryList")[1], "lastCommit"), "summary"));
    assert_eq!(file_last, "First");
    assert_eq!(get(&tree, "branch"), "main");
    let sub = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.get#GitTree",
        &[
            ("repositoryId", s("code")),
            ("path", s("dir")),
            ("revision", s(&second[..12])),
        ],
    );
    assert_eq!(list(&sub, "entryList").len(), 2);

    // refs: the reader may not make one; a branch and an annotated tag
    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.create#GitRef",
        &[("repositoryId", s("code")), ("refName", s("feature"))],
    );
    assert!(errors.contains("may not create"), "{errors}");
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.create#GitRef",
        &[("repositoryId", s("code")), ("refName", s("feature"))],
    );
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.create#GitRef",
        &[
            ("repositoryId", s("code")),
            ("refName", s("v1")),
            ("refType", s("tag")),
            ("message", s("Version one")),
        ],
    );
    let tags = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.get#GitRefs",
        &[("repositoryId", s("code")), ("refType", s("tag"))],
    );
    assert_eq!(list(&tags, "refList").len(), 1);
    assert_eq!(entry(&list(&tags, "refList")[0], "annotated"), &Value::Bool(true));
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.create#GitRef",
        &[("repositoryId", s("code")), ("refName", s("feature"))],
    );
    assert!(errors.contains("already exists"), "{errors}");

    // diverge feature (first line) and main (last line)
    work.ok(&["fetch", "origin"]);
    work.ok(&["checkout", "-b", "feature", "origin/feature"]);
    work.commit(&[("a.txt", &BASE.replace("one", "ONE"))], "Feature: first line");
    work.ok(&["push", "origin", "feature"]);
    work.ok(&["checkout", "main"]);
    work.commit(&[("a.txt", &BASE.replace("seven", "SEVEN"))], "Main: last line");
    work.ok(&["push", "origin", "main"]);

    let compared = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.get#GitCompare",
        &[
            ("repositoryId", s("code")),
            ("base", s("main")),
            ("head", s("feature")),
            ("withDiffs", Value::Bool(true)),
            ("checkMergeable", Value::Bool(true)),
        ],
    );
    assert_eq!(text_of(compared.get("aheadBy").unwrap()), "1");
    assert_eq!(text_of(compared.get("behindBy").unwrap()), "1");
    assert_eq!(get(&compared, "mergeBaseId"), second);
    assert_eq!(compared.get("mergeable"), Some(&Value::Bool(true)));
    let files = list(&compared, "fileList");
    assert_eq!(files.len(), 1);
    assert_eq!(text_of(entry(&files[0], "path")), "a.txt");
    assert!(text_of(entry(&files[0], "html")).contains("moqui-diff"));
    let hunk = &match entry(&files[0], "hunks") {
        Value::List(hunks) => hunks.clone(),
        other => panic!("{other:?}"),
    }[0];
    let lines = match entry(hunk, "lines") {
        Value::List(lines) => lines.clone(),
        other => panic!("{other:?}"),
    };
    assert!(lines
        .iter()
        .any(|line| text_of(entry(line, "text")).contains("ONE") && text_of(entry(line, "newNo")) == "1"));

    // squash: one commit on main with both changes
    let main_before = head_of(&fixture, "code", "refs/heads/main").unwrap();
    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s("feature")),
            ("target", s("main")),
        ],
    );
    assert!(errors.contains("may not update"), "{errors}");
    let squashed = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s("feature")),
            ("target", s("main")),
            ("method", s("GmmSquash")),
            ("message", s("Feature, squashed")),
        ],
    );
    let head = head_of(&fixture, "code", "refs/heads/main").unwrap();
    assert_eq!(get(&squashed, "commitId"), head);
    work.ok(&["fetch", "origin"]);
    assert_eq!(
        work.ok(&["rev-parse", "origin/main^"]).trim(),
        main_before,
        "one parent, the old main"
    );
    let text = work.ok(&["show", "origin/main:a.txt"]);
    assert!(
        text.starts_with("ONE\n") && text.trim_end().ends_with("SEVEN"),
        "{text}"
    );

    // a conflict: refused, and a trial merge lists the paths
    work.ok(&["checkout", "-b", "clash", "origin/main"]);
    work.commit(
        &[(
            "a.txt",
            &BASE
                .replace("one", "ONE")
                .replace("seven", "SEVEN")
                .replace("four", "four, clash"),
        )],
        "Clash",
    );
    work.ok(&["push", "origin", "clash"]);
    work.ok(&["checkout", "main"]);
    work.ok(&["reset", "--hard", "origin/main"]);
    work.commit(
        &[(
            "a.txt",
            &BASE
                .replace("one", "ONE")
                .replace("seven", "SEVEN")
                .replace("four", "four, main"),
        )],
        "Main four",
    );
    work.ok(&["push", "origin", "main"]);
    let trial = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s("clash")),
            ("target", s("main")),
            ("checkOnly", Value::Bool(true)),
        ],
    );
    assert_eq!(trial.get("mergeable"), Some(&Value::Bool(false)));
    assert_eq!(
        list(&trial, "conflictPaths").iter().map(text_of).collect::<Vec<_>>(),
        ["a.txt"]
    );
    let main_now = head_of(&fixture, "code", "refs/heads/main");
    let (refused, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s("clash")),
            ("target", s("main")),
        ],
    );
    assert!(errors.contains("conflicts in a.txt"), "{errors}");
    assert_eq!(list(&refused, "conflictPaths").len(), 1);
    assert_eq!(head_of(&fixture, "code", "refs/heads/main"), main_now, "nothing moved");

    // rebase: the source's commits replayed on main, committed by the writer
    work.ok(&["checkout", "-b", "topic", "origin/main~1"]);
    work.commit(&[("t1.txt", "1\n")], "Topic one");
    work.commit(&[("t2.txt", "2\n")], "Topic two");
    work.ok(&["push", "origin", "topic"]);
    let rebased = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s("topic")),
            ("target", s("main")),
            ("method", s("GmmRebase")),
        ],
    );
    assert_eq!(list(&rebased, "madeCommitIds").len(), 2);
    work.ok(&["fetch", "origin"]);
    let log = work.ok(&["log", "--format=%s|%an|%cn", "-3", "origin/main"]);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines[0], "Topic two|Test Pusher|Test git.writer", "{log}");
    assert_eq!(lines[1], "Topic one|Test Pusher|Test git.writer", "{log}");
    assert_eq!(lines[2], "Main four|Test Pusher|Test Pusher", "{log}");
    // a merge commit of a branch main already has is refused; a fast-forward of one it lacks is made
    let main_parent = work.ok(&["rev-parse", "origin/main~1"]).trim().to_string();
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s(&main_parent)),
            ("target", s("main")),
        ],
    );
    assert!(errors.contains("already has"), "{errors}");
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.create#GitRef",
        &[
            ("repositoryId", s("code")),
            ("refName", s("behind")),
            ("revision", s(&main_parent)),
        ],
    );
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.merge#GitRefs",
        &[
            ("repositoryId", s("code")),
            ("source", s("main")),
            ("target", s("behind")),
            ("method", s("GmmFastForward")),
        ],
    );
    assert_eq!(
        head_of(&fixture, "code", "refs/heads/behind"),
        head_of(&fixture, "code", "refs/heads/main")
    );

    // deletes, and the push log of the writes
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.delete#GitRef",
        &[("repositoryId", s("code")), ("refName", s("feature"))],
    );
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.delete#GitRef",
        &[("repositoryId", s("code")), ("refName", s("main"))],
    );
    assert!(errors.contains("default branch"), "{errors}");
    settled(&fixture);
    let updates = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.find#GitUpdates",
        &[
            ("repositoryId", s("code")),
            ("source", s("write")),
            ("pageSize", Value::I64(50)),
        ],
    );
    let writes = list(&updates, "updateList");
    assert!(writes.len() >= 7, "{writes:?}");
    let delete = &writes[0];
    assert_eq!(text_of(entry(delete, "username")), WRITER);
    assert!(
        matches!(entry(delete, "refList"), Value::List(refs) if text_of(entry(&refs[0], "changeType")) == "delete")
    );
}

#[test]
fn merge_requests_follow_reviews_checks_and_pushes() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("hosting-review", crate::context::framework_dir());
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "review", "db");
    // a second user who may push, to approve the writer's merge request
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        user(&fixture.entity, APPROVER);
        let mut row = EntityValue::make(&fixture.entity, "moqui.security.UserGroupMember").unwrap();
        row.set("userId", s(APPROVER));
        row.set("userGroupId", s("GIT_WRITERS"));
        row.set(
            "fromDate",
            moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value,
        );
        row.create(&fixture.entity).unwrap();
    }
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "review", WRITER);
    work.commit(&[("a.txt", BASE)], "First");
    work.ok(&["push", "origin", "main"]);
    work.ok(&["checkout", "-b", "topic"]);
    work.commit(&[("t.txt", "topic\n")], "Topic work");
    work.ok(&["push", "origin", "topic"]);
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.store#GitBranchRule",
        &[
            ("repositoryId", s("review")),
            ("refPattern", s("main")),
            ("requireMergeRequest", s("Y")),
            ("requiredApprovals", Value::I64(1)),
            ("dismissStaleApprovals", s("Y")),
            ("requiredStatusContexts", s("ci/build")),
        ],
    );

    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("review")),
            ("sourceRef", s("topic")),
            ("title", s("Topic")),
        ],
    );
    assert!(errors.contains("may not push"), "{errors}");
    let created = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("review")),
            ("sourceRef", s("topic")),
            ("title", s("Topic work")),
            ("deleteSourceBranch", Value::Bool(true)),
            ("reviewerUserIdList", Value::List(vec![s(READER)])),
        ],
    );
    let mr_id = get(&created, "mergeRequestId").to_string();
    assert_eq!(text_of(created.get("mergeRequestNum").unwrap()), "1");
    let mr = row(&fixture, "moqui.resource.GitMergeRequest", "mergeRequestId", &mr_id);
    assert_eq!(field(&mr, "statusId"), "GmrOpen");
    assert_eq!(field(&mr, "targetRef"), "main");
    assert_eq!(field(&mr, "mergeableEnumId"), "GmaMergeable");
    assert_eq!(field(&mr, "headCommitId"), work.head());
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("review")),
            ("sourceRef", s("topic")),
            ("title", s("Again")),
        ],
    );
    assert!(errors.contains("already open"), "{errors}");

    let waiting = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.find#MergeRequests",
        &[("waitingForUserId", s(READER))],
    );
    assert_eq!(list(&waiting, "mergeRequestList").len(), 1);
    let detail = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.get#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    let missing: Vec<String> = list(&detail, "missingList").iter().map(text_of).collect();
    assert!(missing.iter().any(|m| m.contains("0 of 1 approvals")), "{missing:?}");
    assert!(missing.iter().any(|m| m.contains("ci/build")), "{missing:?}");

    // not yet: no approval, then no check
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.merge#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    assert!(errors.contains("0 of 1 approvals"), "{errors}");
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.submit#MergeRequestReview",
        &[("mergeRequestId", s(&mr_id)), ("decision", s("approve"))],
    );
    assert!(errors.contains("your own"), "{errors}");
    // F132: a viewer reviews but does not approve, and an approval of one who may not push does not count
    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.submit#MergeRequestReview",
        &[("mergeRequestId", s(&mr_id)), ("decision", s("approve"))],
    );
    assert!(errors.contains("right to push"), "{errors}");
    ok(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.submit#MergeRequestReview",
        &[
            ("mergeRequestId", s(&mr_id)),
            ("decision", s("comment")),
            ("note", s("Looks good to me")),
        ],
    );
    set_review(&fixture, &mr_id, READER, "GmrrApproved");
    let detail = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.get#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    assert_eq!(
        text_of(detail.get("approvalCount").unwrap()),
        "0",
        "a viewer's approval row does not count"
    );
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.merge#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    assert!(errors.contains("0 of 1 approvals"), "{errors}");
    ok(
        &fixture,
        APPROVER,
        "org.moqui.impl.GitReviewServices.submit#MergeRequestReview",
        &[
            ("mergeRequestId", s(&mr_id)),
            ("decision", s("approve")),
            ("note", s("Looks good")),
        ],
    );
    let approvals = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.get#MergeRequestApprovals",
        &[("mergeRequestId", s(&mr_id))],
    );
    let approvers: Vec<String> = list(&approvals, "approverUserIdList").iter().map(text_of).collect();
    assert_eq!(approvers, vec![APPROVER.to_string()]);
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.merge#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    assert!(errors.contains("checks not passed: ci/build"), "{errors}");

    // the source moves: the head follows and the approval is dismissed
    let moved = work.commit(&[("t.txt", "topic, more\n")], "More topic work");
    work.ok(&["push", "origin", "topic"]);
    settled(&fixture);
    let mr = row(&fixture, "moqui.resource.GitMergeRequest", "mergeRequestId", &mr_id);
    assert_eq!(field(&mr, "headCommitId"), moved);
    let reviewer = {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.resource.GitMergeRequestReviewer")
            .unwrap()
            .eq("mergeRequestId", s(&mr_id))
            .eq("userId", s(APPROVER))
            .one()
            .unwrap()
            .unwrap()
    };
    assert_eq!(
        field(&reviewer, "statusId"),
        "GmrrRequested",
        "a stale approval is dismissed"
    );

    // the check comes from CI, the approval again; then it merges and the source goes
    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.create#GitCommitStatus",
        &[
            ("repositoryId", s("review")),
            ("commitId", s(&moved)),
            ("state", s("success")),
            ("context", s("ci/build")),
        ],
    );
    assert!(errors.contains("push right"), "{errors}");
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.create#GitCommitStatus",
        &[
            ("repositoryId", s("review")),
            ("commitId", s(&moved)),
            ("state", s("success")),
            ("context", s("ci/build")),
            ("target_url", s("https://ci.example.com/run/1")),
        ],
    );
    ok(
        &fixture,
        APPROVER,
        "org.moqui.impl.GitReviewServices.submit#MergeRequestReview",
        &[("mergeRequestId", s(&mr_id)), ("decision", s("approve"))],
    );
    let detail = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.get#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    assert_eq!(detail.get("canMerge"), Some(&Value::Bool(true)), "{detail:?}");
    let merged = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.merge#MergeRequest",
        &[("mergeRequestId", s(&mr_id))],
    );
    let main = head_of(&fixture, "review", "refs/heads/main").unwrap();
    assert_eq!(get(&merged, "commitId"), main);
    assert_eq!(
        head_of(&fixture, "review", "refs/heads/topic"),
        None,
        "the source branch is deleted"
    );
    let mr = row(&fixture, "moqui.resource.GitMergeRequest", "mergeRequestId", &mr_id);
    assert_eq!(field(&mr, "statusId"), "GmrMerged");
    assert_eq!(field(&mr, "mergeCommitId"), main);
    assert_eq!(field(&mr, "mergedByUserId"), WRITER);
    settled(&fixture);
    let diff = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitReviewServices.get#MergeRequestDiff",
        &[("mergeRequestId", s(&mr_id))],
    );
    assert_eq!(list(&diff, "fileList").len(), 1, "{diff:?}");

    // a second request is !2; closed and reopened; merged when its head reaches the target by a push
    work.ok(&["fetch", "origin"]);
    work.ok(&["checkout", "-b", "second", &main]);
    work.commit(&[("s.txt", "second\n")], "Second work");
    work.ok(&["push", "origin", "second"]);
    let created = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("review")),
            ("sourceRef", s("second")),
            ("title", s("Second")),
            ("draft", Value::Bool(true)),
        ],
    );
    assert_eq!(text_of(created.get("mergeRequestNum").unwrap()), "2");
    let second_id = get(&created, "mergeRequestId").to_string();
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.merge#MergeRequest",
        &[("mergeRequestId", s(&second_id))],
    );
    assert!(errors.contains("draft"), "{errors}");
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.close#MergeRequest",
        &[("mergeRequestId", s(&second_id))],
    );
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitReviewServices.reopen#MergeRequest",
        &[("mergeRequestId", s(&second_id))],
    );
    // the writers may push to main directly: the request is merged when main holds its head
    {
        let rules = ok(
            &fixture,
            READER,
            "org.moqui.impl.GitServices.get#GitBranchRules",
            &[("repositoryId", s("review")), ("refName", s("main"))],
        );
        let id = text_of(entry(&list(&rules, "ruleList")[0], "branchRuleId"));
        ok(
            &fixture,
            WRITER,
            "org.moqui.impl.GitServices.delete#GitBranchRule",
            &[("branchRuleId", s(&id))],
        );
    }
    work.ok(&["push", "origin", "second:main"]);
    settled(&fixture);
    let second = row(&fixture, "moqui.resource.GitMergeRequest", "mergeRequestId", &second_id);
    assert_eq!(field(&second, "statusId"), "GmrMerged");
    assert_eq!(field(&second, "mergeableEnumId"), "GmaMerged");
}

/// A listener that records each request's headers and body and answers with the statuses given, in turn (the last
/// one after that).
/// The requests a [`webhook_receiver`] took: headers and body.
type Seen = Arc<Mutex<Vec<(BTreeMap<String, String>, String)>>>;

fn webhook_receiver(statuses: Vec<u16>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let recorded = Arc::clone(&seen);
    std::thread::spawn(move || {
        for (index, stream) in listener.incoming().flatten().enumerate() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut headers = BTreeMap::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            recorded
                .lock()
                .unwrap()
                .push((headers, String::from_utf8_lossy(&body).into_owned()));
            let status = statuses.get(index).or(statuses.last()).copied().unwrap_or(200);
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            );
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/hook"), seen)
}

fn produced(fixture: &Fixture) -> Vec<String> {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find("moqui.service.message.SystemMessage")
        .unwrap()
        .eq("systemMessageTypeId", s("GitWebhook"))
        .eq("statusId", s("SmsgProduced"))
        .list()
        .unwrap()
        .iter()
        .map(|row| field(row, "systemMessageId"))
        .collect()
}

#[test]
fn webhooks_are_signed_logged_and_retried() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_with(
        "hosting-hooks",
        crate::context::framework_dir(),
        r#"<git-server webhook-private-network="true"/>"#,
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "hooked", "db");
    {
        // the seed data of moqui.resource.GitWebhook and the SystemMessage status flow, which the test database does not
        // load
        crate::testing::seed_status_flows(&fixture.entity);
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.service.message.SystemMessageType",
            &[
                ("systemMessageTypeId", "GitWebhook"),
                ("sendServiceName", "org.moqui.impl.GitServices.send#GitWebhookDelivery"),
            ],
        );
    }
    let (url, seen) = webhook_receiver(vec![500, 200]);
    let stored = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.store#GitWebhook",
        &[
            ("repositoryId", s("hooked")),
            ("url", s(&url)),
            ("events", s("push,tag")),
            ("secret", s("s3cret")),
        ],
    );
    let webhook_id = get(&stored, "webhookId").to_string();
    let (_, errors) = call(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.store#GitWebhook",
        &[("repositoryId", s("hooked")), ("url", s(&url))],
    );
    assert!(errors.contains("may not change the webhooks"), "{errors}");
    let hook = row(&fixture, "moqui.resource.GitWebhook", "webhookId", &webhook_id);
    assert_eq!(
        field(&hook, "secret"),
        "s3cret",
        "the secret reads back, kept encrypted"
    );

    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "hooked", WRITER);
    let head = work.commit(&[("a.txt", "a\n")], "First");
    work.ok(&["push", "origin", "main"]);
    settled(&fixture);
    // the delivery is queued by the post-receive rule; it is sent here, as the job would
    let queued = produced(&fixture);
    assert_eq!(queued.len(), 1, "one push event");
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &[("systemMessageId", s(&queued[0]))],
    );
    assert!(errors.is_empty() || errors.contains("500"), "{errors}");
    let message = row(
        &fixture,
        "moqui.service.message.SystemMessage",
        "systemMessageId",
        &queued[0],
    );
    assert_eq!(field(&message, "statusId"), "SmsgProduced", "a 500 leaves it to retry");
    assert_eq!(field(&message, "failCount"), "1");
    assert_eq!(
        field(
            &row(&fixture, "moqui.resource.GitWebhook", "webhookId", &webhook_id),
            "lastDeliveryStatus"
        ),
        "500"
    );
    ok(
        &fixture,
        WRITER,
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &[("systemMessageId", s(&queued[0]))],
    );
    let message = row(
        &fixture,
        "moqui.service.message.SystemMessage",
        "systemMessageId",
        &queued[0],
    );
    assert_eq!(field(&message, "statusId"), "SmsgSent");
    assert_eq!(field(&message, "remoteMessageId"), "200");

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let (headers, body) = &requests[1];
    assert_eq!(headers.get("x-moqui-event").map(String::as_str), Some("push"));
    assert_eq!(
        headers.get("x-hub-signature-256"),
        Some(&crate::service::git_hosting_signature("s3cret", body.as_bytes())),
        "signed with the secret"
    );
    let payload: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(payload["ref"], "refs/heads/main");
    assert_eq!(payload["after"], head.as_str());
    assert_eq!(payload["created"], true);
    assert_eq!(payload["commits"][0]["message"].as_str().map(str::trim), Some("First"));
    assert_eq!(payload["repository"]["name"], "hooked");

    let deliveries = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.find#GitWebhookDeliveries",
        &[("webhookId", s(&webhook_id))],
    );
    let delivery = &list(&deliveries, "deliveryList")[0];
    assert_eq!(text_of(entry(delivery, "statusId")), "SmsgSent");
    assert!(matches!(entry(delivery, "errorList"), Value::List(errors) if errors.len() == 1));

    let tested = ok(
        &fixture,
        WRITER,
        "org.moqui.impl.GitServices.test#GitWebhook",
        &[("webhookId", s(&webhook_id))],
    );
    assert_eq!(tested.get("delivered"), Some(&Value::Bool(true)), "{tested:?}");
    assert_eq!(get(&tested, "statusCode"), "200");
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.0.get("x-moqui-event").map(String::as_str), Some("ping"));
}

#[test]
fn commit_statuses_post_over_rest_with_an_access_token() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("hosting-rest", crate::context::framework_dir());
    grant_entities(&fixture);
    define(&fixture, "ci", "db");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.security.ArtifactGroup",
            &[("artifactGroupId", "MOQUI_API_GIT")],
        );
        create(
            &fixture.entity,
            "moqui.security.ArtifactGroupMember",
            &[
                ("artifactGroupId", "MOQUI_API_GIT"),
                ("artifactName", "/moqui/git"),
                ("artifactTypeEnumId", "AT_REST_PATH"),
                ("nameIsPattern", "N"),
                ("inheritAuthz", "Y"),
            ],
        );
        create(
            &fixture.entity,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "MOQUI_API_GIT_USERS"),
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "MOQUI_API_GIT"),
                ("authzTypeEnumId", "AUTHZT_ALLOW"),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
    }
    let rest_file = crate::context::framework_dir()
        .parent()
        .unwrap()
        .join("tools/service/moqui.rest.xml");
    crate::service::install_rest(fixture.context.factory(), &[rest_file]);
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "ci", WRITER);
    let head = work.commit(&[("a.txt", "a\n")], "First");
    work.ok(&["push", "origin", "main"]);
    let token_of = |user: &str| {
        let _entered = fixture.context.enter();
        crate::user::internal_login_user(user).unwrap();
        let key = crate::user::get_login_key(Some(1.0)).unwrap();
        crate::user::logout().unwrap();
        key
    };
    let path = ["moqui", "git", "ci", "commits", head.as_str(), "statuses"];
    let post = |token: Option<&str>, state: &str| {
        let _entered = fixture.context.enter();
        if let Some(token) = token {
            crate::user::login_user_key(token).unwrap();
        }
        let parameters: BTreeMap<String, Value> = [
            ("state", s(state)),
            ("target_url", s("https://ci.example.com/build/7")),
            ("description", s("Build")),
            ("context", s("ci/build")),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
        let posted = crate::service::run(&path, "post", &parameters);
        let errors = ExecutionContext::with_current(|ec| {
            let text = ec.message().errors_text().join("\n");
            ec.message().clear();
            text
        })
        .unwrap_or_default();
        let listed = crate::service::run(&path, "get", &BTreeMap::new());
        let _ = crate::user::logout();
        (posted, errors, listed)
    };

    let (posted, errors, listed) = post(Some(&token_of(WRITER)), "success");
    let posted = posted.unwrap();
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(text_of(entry(&posted, "state")), "success");
    assert_eq!(text_of(entry(&posted, "target_url")), "https://ci.example.com/build/7");
    assert_eq!(text_of(entry(&posted, "creator")), WRITER);
    let listed = listed.unwrap();
    assert_eq!(text_of(entry(&listed, "state")), "success");

    // a viewer's token may not post one; no token is asked to log in
    let (posted, errors, _) = post(Some(&token_of(READER)), "failure");
    assert!(posted.is_err() || errors.contains("push right"), "{posted:?} {errors}");
    let (posted, _, _) = post(None, "failure");
    assert!(posted.is_err(), "{posted:?}");
    let statuses = ok(
        &fixture,
        READER,
        "org.moqui.impl.GitServices.get#GitCommitStatuses",
        &[
            ("repositoryId", s("ci")),
            ("commitId", s("main")),
            ("allStatuses", Value::Bool(true)),
        ],
    );
    assert_eq!(list(&statuses, "statusList").len(), 1, "only the writer's status");
}
