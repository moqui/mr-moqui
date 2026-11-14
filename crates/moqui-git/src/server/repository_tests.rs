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

//! A repository's usage, delete, blame and archive through the services, the pack size a pre-receive rule is told,
//! and the service asked before a Git LFS upload (plans/Dogfood.md DG2: N-USAGE, N-DELETE, N-BLAME, N-ARCHIVE,
//! N-PRERECEIVE, N-LFS-HOOK), on the in-memory database against the git command line. Each test with git is skipped
//! when git is not installed.

use std::collections::BTreeMap;
use std::io::Read;

use moqui_lib::object::Value;

use super::hosting_tests::{call, entry, get, grant_entities, list, ok, s, text_of, Work};
use super::tests::{
    create, define, fixture_in, fixture_with, git_available, http, http_server, settled, user, Fixture, PASSWORD,
    READER, WRITER,
};
use moqui_core::entity::EntityValue;

const GIT: &str = "org.moqui.git.GitServices";

/// A user with no right on any repository.
const STRANGER: &str = "git.stranger";

fn service(name: &str) -> String {
    format!("{GIT}.{name}")
}

fn number(value: &Value) -> i64 {
    text_of(value).parse().unwrap_or(-1)
}

/// How many rows of `entity` have `field` equal to `value`.
fn rows(fixture: &Fixture, entity: &str, field: &str, value: &str) -> i64 {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find(entity)
        .unwrap()
        .eq(field, Value::string(value))
        .count()
        .unwrap()
}

/// The usage of one repository as `user` sees it; `None` when it is left out.
fn usage(fixture: &Fixture, user: &str, id: &str) -> Option<BTreeMap<String, Value>> {
    let out = ok(
        fixture,
        user,
        &service("get#GitRepositoryUsage"),
        &[("repositoryId", s(id))],
    );
    match list(&out, "usageList").first() {
        Some(Value::Map(found)) => Some(found.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
        _ => None,
    }
}

/// Bytes that do not compress, so a pack of them is about as large.
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

#[test]
fn usage_is_current_after_a_push_and_a_deleted_repository_is_gone() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("repo-usage", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    define(&fixture, "depot", "db");
    // a repository on disk under the runtime directory (the crate's, in a test), as one with no localPath is
    let disk_path = format!("tmp/git-delete-test-{}/disk.git", std::process::id());
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        user(&fixture.entity, STRANGER);
        create(
            &fixture.entity,
            crate::GIT_REPOSITORY,
            &[
                ("repositoryId", "disk"),
                ("repositoryKind", "local"),
                ("localPath", &disk_path),
            ],
        );
    }
    // one outside it, which a delete leaves where it is
    define(&fixture, "outside", "local");
    let base = http_server(fixture.context.factory());

    let empty = usage(&fixture, READER, "depot").expect("a viewer sees usage");
    assert_eq!((number(&empty["storedBytes"]), number(&empty["objectCount"])), (0, 0));

    // each push shows at once, on both kinds
    let mut works = Vec::new();
    for id in ["depot", "disk", "outside"] {
        let work = Work::clone(&fixture, &base, id, WRITER);
        work.commit(&[("a.txt", "one\n")], "First");
        work.ok(&["push", "origin", "main"]);
        let first = usage(&fixture, READER, id).unwrap();
        assert!(number(&first["storedBytes"]) > 0, "{id}: {first:?}");
        work.commit(&[("big.txt", &noise(40_000))], "Second");
        work.ok(&["push", "origin", "main"]);
        let second = usage(&fixture, READER, id).unwrap();
        assert!(
            number(&second["storedBytes"]) >= number(&first["storedBytes"]) + 30_000,
            "{id}: {first:?} then {second:?}"
        );
        if id == "depot" {
            assert_eq!(text_of(&second["repositoryKind"]), "db");
            assert_eq!(number(&first["objectCount"]), 3, "a commit, a tree and a file");
            assert_eq!(number(&second["objectCount"]), 6);
        } else {
            assert_eq!(text_of(&second["repositoryKind"]), "local");
            assert_eq!(second["objectCount"], Value::Null);
        }
        works.push(work);
    }
    settled(&fixture);

    // LFS objects count apart; a list answers in its order with totals, and leaves out what is not there
    let oid = "a".repeat(64);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let mut row = EntityValue::make(&fixture.entity, super::lfs::GIT_LFS_OBJECT).unwrap();
        row.set("repositoryId", s("depot"));
        row.set("oid", s(&oid));
        row.set("objectSize", Value::I64(1234));
        row.create(&fixture.entity).unwrap();
    }
    let both = ok(
        &fixture,
        READER,
        &service("get#GitRepositoryUsage"),
        &[("repositoryIdList", s("disk, nosuch, depot"))],
    );
    let ids: Vec<String> = list(&both, "usageList")
        .iter()
        .map(|usage| text_of(entry(usage, "repositoryId")))
        .collect();
    assert_eq!(ids, ["disk", "depot"]);
    let depot = &list(&both, "usageList")[1];
    assert_eq!(
        (number(entry(depot, "lfsBytes")), number(entry(depot, "lfsObjectCount"))),
        (1234, 1)
    );
    assert_eq!(number(&both["totalLfsBytes"]), 1234);
    assert_eq!(
        number(&both["totalStoredBytes"]),
        number(entry(&list(&both, "usageList")[0], "storedBytes")) + number(entry(depot, "storedBytes"))
    );
    // a user who may not view a repository is told nothing of it
    assert!(usage(&fixture, STRANGER, "depot").is_none());

    // what a repository gathers: a path it had, a rule, a webhook, a status, a merge request, an LFS lock
    for path in ["team/depot", "team/store"] {
        ok(
            &fixture,
            WRITER,
            &service("set#GitRepositoryPath"),
            &[("repositoryId", s("depot")), ("repositoryPath", s(path))],
        );
    }
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let of = |key: &'static str, id: &'static str| [(key, id), ("repositoryId", "depot")];
        create(
            &fixture.entity,
            "moqui.resource.GitBranchRule",
            &of("branchRuleId", "DR1"),
        );
        create(&fixture.entity, "moqui.resource.GitWebhook", &of("webhookId", "DW1"));
        create(
            &fixture.entity,
            "moqui.resource.GitCommitStatus",
            &of("commitStatusId", "DS1"),
        );
        create(
            &fixture.entity,
            "moqui.resource.GitMergeRequest",
            &of("mergeRequestId", "DM1"),
        );
        create(
            &fixture.entity,
            "moqui.resource.GitMergeRequestReviewer",
            &[("mergeRequestId", "DM1"), ("userId", READER)],
        );
        create(&fixture.entity, super::lfs::GIT_LFS_LOCK, &of("lockId", "DL1"));
    }
    let answers = |path: &str| {
        let url = format!("{base}/git/{path}.git").replace("://", &format!("://{WRITER}:{PASSWORD}@"));
        works[0].run(&["ls-remote", &url])
    };
    for path in ["depot", "team/store", "team/depot"] {
        let (found, out) = answers(path);
        assert!(found, "{path}: {out}");
    }
    let by_repository = [
        "moqui.resource.GitObject",
        "moqui.resource.GitRef",
        "moqui.resource.GitLfsObject",
        "moqui.resource.GitUpdate",
        "moqui.resource.GitBranchRule",
        "moqui.resource.GitWebhook",
        "moqui.resource.GitCommitStatus",
        "moqui.resource.GitMergeRequest",
        "moqui.resource.GitLfsLock",
        "moqui.resource.GitRepositoryPathRedirect",
        "moqui.resource.GitRepository",
    ];
    for entity in by_repository {
        assert!(
            rows(&fixture, entity, "repositoryId", "depot") > 0,
            "no {entity} to delete"
        );
    }
    let update_ids: Vec<String> = {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let updates = fixture
            .entity
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .eq("repositoryId", s("depot"))
            .list()
            .unwrap();
        updates
            .iter()
            .map(|row| text_of(row.get("updateId").unwrap()))
            .collect()
    };
    let update_refs = |fixture: &Fixture| -> i64 {
        update_ids
            .iter()
            .map(|id| rows(fixture, "moqui.resource.GitUpdateRef", "updateId", id))
            .sum()
    };
    assert!(update_refs(&fixture) > 0);

    // delete needs the delete right, and says nothing of a repository to one who may not view it
    let (_, refused) = call(
        &fixture,
        READER,
        &service("delete#GitRepository"),
        &[("repositoryId", s("depot"))],
    );
    assert!(refused.contains("may not delete"), "{refused}");
    let (_, refused) = call(
        &fixture,
        STRANGER,
        &service("delete#GitRepository"),
        &[("repositoryId", s("depot"))],
    );
    assert!(refused.contains("not found"), "{refused}");
    assert!(rows(&fixture, "moqui.resource.GitObject", "repositoryId", "depot") > 0);

    let deleted = ok(
        &fixture,
        WRITER,
        &service("delete#GitRepository"),
        &[("repositoryId", s("depot"))],
    );
    assert_eq!(
        deleted.get("directoryRemoved"),
        Some(&Value::Bool(false)),
        "a db repository has none"
    );
    for entity in by_repository {
        assert_eq!(rows(&fixture, entity, "repositoryId", "depot"), 0, "{entity} rows left");
    }
    assert_eq!(update_refs(&fixture), 0);
    assert_eq!(
        rows(
            &fixture,
            "moqui.resource.GitMergeRequestReviewer",
            "mergeRequestId",
            "DM1"
        ),
        0
    );
    // gone for git and the services, by its id and by the paths it had
    assert!(usage(&fixture, WRITER, "depot").is_none());
    let (_, missing) = call(
        &fixture,
        WRITER,
        &service("get#GitRefs"),
        &[("repositoryId", s("depot"))],
    );
    assert!(!missing.trim().is_empty());
    for path in ["depot", "team/store", "team/depot"] {
        let (found, out) = answers(path);
        assert!(!found, "{path} still answers: {out}");
    }
    // rows a push that was already storing left under the id go with the delete's second pass
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.resource.GitRef",
            &[("repositoryId", "depot"), ("refName", "refs/heads/late")],
        );
        crate::provider(fixture.context.factory())
            .unwrap()
            .delete_left_rows("depot");
    }
    assert_eq!(rows(&fixture, "moqui.resource.GitRef", "repositoryId", "depot"), 0);
    // the others are as they were
    assert!(rows(&fixture, "moqui.resource.GitUpdate", "repositoryId", "disk") > 0);
    // no more than a thousand repositories are asked of at once
    let many: Vec<Value> = (0..=1000).map(|n| s(&format!("r{n}"))).collect();
    let (out, refused) = call(
        &fixture,
        READER,
        &service("get#GitRepositoryUsage"),
        &[("repositoryIdList", Value::List(many))],
    );
    assert!(
        out.is_empty() && refused.contains("at most 1000 repositories"),
        "{refused}"
    );
    works[1].ok(&["fetch", "origin"]);

    // a repository on disk: its directory goes when it is under the runtime directory, and stays when it is not
    let disk_dir = std::path::Path::new(".").join(&disk_path);
    assert!(disk_dir.join("HEAD").is_file());
    let deleted = ok(
        &fixture,
        WRITER,
        &service("delete#GitRepository"),
        &[("repositoryId", s("disk"))],
    );
    assert_eq!(deleted.get("directoryRemoved"), Some(&Value::Bool(true)));
    assert!(!disk_dir.exists(), "{} is still there", disk_dir.display());
    let _ = std::fs::remove_dir_all(disk_dir.parent().unwrap());
    let outside_dir = fixture.dir.join("outside.git");
    let deleted = ok(
        &fixture,
        WRITER,
        &service("delete#GitRepository"),
        &[("repositoryId", s("outside"))],
    );
    assert_eq!(deleted.get("directoryRemoved"), Some(&Value::Bool(false)));
    assert_eq!(get(&deleted, "directoryKept"), outside_dir.to_string_lossy());
    assert!(outside_dir.join("HEAD").is_file());
    assert_eq!(
        rows(&fixture, "moqui.resource.GitRepository", "repositoryId", "outside"),
        0
    );
}

/// A row of a repository on disk at `local_path` (under the runtime directory unless absolute), with nothing there.
fn define_at(fixture: &Fixture, id: &str, local_path: &str) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    create(
        &fixture.entity,
        crate::GIT_REPOSITORY,
        &[
            ("repositoryId", id),
            ("repositoryKind", "local"),
            ("localPath", local_path),
        ],
    );
}

/// What looks like a bare repository at `dir`.
fn bare_at(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("objects")).unwrap();
    std::fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
}

#[test]
fn a_delete_removes_only_a_directory_that_is_its_own_and_a_rollback_keeps_everything() {
    let fixture = fixture_in("repo-delete-rules", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    // the runtime directory of a test is the crate's
    let runtime = std::fs::canonicalize(".").unwrap();
    let base = format!("tmp/git-delete-rules-{}", std::process::id());
    let under = |name: &str| format!("{base}/{name}");
    let at = |name: &str| runtime.join(&base).join(name);
    let delete = |id: &str| {
        ok(
            &fixture,
            WRITER,
            &service("delete#GitRepository"),
            &[("repositoryId", s(id))],
        )
    };
    let kept = |out: &BTreeMap<String, Value>| {
        (out.get("directoryRemoved") == Some(&Value::Bool(false))).then(|| get(out, "directoryKept").to_string())
    };

    // its own bare repository under the runtime directory: removed, by a path with `..` in it too
    bare_at(&at("own.git"));
    define_at(&fixture, "own", &under("x/../own.git"));
    std::fs::create_dir_all(at("x")).unwrap();
    assert_eq!(delete("own").get("directoryRemoved"), Some(&Value::Bool(true)));
    assert!(!at("own.git").exists());

    // a `..` that leaves the runtime directory, and a link that does: kept
    let outside = fixture.dir.join("outside.git");
    bare_at(&outside);
    let climb = "../".repeat(runtime.components().count());
    let dotted = format!("{climb}{}", outside.to_string_lossy().trim_start_matches('/'));
    define_at(&fixture, "dotted", &dotted);
    assert!(kept(&delete("dotted")).is_some_and(|path| path.contains("..")));
    std::os::unix::fs::symlink(&outside, at("link.git")).unwrap();
    define_at(&fixture, "linked", &under("link.git"));
    assert!(kept(&delete("linked")).is_some());
    assert!(
        outside.join("HEAD").is_file(),
        "what is outside the runtime directory stays"
    );

    // the runtime directory itself, and a directory that is no repository: kept
    define_at(&fixture, "root", ".");
    assert!(kept(&delete("root")).is_some());
    assert!(runtime.join("Cargo.toml").is_file());
    std::fs::create_dir_all(at("plain")).unwrap();
    std::fs::write(at("plain/notes.txt"), "not a repository").unwrap();
    define_at(&fixture, "plain", &under("plain"));
    assert!(kept(&delete("plain")).is_some());
    assert!(at("plain/notes.txt").is_file());

    // a directory another repository's is in, or that is in another's: kept while the other has its row
    bare_at(&at("outer.git"));
    bare_at(&at("outer.git/inner.git"));
    define_at(&fixture, "outer", &under("outer.git"));
    define_at(&fixture, "inner", &under("outer.git/inner.git"));
    define_at(&fixture, "twin", &under("outer.git/"));
    assert!(
        kept(&delete("twin")).is_some(),
        "another repository names the same directory"
    );
    assert!(kept(&delete("inner")).is_some(), "inside another repository's");
    assert!(at("outer.git/inner.git/HEAD").is_file());
    assert_eq!(
        delete("outer").get("directoryRemoved"),
        Some(&Value::Bool(true)),
        "no other row names it now"
    );
    assert!(!at("outer.git").exists());
    // one with nothing on disk has nothing to remove
    define_at(&fixture, "never", &under("never.git"));
    assert_eq!(kept(&delete("never")).as_deref(), Some(""));

    // a repository that holds a drive is refused whole
    bare_at(&at("drive.git"));
    define_at(&fixture, "drive", &under("drive.git"));
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.resource.ResourceDrive",
            &[("driveId", "D1"), ("repositoryId", "drive")],
        );
    }
    let (_, refused) = call(
        &fixture,
        WRITER,
        &service("delete#GitRepository"),
        &[("repositoryId", s("drive"))],
    );
    assert!(refused.contains("holds drive D1"), "{refused}");
    assert!(at("drive.git/HEAD").is_file() && rows(&fixture, crate::GIT_REPOSITORY, "repositoryId", "drive") == 1);

    // a delete whose transaction rolls back: the rows and the directory stay, and the repository is writable again
    bare_at(&at("back.git"));
    define_at(&fixture, "back", &under("back.git"));
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.resource.GitBranchRule",
            &[("branchRuleId", "BR1"), ("repositoryId", "back")],
        );
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let transactions = fixture.entity.transactions_for(crate::GIT_REPOSITORY).unwrap();
        let rolled = transactions.run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
            let directory = provider.delete_repository("back")?;
            assert!(
                matches!(directory, crate::storage::Directory::Removed(_)),
                "{directory:?}"
            );
            // while it goes nothing is written to it: the mark is committed, whatever this transaction does
            assert!(provider.still_writable("back").is_err());
            Err::<(), _>(moqui_lib::error::Error::new("rolled back on purpose"))
        });
        assert!(rolled.is_err());
        assert!(provider.still_writable("back").is_ok(), "the mark is set back");
    }
    assert!(
        at("back.git/HEAD").is_file(),
        "the directory goes only when the delete commits"
    );
    assert_eq!(rows(&fixture, crate::GIT_REPOSITORY, "repositoryId", "back"), 1);
    assert_eq!(
        rows(&fixture, "moqui.resource.GitBranchRule", "repositoryId", "back"),
        1
    );
    assert_eq!(delete("back").get("directoryRemoved"), Some(&Value::Bool(true)));
    assert!(!at("back.git").exists());

    let _ = std::fs::remove_dir_all(runtime.join(&base));
}

const QUOTA_RULES: &str = r#"<gitecas>
    <giteca rule-name="PackQuota" event="pre-receive">
        <condition><expression>packBytes > 30000</expression></condition>
        <actions><message error="true">over quota: this push is ${packBytes} bytes</message></actions>
    </giteca>
    <giteca rule-name="PackSize" event="pre-receive">
        <actions><message>a pack of ${packBytes} bytes from ${userId}</message></actions>
    </giteca>
</gitecas>"#;

#[test]
fn a_pre_receive_rule_is_told_the_pack_size_and_refuses_by_it() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("repo-quota", std::path::PathBuf::from("."));
    moqui_core::service::git_eca::install(fixture.context.factory(), QUOTA_RULES);
    define(&fixture, "quota", "db");
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "quota", WRITER);
    work.commit(&[("a.txt", "one\n")], "Small");
    let out = work.ok(&["push", "origin", "main"]);
    let told = out
        .split("a pack of ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|bytes| bytes.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("no pack size in: {out}"));
    assert!(told > 0 && told < 30_000, "{told}");
    assert!(out.contains(&format!("from {WRITER}")), "{out}");
    // a push over the rule's limit is refused with the rule's words, and nothing of it is kept
    let head = work.head();
    work.commit(&[("big.txt", &noise(60_000))], "Big");
    let (pushed, out) = work.run(&["push", "origin", "main"]);
    assert!(!pushed && out.contains("over quota: this push is "), "{out}");
    assert_eq!(super::tests::head_of(&fixture, "quota", "refs/heads/main"), Some(head));
    // a delete sends no pack
    work.ok(&["push", "origin", "HEAD~1:refs/heads/side"]);
    let out = work.ok(&["push", "origin", ":refs/heads/side"]);
    assert!(out.contains("a pack of 0 bytes"), "{out}");
}

/// An application's quota: 1000 bytes of new LFS objects in one request.
const LFS_CHECK: &str = r#"<services>
    <service verb="check" noun="LfsUpload">
        <implements service="org.moqui.git.GitServices.check#GitLfsUpload"/>
        <actions><script><![CDATA[
            if (Number(uploadBytes) > 1000)
                ec.message.addError('LFS quota: ' + uploadBytes + ' bytes in ' + objectCount + ' objects asked by ' + userId + ' for ' + repositoryId)
            // asked again once an object is the repository's: one of 777 bytes is past the limit by then
            if (stored === true && Number(objectList[0].size) === 777)
                ec.message.addError('LFS quota: full since this upload began')
        ]]></script></actions>
    </service>
</services>"#;

#[test]
fn an_lfs_upload_asks_the_service_the_conf_names() {
    let fixture = fixture_with(
        "repo-lfs-check",
        moqui_core::context::framework_dir(),
        r#"<git-server lfs-upload-check-service="test.GitQuota.check#LfsUpload"/>"#,
    );
    moqui_lib::env::set_property("git_lfs_token_key", "the key of the lfs check test".to_string());
    {
        let _entered = fixture.context.enter();
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/GitQuota.xml");
        std::fs::write(&stub, LFS_CHECK).unwrap();
        let failures = moqui_core::service::lint(&[
            moqui_core::context::framework_dir().join("service/org/moqui/git/GitServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    define(&fixture, "art", "db");
    let base = http_server(fixture.context.factory());
    let basic = format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{WRITER}:{PASSWORD}")
        )
    );
    let small: Vec<u8> = vec![b's'; 400];
    let large: Vec<u8> = vec![b'l'; 900];
    let (small_oid, large_oid) = (
        moqui_core::resource::blob::blob_id(&small),
        moqui_core::resource::blob::blob_id(&large),
    );
    let lfs_json = "application/vnd.git-lfs+json";
    let batch = |operation: &str, objects: &[(&str, usize)]| {
        let objects: Vec<serde_json::Value> = objects
            .iter()
            .map(|(oid, size)| serde_json::json!({ "oid": oid, "size": size }))
            .collect();
        let body = serde_json::json!({ "operation": operation, "transfers": ["basic"], "objects": objects,
            "hash_algo": "sha256" });
        let (status, _, body) = http(
            &base,
            "POST",
            "/git/art.git/info/lfs/objects/batch",
            &[
                ("Authorization", &basic),
                ("Accept", lfs_json),
                ("Content-Type", lfs_json),
            ],
            body.to_string().as_bytes(),
        );
        let reply: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        (status, reply)
    };
    let put = |oid: &str, content: &[u8]| {
        let (status, _, body) = http(
            &base,
            "PUT",
            &format!("/git/art.git/info/lfs/objects/{oid}"),
            &[("Authorization", &basic)],
            content,
        );
        (status, String::from_utf8_lossy(&body).into_owned())
    };

    // within the quota: the batch offers the upload, and the upload is taken
    let (status, reply) = batch("upload", &[(&small_oid, small.len())]);
    assert_eq!(status, 200, "{reply}");
    assert!(reply["objects"][0]["actions"]["upload"].is_object(), "{reply}");
    assert_eq!(put(&small_oid, &small).0, 200);
    // over it: the whole batch is refused with the service's words, which name what it was told
    let (status, reply) = batch(
        "upload",
        &[
            (&small_oid, small.len()),
            (&large_oid, large.len()),
            ("c".repeat(64).as_str(), 300),
        ],
    );
    assert_eq!(status, 403, "{reply}");
    let message = reply["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!("LFS quota: 1200 bytes in 2 objects asked by {WRITER} for art")),
        "the object already there is not counted: {message}"
    );
    // an upload that never asked the batch is checked too
    let big: Vec<u8> = vec![b'b'; 1500];
    let (status, body) = put(&moqui_core::resource::blob::blob_id(&big), &big);
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("LFS quota: 1500 bytes in 1 objects"), "{body}");
    assert_eq!(put(&large_oid, &large).0, 200, "900 bytes are within it");
    // a download asks nothing
    let (status, reply) = batch("download", &[(&small_oid, small.len()), (&large_oid, large.len())]);
    assert_eq!(status, 200, "{reply}");
    assert!(reply["objects"][1]["actions"]["download"].is_object(), "{reply}");

    // within the limit when it asked and past it once stored (other uploads ran beside it): refused and taken back
    let late: Vec<u8> = vec![b't'; 777];
    let late_oid = moqui_core::resource::blob::blob_id(&late);
    let (status, body) = put(&late_oid, &late);
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("full since this upload began"), "{body}");
    let (_, reply) = batch("download", &[(&late_oid, late.len())]);
    assert_eq!(reply["objects"][0]["error"]["code"], 404, "{reply}");
    // an upload that says no size cannot be checked: refused before any of it is read
    {
        use std::io::Write as _;
        let address = base.trim_start_matches("http://");
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        let oid = "d".repeat(64);
        let head = format!(
            "PUT /git/art.git/info/lfs/objects/{oid} HTTP/1.1\r\nHost: {address}\r\nAuthorization: {basic}\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 411 "), "{answer}");
    }

    // a file kept in LFS written through Moqui asks the same service
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(WRITER).unwrap();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    provider.setup_lfs("art", &["*.png"]).unwrap();
    let resources = fixture.context.factory().resource();
    let before = moqui_core::context::ExecutionContext::with_current(|ec| {
        ec.message().add_error("an error from before");
        ec.message().errors().len()
    });
    let refused = resources
        .write_bytes("git://art/big.png", &vec![b'p'; 1500])
        .unwrap_err();
    assert!(
        refused.message().contains("LFS quota: 1500 bytes in 1 objects"),
        "{}",
        refused.message()
    );
    // the service's errors were taken out again, and the one that was there stayed
    let after = moqui_core::context::ExecutionContext::with_current(|ec| {
        let errors = ec.message().errors().to_vec();
        ec.message().clear();
        errors
    });
    assert_eq!(
        (before, after),
        (Some(1), Some(vec!["an error from before".to_string()]))
    );
    assert_eq!(
        resources.location_bytes("git://art/big.png").unwrap(),
        None,
        "nothing was committed"
    );
    resources.write_bytes("git://art/ok.png", &vec![b'p'; 300]).unwrap();
    assert_eq!(
        resources
            .location_bytes("git://art/ok.png")
            .unwrap()
            .map(|bytes| bytes.len()),
        Some(300)
    );
    moqui_core::user::logout().unwrap();
}

#[test]
fn blame_and_archive_through_the_services() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("repo-blame", moqui_core::context::framework_dir());
    grant_entities(&fixture);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        user(&fixture.entity, STRANGER);
    }
    define(&fixture, "code", "db");
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "code", WRITER);
    let first = work.commit(&[("a.txt", "one\ntwo\nthree\n"), ("dir/b.txt", "b\n")], "First");
    let second = work.commit(
        &[("a.txt", "one\nTWO\nthree\n"), ("run.sh", "#!/bin/sh\n")],
        "Second\n\nIts body",
    );
    work.ok(&["update-index", "--chmod=+x", "run.sh"]);
    work.ok(&["commit", "-m", "Executable"]);
    // links (inside the tree, absolute, and climbing out of it) and a submodule
    for (name, target) in [
        ("link", "dir/b.txt"),
        ("evil", "/etc/passwd"),
        ("climb", "../../outside"),
    ] {
        std::os::unix::fs::symlink(target, work.dir.join(name)).unwrap();
    }
    // on disk too, so adding the work tree keeps the mode the index was given
    std::fs::set_permissions(
        work.dir.join("run.sh"),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    work.ok(&["add", "."]);
    work.ok(&["update-index", "--add", "--cacheinfo", &format!("160000,{first},sub")]);
    work.ok(&["commit", "-m", "Links and a submodule"]);
    work.ok(&["push", "origin", "main"]);
    let head = work.head();

    let blame = ok(
        &fixture,
        READER,
        &service("get#GitBlame"),
        &[
            ("repositoryId", s("code")),
            ("path", s("a.txt")),
            ("withLines", Value::Bool(true)),
        ],
    );
    assert_eq!(get(&blame, "commitId"), head);
    assert_eq!(number(&blame["lineCount"]), 3);
    let hunks: Vec<(i64, i64, String, String, String)> = list(&blame, "hunkList")
        .iter()
        .map(|hunk| {
            let lines = match entry(hunk, "lines") {
                Value::List(lines) => lines.iter().map(text_of).collect::<Vec<_>>().join("|"),
                _ => String::new(),
            };
            (
                number(entry(hunk, "startLine")),
                number(entry(hunk, "lineCount")),
                text_of(entry(hunk, "commitId")),
                text_of(entry(hunk, "summary")),
                lines,
            )
        })
        .collect();
    assert_eq!(
        hunks,
        [
            (1, 1, first.clone(), "First".to_string(), "one".to_string()),
            (2, 1, second.clone(), "Second".to_string(), "TWO".to_string()),
            (3, 1, first.clone(), "First".to_string(), "three".to_string()),
        ]
    );
    let hunk = &list(&blame, "hunkList")[0];
    assert_eq!(text_of(entry(hunk, "authorName")), "Test Pusher");
    assert!(!text_of(entry(hunk, "authorDate")).is_empty() && first.starts_with(&text_of(entry(hunk, "shortId"))));
    // at the first commit the second is not there; a directory has no blame; view is needed
    let at_first = ok(
        &fixture,
        READER,
        &service("get#GitBlame"),
        &[
            ("repositoryId", s("code")),
            ("revision", s(&first)),
            ("path", s("a.txt")),
        ],
    );
    assert_eq!(list(&at_first, "hunkList").len(), 1);
    assert_eq!(entry(&list(&at_first, "hunkList")[0], "lines"), &Value::Null);
    let of_dir = ok(
        &fixture,
        READER,
        &service("get#GitBlame"),
        &[("repositoryId", s("code")), ("path", s("dir"))],
    );
    assert!(of_dir.is_empty(), "{of_dir:?}");
    let (out, refused) = call(
        &fixture,
        STRANGER,
        &service("get#GitBlame"),
        &[("repositoryId", s("code")), ("path", s("a.txt"))],
    );
    assert!(out.is_empty() && !refused.trim().is_empty(), "{out:?}");

    // the archive, each entry under <name>-<short id>/
    let top = format!("code-{}", &head[..10]);
    let zip = ok(
        &fixture,
        READER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code"))],
    );
    assert_eq!(get(&zip, "fileName"), format!("{top}.zip"));
    assert_eq!(get(&zip, "contentType"), "application/zip");
    // three files and the link that stays inside; the two links that do not are left out and named
    assert_eq!((number(&zip["fileCount"]), get(&zip, "commitId")), (4, head.as_str()));
    let skipped: Vec<String> = list(&zip, "skippedList").iter().map(text_of).collect();
    assert_eq!(skipped, ["climb", "evil"]);
    assert_eq!(number(&zip["contentBytes"]), 14 + 2 + 10);
    let zip_path = std::path::PathBuf::from(get(&zip, "archiveLocation"));
    assert_eq!(
        std::fs::metadata(&zip_path).unwrap().len() as i64,
        number(&zip["archiveSize"])
    );
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let mut names: Vec<String> = archive.file_names().map(|name| name.unwrap().into_owned()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            format!("{top}/a.txt"),
            format!("{top}/dir/b.txt"),
            format!("{top}/link"),
            format!("{top}/run.sh"),
            format!("{top}/sub/"),
        ]
    );
    {
        let mut link = archive.by_name(&format!("{top}/link")).unwrap();
        assert!(link.is_symlink(), "a link is a link entry, not the file it names");
        let mut target = String::new();
        link.read_to_string(&mut target).unwrap();
        assert_eq!(target, "dir/b.txt");
    }
    assert!(archive.by_name(&format!("{top}/sub/")).unwrap().is_dir());
    let mut text = String::new();
    archive
        .by_name(&format!("{top}/a.txt"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "one\nTWO\nthree\n");
    assert_eq!(
        archive
            .by_name(&format!("{top}/run.sh"))
            .unwrap()
            .unix_mode()
            .map(|mode| mode & 0o777),
        Some(0o755)
    );
    drop(archive);
    // the same archive asked again is the same file, marked as used; another format is another file
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&zip_path)
        .unwrap()
        .set_modified(stale)
        .unwrap();
    let again = ok(
        &fixture,
        WRITER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code"))],
    );
    assert_eq!(get(&again, "archiveLocation"), get(&zip, "archiveLocation"));
    assert_eq!(number(&again["archiveSize"]), number(&zip["archiveSize"]));
    assert!(std::fs::metadata(&zip_path).unwrap().modified().unwrap() > stale + std::time::Duration::from_secs(1800));
    let whole_tar = ok(
        &fixture,
        READER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code")), ("format", s("tgz"))],
    );
    let whole_tar_path = std::path::PathBuf::from(get(&whole_tar, "archiveLocation"));
    assert_ne!(whole_tar_path, zip_path);
    {
        let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(
            std::fs::File::open(&whole_tar_path).unwrap(),
        ));
        let kinds: Vec<(String, tar::EntryType, Option<String>)> = entries
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    entry.header().entry_type(),
                    entry
                        .link_name()
                        .unwrap()
                        .map(|target| target.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(kinds.contains(&(
            format!("{top}/link"),
            tar::EntryType::Symlink,
            Some("dir/b.txt".to_string())
        )));
        assert!(kinds
            .iter()
            .any(|(path, kind, _)| path == &format!("{top}/sub/") && *kind == tar::EntryType::Directory));
        assert!(
            !kinds
                .iter()
                .any(|(path, _, _)| path.ends_with("evil") || path.ends_with("climb")),
            "{kinds:?}"
        );
    }
    // an archive not yet written is refused when the files kept would pass the budget; one that is kept is still given
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let full = crate::archive::Limits {
            max_bytes: 1 << 20,
            max_entries: 100,
            max_disk_bytes: 1,
            max_concurrent: 1,
        };
        let new_one = provider.archive("code", &first, &[], "other-top", crate::archive::Format::Zip, full);
        let refused = new_one.unwrap_err();
        assert!(
            refused.message().contains("archives kept already take"),
            "{}",
            refused.message()
        );
        let kept = provider
            .archive("code", "", &[], &top, crate::archive::Format::Zip, full)
            .unwrap()
            .unwrap();
        assert_eq!(kept.path, zip_path);
    }
    std::fs::remove_file(&whole_tar_path).unwrap();
    std::fs::remove_file(&zip_path).unwrap();

    // tar.gz of one directory, at an older revision
    let tar = ok(
        &fixture,
        READER,
        &service("get#GitArchive"),
        &[
            ("repositoryId", s("code")),
            ("revision", s(&first)),
            ("format", s("tar.gz")),
            ("path", s("dir")),
        ],
    );
    let first_top = format!("code-{}", &first[..10]);
    assert_eq!(get(&tar, "fileName"), format!("{first_top}.tar.gz"));
    let tar_path = std::path::PathBuf::from(get(&tar, "archiveLocation"));
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&tar_path).unwrap()));
    let found: Vec<(String, String)> = entries
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            (entry.path().unwrap().to_string_lossy().into_owned(), text)
        })
        .collect();
    assert_eq!(found, [(format!("{first_top}/b.txt"), "b\n".to_string())]);
    std::fs::remove_file(&tar_path).unwrap();
    // what is not there, what is not a format, and who may not view
    let nothing = ok(
        &fixture,
        READER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code")), ("path", s("a.txt"))],
    );
    assert!(nothing.is_empty(), "{nothing:?}");
    let (_, refused) = call(
        &fixture,
        READER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code")), ("format", s("rar"))],
    );
    assert!(refused.contains("zip or tar.gz"), "{refused}");
    let (out, refused) = call(
        &fixture,
        STRANGER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code"))],
    );
    assert!(out.is_empty() && !refused.trim().is_empty(), "{out:?}");
}

#[test]
fn blame_and_archive_past_the_conf_limits_are_refused() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_with(
        "repo-limits",
        moqui_core::context::framework_dir(),
        r#"<git-server blame-max-commits="1" archive-max-files="1"/>"#,
    );
    grant_entities(&fixture);
    define(&fixture, "code", "db");
    let base = http_server(fixture.context.factory());
    let work = Work::clone(&fixture, &base, "code", WRITER);
    work.commit(&[("a.txt", "one\n")], "First");
    work.commit(&[("b.txt", "b\n")], "Second");
    work.ok(&["push", "origin", "main"]);
    let (_, refused) = call(
        &fixture,
        READER,
        &service("get#GitBlame"),
        &[("repositoryId", s("code")), ("path", s("a.txt"))],
    );
    assert!(refused.contains("longer than 1 commits"), "{refused}");
    let (out, refused) = call(
        &fixture,
        READER,
        &service("get#GitArchive"),
        &[("repositoryId", s("code"))],
    );
    assert!(out.is_empty() && refused.contains("more than 1 entries"), "{refused}");
}
