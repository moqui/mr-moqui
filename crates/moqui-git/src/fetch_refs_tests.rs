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

//! `fetch#GitRefs` against a server of the test's own (one advertisement, one pack it chose) on the in-memory
//! database: a caller with authorization on is held to a push's rules, the branch rules, the reachability rules
//! (a hidden merge request's id neither told to the server nor written as a ref, a pack built on its file refused)
//! and the merge request refs (only the first head of an imported one).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use gix::objs::Kind;
use gix::ObjectId;
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityValue;
use moqui_lib::object::Value;

use crate::server::hosting_tests::{call, get, grant_entities, list, ok, s, text_of};
use crate::server::review::tests::{commit_data, commit_files, file_map, id_of, pack_of, tree_data, Packed};
use crate::server::tests::{create, fixture_with, head_of, Fixture, WRITER};

const GIT: &str = "org.moqui.git.GitServices";

fn fixture(name: &str) -> Fixture {
    let fixture = fixture_with(
        name,
        moqui_core::context::framework_dir(),
        r#"<git-server webhook-private-network="true"/>"#,
    );
    moqui_core::testing::seed_status_flows(&fixture.entity);
    grant_entities(&fixture);
    fixture
}

fn pkt(line: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", line.len() + 4).into_bytes();
    out.extend_from_slice(line);
    out
}

/// A smart HTTP server that advertises `refs` and answers every fetch with `pack`, whatever was asked for; the
/// bodies of the fetches it was sent.
struct FakeRemote {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

fn fake_remote(refs: &[(ObjectId, &str)], pack: Vec<u8>) -> FakeRemote {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x.git", listener.local_addr().unwrap());
    let mut advertisement = pkt(b"# service=git-upload-pack\n");
    advertisement.extend_from_slice(b"0000");
    for (index, (id, name)) in refs.iter().enumerate() {
        let line = if index == 0 {
            format!("{id} {name}\0ofs-delta\n")
        } else {
            format!("{id} {name}\n")
        };
        advertisement.extend(pkt(line.as_bytes()));
    }
    advertisement.extend_from_slice(b"0000");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let (mut length, mut chunked) = (0usize, false);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).is_err() {
                    break;
                }
                let header = header.trim_end().to_ascii_lowercase();
                if header.is_empty() {
                    break;
                }
                if let Some(value) = header.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                if header.starts_with("transfer-encoding:") && header.contains("chunked") {
                    chunked = true;
                }
            }
            let mut body = Vec::new();
            if chunked {
                loop {
                    let mut size = String::new();
                    reader.read_line(&mut size).unwrap();
                    let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                    let mut chunk = vec![0; size + 2];
                    if size == 0 {
                        let _ = reader.read_line(&mut String::new());
                        break;
                    }
                    reader.read_exact(&mut chunk).unwrap();
                    body.extend_from_slice(&chunk[..size]);
                }
            } else {
                body.resize(length, 0);
                reader.read_exact(&mut body).unwrap();
            }
            let (kind, answer) = if line.starts_with("GET") {
                ("advertisement", advertisement.clone())
            } else {
                kept.lock().unwrap().push(String::from_utf8_lossy(&body).into_owned());
                let mut answer = pkt(b"NAK\n");
                answer.extend_from_slice(&pack);
                ("result", answer)
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-{kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                answer.len()
            );
            let _ = stream.write_all(&answer);
        }
    });
    FakeRemote { url, requests }
}

fn ref_map(pairs: &[(&str, &str)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(remote, local)| ((*remote).to_string(), s(local)))
            .collect(),
    )
}

/// `fetch#GitRefs` into `imp` as the writer, who administers it: its result and its errors.
fn fetch(fixture: &Fixture, url: &str, pairs: &[(&str, &str)]) -> (BTreeMap<String, Value>, String) {
    call(
        fixture,
        WRITER,
        &format!("{GIT}.fetch#GitRefs"),
        &[
            ("repositoryId", s("imp")),
            ("remoteUrl", s(url)),
            ("refMap", ref_map(pairs)),
        ],
    )
}

/// [`fetch`] with authorization disabled, as the application's importer calls it.
fn fetch_unchecked(fixture: &Fixture, url: &str, pairs: &[(&str, &str)]) -> (BTreeMap<String, Value>, String) {
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(WRITER).unwrap();
    let _authz = fixture.context.disable_authz();
    let parameters: BTreeMap<String, Value> = [
        ("repositoryId".to_string(), s("imp")),
        ("remoteUrl".to_string(), s(url)),
        ("refMap".to_string(), ref_map(pairs)),
    ]
    .into_iter()
    .collect();
    let result = moqui_core::service::sync(&format!("{GIT}.fetch#GitRefs"), &parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors_text().join("\n");
        ec.message().clear();
        text
    })
    .unwrap_or_default();
    moqui_core::user::logout_local().unwrap();
    (result.unwrap_or_default(), errors)
}

fn problems(out: &BTreeMap<String, Value>) -> String {
    format!("{:?}", list(out, "problems"))
}

fn fetched(out: &BTreeMap<String, Value>) -> Vec<String> {
    list(out, "fetched").iter().map(text_of).collect()
}

fn set_ref(fixture: &Fixture, name: &str, id: Option<ObjectId>) {
    let _entered = fixture.context.enter();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    let store = &provider.repo("imp").unwrap().store;
    let edit = crate::store::RefEdit {
        name: name.to_string(),
        expected: store.read_ref(name).unwrap(),
        new: id,
    };
    store.update_refs(&[edit], false).unwrap();
}

/// The repository `imp`: its branch main (a.txt "one"), and merge request !1, which is hidden, whose head is a
/// commit and a file (secret.txt "secret") that nothing else reaches. (main, the hidden head)
fn with_hidden(fixture: &Fixture) -> (ObjectId, ObjectId) {
    crate::server::tests::define(fixture, "imp", "db");
    let main = commit_files(
        fixture,
        WRITER,
        "imp",
        &[("message", s("First")), ("fileMap", file_map(&[("a.txt", "one\n")]))],
    );
    let held = commit_files(
        fixture,
        WRITER,
        "imp",
        &[
            ("message", s("Held")),
            ("newBranch", s("held")),
            ("fileMap", file_map(&[("secret.txt", "secret\n")])),
        ],
    );
    let (main, held) = (
        ObjectId::from_hex(main.as_bytes()).unwrap(),
        ObjectId::from_hex(held.as_bytes()).unwrap(),
    );
    set_ref(fixture, "refs/merge-requests/1/head", Some(held));
    set_ref(fixture, "refs/heads/held", None);
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut row = EntityValue::make(&fixture.entity, "moqui.resource.GitMergeRequest").unwrap();
    for (name, value) in [
        ("mergeRequestId", s("HELD1")),
        ("repositoryId", s("imp")),
        ("mergeRequestNum", Value::I64(1)),
        ("title", s("Held")),
        ("targetRef", s("main")),
        ("statusId", s("GmrOpen")),
        ("pushedForReview", s("Y")),
        ("headCommitId", s(&held.to_string())),
        ("hiddenDate", moqui_core::user::now_value()),
    ] {
        row.set(name, value);
    }
    row.create(&fixture.entity).unwrap();
    crate::provider(fixture.context.factory())
        .unwrap()
        .forget_reached("imp");
    (main, held)
}

/// A pack of a new commit on `parent` (a.txt "one", b.txt `text`): the pack and the commit.
fn new_commit(parent: &ObjectId, text: &str) -> (Vec<u8>, ObjectId) {
    let blob = text.as_bytes().to_vec();
    let tree = tree_data(&[
        ("a.txt", id_of(Kind::Blob, b"one\n")),
        ("b.txt", id_of(Kind::Blob, &blob)),
    ]);
    let commit = commit_data(&id_of(Kind::Tree, &tree), &parent.to_string(), text.trim());
    let id = id_of(Kind::Commit, &commit);
    let pack = pack_of(&[
        Packed::Whole(Kind::Blob, blob),
        Packed::Whole(Kind::Tree, tree),
        Packed::Whole(Kind::Commit, commit),
    ]);
    (pack, id)
}

/// The haves of a fetch are the refs a reader reads: never the head of a hidden merge request.
fn told_haves(remote: &FakeRemote, main: &ObjectId, hidden: &ObjectId) {
    let requests = remote.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(requests[0].contains(&format!("have {main}")), "{}", requests[0]);
    assert!(!requests[0].contains(&format!("have {hidden}")), "{}", requests[0]);
}

/// An id the store holds as a hidden merge request's head is not the caller's: a server that names it and does not
/// send it gets no ref made at it, and the hidden head is not among the haves the server is told.
#[test]
fn a_hidden_id_the_store_holds_is_not_written_as_a_ref() {
    let fixture = fixture("fetch-hidden-id");
    let (main, hidden) = with_hidden(&fixture);
    {
        let _entered = fixture.context.enter();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        let repo = provider.repo("imp").unwrap();
        let haves = provider.fetch_haves(&repo).unwrap();
        assert_eq!(haves, vec![main]);
    }
    let remote = fake_remote(&[(hidden, "refs/heads/evil")], pack_of(&[]));
    let (out, errors) = fetch(&fixture, &remote.url, &[("refs/heads/evil", "refs/heads/stolen")]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert!(fetched(&out).is_empty(), "{out:?}");
    assert!(problems(&out).contains(&format!("did not send {hidden}")), "{out:?}");
    assert_eq!(head_of(&fixture, "imp", "refs/heads/stolen"), None);
    told_haves(&remote, &main, &hidden);
}

/// A pack whose delta is against a hidden merge request's file (a thin pack's base the store holds) is refused, and
/// nothing is written; one against a file of a branch the caller reads is taken.
#[test]
fn a_thin_base_from_a_hidden_object_is_refused() {
    let fixture = fixture("fetch-thin-hidden");
    let (main, hidden) = with_hidden(&fixture);
    let secret = id_of(Kind::Blob, b"secret\n");
    let copied = id_of(Kind::Blob, b"secret\nx");
    let tree = tree_data(&[("a.txt", id_of(Kind::Blob, b"one\n")), ("copy.txt", copied)]);
    let commit = commit_data(&id_of(Kind::Tree, &tree), &main.to_string(), "By a delta");
    let commit_id = id_of(Kind::Commit, &commit);
    let pack = pack_of(&[
        Packed::RefDelta(secret, vec![7, 8, 0x90, 7, 1, b'x']),
        Packed::Whole(Kind::Tree, tree),
        Packed::Whole(Kind::Commit, commit),
    ]);
    let remote = fake_remote(&[(commit_id, "refs/heads/topic")], pack);
    let (_, errors) = fetch(&fixture, &remote.url, &[("refs/heads/topic", "refs/heads/topic")]);
    assert!(errors.contains(&format!("missing object {secret}")), "{errors}");
    assert_eq!(head_of(&fixture, "imp", "refs/heads/topic"), None);
    told_haves(&remote, &main, &hidden);

    // the same against a.txt of main, which the caller reads
    let public = id_of(Kind::Blob, b"one\n");
    let tree = tree_data(&[("a.txt", public), ("copy.txt", id_of(Kind::Blob, b"one\nx"))]);
    let commit = commit_data(&id_of(Kind::Tree, &tree), &main.to_string(), "By a public delta");
    let commit_id = id_of(Kind::Commit, &commit);
    let pack = pack_of(&[
        Packed::RefDelta(public, vec![4, 5, 0x90, 4, 1, b'x']),
        Packed::Whole(Kind::Tree, tree),
        Packed::Whole(Kind::Commit, commit),
    ]);
    let remote = fake_remote(&[(commit_id, "refs/heads/topic")], pack);
    let (out, errors) = fetch(&fixture, &remote.url, &[("refs/heads/topic", "refs/heads/topic")]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(fetched(&out), ["refs/heads/topic"], "{out:?}");
    assert_eq!(
        head_of(&fixture, "imp", "refs/heads/topic"),
        Some(commit_id.to_string())
    );
}

/// A protected branch is no more moved by a fetch than by a push: a rule that wants merge requests refuses it, and
/// a branch no rule names is fetched from the same pack.
#[test]
fn a_fetch_does_not_move_a_protected_branch() {
    let fixture = fixture("fetch-protected");
    let (main, _) = with_hidden(&fixture);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.resource.GitBranchRule",
            &[
                ("branchRuleId", "PROTECT_MAIN"),
                ("repositoryId", "imp"),
                ("refPattern", "main"),
                ("requireMergeRequest", "Y"),
            ],
        );
    }
    let (pack, id) = new_commit(&main, "two\n");
    let remote = fake_remote(&[(id, "refs/heads/main"), (id, "refs/heads/copy")], pack);
    let (out, errors) = fetch(
        &fixture,
        &remote.url,
        &[
            ("refs/heads/main", "refs/heads/main"),
            ("refs/heads/copy", "refs/heads/other"),
        ],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    assert!(problems(&out).contains("GitProtectedBranch"), "{out:?}");
    assert_eq!(fetched(&out), ["refs/heads/other"], "{out:?}");
    assert_eq!(head_of(&fixture, "imp", "refs/heads/main"), Some(main.to_string()));
    assert_eq!(head_of(&fixture, "imp", "refs/heads/other"), Some(id.to_string()));
}

/// With authorization on a fetch sets no merge request's head but the first one of an imported merge request, which
/// then records it: not a hidden one's, not one that is not there, not one that has a head. The importer
/// (authorization disabled) sets any.
#[test]
fn a_fetch_sets_only_the_first_head_of_an_imported_merge_request() {
    let fixture = fixture("fetch-review-heads");
    let (main, hidden) = with_hidden(&fixture);
    let (pack, id) = new_commit(&main, "pull\n");
    let remote = fake_remote(&[(id, "refs/pull/5/head")], pack);
    let (out, _) = fetch(
        &fixture,
        &remote.url,
        &[("refs/pull/5/head", "refs/merge-requests/1/head")],
    );
    assert!(problems(&out).contains("first head of an imported"), "{out:?}");
    assert_eq!(
        head_of(&fixture, "imp", "refs/merge-requests/1/head"),
        Some(hidden.to_string())
    );
    let (out, _) = fetch(
        &fixture,
        &remote.url,
        &[("refs/pull/5/head", "refs/merge-requests/77/head")],
    );
    assert!(problems(&out).contains("first head of an imported"), "{out:?}");

    let imported = ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("imp")),
            ("title", s("Pull request 5")),
            ("imported", s("true")),
            ("pushedForReview", s("true")),
            ("mergeRequestNum", s("5")),
        ],
    );
    let request_id = get(&imported, "mergeRequestId").to_string();
    let (out, errors) = fetch(
        &fixture,
        &remote.url,
        &[("refs/pull/5/head", "refs/merge-requests/5/head")],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(fetched(&out), ["refs/merge-requests/5/head"], "{out:?}");
    let head = {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.resource.GitMergeRequest")
            .unwrap()
            .eq("mergeRequestId", s(&request_id))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
            .get("headCommitId")
            .map(text_of)
            .unwrap_or_default()
    };
    assert_eq!(head, id.to_string(), "the merge request records the head its ref has");
    // a second head is the merge request's own to take (sync#MergeRequestHead), not a fetch's
    let (out, _) = fetch(
        &fixture,
        &remote.url,
        &[("refs/pull/5/head", "refs/merge-requests/5/head")],
    );
    assert!(problems(&out).contains("first head of an imported"), "{out:?}");

    let (out, errors) = fetch_unchecked(
        &fixture,
        &remote.url,
        &[("refs/pull/5/head", "refs/merge-requests/9/head")],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(fetched(&out), ["refs/merge-requests/9/head"], "{out:?}");
}
