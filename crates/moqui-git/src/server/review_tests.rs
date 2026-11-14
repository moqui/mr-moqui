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
    // nor by who works in the repository: what was held is not a commit its refs have left
    assert!(!reads_commit(&fixture, WRITER, "held", commit));

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
    push_refs(base, user, &[("refs/for/main", *new)], shallow, objects)
}

/// A push by `user` to the repository `proj` that makes each ref of `commands` at an id, as a client of its own
/// writes it: the client's `shallow` commits, and a pack of `objects`. What the server reported.
fn push_refs(
    base: &str,
    user: &str,
    commands: &[(&str, gix::ObjectId)],
    shallow: &[&str],
    objects: &[Packed],
) -> String {
    let mut body = Vec::new();
    for commit in shallow {
        body.extend(pkt(format!("shallow {commit}\n").as_bytes()));
    }
    for (index, (name, new)) in commands.iter().enumerate() {
        let mut command = format!("{} {new} {name}", "0".repeat(40)).into_bytes();
        if index == 0 {
            command.push(0);
            command.extend_from_slice(b"report-status");
        }
        command.push(b'\n');
        body.extend(pkt(&command));
    }
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

fn tag_data(target: &gix::ObjectId, name: &str) -> Vec<u8> {
    format!("object {target}\ntype commit\ntag {name}\ntagger Other <other@example.com> 1700000000 +0000\n\n{name}\n")
        .into_bytes()
}

fn commit_files(fixture: &Fixture, user: &str, repository: &str, extra: &[(&str, Value)]) -> String {
    let mut parameters = vec![("repositoryId", s(repository))];
    parameters.extend(extra.iter().cloned());
    get(&ok(fixture, user, &git("commit#GitFiles"), &parameters), "commitId").to_string()
}

fn file_map(pairs: &[(&str, &str)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(path, text)| ((*path).to_string(), s(text)))
            .collect(),
    )
}

/// The new value of a ref, like everything else a push names and does not bring, is the pusher's to read or the
/// push is refused, and an object that is another's is answered as one that is not there: for a ref's value, the
/// base of a delta and a parent the client says its history is cut at.
#[test]
fn a_ref_is_made_only_at_what_its_pusher_reads() {
    use gix::objs::Kind;
    let fixture = fixture_with("review-values", moqui_core::context::framework_dir(), "");
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "proj", "db");
    let base = http_server(fixture.context.factory());
    let main = commit_files(
        &fixture,
        WRITER,
        "proj",
        &[("message", s("First")), ("fileMap", file_map(&[("a.txt", "one\n")]))],
    );
    let main_id = gix::ObjectId::from_hex(main.as_bytes()).unwrap();
    let public_blob = id_of(Kind::Blob, b"one\n");
    let main_tree_id = id_of(Kind::Tree, &tree_data(&[("a.txt", public_blob)]));

    // a reader's push for review that the application holds: a commit, its directory, its file, and a tag of it
    let secret_blob = id_of(Kind::Blob, b"secret\n");
    let held_tree = tree_data(&[("a.txt", public_blob), ("secret.txt", secret_blob)]);
    let held_tree_id = id_of(Kind::Tree, &held_tree);
    let held = commit_data(&held_tree_id, &main, "Held");
    let held_id = id_of(Kind::Commit, &held);
    let held_tag = tag_data(&held_id, "held");
    let held_tag_id = id_of(Kind::Tag, &held_tag);
    let answer = push_for_review(
        &base,
        READER,
        &held_id,
        &[],
        &[
            Packed::Whole(Kind::Blob, b"secret\n".to_vec()),
            Packed::Whole(Kind::Tree, held_tree),
            Packed::Whole(Kind::Commit, held),
            Packed::Whole(Kind::Tag, held_tag),
        ],
    );
    assert!(answer.contains("ok refs/for/main"), "{answer}");
    let request_id = field(&merge_requests(&fixture, "proj")[0], "mergeRequestId");
    ok(
        &fixture,
        WRITER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id))],
    );
    settled(&fixture);

    // a tag at each of them, by a user who may make tags and by one who may not, with no object sent: refused, and
    // in the words an id that names nothing is refused in
    let nothing = gix::ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
    for user in [WRITER, READER] {
        let tagged =
            |id: gix::ObjectId| push_refs(&base, user, &[("refs/tags/x", id)], &[], &[]).replace(&id.to_string(), "ID");
        let absent = tagged(nothing);
        assert!(
            absent.contains("ng refs/tags/x missing necessary objects"),
            "{user}: {absent}"
        );
        for foreign in [secret_blob, held_tree_id, held_tag_id, held_id] {
            assert_eq!(tagged(foreign), absent, "{user} {foreign}");
        }
    }
    assert_eq!(head_of(&fixture, "proj", "refs/tags/x"), None);

    // what the pusher reads is taken: a file and a directory of the branch, and a tag object under a second name
    let answer = push_refs(
        &base,
        WRITER,
        &[("refs/tags/file", public_blob), ("refs/tags/dir", main_tree_id)],
        &[],
        &[],
    );
    assert!(
        answer.contains("ok refs/tags/file") && answer.contains("ok refs/tags/dir"),
        "{answer}"
    );
    let tag = tag_data(&main_id, "v1");
    let tag_id = id_of(Kind::Tag, &tag);
    let answer = push_refs(
        &base,
        WRITER,
        &[("refs/tags/v1", tag_id)],
        &[],
        &[Packed::Whole(Kind::Tag, tag)],
    );
    assert!(answer.contains("ok refs/tags/v1"), "{answer}");
    let answer = push_refs(&base, WRITER, &[("refs/tags/v1-again", tag_id)], &[], &[]);
    assert!(answer.contains("ok refs/tags/v1-again"), "{answer}");

    // a delta against a file the pusher may not read, and against an id that names nothing: one answer
    let by_delta = |delta_base: gix::ObjectId| {
        let tree = tree_data(&[("a.txt", public_blob), ("copy.txt", id_of(Kind::Blob, b"secret\nx"))]);
        let commit = commit_data(&id_of(Kind::Tree, &tree), &main, "By a delta");
        push_refs(
            &base,
            WRITER,
            &[("refs/heads/copy", id_of(Kind::Commit, &commit))],
            &[],
            &[
                Packed::RefDelta(delta_base, vec![7, 8, 0x90, 7, 1, b'x']),
                Packed::Whole(Kind::Tree, tree),
                Packed::Whole(Kind::Commit, commit),
            ],
        )
    };
    let on_secret = by_delta(secret_blob);
    assert!(
        on_secret.contains("unpack ok") && on_secret.contains("ng refs/heads/copy missing necessary objects"),
        "{on_secret}"
    );
    assert_eq!(by_delta(nothing), on_secret);
    assert_eq!(head_of(&fixture, "proj", "refs/heads/copy"), None);

    // a parent that is not there is no parent, also for a client that says its history is cut at it
    let orphan = commit_data(&main_tree_id, &nothing.to_string(), "On nothing");
    let answer = push_refs(
        &base,
        WRITER,
        &[("refs/heads/orphan", id_of(Kind::Commit, &orphan))],
        &[&nothing.to_string()],
        &[Packed::Whole(Kind::Commit, orphan)],
    );
    assert!(answer.contains("ng refs/heads/orphan"), "{answer}");

    // more refs in one push than a user with no push right names
    let many: Vec<(String, gix::ObjectId)> = (0..9).map(|n| (format!("refs/for/b{n}"), main_id)).collect();
    let many: Vec<(&str, gix::ObjectId)> = many.iter().map(|(name, id)| (name.as_str(), *id)).collect();
    let answer = push_refs(&base, READER, &many, &[], &[]);
    assert!(answer.contains("GitPushRequestTooLarge"), "{answer}");
    settled(&fixture);

    // the push log, for a reader: the rows it is given counted, no word of why a push was refused or a rule failed
    let log = |user: &str| {
        ok(
            &fixture,
            user,
            &git("find#GitUpdates"),
            &[("repositoryId", s("proj")), ("pageNoLimit", s("true"))],
        )
    };
    let (whole, given) = (log(WRITER), log(READER));
    let count = |out: &std::collections::BTreeMap<String, Value>| text_of(out.get("updateListCount").unwrap());
    assert_eq!(count(&whole), list(&whole, "updateList").len().to_string());
    assert_eq!(count(&given), list(&given, "updateList").len().to_string());
    assert!(list(&given, "updateList").len() < list(&whole, "updateList").len());
    let results = |out: &std::collections::BTreeMap<String, Value>| -> Vec<String> {
        let mut found = Vec::new();
        for update in list(out, "updateList") {
            assert!(
                user_sees_no_error(out, &whole)
                    || text_of(crate::server::hosting_tests::entry(update, "ecaError")).is_empty()
            );
            if let Value::List(refs) = crate::server::hosting_tests::entry(update, "refList") {
                for reference in refs {
                    found.push(text_of(crate::server::hosting_tests::entry(reference, "result")));
                }
            }
        }
        found
    };
    assert!(results(&whole)
        .iter()
        .any(|result| result.starts_with("missing necessary objects")));
    let given_results = results(&given);
    assert!(
        given_results.iter().any(|result| result == "refused"),
        "{given_results:?}"
    );
    assert!(
        given_results
            .iter()
            .all(|result| matches!(result.as_str(), "ok" | "pending" | "refused")),
        "{given_results:?}"
    );
}

/// Whether `out` is the whole log (its errors are its reader's to see).
fn user_sees_no_error(
    out: &std::collections::BTreeMap<String, Value>,
    whole: &std::collections::BTreeMap<String, Value>,
) -> bool {
    std::ptr::eq(out, whole)
}

fn webhook_bodies(fixture: &Fixture) -> Vec<String> {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find("moqui.service.message.SystemMessage")
        .unwrap()
        .eq("systemMessageTypeId", s("GitWebhook"))
        .order_by("systemMessageId", false)
        .list()
        .unwrap()
        .iter()
        .map(|row| field(row, "messageText"))
        .collect()
}

/// The commits a repository's refs have left are read by id by who works in it only while nobody could have put a
/// commit in its store that its readers were not to read: not once it is in a fork network (as its row says now,
/// whatever this node has open), and not once a merge request of it was hidden. A hidden merge request is its
/// author's and the application's, for the services that change it and for the repository's webhooks.
#[test]
fn a_hidden_merge_request_is_its_authors_and_the_refs_alone_decide_after_it() {
    let fixture = fixture_with(
        "review-hidden-rules",
        moqui_core::context::framework_dir(),
        r#"<git-server webhook-private-network="true"/>"#,
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "solo", "db");
    {
        moqui_core::testing::seed_status_flows(&fixture.entity);
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        crate::server::tests::create(
            &fixture.entity,
            "moqui.service.message.SystemMessageType",
            &[
                ("systemMessageTypeId", "GitWebhook"),
                ("sendServiceName", "org.moqui.git.GitServices.send#GitWebhookDelivery"),
            ],
        );
    }
    commit_files(
        &fixture,
        WRITER,
        "solo",
        &[("message", s("First")), ("fileMap", file_map(&[("a.txt", "one\n")]))],
    );
    let left = commit_files(
        &fixture,
        WRITER,
        "solo",
        &[
            ("message", s("Side")),
            ("newBranch", s("side")),
            ("fileMap", file_map(&[("b.txt", "side\n")])),
        ],
    );
    ok(
        &fixture,
        WRITER,
        &git("delete#GitRef"),
        &[("repositoryId", s("solo")), ("refName", s("side"))],
    );
    settled(&fixture);
    // left by its branch: read by id by who works in the repository, not by a reader
    assert!(reads_commit(&fixture, WRITER, "solo", &left));
    assert!(!reads_commit(&fixture, READER, "solo", &left));

    // another node made the repository's first fork: its row says so, the repository this node has open does not
    let set_network = |network: Value| {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        assert!(provider.repo("solo").unwrap().definition.network.is_none());
        fixture
            .entity
            .find(crate::GIT_REPOSITORY)
            .unwrap()
            .eq("repositoryId", s("solo"))
            .update_all([("forkNetworkId".to_string(), network)])
            .unwrap();
        assert!(provider.repo("solo").unwrap().definition.network.is_none());
    };
    set_network(s("solo"));
    assert!(!reads_commit(&fixture, WRITER, "solo", &left));
    set_network(Value::Null);
    assert!(reads_commit(&fixture, WRITER, "solo", &left));

    // a reader's proposal, which the application hides; and its next one, which replaces the head
    let propose = |text: &str| {
        commit_files(
            &fixture,
            READER,
            "solo",
            &[
                ("message", s("Proposed")),
                ("asProposal", s("true")),
                ("fileMap", file_map(&[("a.txt", text)])),
            ],
        )
    };
    let first = propose("p1\n");
    let request_id = field(&merge_requests(&fixture, "solo")[0], "mergeRequestId");
    ok(
        &fixture,
        WRITER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id))],
    );
    // a webhook of the repository, from here on: it is told nothing of a hidden merge request
    ok(
        &fixture,
        WRITER,
        &git("store#GitWebhook"),
        &[
            ("repositoryId", s("solo")),
            ("url", s("http://127.0.0.1:9/hook")),
            ("events", s("merge_request")),
        ],
    );
    let second = propose("p2\n");
    assert_eq!(field(&merge_requests(&fixture, "solo")[0], "headCommitId"), second);
    // neither head is read by id, nor what the branch left: from now on the refs alone decide
    for commit in [&first, &second, &left] {
        assert!(!reads_commit(&fixture, WRITER, "solo", commit), "{commit}");
    }
    let (_, errors) = call(
        &fixture,
        WRITER,
        &git("create#GitRef"),
        &[
            ("repositoryId", s("solo")),
            ("refName", s("stolen")),
            ("revision", s(&first)),
        ],
    );
    assert!(errors.contains("names no commit"), "{errors}");

    // a check of the hidden head is the application's to write and to read
    let status = [
        ("repositoryId", s("solo")),
        ("commitId", s(&second)),
        ("state", s("success")),
        ("context", s("build")),
        ("includeHidden", s("true")),
    ];
    let (_, errors) = call(&fixture, WRITER, &git("create#GitCommitStatus"), &status);
    assert!(errors.contains("no commit"), "{errors}");
    let (_, errors) = call(&fixture, READER, &git("create#GitCommitStatus"), &status);
    assert!(errors.contains("needs a push right"), "{errors}");
    let (_, errors) = internal(&fixture, WRITER, &git("create#GitCommitStatus"), &status);
    assert!(errors.trim().is_empty(), "{errors}");
    let asked = [
        ("repositoryId", s("solo")),
        ("commitIdList", Value::List(vec![s(&second)])),
        ("includeHidden", s("true")),
    ];
    let seen = ok(&fixture, WRITER, &git("get#GitCommitStatuses"), &asked);
    assert!(list(&seen, "statusList").is_empty());
    let (seen, errors) = internal(&fixture, WRITER, &git("get#GitCommitStatuses"), &asked);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(list(&seen, "statusList").len(), 1);

    // to a user who is not its author there is no such merge request, whatever their rights
    let changes: [(&str, Vec<(&str, Value)>); 6] = [
        ("update#MergeRequest", vec![("title", s("Mine now"))]),
        (
            "set#MergeRequestReviewers",
            vec![("addUserIdList", Value::List(vec![s(WRITER)]))],
        ),
        ("submit#MergeRequestReview", vec![("decision", s("comment"))]),
        ("get#MergeRequestApprovals", Vec::new()),
        ("close#MergeRequest", Vec::new()),
        ("reopen#MergeRequest", Vec::new()),
    ];
    for (name, extra) in &changes {
        let mut parameters = vec![("mergeRequestId", s(&request_id))];
        parameters.extend(extra.iter().cloned());
        let (_, errors) = call(&fixture, WRITER, &review(name), &parameters);
        assert!(errors.to_lowercase().contains("no merge request"), "{name}: {errors}");
    }
    // its author changes it; a target that is no branch is refused
    ok(
        &fixture,
        READER,
        &review("update#MergeRequest"),
        &[("mergeRequestId", s(&request_id)), ("title", s("Better"))],
    );
    let (_, errors) = call(
        &fixture,
        READER,
        &review("update#MergeRequest"),
        &[("mergeRequestId", s(&request_id)), ("targetRef", s("nosuch"))],
    );
    assert!(errors.contains("GitNoSuchRef"), "{errors}");
    let found = merge_requests(&fixture, "solo");
    assert_eq!(field(&found[0], "title"), "Better");
    assert_eq!(field(&found[0], "targetRef"), "main");
    // the webhook was told nothing of it while it was hidden (a new head, a new title), by the services or by the
    // event itself; shown, it is opened for the webhook
    assert!(webhook_bodies(&fixture).is_empty());
    let event = |action: &str| {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let row = fixture
            .entity
            .find(MERGE_REQUEST)
            .unwrap()
            .eq("mergeRequestId", s(&request_id))
            .one()
            .unwrap()
            .unwrap();
        let queued = crate::service::hosting::queue_merge_request_event(&fixture.entity, &row, action).unwrap();
        // the test factory's service pool is idle, so the send that follows a queued delivery leaves an error
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().clear());
        text_of(queued.get("queuedCount").unwrap())
    };
    assert_eq!(event("synchronize"), "0");
    assert!(webhook_bodies(&fixture).is_empty());
    let active = |flag: &str| {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.resource.GitWebhook")
            .unwrap()
            .eq("repositoryId", s("solo"))
            .update_all([("active".to_string(), s(flag))])
            .unwrap();
    };
    // (off while the service shows it: the idle pool's error would take the service's transaction with it)
    active("N");
    ok(
        &fixture,
        WRITER,
        &review("set#MergeRequestHidden"),
        &[("mergeRequestId", s(&request_id)), ("hidden", s("false"))],
    );
    active("Y");
    assert_eq!(event("opened"), "1");
    let sent = webhook_bodies(&fixture);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].contains(r#""action":"opened""#) && sent[0].contains("Better") && sent[0].contains(&second),
        "{}",
        sent[0]
    );
    // its head is read as any other; the head it replaced while hidden never is
    assert!(reads_commit(&fixture, WRITER, "solo", &second));
    assert!(!reads_commit(&fixture, WRITER, "solo", &first));
    assert!(!reads_commit(&fixture, WRITER, "solo", &left));

    // closed and its ref let go: its commits are not what a branch left
    active("N");
    ok(
        &fixture,
        READER,
        &review("close#MergeRequest"),
        &[("mergeRequestId", s(&request_id))],
    );
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
            .eq("mergeRequestId", s(&request_id))
            .update_all([("closedDate".to_string(), long_ago)])
            .unwrap();
        let released = crate::provider(fixture.context.factory())
            .unwrap()
            .release_closed_heads(&fixture.entity, "solo")
            .unwrap();
        assert_eq!(released, 1);
    }
    assert!(!reads_commit(&fixture, WRITER, "solo", &second));
    settled(&fixture);
}

/// What the server does for an account with no push right is small, and its refused pushes are counted: a push
/// for review builds on few commits, names files within a small walk of the branch, and after too many refusals in
/// an hour the account sends no more.
#[test]
fn what_the_server_does_for_a_reader_is_small_and_counted() {
    use gix::objs::Kind;
    let fixture = fixture_with(
        "review-cost",
        moqui_core::context::framework_dir(),
        r#"<git-server review-max-built-on="1" review-max-trees="1" review-max-refused="3"/>"#,
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "proj", "db");
    let base = http_server(fixture.context.factory());
    let first = commit_files(
        &fixture,
        WRITER,
        "proj",
        &[
            ("message", s("First")),
            ("fileMap", file_map(&[("a.txt", "one\n"), ("dir/sub/f.txt", "deep\n")])),
        ],
    );
    let main = commit_files(
        &fixture,
        WRITER,
        "proj",
        &[("message", s("Second")), ("fileMap", file_map(&[("a.txt", "two\n")]))],
    );
    let (top, deep) = (id_of(Kind::Blob, b"two\n"), id_of(Kind::Blob, b"deep\n"));

    // on two commits of the repository, where one is the most
    let tree = tree_data(&[("a.txt", top), ("b.txt", top)]);
    let wide = format!(
        "tree {}\nparent {first}\nparent {main}\nauthor A <a@example.com> 1700000000 +0000\ncommitter A <a@example.com> 1700000000 +0000\n\nWide\n",
        id_of(Kind::Tree, &tree)
    )
    .into_bytes();
    let answer = push_for_review(
        &base,
        READER,
        &id_of(Kind::Commit, &wide),
        &[],
        &[Packed::Whole(Kind::Tree, tree), Packed::Whole(Kind::Commit, wide)],
    );
    assert!(answer.contains("GitReviewPushTooWide"), "{answer}");

    // a file two directories down, named and not sent: past the trees read for a reader, within those for a pusher
    let deep_tree = tree_data(&[("a.txt", top), ("z.txt", deep)]);
    let deep_commit = commit_data(&id_of(Kind::Tree, &deep_tree), &main, "Names a deep file");
    let deep_id = id_of(Kind::Commit, &deep_commit);
    let deep_pack = || {
        [
            Packed::Whole(Kind::Tree, deep_tree.clone()),
            Packed::Whole(Kind::Commit, deep_commit.clone()),
        ]
    };
    let answer = push_for_review(&base, READER, &deep_id, &[], &deep_pack());
    assert!(
        answer.contains("ng refs/for/main missing necessary objects"),
        "{answer}"
    );
    let answer = push_refs(&base, WRITER, &[("refs/heads/deep", deep_id)], &[], &deep_pack());
    assert!(answer.contains("ok refs/heads/deep"), "{answer}");

    // a third refusal, and the account sends no more this hour: by a push, or through the service
    let fine_tree = tree_data(&[("a.txt", id_of(Kind::Blob, b"three\n"))]);
    let fine = commit_data(&id_of(Kind::Tree, &fine_tree), &main, "Fine");
    let fine_id = id_of(Kind::Commit, &fine);
    let fine_pack = [
        Packed::Whole(Kind::Blob, b"three\n".to_vec()),
        Packed::Whole(Kind::Tree, fine_tree),
        Packed::Whole(Kind::Commit, fine),
    ];
    let answer = push_refs(&base, READER, &[("refs/for/nosuch", fine_id)], &[], &fine_pack);
    assert!(answer.contains("GitNoSuchRef"), "{answer}");
    let answer = push_for_review(&base, READER, &fine_id, &[], &fine_pack);
    assert!(
        answer.contains("GitReviewRefusals") && !answer.contains("ok refs/for/main"),
        "{answer}"
    );
    let (_, errors) = call(
        &fixture,
        READER,
        &git("commit#GitFiles"),
        &[
            ("repositoryId", s("proj")),
            ("asProposal", s("true")),
            ("message", s("Proposed")),
            ("fileMap", file_map(&[("a.txt", "four\n")])),
        ],
    );
    assert!(errors.contains("GitReviewRefusals"), "{errors}");
    assert!(merge_requests(&fixture, "proj").is_empty());
    settled(&fixture);
}
