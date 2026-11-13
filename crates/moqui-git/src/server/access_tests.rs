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

//! Anonymous read and repository paths (plans/Dogfood.md F1, F2) against the git command line, on the in-memory
//! database with the framework's git services: a request with no credentials is served what an authorization service
//! of `ALL_USERS` lets anyone view and asked to log in for everything else, and a repository is reached by its path,
//! its id and a path it had. Each test with git is skipped when git is not installed.

use std::collections::BTreeMap;
use std::io::Write;

use moqui_lib::object::{to_plain_string, Value};

use super::tests::{
    create, daemon, define, fixture_in, git_available, git_in, git_ok, http, http_server, url_with, user, write_file,
    Fixture, PASSWORD, WRITER,
};
use super::{GitHttpRequest, GitService, HttpResponder};
use moqui_core::context::ExecutionContext;

/// A user with no right on any repository.
const STRANGER: &str = "git.stranger";

/// As Mantle's check will: anyone, a request with no user too, may view a repository that is public (here, one whose
/// id starts with `pub`); for the rest it says nothing, so the other records decide. One whose id starts with `open`
/// lets everyone do everything, as a careless record would.
const PUBLIC_CHECK: &str = r#"<services>
    <service verb="check" noun="Authz">
        <in-parameters>
            <parameter name="userId"/><parameter name="authzActionEnumId"/>
            <parameter name="artifactTypeEnumId"/><parameter name="artifactName"/>
        </in-parameters>
        <out-parameters><parameter name="authzTypeEnumId"/></out-parameters>
        <actions><script><![CDATA[
            const name = String(artifactName)
            authzTypeEnumId = (name.startsWith('open') || (authzActionEnumId === 'AUTHZA_VIEW' && name.startsWith('pub')))
                ? 'AUTHZT_ALLOW' : 'AUTHZT_NONE'
        ]]></script></actions>
    </service>
</services>"#;

/// The fixture of `tests.rs` with the git services, a stranger, and the `ALL_USERS` record whose service is
/// [`PUBLIC_CHECK`].
fn public_fixture(name: &str) -> Fixture {
    let fixture = fixture_in(name, moqui_core::context::framework_dir());
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/GitPublic.xml");
        std::fs::write(&stub, PUBLIC_CHECK).unwrap();
        let failures = moqui_core::service::lint(&[
            moqui_core::context::framework_dir().join("service/org/moqui/git/GitServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        user(&fixture.entity, STRANGER);
        create(
            &fixture.entity,
            "moqui.security.UserGroup",
            &[("userGroupId", "ALL_USERS")],
        );
        create(
            &fixture.entity,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "GIT_PUBLIC"),
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "GIT_ALL"),
                ("authzTypeEnumId", "AUTHZT_ALLOW"),
                ("authzActionEnumId", "AUTHZA_ALL"),
                ("authzServiceName", "test.GitPublic.check#Authz"),
            ],
        );
    }
    fixture
}

fn basic(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{username}:{password}")
        )
    )
}

/// `GET <path>/info/refs?service=<service>` with an optional `Authorization`: the status, the headers and the body.
fn refs(
    base: &str,
    path: &str,
    service: &str,
    authorization: Option<&str>,
) -> (u16, std::collections::HashMap<String, String>, String) {
    let headers: Vec<(&str, &str)> = authorization.iter().map(|value| ("Authorization", *value)).collect();
    let (status, headers, body) = http(
        base,
        "GET",
        &format!("/git/{path}/info/refs?service={service}"),
        &headers,
        b"",
    );
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// Call a service as `username`: its result and the errors it left.
fn call(
    fixture: &Fixture,
    username: &str,
    name: &str,
    parameters: &[(&str, &str)],
) -> (BTreeMap<String, Value>, String) {
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(username).unwrap();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect();
    let result = moqui_core::service::sync(name, &parameters);
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

const SET_PATH: &str = "org.moqui.git.GitServices.set#GitRepositoryPath";

fn set_path(fixture: &Fixture, id: &str, path: &str) -> String {
    call(
        fixture,
        WRITER,
        SET_PATH,
        &[("repositoryId", id), ("repositoryPath", path)],
    )
    .1
}

/// A first commit pushed to repository `id` by the writer, through `url_path` under `/git/`; the clone.
fn seeded(fixture: &Fixture, base: &str, home: &std::path::Path, url_path: &str, file: &str) -> std::path::PathBuf {
    let work = fixture.dir.join(format!("seed-{}", url_path.replace('/', "-")));
    git_ok(
        &fixture.dir,
        home,
        &["clone", &url_with(base, WRITER, url_path), &work.to_string_lossy()],
    );
    write_file(&work, file, "content\n");
    git_ok(&work, home, &["add", "."]);
    git_ok(&work, home, &["commit", "-m", &format!("Add {file}")]);
    git_ok(&work, home, &["push", "origin", "main"]);
    work
}

struct Captured {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponder for Captured {
    fn head(&mut self, status: u16, headers: &[(&str, String)]) -> std::io::Result<()> {
        self.status = status;
        self.headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        Ok(())
    }

    fn body(&mut self) -> &mut dyn Write {
        &mut self.body
    }
}

/// An advertisement request served on this thread, so the thread's auth-failure mark (what the host counts toward a
/// ban) can be read: the status and whether the mark was set.
fn served_here(fixture: &Fixture, repository: &str, authorization: Option<String>) -> (u16, bool) {
    served_with(fixture, repository, authorization, None)
}

/// [`served_here`] with an `api_key` header too.
fn served_with(
    fixture: &Fixture,
    repository: &str,
    authorization: Option<String>,
    api_key: Option<String>,
) -> (u16, bool) {
    let context = fixture.context.factory().new_context();
    let _entered = context.enter();
    moqui_core::web::take_auth_failure();
    let request = GitHttpRequest {
        repository: repository.to_string(),
        service: GitService::UploadPack,
        advertise: true,
        git_protocol: None,
        gzip: false,
        authorization,
        api_key,
        secure: true,
        client_ip: "127.0.0.1".into(),
    };
    let mut out = Captured {
        status: 0,
        headers: Vec::new(),
        body: Vec::new(),
    };
    super::serve_http(&request, &mut std::io::empty(), &mut out);
    if out.status == 401 {
        assert!(
            out.headers.iter().any(|(name, _)| name == "WWW-Authenticate"),
            "a 401 asks for a login"
        );
    }
    if out.status == 503 {
        assert!(
            out.headers.iter().any(|(name, _)| name == "Retry-After"),
            "a 503 says when to try again"
        );
    }
    (out.status, moqui_core::web::take_auth_failure())
}

fn authz_failures(fixture: &Fixture) -> usize {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find("moqui.security.ArtifactAuthzFailure")
        .unwrap()
        .list()
        .unwrap()
        .len()
}

/// F1: with no credentials a repository anyone may view is cloned, and every other request (a repository the request
/// may not view, one that is not there, a push) is answered 401 with nothing about the repository; with credentials
/// and no right it is 404 as before. A 401 for no credentials is not an auth failure, and records no authz failure.
#[test]
fn a_request_with_no_credentials_reads_what_anyone_may_view() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = public_fixture("anonymous");
    moqui_lib::env::set_property("git_lfs_token_key", "the key of the anonymous test".to_string());
    define(&fixture, "pub-code", "db");
    define(&fixture, "guarded", "db");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    seeded(&fixture, &base, &home, "pub-code", "README.md");
    seeded(&fixture, &base, &home, "guarded", "secret.txt");

    // anyone clones the public one, at both protocol versions
    for version in ["0", "2"] {
        let clone = fixture.dir.join(format!("anon-v{version}"));
        git_ok(
            &fixture.dir,
            &home,
            &[
                "-c",
                &format!("protocol.version={version}"),
                "clone",
                &format!("{base}/git/pub-code.git"),
                &clone.to_string_lossy(),
            ],
        );
        assert!(clone.join("README.md").is_file());
    }

    // one the request may not view and one that is not there are the same 401
    let (status, headers, guarded) = refs(&base, "guarded.git", "git-upload-pack", None);
    assert_eq!(status, 401, "{guarded}");
    assert!(headers.contains_key("www-authenticate"), "{headers:?}");
    let (status, missing_headers, missing) = refs(&base, "nosuch.git", "git-upload-pack", None);
    assert_eq!(status, 401, "{missing}");
    assert_eq!(headers, missing_headers);
    assert_eq!(guarded, missing, "the two answers are the same, byte for byte");
    let (ok, out) = git_in(
        &fixture.dir,
        &home,
        &["clone", &format!("{base}/git/guarded.git"), "anon-guarded"],
    );
    assert!(!ok && !fixture.dir.join("anon-guarded/secret.txt").exists(), "{out}");

    // with credentials and no right: not found, there or not; a public one is read
    let stranger = basic(STRANGER, PASSWORD);
    for path in ["guarded.git", "nosuch.git"] {
        let (status, _, body) = refs(&base, path, "git-upload-pack", Some(&stranger));
        assert_eq!(status, 404, "{path}: {body}");
    }
    let (status, _, body) = refs(&base, "pub-code.git", "git-upload-pack", Some(&stranger));
    assert_eq!(status, 200, "{body}");

    // a push with no credentials asks for a login, on a public repository too; the stranger's is forbidden
    for path in ["pub-code.git", "guarded.git", "nosuch.git"] {
        let (status, headers, body) = refs(&base, path, "git-receive-pack", None);
        assert_eq!(status, 401, "{path}: {body}");
        assert!(headers.contains_key("www-authenticate"), "{headers:?}");
    }
    let (status, _, body) = refs(&base, "pub-code.git", "git-receive-pack", Some(&stranger));
    assert_eq!(status, 403, "{body}");
    let anon = fixture.dir.join("anon-v2");
    write_file(&anon, "more.txt", "more\n");
    git_ok(&anon, &home, &["add", "."]);
    git_ok(&anon, &home, &["commit", "-m", "More"]);
    let (ok, out) = git_in(&anon, &home, &["push", "origin", "main"]);
    assert!(!ok, "{out}");

    // LFS: a download batch is answered with no token, an upload one and a private repository's ask for a login
    let lfs = [
        ("Accept", "application/vnd.git-lfs+json"),
        ("Content-Type", "application/vnd.git-lfs+json"),
    ];
    let oid = "a".repeat(64);
    let batch = |operation: &str| {
        format!(r#"{{"operation":"{operation}","objects":[{{"oid":"{oid}","size":3}}]}}"#).into_bytes()
    };
    let (status, _, body) = http(
        &base,
        "POST",
        "/git/pub-code.git/info/lfs/objects/batch",
        &lfs,
        &batch("download"),
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(!String::from_utf8_lossy(&body).contains("MoquiLfs"));
    for (path, operation) in [("pub-code", "upload"), ("guarded", "download"), ("nosuch", "download")] {
        let (status, headers, body) = http(
            &base,
            "POST",
            &format!("/git/{path}.git/info/lfs/objects/batch"),
            &lfs,
            &batch(operation),
        );
        assert_eq!(status, 401, "{path} {operation}: {}", String::from_utf8_lossy(&body));
        assert!(headers.contains_key("lfs-authenticate"), "{headers:?}");
    }

    // only wrong credentials are an auth failure, and asking as no user records no authorization failure
    let before = authz_failures(&fixture);
    assert_eq!(served_here(&fixture, "pub-code", None), (200, false));
    assert_eq!(served_here(&fixture, "guarded", None), (401, false));
    assert_eq!(served_here(&fixture, "nosuch", None), (401, false));
    assert_eq!(authz_failures(&fixture), before);
    assert_eq!(
        served_here(&fixture, "guarded", Some(basic(WRITER, "wrong"))),
        (401, true)
    );
    assert_eq!(
        served_here(&fixture, "guarded", Some(basic(STRANGER, PASSWORD))),
        (404, false)
    );
    // a header that is there and is not credentials is not "no credentials": it reads nothing, a public repository
    // included, and is an auth failure (web protection's anonymous limit does not count a request that has one)
    let no_colon = format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "nocolon")
    );
    for junk in ["x", "Basic !!!", no_colon.as_str(), "Negotiate abc", ""] {
        for repository in ["pub-code", "guarded"] {
            assert_eq!(
                served_here(&fixture, repository, Some(junk.to_string())),
                (401, true),
                "{junk:?} on {repository}"
            );
        }
    }
    assert_eq!(
        served_with(&fixture, "pub-code", None, Some(String::new())),
        (401, true)
    );
    let (status, _, _) = http(
        &base,
        "GET",
        &format!("/git/pub-code.git/info/lfs/objects/{oid}"),
        &[("Authorization", "MoquiLfs v1.junk.junk")],
        b"",
    );
    assert_eq!(status, 401, "an LFS token that is not good");
}

/// Nobody pushes without a login, even where a record lets everyone do everything: the server itself refuses, before
/// any record is asked. A user the record allows does push.
#[test]
fn no_record_lets_a_request_with_no_credentials_push() {
    let fixture = public_fixture("anonymous-push");
    define(&fixture, "open-all", "db");
    let base = http_server(fixture.context.factory());
    let (status, _, body) = refs(&base, "open-all.git", "git-upload-pack", None);
    assert_eq!(status, 200, "{body}");
    let (status, headers, body) = refs(&base, "open-all.git", "git-receive-pack", None);
    assert_eq!(status, 401, "{body}");
    assert!(headers.contains_key("www-authenticate"), "{headers:?}");
    let (status, _, missing) = refs(&base, "nosuch.git", "git-receive-pack", None);
    assert_eq!((status, &missing), (401, &body));
    let (status, _, body) = refs(
        &base,
        "open-all.git",
        "git-receive-pack",
        Some(&basic(STRANGER, PASSWORD)),
    );
    assert_eq!(status, 200, "{body}");
}

/// Git LFS with no credentials: an object of a repository anyone may view is downloaded, and nothing else is done
/// or told: no object of another repository, no upload, no verify, and no lock is listed (locks name their owners),
/// made, verified or released.
#[test]
fn lfs_with_no_credentials_downloads_and_nothing_else() {
    let fixture = public_fixture("anonymous-lfs");
    moqui_lib::env::set_property("git_lfs_token_key", "the key of the anonymous lfs test".to_string());
    define(&fixture, "pub-code", "db");
    define(&fixture, "guarded", "db");
    let base = http_server(fixture.context.factory());
    let content = b"an lfs object";
    let oid = moqui_core::resource::blob::blob_id(content);
    let writer = basic(WRITER, PASSWORD);
    for repository in ["pub-code", "guarded"] {
        let (status, _, body) = http(
            &base,
            "PUT",
            &format!("/git/{repository}.git/info/lfs/objects/{oid}"),
            &[("Authorization", &writer)],
            content,
        );
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    }
    let object = format!("/info/lfs/objects/{oid}");
    let (status, _, body) = http(&base, "GET", &format!("/git/pub-code.git{object}"), &[], b"");
    assert_eq!(status, 200);
    assert_eq!(body, content);
    let json = [("Content-Type", "application/vnd.git-lfs+json")];
    let verify = format!(r#"{{"oid":"{oid}","size":{}}}"#, content.len());
    let refused: [(&str, &str, String, &[u8]); 9] = [
        ("GET", "guarded", object.clone(), b""),
        ("GET", "nosuch", object.clone(), b""),
        ("PUT", "pub-code", object.clone(), content),
        ("POST", "pub-code", "/info/lfs/verify".into(), verify.as_bytes()),
        ("GET", "pub-code", "/info/lfs/locks".into(), b""),
        ("GET", "pub-code", "/info/lfs/locks?path=a.bin".into(), b""),
        ("POST", "pub-code", "/info/lfs/locks".into(), br#"{"path":"a.bin"}"#),
        ("POST", "pub-code", "/info/lfs/locks/verify".into(), b"{}"),
        ("POST", "pub-code", "/info/lfs/locks/1/unlock".into(), b"{}"),
    ];
    for (method, repository, path, body) in refused {
        let (status, headers, answer) = http(&base, method, &format!("/git/{repository}.git{path}"), &json, body);
        assert_eq!(
            status,
            401,
            "{method} {repository}{path}: {}",
            String::from_utf8_lossy(&answer)
        );
        assert!(headers.contains_key("lfs-authenticate"), "{method} {path}: {headers:?}");
    }
    // a user lists the locks
    let (status, _, _) = http(
        &base,
        "GET",
        "/git/pub-code.git/info/lfs/locks",
        &[("Authorization", &writer)],
        b"",
    );
    assert_eq!(status, 200);
}

/// Requests with no credentials take at most `anonymous-max-concurrent` git jobs at once (half of `max-concurrent`,
/// so 2 of 4 here): one more is 503 with `Retry-After`. A signed-in user is not counted, and a request that is only
/// asked to log in takes no slot.
#[test]
fn requests_with_no_credentials_share_a_part_of_the_git_pool() {
    let fixture = public_fixture("anonymous-cap");
    define(&fixture, "pub-code", "db");
    define(&fixture, "guarded", "db");
    let held: Vec<_> = {
        let _entered = fixture.context.enter();
        (0..2).map(|_| super::anonymous_slot().expect("a free slot")).collect()
    };
    assert_eq!(served_here(&fixture, "pub-code", None), (503, false));
    assert_eq!(served_here(&fixture, "guarded", None), (401, false));
    assert_eq!(
        served_here(&fixture, "pub-code", Some(basic(WRITER, PASSWORD))),
        (200, false)
    );
    drop(held);
    assert_eq!(served_here(&fixture, "pub-code", None), (200, false));
    // each request gave its slot back
    for _ in 0..3 {
        assert_eq!(served_here(&fixture, "pub-code", None), (200, false));
    }
}

/// With no authorization record for it, a request with no credentials views nothing: the framework alone has no
/// anonymous access.
#[test]
fn without_a_record_for_everyone_no_credentials_views_nothing() {
    let fixture = fixture_in("anonymous-none", moqui_core::context::framework_dir());
    define(&fixture, "pub-code", "db");
    assert_eq!(served_here(&fixture, "pub-code", None), (401, false));
    assert_eq!(
        served_here(&fixture, "pub-code", Some(basic(WRITER, PASSWORD))),
        (200, false)
    );
}

/// F2: a repository is reached by its path of several segments, by its id, and by a path it had before it was renamed,
/// over HTTP, a stateful connection (as SSH) and LFS; the clone URLs carry the path; a path that is not one, another
/// repository's path and another repository's id are refused.
#[test]
fn a_repository_is_reached_by_its_path_its_id_and_a_path_it_had() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = public_fixture("paths");
    moqui_lib::env::set_property("git_lfs_token_key", "the key of the paths test".to_string());
    define(&fixture, "p1", "db");
    define(&fixture, "p2", "db");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    assert_eq!(set_path(&fixture, "p1", "moqui/mr-moqui"), "");
    seeded(&fixture, &base, &home, "moqui/mr-moqui", "one.txt");
    seeded(&fixture, &base, &home, "p2", "two.txt");

    let lists = |path: &str| {
        let (ok, out) = git_in(&fixture.dir, &home, &["ls-remote", &url_with(&base, WRITER, path)]);
        ok && out.contains("refs/heads/main")
    };
    let head = |path: &str| {
        git_ok(&fixture.dir, &home, &["ls-remote", &url_with(&base, WRITER, path)])
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let first = head("moqui/mr-moqui");
    // a path in another case finds the lower-case one when nothing has it exactly; an id is exact
    assert_eq!(head("Moqui/MR-Moqui"), first);
    assert!(!lists("P1"));
    // the id still works, and `.git` is optional (url_with adds it; here without)
    assert_eq!(head("p1"), first);
    let bare = url_with(&base, WRITER, "moqui/mr-moqui");
    let out = git_ok(&fixture.dir, &home, &["ls-remote", bare.trim_end_matches(".git")]);
    assert!(out.starts_with(&first), "{out}");
    assert_ne!(head("p2"), first);

    // the URLs and the list carry the path
    let (urls, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.get#GitCloneUrls",
        &[("repositoryId", "p1")],
    );
    assert_eq!(errors, "");
    let https = urls.get("httpsUrl").map(to_plain_string).unwrap_or_default();
    assert!(https.ends_with("/git/moqui/mr-moqui.git"), "{https}");
    assert_eq!(
        urls.get("repositoryPath").map(to_plain_string).as_deref(),
        Some("moqui/mr-moqui")
    );
    let (found, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.find#GitRepositories",
        &[("text", "mr-moqui")],
    );
    assert_eq!(errors, "");
    let Some(Value::List(list)) = found.get("repositoryList") else {
        panic!("{found:?}")
    };
    let [Value::Map(only)] = list.as_slice() else {
        panic!("{list:?}")
    };
    assert_eq!(
        only.get("repositoryPath").map(to_plain_string).as_deref(),
        Some("moqui/mr-moqui")
    );

    // what is refused: not a path, another repository's path, another repository's id; and only with the update right
    for bad in ["../etc", "a//b", "-x/y", "a/.b", "a b", "a/b.git", "a/b/c/d/e/f/g/h/i"] {
        let errors = set_path(&fixture, "p2", bad);
        assert!(errors.contains("is not a repository path"), "{bad}: {errors}");
    }
    // paths are one path whatever their case, so no repository takes another's in other letters
    for taken in ["moqui/mr-moqui", "Moqui/mr-moqui", "MOQUI/MR-MOQUI"] {
        assert!(
            set_path(&fixture, "p2", taken).contains("another repository's"),
            "{taken}"
        );
    }
    // a one-segment address is an id's: no path stands in front of a repository's id, there now or made later
    for single in ["p1", "P1", "solo", "DRIVE_7"] {
        assert!(
            set_path(&fixture, "p2", single).contains("a path of one segment"),
            "{single}"
        );
    }
    assert!(!lists("solo"));
    // the same path in other letters is the same repository's: kept as written, and found in any case
    assert_eq!(set_path(&fixture, "p1", "Moqui/MR-Moqui"), "");
    assert_eq!(head("moqui/mr-moqui"), first);
    let (urls, _) = call(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.get#GitCloneUrls",
        &[("repositoryId", "p1")],
    );
    let https = urls.get("httpsUrl").map(to_plain_string).unwrap_or_default();
    assert!(https.ends_with("/git/Moqui/MR-Moqui.git"), "{https}");
    assert_eq!(set_path(&fixture, "p1", "moqui/mr-moqui"), "");
    // webhooks name the repository by its path
    {
        let _entered = fixture.context.enter();
        let json = crate::service::hosting::repository_json("p1", "main");
        assert_eq!(json["id"], "p1");
        assert_eq!(json["full_name"], "moqui/mr-moqui");
        assert_eq!(json["name"], "mr-moqui");
        let clone_url = json["clone_url"].as_str().unwrap_or_default();
        assert!(clone_url.ends_with("/git/moqui/mr-moqui.git"), "{clone_url}");
    }
    let (_, errors) = call(
        &fixture,
        STRANGER,
        SET_PATH,
        &[("repositoryId", "p2"), ("repositoryPath", "x/y")],
    );
    assert!(errors.contains("not found"), "{errors}");
    assert!(!lists("x/y"));

    // renamed: the new path and the old one both reach it, and the id
    let (out, errors) = call(
        &fixture,
        WRITER,
        SET_PATH,
        &[("repositoryId", "p1"), ("repositoryPath", "moqui/framework/core")],
    );
    assert_eq!(errors, "");
    assert_eq!(
        out.get("oldRepositoryPath").map(to_plain_string).as_deref(),
        Some("moqui/mr-moqui")
    );
    for path in ["moqui/framework/core", "moqui/mr-moqui", "Moqui/Mr-Moqui", "p1"] {
        assert_eq!(head(path), first, "{path}");
    }
    let clone = fixture.dir.join("by-old-path");
    git_ok(
        &fixture.dir,
        &home,
        &[
            "clone",
            &url_with(&base, WRITER, "moqui/mr-moqui"),
            &clone.to_string_lossy(),
        ],
    );
    assert!(clone.join("one.txt").is_file());

    // an old path of a repository the caller may not view answers as a path that names nothing
    let stranger = basic(STRANGER, PASSWORD);
    let (status, _, old) = refs(&base, "moqui/mr-moqui.git", "git-upload-pack", Some(&stranger));
    let (none_status, _, none) = refs(&base, "moqui/mr-nothing.git", "git-upload-pack", Some(&stranger));
    assert_eq!((status, none_status), (404, 404));
    assert_eq!(old.replace("mr-moqui", "X"), none.replace("mr-nothing", "X"));
    let (status, old_headers, old) = refs(&base, "moqui/mr-moqui.git", "git-upload-pack", None);
    let (none_status, none_headers, none) = refs(&base, "moqui/mr-nothing.git", "git-upload-pack", None);
    assert_eq!((status, none_status), (401, 401));
    assert_eq!((old, old_headers), (none, none_headers));

    // a stateful connection, as SSH: by the path, the old path and the id
    let daemon = daemon(fixture.context.factory(), WRITER);
    for path in ["moqui/framework/core.git", "moqui/mr-moqui", "p1.git"] {
        let out = git_ok(&fixture.dir, &home, &["ls-remote", &format!("{daemon}/{path}")]);
        assert!(out.starts_with(&first), "{path}: {out}");
    }
    let (ok, out) = git_in(
        &fixture.dir,
        &home,
        &["ls-remote", &format!("{daemon}/moqui/nosuch.git")],
    );
    assert!(!ok, "{out}");

    // LFS by the path: the actions name the path, and the token (made for the repository's id) is taken at it
    let lfs_headers = [
        ("Authorization", basic(WRITER, PASSWORD)),
        ("Content-Type", "application/vnd.git-lfs+json".to_string()),
    ];
    let lfs_headers: Vec<(&str, &str)> = lfs_headers
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let content = b"lfs";
    let oid = moqui_core::resource::blob::blob_id(content);
    let (status, _, body) = http(
        &base,
        "POST",
        "/git/moqui/mr-moqui.git/info/lfs/objects/batch",
        &lfs_headers,
        format!(r#"{{"operation":"upload","objects":[{{"oid":"{oid}","size":3}}]}}"#).as_bytes(),
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let upload = &reply["objects"][0]["actions"]["upload"];
    let href = upload["href"].as_str().unwrap();
    assert!(
        href.contains("/git/moqui/framework/core.git/info/lfs/objects/"),
        "{href}"
    );
    let token = upload["header"]["Authorization"].as_str().unwrap();
    let (status, _, body) = http(
        &base,
        "PUT",
        &href[href.find("/git/").unwrap()..],
        &[("Authorization", token)],
        content,
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    // the token is for this repository only, by whatever path
    let (status, _, _) = http(
        &base,
        "PUT",
        &format!("/git/p2.git/info/lfs/objects/{oid}"),
        &[("Authorization", token)],
        content,
    );
    assert_eq!(status, 401);

    // another repository takes the old path: it is that repository's now
    assert_eq!(set_path(&fixture, "p2", "moqui/mr-moqui"), "");
    assert_eq!(head("moqui/mr-moqui"), head("p2"));
    assert_eq!(head("moqui/framework/core"), first);
    // and a path cleared leaves the id, with the path it had still a way in
    assert_eq!(set_path(&fixture, "p1", ""), "");
    assert_eq!(head("p1"), first);
    assert_eq!(head("moqui/framework/core"), first);

    // a path that is not one names nothing
    for path in ["moqui/-x.git", "moqui/.hidden.git", "a/b/c/d/e/f/g/h/i.git"] {
        let (status, _, body) = refs(&base, path, "git-upload-pack", Some(&basic(WRITER, PASSWORD)));
        assert_eq!(status, 404, "{path}: {body}");
    }
}
