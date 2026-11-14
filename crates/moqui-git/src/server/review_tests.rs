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

//! Push to review against the git command line (plans/Dogfood.md DG4: F5, N-REACH), on the in-memory database with
//! the framework's services and gitECA rules: a reader pushes `refs/for/main` and gets a merge request, pushes again
//! and updates it, and can push nothing else; the head is fetched by name, not advertised, and hidden when the
//! application says so; the caps; what a push may name without bringing it. A test with git is skipped when git is
//! not installed.

use moqui_lib::object::Value;

use crate::server::hosting_tests::{call, framework_rules, get, grant_entities, list, ok, s, text_of, Work};
use crate::server::tests::{
    define, fixture_with, git_available, git_in, git_ok, head_of, http_server, settled, url_with, Fixture, READER,
    WRITER,
};
use crate::service::commit::tests::internal;
use moqui_core::entity::EntityValue;

const GIT: &str = "org.moqui.git.GitServices";
const REVIEW: &str = "org.moqui.git.GitReviewServices";
const MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";

pub(crate) fn git(name: &str) -> String {
    format!("{GIT}.{name}")
}

pub(crate) fn review(name: &str) -> String {
    format!("{REVIEW}.{name}")
}

/// The merge requests of a repository, by number.
pub(crate) fn merge_requests(fixture: &Fixture, id: &str) -> Vec<EntityValue> {
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

pub(crate) fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(text_of).unwrap_or_default()
}

/// Text that does not compress, so a pack of it is about as large.
pub(crate) fn noise(len: usize) -> String {
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
pub(crate) fn reads_commit(fixture: &Fixture, user: &str, repository: &str, id: &str) -> bool {
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
    // the push log names it only to who may push: a reader is told no id no ref reaches, and no address
    let logged = |user: &str| -> (Vec<String>, bool) {
        let out = ok(
            &fixture,
            user,
            &git("find#GitUpdates"),
            &[("repositoryId", s("proj")), ("pageNoLimit", s("true"))],
        );
        let mut ids = Vec::new();
        let mut addressed = false;
        for update in list(&out, "updateList") {
            addressed |= !text_of(crate::server::hosting_tests::entry(update, "remoteAddress")).is_empty();
            if let Value::List(refs) = crate::server::hosting_tests::entry(update, "refList") {
                for reference in refs {
                    for key in ["oldObjectId", "newObjectId"] {
                        ids.push(text_of(crate::server::hosting_tests::entry(reference, key)));
                    }
                }
            }
        }
        (ids, addressed)
    };
    let (ids, addressed) = logged(WRITER);
    assert!(ids.contains(&second) && addressed, "{ids:?}");
    let (ids, addressed) = logged(READER);
    assert!(!ids.contains(&second) && ids.contains(&first) && !addressed, "{ids:?}");
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

    // closed without merging: its head ref is let go after the retention time, not before
    ok(
        &fixture,
        READER,
        &review("close#MergeRequest"),
        &[("mergeRequestId", s(request_id))],
    );
    let release = || {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        crate::provider(fixture.context.factory())
            .unwrap()
            .release_closed_heads(&fixture.entity, "held")
            .unwrap()
    };
    assert_eq!(release(), 0);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let long_ago = moqui_lib::object::basic_convert(
            &Value::I64(jiff::Timestamp::now().as_millisecond() - 40 * 24 * 60 * 60 * 1000),
            "Timestamp",
        )
        .value;
        fixture
            .entity
            .find(MERGE_REQUEST)
            .unwrap()
            .eq("mergeRequestId", s(request_id))
            .update_all([("closedDate".to_string(), long_ago)])
            .unwrap();
    }
    assert_eq!(release(), 1);
    assert_eq!(head_of(&fixture, "held", "refs/merge-requests/1/head"), None);
    assert!(!reads_commit(&fixture, READER, "held", commit));

    // a merge request from a fork starts hidden too: its commits come from outside as a push for review's do
    ok(
        &fixture,
        WRITER,
        &git("fork#GitRepository"),
        &[("sourceRepositoryId", s("held")), ("repositoryId", s("held-fork"))],
    );
    let forked = ok(
        &fixture,
        WRITER,
        &git("commit#GitFiles"),
        &[
            ("repositoryId", s("held-fork")),
            ("message", s("In the fork")),
            ("fileMap", files("fork\n")),
        ],
    );
    let opened = ok(
        &fixture,
        WRITER,
        &review("create#MergeRequest"),
        &[
            ("repositoryId", s("held")),
            ("sourceRepositoryId", s("held-fork")),
            ("sourceRef", s("main")),
            ("title", s("From the fork")),
        ],
    );
    let from_fork = merge_requests(&fixture, "held")
        .into_iter()
        .find(|row| field(row, "mergeRequestId") == get(&opened, "mergeRequestId"))
        .unwrap();
    assert!(!field(&from_fork, "hiddenDate").is_empty());
    assert_eq!(field(&from_fork, "headCommitId"), get(&forked, "commitId"));
    assert!(!reads_commit(&fixture, READER, "held", get(&forked, "commitId")));
    settled(&fixture);
}

fn pkt(line: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", line.len() + 4).into_bytes();
    out.extend_from_slice(line);
    out
}

/// One object of a pack made by hand: whole, or a delta against an object the pack does not hold.
enum Packed {
    Whole(gix::objs::Kind, Vec<u8>),
    RefDelta(gix::ObjectId, Vec<u8>),
}

/// A pack of `objects`, as a client that writes its own sends it.
fn pack_of(objects: &[Packed]) -> Vec<u8> {
    use std::io::Write;
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&u32::try_from(objects.len()).unwrap().to_be_bytes());
    for object in objects {
        let (kind, data, base) = match object {
            Packed::Whole(gix::objs::Kind::Commit, data) => (1u8, data, None),
            Packed::Whole(gix::objs::Kind::Tree, data) => (2, data, None),
            Packed::Whole(gix::objs::Kind::Blob, data) => (3, data, None),
            Packed::Whole(gix::objs::Kind::Tag, data) => (4, data, None),
            Packed::RefDelta(base, delta) => (7, delta, Some(base)),
        };
        let mut size = data.len();
        let mut byte = (kind << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size > 0 {
            pack.push(byte | 0x80);
            byte = (size & 0x7f) as u8;
            size >>= 7;
        }
        pack.push(byte);
        if let Some(base) = base {
            pack.extend_from_slice(base.as_bytes());
        }
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        pack.extend_from_slice(&encoder.finish().unwrap());
    }
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    hasher.update(&pack);
    let digest = hasher.try_finalize().unwrap();
    pack.extend_from_slice(digest.as_bytes());
    pack
}

fn commit_data(tree: &gix::ObjectId, parent: &str, message: &str) -> Vec<u8> {
    let who = "Other <other@example.com> 1700000000 +0000";
    format!("tree {tree}\nparent {parent}\nauthor {who}\ncommitter {who}\n\n{message}\n").into_bytes()
}

fn tree_data(entries: &[(&str, gix::ObjectId)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (name, id) in entries {
        data.extend_from_slice(format!("100644 {name}").as_bytes());
        data.push(0);
        data.extend_from_slice(id.as_bytes());
    }
    data
}

fn id_of(kind: gix::objs::Kind, data: &[u8]) -> gix::ObjectId {
    crate::store::object_id(kind, data).unwrap()
}

/// A push of `new` to `refs/for/main` of the repository `proj` by `user`, as a client of its own writes it: the
/// client's `shallow` commits, and a pack of `objects`. What the server reported.
fn push_for_review(base: &str, user: &str, new: &gix::ObjectId, shallow: &[&str], objects: &[Packed]) -> String {
    let mut body = Vec::new();
    for commit in shallow {
        body.extend(pkt(format!("shallow {commit}\n").as_bytes()));
    }
    let mut command = format!("{} {new} refs/for/main", "0".repeat(40)).into_bytes();
    command.push(0);
    command.extend_from_slice(b"report-status\n");
    body.extend(pkt(&command));
    body.extend_from_slice(b"0000");
    body.extend(pack_of(objects));
    let authorization = format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{user}:{}", crate::server::tests::PASSWORD)
        )
    );
    let (status, _, answer) = crate::server::tests::http(
        base,
        "POST",
        "/git/proj.git/git-receive-pack",
        &[
            ("Authorization", &authorization),
            ("Content-Type", "application/x-git-receive-pack-request"),
        ],
        &body,
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&answer));
    String::from_utf8_lossy(&answer).into_owned()
}

/// A push names what it does not bring by id only: a commit as a parent, a blob as a tree's entry or as the base of
/// a delta. What its pusher may not read is refused, whatever the client says of itself (that it is shallow).
#[test]
fn a_push_names_only_what_its_pusher_reads() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_with("review-names", moqui_core::context::framework_dir(), "");
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "proj", "db");
    const OTHER: &str = "git.other";
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        crate::server::tests::user(&fixture.entity, OTHER);
        let mut row = EntityValue::make(&fixture.entity, "moqui.security.UserGroupMember").unwrap();
        row.set("userId", s(OTHER));
        row.set("userGroupId", s("GIT_READERS"));
        row.set(
            "fromDate",
            moqui_lib::object::basic_convert(&Value::I64(1_000), "Timestamp").value,
        );
        row.create(&fixture.entity).unwrap();
    }
    let base = http_server(fixture.context.factory());
    let writer = Work::clone(&fixture, &base, "proj", WRITER);
    let main = writer.commit(&[("a.txt", "one\n")], "First");
    writer.ok(&["push", "origin", "main"]);
    // a reader's push for review that the application holds: its commit and its file are nobody else's to read
    let reader = Work::clone(&fixture, &base, "proj", READER);
    let held = reader.commit(&[("secret.txt", "secret\n")], "Held");
    reader.ok(&["push", "origin", "HEAD:refs/for/main"]);
    let request_id = field(&merge_requests(&fixture, "proj")[0], "mergeRequestId");
    ok(
        &fixture,
        WRITER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id))],
    );
    settled(&fixture);
    let public_blob = id_of(gix::objs::Kind::Blob, b"one\n");
    let secret_blob = id_of(gix::objs::Kind::Blob, b"secret\n");
    let main_tree = tree_data(&[("a.txt", public_blob)]);
    let main_tree_id = id_of(gix::objs::Kind::Tree, &main_tree);
    let refused = |answer: &str| answer.contains("ng refs/for/main") && !answer.contains("ok refs/for/main");

    // the held commit as a parent, with a client that says the parent is where its history is cut
    let on_held = commit_data(&main_tree_id, &held, "On the held commit");
    let on_held_id = id_of(gix::objs::Kind::Commit, &on_held);
    for shallow in [vec![held.as_str()], Vec::new()] {
        let answer = push_for_review(
            &base,
            OTHER,
            &on_held_id,
            &shallow,
            &[Packed::Whole(gix::objs::Kind::Commit, on_held.clone())],
        );
        assert!(refused(&answer), "shallow {shallow:?}: {answer}");
    }

    // the held file as an entry of a pushed tree
    let with_secret = tree_data(&[("a.txt", public_blob), ("stolen.txt", secret_blob)]);
    let with_secret_id = id_of(gix::objs::Kind::Tree, &with_secret);
    let names_blob = commit_data(&with_secret_id, &main, "Names a blob");
    let answer = push_for_review(
        &base,
        OTHER,
        &id_of(gix::objs::Kind::Commit, &names_blob),
        &[],
        &[
            Packed::Whole(gix::objs::Kind::Tree, with_secret),
            Packed::Whole(gix::objs::Kind::Commit, names_blob),
        ],
    );
    assert!(refused(&answer), "{answer}");

    // the held file as the base of a delta: a new blob that is its content and a letter more
    let copied = b"secret\nx".to_vec();
    let copied_id = id_of(gix::objs::Kind::Blob, &copied);
    let delta = vec![7, 8, 0x90, 7, 1, b'x'];
    let with_copy = tree_data(&[("a.txt", public_blob), ("copy.txt", copied_id)]);
    let with_copy_id = id_of(gix::objs::Kind::Tree, &with_copy);
    let by_delta = commit_data(&with_copy_id, &main, "By a delta");
    let answer = push_for_review(
        &base,
        OTHER,
        &id_of(gix::objs::Kind::Commit, &by_delta),
        &[],
        &[
            Packed::RefDelta(secret_blob, delta.clone()),
            Packed::Whole(gix::objs::Kind::Tree, with_copy),
            Packed::Whole(gix::objs::Kind::Commit, by_delta),
        ],
    );
    assert!(refused(&answer), "{answer}");
    assert_eq!(
        merge_requests(&fixture, "proj").len(),
        1,
        "none of them opened a merge request"
    );

    // the same of a file of the branch the push is for is taken: its entry, and a delta against it
    let public_copy = b"one\nx".to_vec();
    let public_copy_id = id_of(gix::objs::Kind::Blob, &public_copy);
    let fine = tree_data(&[
        ("a.txt", public_blob),
        ("b.txt", public_blob),
        ("copy.txt", public_copy_id),
    ]);
    let fine_id = id_of(gix::objs::Kind::Tree, &fine);
    let fine_commit = commit_data(&fine_id, &main, "Names what it reads");
    let fine_commit_id = id_of(gix::objs::Kind::Commit, &fine_commit);
    let answer = push_for_review(
        &base,
        OTHER,
        &fine_commit_id,
        &[],
        &[
            Packed::RefDelta(public_blob, vec![4, 5, 0x90, 4, 1, b'x']),
            Packed::Whole(gix::objs::Kind::Tree, fine),
            Packed::Whole(gix::objs::Kind::Commit, fine_commit),
        ],
    );
    assert!(answer.contains("ok refs/for/main"), "{answer}");
    let found = merge_requests(&fixture, "proj");
    assert_eq!(found.len(), 2);
    assert_eq!(field(&found[1], "headCommitId"), fine_commit_id.to_string());
    assert!(!reads_commit(&fixture, OTHER, "proj", &held));
    settled(&fixture);
}
