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

//! The git server against the git command line, without the host: a small HTTP/1.1 listener here calls
//! [`super::serve_http`] (stateless RPC, protocol v0 and v2) and a `git://` listener calls [`super::serve_ssh`] (one
//! stateful conversation, as over SSH), on the in-memory database. Each test is skipped when git is not installed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gix::objs::Kind;
use moqui_lib::object::Value;

use super::{pack_out, plan, GitHttpRequest, GitService, HttpResponder};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::resource::git::ops::{self, Author, Edit};
use crate::resource::git::store::{GitStore, MemoryStore};

pub(super) const WRITER: &str = "git.writer";
pub(super) const READER: &str = "git.reader";
pub(super) const PASSWORD: &str = "moqui-test-1";

pub(super) struct Fixture {
    pub(super) context: ExecutionContext,
    pub(super) entity: EntityFacade,
    pub(super) dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) fn create(entity: &EntityFacade, name: &str, pairs: &[(&str, &str)]) {
    let mut row = EntityValue::make(entity, name).unwrap();
    for (field, value) in pairs {
        row.set(*field, Value::string(*value));
    }
    row.create(entity).unwrap();
}

pub(super) fn user(entity: &EntityFacade, username: &str) {
    let mut row = EntityValue::make(entity, "moqui.security.UserAccount").unwrap();
    row.set("userId", Value::string(username));
    row.set("username", Value::string(username));
    row.set("userFullName", Value::string(format!("Test {username}")));
    row.set("emailAddress", Value::string(format!("{username}@example.com")));
    row.set(
        "currentPassword",
        Value::string(crate::user::hash_password(PASSWORD).unwrap()),
    );
    row.create(entity).unwrap();
}

/// Two users: the writer has every action on every repository, the reader only `view`.
pub(super) fn fixture(name: &str) -> Fixture {
    fixture_in(name, std::path::PathBuf::from("."))
}

/// [`fixture`] in a framework directory: with its services, for the drive rule.
pub(super) fn fixture_in(name: &str, framework: std::path::PathBuf) -> Fixture {
    fixture_with(name, framework, "")
}

/// [`fixture_in`] with more of the conf (`extra`, elements of `moqui-conf`).
pub(super) fn fixture_with(name: &str, framework: std::path::PathBuf, extra: &str) -> Fixture {
    let context = ExecutionContextFactory::test_context_in(
        &format!(
            r#"<moqui-conf><artifact-execution-facade>
            <artifact-execution type="AT_GIT_REPOSITORY" authz-enabled="true"/>
            <artifact-execution type="AT_ENTITY" authz-enabled="true"/>
        </artifact-execution-facade>{extra}</moqui-conf>"#
        ),
        crate::service::ServicePools::idle(),
        framework,
    );
    let entity = crate::testing::open();
    context.factory().set_entity(entity.clone());
    context.factory().resource().install_git().unwrap();
    let dir = std::env::temp_dir().join(format!("moqui-git-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    {
        let _entered = context.enter();
        let _authz = context.disable_authz();
        user(&entity, WRITER);
        user(&entity, READER);
        create(
            &entity,
            "moqui.security.ArtifactGroup",
            &[("artifactGroupId", "GIT_ALL")],
        );
        create(
            &entity,
            "moqui.security.ArtifactGroupMember",
            &[
                ("artifactGroupId", "GIT_ALL"),
                ("artifactName", ".*"),
                ("artifactTypeEnumId", "AT_GIT_REPOSITORY"),
                ("nameIsPattern", "Y"),
                ("inheritAuthz", "N"),
            ],
        );
        for (group, member, action) in [
            ("GIT_WRITERS", WRITER, "AUTHZA_ALL"),
            ("GIT_READERS", READER, "AUTHZA_VIEW"),
        ] {
            create(&entity, "moqui.security.UserGroup", &[("userGroupId", group)]);
            let mut row = EntityValue::make(&entity, "moqui.security.UserGroupMember").unwrap();
            row.set("userId", Value::string(member));
            row.set("userGroupId", Value::string(group));
            row.set(
                "fromDate",
                moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value,
            );
            row.create(&entity).unwrap();
            create(
                &entity,
                "moqui.security.ArtifactAuthz",
                &[
                    ("artifactAuthzId", group),
                    ("userGroupId", group),
                    ("artifactGroupId", "GIT_ALL"),
                    ("authzTypeEnumId", "AUTHZT_ALWAYS"),
                    ("authzActionEnumId", action),
                ],
            );
        }
    }
    Fixture { context, entity, dir }
}

pub(super) fn define(fixture: &Fixture, id: &str, kind: &str) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut row = EntityValue::make(&fixture.entity, crate::resource::git::GIT_REPOSITORY).unwrap();
    row.set("repositoryId", Value::string(id));
    row.set("repositoryKind", Value::string(kind));
    if kind != "db" {
        row.set(
            "localPath",
            Value::string(fixture.dir.join(format!("{id}.git")).to_string_lossy()),
        );
    }
    row.create(&fixture.entity).unwrap();
}

// ---- an HTTP/1.1 listener over serve_http ----

struct Chunked<R> {
    inner: R,
    left: usize,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            self.inner.read_line(&mut line)?;
            if line.trim().is_empty() {
                line.clear();
                self.inner.read_line(&mut line)?;
            }
            self.left = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16)
                .map_err(|_| std::io::Error::other(format!("bad chunk size {line:?}")))?;
            if self.left == 0 {
                self.done = true;
                let mut end = String::new();
                self.inner.read_line(&mut end)?;
                return Ok(0);
            }
        }
        let wanted = buf.len().min(self.left);
        let read = self.inner.read(&mut buf[..wanted])?;
        self.left -= read;
        if self.left == 0 {
            let mut end = [0u8; 2];
            self.inner.read_exact(&mut end)?;
        }
        Ok(read)
    }
}

struct Responder {
    stream: std::io::BufWriter<TcpStream>,
    status: Option<u16>,
}

impl HttpResponder for Responder {
    fn head(&mut self, status: u16, headers: &[(&str, String)]) -> std::io::Result<()> {
        self.status = Some(status);
        write!(self.stream, "HTTP/1.1 {status} X\r\n")?;
        for (name, value) in headers {
            write!(self.stream, "{name}: {value}\r\n")?;
        }
        write!(self.stream, "Connection: close\r\n\r\n")
    }

    fn body(&mut self) -> &mut dyn Write {
        &mut self.stream
    }
}

fn handle(factory: &Arc<ExecutionContextFactory>, stream: TcpStream) {
    let context = factory.new_context();
    let _entered = context.enter();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let mut headers = HashMap::new();
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
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let rest = path.strip_prefix("/git/").unwrap_or("");
    let (repository, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if let Some(lfs_path) = tail.strip_prefix("info/lfs/") {
        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<u64>().ok());
        let request = super::lfs::LfsHttpRequest {
            repository: super::repository_of(repository).unwrap_or_default(),
            method: method.clone(),
            path: lfs_path.to_string(),
            query: query.to_string(),
            authorization: headers.get("authorization").cloned(),
            api_key: None,
            range: headers.get("range").cloned(),
            content_length,
            secure: true,
            client_ip: "127.0.0.1".into(),
        };
        let mut body = reader.take(content_length.unwrap_or(0));
        let mut responder = Responder {
            stream: std::io::BufWriter::new(stream),
            status: None,
        };
        super::lfs::serve_lfs(&request, &mut body, &mut responder);
        let _ = responder.stream.flush();
        // a body not read (a refused request) is read, so closing does not reset the connection
        let _ = std::io::copy(&mut body, &mut std::io::sink());
        return;
    }
    let (service, advertise) = match (method.as_str(), tail) {
        ("GET", "info/refs") => (query.strip_prefix("service=").and_then(GitService::from_name), true),
        ("POST", name) => (GitService::from_name(name), false),
        _ => (None, false),
    };
    let mut responder = Responder {
        stream: std::io::BufWriter::new(stream),
        status: None,
    };
    let Some(service) = service else {
        responder.head(404, &[]).unwrap();
        return;
    };
    let request = GitHttpRequest {
        repository: super::repository_of(repository).unwrap_or_default(),
        service,
        advertise,
        git_protocol: headers.get("git-protocol").cloned(),
        gzip: headers.get("content-encoding").is_some_and(|value| value == "gzip"),
        authorization: headers.get("authorization").cloned(),
        api_key: headers.get("api_key").cloned(),
        secure: true,
        client_ip: "127.0.0.1".into(),
    };
    let mut body: Box<dyn Read> = if headers.get("transfer-encoding").is_some_and(|value| value == "chunked") {
        Box::new(Chunked {
            inner: reader,
            left: 0,
            done: false,
        })
    } else {
        let length: u64 = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        Box::new(reader.take(length))
    };
    super::serve_http(&request, &mut body, &mut responder);
    let _ = responder.stream.flush();
}

/// Serve HTTP on a free port of loopback; the base URL.
pub(super) fn http_server(factory: &Arc<ExecutionContextFactory>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let factory = Arc::clone(factory);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let factory = Arc::clone(&factory);
            std::thread::spawn(move || handle(&factory, stream));
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Serve `git://` on a free port: the first pkt-line names the service and repository (and `version=2` in its extra
/// parameters), then the conversation is stateful, as over SSH, as `username`.
fn daemon(factory: &Arc<ExecutionContextFactory>, username: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let factory = Arc::clone(factory);
    let username = username.to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let factory = Arc::clone(&factory);
            let username = username.clone();
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                let mut input = stream.try_clone().unwrap();
                let Some(super::pktline::Packet::Data(first)) = super::pktline::read(&mut input).unwrap() else {
                    return;
                };
                let mut fields = first.split(|byte| *byte == 0);
                let command = String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned();
                let protocol = fields
                    .map(|field| String::from_utf8_lossy(field).into_owned())
                    .find(|field| field.starts_with("version="));
                let (service, path) = command.split_once(' ').unwrap_or((&command, ""));
                let mut output = std::io::BufWriter::new(stream);
                let mut errors = Vec::new();
                super::serve_ssh(
                    &username,
                    &format!("{service} '{path}'"),
                    protocol.as_deref(),
                    &mut input,
                    &mut output,
                    &mut errors,
                );
                let _ = output.flush();
            });
        }
    });
    format!("git://127.0.0.1:{port}")
}

// ---- the git command line ----

pub(super) fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Run git in `dir` (with a home of its own, so no user config is read). Its output, and whether it succeeded.
pub(super) fn git_in(dir: &Path, home: &Path, args: &[&str]) -> (bool, String) {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "user.name=Test Pusher",
            "-c",
            "user.email=pusher@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(["-c", "credential.helper="])
        .args(args)
        .current_dir(dir)
        // nothing of the desktop: no config but this, no credential helper or keyring, and no askpass dialog when a
        // test sends no credentials (GIT_TERMINAL_PROMPT only stops the terminal prompt)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("XDG_CONFIG_HOME")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS", "false")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .env("GIT_ALLOW_PROTOCOL", "file:git:http:https");
    // MOQUI_GIT_TRACE=1 shows each pkt-line in the failure output
    if std::env::var_os("MOQUI_GIT_TRACE").is_some() {
        command.env("GIT_TRACE_PACKET", "1");
    }
    let started = std::time::Instant::now();
    let output = command.output().expect("git runs");
    if std::env::var_os("MOQUI_GIT_TIMES").is_some() {
        eprintln!("{:>6} ms git {args:?}", started.elapsed().as_millis());
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), text)
}

pub(super) fn git_ok(dir: &Path, home: &Path, args: &[&str]) -> String {
    let (ok, text) = git_in(dir, home, args);
    assert!(ok, "git {args:?} failed:\n{text}");
    text
}

pub(super) fn url_with(base: &str, username: &str, id: &str) -> String {
    base.replacen("http://", &format!("http://{username}:{PASSWORD}@"), 1) + &format!("/git/{id}.git")
}

pub(super) fn write_file(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

pub(super) fn head_of(fixture: &Fixture, id: &str, name: &str) -> Option<String> {
    let _entered = fixture.context.enter();
    let provider = fixture.context.factory().resource().git().unwrap().clone();
    provider
        .repo(id)
        .unwrap()
        .store
        .read_ref(name)
        .unwrap()
        .map(|id| id.to_string())
}

/// Clone, commit, push, fetch into another clone, and read the pushed file through `git://`, at a protocol version.
fn round_trip(fixture: &Fixture, kind: &str, version: &str) {
    let id = format!("rt-{kind}-v{version}");
    define(fixture, &id, kind);
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join(format!("home-{id}"));
    std::fs::create_dir_all(&home).unwrap();
    let url = url_with(&base, WRITER, &id);
    let protocol = format!("protocol.version={version}");

    // an empty repository clones, then takes a first push that names its default branch
    let first = fixture.dir.join(format!("first-{id}"));
    let out = git_ok(
        &fixture.dir,
        &home,
        &["-c", &protocol, "clone", &url, &first.to_string_lossy()],
    );
    assert!(out.contains("empty repository"), "{out}");
    write_file(&first, "README.md", "# Round trip\n");
    write_file(&first, "src/lib.rs", "pub fn one() -> u32 {\n    1\n}\n");
    git_ok(&first, &home, &["add", "."]);
    git_ok(&first, &home, &["commit", "-m", "First commit\n\nTask: RT-100001"]);
    git_ok(&first, &home, &["-c", &protocol, "push", "origin", "main"]);
    let pushed = git_ok(&first, &home, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(
        head_of(fixture, &id, "refs/heads/main").as_deref(),
        Some(pushed.as_str())
    );

    // a second clone sees it; a commit there is fetched into the first
    let second = fixture.dir.join(format!("second-{id}"));
    git_ok(
        &fixture.dir,
        &home,
        &["-c", &protocol, "clone", &url, &second.to_string_lossy()],
    );
    assert_eq!(
        std::fs::read_to_string(second.join("README.md")).unwrap(),
        "# Round trip\n"
    );
    for round in 0..3 {
        write_file(&second, &format!("notes/{round}.md"), &format!("note {round}\n"));
        git_ok(&second, &home, &["add", "."]);
        git_ok(&second, &home, &["commit", "-m", &format!("Note {round}")]);
    }
    git_ok(&second, &home, &["tag", "-a", "v1.0", "-m", "Release 1.0"]);
    git_ok(
        &second,
        &home,
        &["-c", &protocol, "push", "--follow-tags", "origin", "main"],
    );
    git_ok(&first, &home, &["-c", &protocol, "pull", "--ff-only", "origin", "main"]);
    git_ok(&first, &home, &["-c", &protocol, "fetch", "--tags", "origin"]);
    assert_eq!(std::fs::read_to_string(first.join("notes/2.md")).unwrap(), "note 2\n");
    assert!(git_ok(&first, &home, &["tag", "-l"]).contains("v1.0"));
    if let Some(fsck) = Some(git_ok(&first, &home, &["fsck", "--strict"])) {
        assert!(!fsck.contains("error"), "{fsck}");
    }

    // the pushed files are git:// resources
    {
        let _entered = fixture.context.enter();
        let resources = fixture.context.factory().resource();
        assert_eq!(
            resources
                .get(&format!("git://{id}/notes/1.md"))
                .unwrap()
                .read_text()
                .unwrap(),
            "note 1\n"
        );
        resources
            .write_bytes(&format!("git://{id}/notes/web.md"), b"from the web\n")
            .unwrap();
    }
    git_ok(&first, &home, &["-c", &protocol, "pull", "--ff-only", "origin", "main"]);
    assert_eq!(
        std::fs::read_to_string(first.join("notes/web.md")).unwrap(),
        "from the web\n"
    );

    // a branch, a force push, a delete
    git_ok(&first, &home, &["checkout", "-b", "feature"]);
    write_file(&first, "feature.txt", "feature\n");
    git_ok(&first, &home, &["add", "."]);
    git_ok(&first, &home, &["commit", "-m", "Feature"]);
    git_ok(&first, &home, &["-c", &protocol, "push", "origin", "feature"]);
    git_ok(&first, &home, &["commit", "--amend", "-m", "Feature, amended"]);
    let (ok, out) = git_in(&first, &home, &["-c", &protocol, "push", "origin", "feature"]);
    assert!(!ok && out.contains("rejected"), "{out}");
    git_ok(
        &first,
        &home,
        &["-c", &protocol, "push", "--force", "origin", "feature"],
    );
    git_ok(
        &first,
        &home,
        &["-c", &protocol, "push", "origin", "--delete", "feature"],
    );
    assert_eq!(head_of(fixture, &id, "refs/heads/feature"), None);
    let (ok, out) = git_in(&first, &home, &["-c", &protocol, "push", "origin", "--delete", "main"]);
    assert!(
        !ok && out.contains("deletion of the current branch prohibited"),
        "{out}"
    );
}

#[test]
fn http_clone_push_and_fetch_on_each_kind_and_protocol() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("http");
    for kind in ["local", "db"] {
        for version in ["0", "2"] {
            round_trip(&fixture, kind, version);
        }
    }
}

/// A text file of about 20 KB, with `round` changed in a few of its lines.
fn big_text(round: usize) -> String {
    (0..600)
        .map(|line| {
            if line % 97 == round % 97 {
                format!("line {line} was changed in round {round}\n")
            } else {
                format!("line {line} of a file that grows a history of small edits\n")
            }
        })
        .collect()
}

/// The `GitObject` rows of a repository: storage, kind and size by id.
fn object_rows(fixture: &Fixture, id: &str) -> HashMap<String, (String, String, i64)> {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let rows = fixture
        .entity
        .find("moqui.resource.GitObject")
        .unwrap()
        .eq("repositoryId", Value::string(id))
        .list()
        .unwrap();
    rows.iter()
        .map(|row| {
            let text = |name: &str| {
                row.get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            let size = text("objectSize").parse().unwrap_or(0);
            (text("objectId"), (text("storage"), text("objectType"), size))
        })
        .collect()
}

#[test]
fn a_db_repository_keeps_pushed_deltas_and_sends_them_back() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("deltas");
    let id = "deltas";
    define(&fixture, id, "db");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let url = url_with(&base, WRITER, id);
    let source = fixture.dir.join("source");
    git_ok(&fixture.dir, &home, &["clone", &url, &source.to_string_lossy()]);
    for round in 0..12 {
        write_file(&source, "big.txt", &big_text(round));
        git_ok(&source, &home, &["add", "."]);
        git_ok(&source, &home, &["commit", "-m", &format!("Round {round}")]);
    }
    git_ok(&source, &home, &["push", "origin", "main"]);

    // the push's deltas are rows of their own; commits are whole, and everything is compressed
    let rows = object_rows(&fixture, id);
    let deltas = rows.values().filter(|(storage, _, _)| storage == "delta").count();
    assert!(deltas >= 5, "{deltas} delta rows of {}", rows.len());
    assert!(rows
        .values()
        .all(|(storage, _, _)| storage == "zlib" || storage == "delta"));
    assert!(rows
        .values()
        .filter(|(_, kind, _)| kind == "commit")
        .all(|(storage, _, _)| storage == "zlib"));

    // every version reads back through the store as git has it
    {
        let _entered = fixture.context.enter();
        let provider = fixture.context.factory().resource().git().unwrap().clone();
        let store = &provider.repo(id).unwrap().store;
        for commit in git_ok(&source, &home, &["rev-list", "HEAD"]).lines() {
            let blob = git_ok(&source, &home, &["rev-parse", &format!("{commit}:big.txt")]);
            let blob = gix::ObjectId::from_hex(blob.trim().as_bytes()).unwrap();
            let expected = git_ok(&source, &home, &["cat-file", "blob", &blob.to_string()]);
            let (kind, data) = store.find(&blob).unwrap().unwrap();
            assert_eq!(kind, Kind::Blob);
            assert_eq!(String::from_utf8(data).unwrap(), expected);
        }
    }

    // a clone gets them as deltas, with ofs-delta (v2) and without (v0 with the capability turned off is not
    // possible from the client, so both protocols)
    for version in ["0", "2"] {
        let clone = fixture.dir.join(format!("clone-v{version}"));
        let protocol = format!("protocol.version={version}");
        // kept as a pack, small as it is
        let keep = ["-c", "transfer.unpackLimit=1", "-c", &protocol];
        git_ok(
            &fixture.dir,
            &home,
            &[&keep[..], &["clone", &url, &clone.to_string_lossy()]].concat(),
        );
        assert_eq!(std::fs::read_to_string(clone.join("big.txt")).unwrap(), big_text(11));
        let fsck = git_ok(&clone, &home, &["fsck", "--strict", "--full"]);
        assert!(!fsck.contains("error"), "{fsck}");
        let pack_dir = clone.join(".git/objects/pack");
        let index = std::fs::read_dir(&pack_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "idx"))
            .unwrap();
        let verified = git_ok(&clone, &home, &["verify-pack", "-v", &index.to_string_lossy()]);
        assert!(
            verified.contains("chain length"),
            "the clone's pack has no deltas:\n{verified}"
        );
    }

    // a fetch of one more edit is a thin pack: the delta against the version the client has
    let clone = fixture.dir.join("clone-v2");
    write_file(&source, "big.txt", &big_text(12));
    git_ok(&source, &home, &["commit", "-am", "Round 12"]);
    git_ok(&source, &home, &["push", "origin", "main"]);
    git_ok(&clone, &home, &["pull", "--ff-only", "origin", "main"]);
    assert_eq!(std::fs::read_to_string(clone.join("big.txt")).unwrap(), big_text(12));
    let fsck = git_ok(&clone, &home, &["fsck", "--strict", "--full"]);
    assert!(!fsck.contains("error"), "{fsck}");

    // the same objects pushed again (another branch) are not stored twice
    let before = object_rows(&fixture, id).len();
    git_ok(&source, &home, &["push", "origin", "main:copy"]);
    assert_eq!(object_rows(&fixture, id).len(), before);

    // a collection after a force push deletes what only the old branch had; a clone after it is whole
    git_ok(&source, &home, &["reset", "--hard", "HEAD~1"]);
    git_ok(&source, &home, &["push", "--force", "origin", "main"]);
    git_ok(&source, &home, &["push", "origin", "--delete", "copy"]);
    {
        let _entered = fixture.context.enter();
        let provider = fixture.context.factory().resource().git().unwrap().clone();
        let store = crate::resource::git::db::DbStore::new(
            fixture.entity.clone(),
            id,
            crate::resource::git::db::object_cache(),
        );
        store.stamp_all(jiff::Timestamp::now().as_millisecond() - 30 * 24 * 60 * 60 * 1000);
        let mut options = crate::resource::git::gc::GcOptions::from_conf(false, std::time::Duration::from_secs(120));
        options.reflog_millis = 0;
        // the updates of the pushes are older than the window, and done
        let _authz = fixture.context.disable_authz();
        fixture
            .entity
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .eq("repositoryId", Value::string(id))
            .update_all([("ecaStatus".to_string(), Value::string("done"))])
            .unwrap();
        let outcome = provider.gc(Some(id), &options).unwrap();
        let report = outcome[0].1.as_ref().unwrap();
        assert!(report.deleted >= 3, "the commit, tree and blob of round 12: {report:?}");
        let fsck = provider
            .fsck(id, std::time::Instant::now() + std::time::Duration::from_secs(60))
            .unwrap();
        assert!(fsck.errors.is_empty(), "{:?}", fsck.errors);
    }
    let again = fixture.dir.join("after-gc");
    git_ok(&fixture.dir, &home, &["clone", &url, &again.to_string_lossy()]);
    assert_eq!(std::fs::read_to_string(again.join("big.txt")).unwrap(), big_text(11));
    let fsck = git_ok(&again, &home, &["fsck", "--strict", "--full"]);
    assert!(!fsck.contains("error"), "{fsck}");
}

#[test]
fn large_objects_are_content_blobs_streamed_in_and_out() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("large");
    let id = "large";
    define(&fixture, id, "db");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let url = url_with(&base, WRITER, id);
    let source = fixture.dir.join("source");
    git_ok(&fixture.dir, &home, &["clone", &url, &source.to_string_lossy()]);
    // 9 MiB that does not compress, and 9 MiB of text
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let binary: Vec<u8> = (0..9 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    let text: String = (0..300_000)
        .map(|line| format!("line {line} of a large text file\n"))
        .collect();
    std::fs::write(source.join("big.bin"), &binary).unwrap();
    write_file(&source, "big.txt", &text);
    write_file(&source, "small.txt", "small\n");
    git_ok(&source, &home, &["add", "."]);
    git_ok(&source, &home, &["commit", "-m", "Large files"]);
    git_ok(&source, &home, &["push", "origin", "main"]);
    let rows = object_rows(&fixture, id);
    let blob_rows: Vec<_> = rows.values().filter(|(storage, _, _)| storage == "blob").collect();
    assert_eq!(blob_rows.len(), 2, "{rows:?}");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let blobs = fixture
            .entity
            .find("moqui.resource.ContentBlob")
            .unwrap()
            .list()
            .unwrap();
        assert_eq!(blobs.len(), 2);
        let resources = fixture.context.factory().resource();
        assert_eq!(
            resources
                .get(&format!("git://{id}/big.bin"))
                .unwrap()
                .read_bytes()
                .unwrap(),
            binary
        );
        assert_eq!(
            resources
                .get(&format!("git://{id}/big.txt"))
                .unwrap()
                .read_text()
                .unwrap(),
            text
        );
    }
    let clone = fixture.dir.join("clone");
    git_ok(
        &fixture.dir,
        &home,
        &["-c", "protocol.version=2", "clone", &url, &clone.to_string_lossy()],
    );
    assert_eq!(std::fs::read(clone.join("big.bin")).unwrap(), binary);
    assert_eq!(std::fs::read_to_string(clone.join("big.txt")).unwrap(), text);
    let fsck = git_ok(&clone, &home, &["fsck", "--strict", "--full"]);
    assert!(!fsck.contains("error"), "{fsck}");

    // the binary goes from the history; its object, then its blob, are collected
    git_ok(&source, &home, &["rm", "big.bin"]);
    git_ok(&source, &home, &["commit", "--amend", "-m", "Large text only"]);
    git_ok(&source, &home, &["push", "--force", "origin", "main"]);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let provider = fixture.context.factory().resource().git().unwrap().clone();
        let store = crate::resource::git::db::DbStore::new(
            fixture.entity.clone(),
            id,
            crate::resource::git::db::object_cache(),
        );
        store.stamp_all(jiff::Timestamp::now().as_millisecond() - 30 * 24 * 60 * 60 * 1000);
        fixture
            .entity
            .find("moqui.resource.GitUpdate")
            .unwrap()
            .eq("repositoryId", Value::string(id))
            .update_all([("ecaStatus".to_string(), Value::string("done"))])
            .unwrap();
        let mut options = crate::resource::git::gc::GcOptions::from_conf(false, std::time::Duration::from_secs(120));
        options.reflog_millis = 0;
        let outcome = provider.gc(Some(id), &options).unwrap();
        let report = outcome[0].1.as_ref().unwrap();
        assert!(report.deleted >= 3, "the old commit, its tree and big.bin: {report:?}");
        let blobs = crate::resource::blob::BlobStore::new(fixture.entity.clone());
        let tomorrow = jiff::Timestamp::now().as_millisecond() + 24 * 60 * 60 * 1000;
        let (deleted, bytes, _) = blobs.collect(tomorrow, false).unwrap();
        assert_eq!(
            (deleted, bytes),
            (1, binary.len() as u64),
            "the binary's blob; the text's is referenced"
        );
        let resources = fixture.context.factory().resource();
        assert_eq!(
            resources
                .get(&format!("git://{id}/big.txt"))
                .unwrap()
                .read_text()
                .unwrap(),
            text
        );
    }
    let again = fixture.dir.join("again");
    git_ok(&fixture.dir, &home, &["clone", &url, &again.to_string_lossy()]);
    assert!(!again.join("big.bin").exists());
    assert_eq!(std::fs::read_to_string(again.join("big.txt")).unwrap(), text);
}

/// One HTTP/1.1 request to `base`: the status, the headers by lowercase name, and the body.
fn http(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, HashMap<String, String>, Vec<u8>) {
    let address = base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    let text = String::from_utf8_lossy(&response[..split]).into_owned();
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    (status, headers, response[split + 4..].to_vec())
}

fn json_of(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).unwrap_or_else(|err| panic!("{err}: {}", String::from_utf8_lossy(body)))
}

#[test]
fn lfs_objects_are_uploaded_downloaded_and_locked_over_the_batch_api() {
    let fixture = fixture("lfs");
    moqui_lib::env::set_property("git_lfs_token_key", "the key of the lfs test".to_string());
    define(&fixture, "art", "db");
    define(&fixture, "other", "db");
    let base = http_server(fixture.context.factory());
    let basic = |user: &str| {
        format!(
            "Basic {}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("{user}:{PASSWORD}"))
        )
    };
    let (writer, reader) = (basic(WRITER), basic(READER));
    let lfs_json = "application/vnd.git-lfs+json";
    let content: Vec<u8> = (0..3_000_000u32).map(|n| (n * 7 % 251) as u8).collect();
    let oid = crate::resource::blob::blob_id(&content);
    let batch = |repo: &str, auth: &str, operation: &str| {
        let body = serde_json::json!({ "operation": operation, "transfers": ["basic"],
            "objects": [{ "oid": oid, "size": content.len() }], "hash_algo": "sha256" });
        http(
            &base,
            "POST",
            &format!("/git/{repo}.git/info/lfs/objects/batch"),
            &[
                ("Authorization", auth),
                ("Accept", lfs_json),
                ("Content-Type", lfs_json),
            ],
            body.to_string().as_bytes(),
        )
    };
    let path_of = |href: &str| href[href.find("/git/").unwrap()..].to_string();

    // without credentials: 401 with LFS-Authenticate
    let (status, headers, _) = http(&base, "POST", "/git/art.git/info/lfs/objects/batch", &[], b"{}");
    assert_eq!(status, 401);
    assert!(headers.contains_key("lfs-authenticate"));

    // the writer uploads: an upload and a verify action, each with a token
    let (status, _, body) = batch("art", &writer, "upload");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let reply = json_of(&body);
    let actions = &reply["objects"][0]["actions"];
    let upload = actions["upload"]["href"].as_str().unwrap().to_string();
    let token = actions["upload"]["header"]["Authorization"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(token.starts_with("MoquiLfs v1."));
    let verify = actions["verify"]["href"].as_str().unwrap().to_string();
    // content that is not the oid's is refused
    let (status, _, _) = http(
        &base,
        "PUT",
        &path_of(&upload),
        &[("Authorization", &token)],
        b"other bytes",
    );
    assert_eq!(status, 422);
    let (status, _, body) = http(&base, "PUT", &path_of(&upload), &[("Authorization", &token)], &content);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let verify_body = serde_json::json!({ "oid": oid, "size": content.len() }).to_string();
    let (status, _, _) = http(
        &base,
        "POST",
        &path_of(&verify),
        &[("Authorization", &token)],
        verify_body.as_bytes(),
    );
    assert_eq!(status, 200);
    // there now: no actions
    let (_, _, body) = batch("art", &writer, "upload");
    assert!(json_of(&body)["objects"][0].get("actions").is_none());

    // the reader may download, a range too, and may not upload
    let (status, _, body) = batch("art", &reader, "download");
    assert_eq!(status, 200);
    let download = json_of(&body)["objects"][0]["actions"]["download"].clone();
    let (href, token) = (
        download["href"].as_str().unwrap().to_string(),
        download["header"]["Authorization"].as_str().unwrap().to_string(),
    );
    let (status, headers, body) = http(&base, "GET", &path_of(&href), &[("Authorization", &token)], b"");
    assert_eq!(status, 200);
    assert_eq!(headers.get("etag"), Some(&format!("\"lfs-{oid}\"")));
    assert!(body == content);
    let (status, _, body) = http(
        &base,
        "GET",
        &path_of(&href),
        &[("Authorization", &token), ("Range", "bytes=1000-1999")],
        b"",
    );
    assert_eq!(status, 206);
    assert_eq!(body, content[1000..2000]);
    let (status, _, _) = batch("art", &reader, "upload");
    assert_eq!(status, 403);
    // a download token does not upload
    let (status, _, _) = http(&base, "PUT", &path_of(&upload), &[("Authorization", &token)], &content);
    assert_eq!(status, 403);

    // another repository does not have the object until it is uploaded there; then it is stored once
    let (_, _, body) = batch("other", &writer, "download");
    assert_eq!(json_of(&body)["objects"][0]["error"]["code"], 404);
    let (status, _, _) = http(
        &base,
        "GET",
        &format!("/git/other.git/info/lfs/objects/{oid}"),
        &[("Authorization", &writer)],
        b"",
    );
    assert_eq!(status, 404);
    let (status, _, _) = http(
        &base,
        "GET",
        &path_of(&href).replace("/art.git/", "/other.git/"),
        &[("Authorization", &token)],
        b"",
    );
    assert_eq!(status, 401, "a token is for its repository");
    let (_, _, body) = batch("other", &writer, "upload");
    let upload = json_of(&body)["objects"][0]["actions"]["upload"].clone();
    let (status, _, _) = http(
        &base,
        "PUT",
        &path_of(upload["href"].as_str().unwrap()),
        &[("Authorization", upload["header"]["Authorization"].as_str().unwrap())],
        &content,
    );
    assert_eq!(status, 200);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let blobs = fixture
            .entity
            .find("moqui.resource.ContentBlob")
            .unwrap()
            .list()
            .unwrap();
        assert_eq!(blobs.len(), 1);
        let objects = fixture
            .entity
            .find("moqui.resource.GitLfsObject")
            .unwrap()
            .list()
            .unwrap();
        assert_eq!(objects.len(), 2);
    }

    // locks
    let locks = "/git/art.git/info/lfs/locks";
    let lock_body = serde_json::json!({ "path": "images/logo.psd", "ref": { "name": "refs/heads/main" } }).to_string();
    let (status, _, body) = http(
        &base,
        "POST",
        locks,
        &[("Authorization", &writer)],
        lock_body.as_bytes(),
    );
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    let lock = json_of(&body)["lock"].clone();
    assert_eq!(lock["owner"]["name"], WRITER);
    let (status, _, _) = http(
        &base,
        "POST",
        locks,
        &[("Authorization", &writer)],
        lock_body.as_bytes(),
    );
    assert_eq!(status, 409);
    let (status, _, _) = http(
        &base,
        "POST",
        locks,
        &[("Authorization", &reader)],
        lock_body.as_bytes(),
    );
    assert_eq!(status, 403);
    let (status, _, body) = http(
        &base,
        "GET",
        &format!("{locks}?path=images/logo.psd"),
        &[("Authorization", &reader)],
        b"",
    );
    assert_eq!(status, 200);
    assert_eq!(json_of(&body)["locks"].as_array().unwrap().len(), 1);
    let (_, _, body) = http(
        &base,
        "POST",
        &format!("{locks}/verify"),
        &[("Authorization", &writer)],
        b"{}",
    );
    assert_eq!(json_of(&body)["ours"].as_array().unwrap().len(), 1);
    let id = lock["id"].as_str().unwrap();
    let (status, _, _) = http(
        &base,
        "POST",
        &format!("{locks}/{id}/unlock"),
        &[("Authorization", &writer)],
        b"{}",
    );
    assert_eq!(status, 200);
    let (_, _, body) = http(&base, "GET", locks, &[("Authorization", &writer)], b"");
    assert!(json_of(&body)["locks"].as_array().unwrap().is_empty());

    // over SSH, git-lfs-authenticate gives the endpoint and a token
    {
        let factory = Arc::clone(fixture.context.factory());
        let context = factory.new_context();
        let _entered = context.enter();
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let status = super::serve_ssh(
            WRITER,
            "git-lfs-authenticate 'art.git' upload",
            None,
            &mut &b""[..],
            &mut output,
            &mut errors,
        );
        assert_eq!(status, 0, "{}", String::from_utf8_lossy(&errors));
        let answer = json_of(&output);
        assert!(answer["href"].as_str().unwrap().ends_with("/git/art.git/info/lfs"));
        assert!(answer["header"]["Authorization"]
            .as_str()
            .unwrap()
            .starts_with("MoquiLfs "));
        let mut errors = Vec::new();
        let status = super::serve_ssh(
            READER,
            "git-lfs-authenticate 'art.git' upload",
            None,
            &mut &b""[..],
            &mut Vec::new(),
            &mut errors,
        );
        assert_ne!(status, 0, "the reader may not push");
        let status = super::serve_ssh(
            WRITER,
            "git-lfs-transfer 'art.git' upload",
            None,
            &mut &b""[..],
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_ne!(status, 0);
    }
    moqui_lib::env::clear_property("git_lfs_token_key");
}

#[test]
fn shallow_fetches_deepen_and_unshallow() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("shallow");
    let id = "shallow";
    define(&fixture, id, "local");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let url = url_with(&base, WRITER, id);
    let source = fixture.dir.join("source");
    git_ok(&fixture.dir, &home, &["clone", &url, &source.to_string_lossy()]);
    for round in 0..6 {
        write_file(&source, "file.txt", &format!("version {round}\n"));
        git_ok(&source, &home, &["add", "."]);
        git_ok(&source, &home, &["commit", "-m", &format!("Version {round}")]);
    }
    git_ok(&source, &home, &["push", "origin", "main"]);
    for version in ["0", "2"] {
        let protocol = format!("protocol.version={version}");
        let shallow = fixture.dir.join(format!("shallow-v{version}"));
        git_ok(
            &fixture.dir,
            &home,
            &[
                "-c",
                &protocol,
                "clone",
                "--depth",
                "1",
                &url,
                &shallow.to_string_lossy(),
            ],
        );
        let count = |dir: &Path| git_ok(dir, &home, &["rev-list", "--count", "HEAD"]).trim().to_string();
        assert_eq!(count(&shallow), "1");
        let out = git_ok(&shallow, &home, &["-c", &protocol, "fetch", "--deepen", "2", "origin"]);
        assert_eq!(count(&shallow), "3", "{out}");
        git_ok(&shallow, &home, &["-c", &protocol, "fetch", "--unshallow", "origin"]);
        assert_eq!(count(&shallow), "6");
        assert!(!shallow.join(".git/shallow").exists());
        let fsck = git_ok(&shallow, &home, &["fsck", "--strict"]);
        assert!(!fsck.contains("error"), "{fsck}");
    }
}

#[test]
fn stateful_conversations_over_a_daemon_connection() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("daemon");
    let id = "daemon";
    define(&fixture, id, "local");
    let base = daemon(fixture.context.factory(), WRITER);
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let url = format!("{base}/{id}.git");
    let work = fixture.dir.join("work");
    git_ok(&fixture.dir, &home, &["clone", &url, &work.to_string_lossy()]);
    write_file(&work, "a.txt", "a\n");
    git_ok(&work, &home, &["add", "."]);
    git_ok(&work, &home, &["commit", "-m", "A"]);
    git_ok(&work, &home, &["push", "origin", "main"]);
    for version in ["0", "2"] {
        let copy = fixture.dir.join(format!("copy-v{version}"));
        git_ok(
            &fixture.dir,
            &home,
            &[
                "-c",
                &format!("protocol.version={version}"),
                "clone",
                &url,
                &copy.to_string_lossy(),
            ],
        );
        assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "a\n");
    }
}

#[test]
fn credentials_and_rights_are_checked() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("rights");
    define(&fixture, "guarded", "db");
    define(&fixture, "mirror", "remote");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let work = fixture.dir.join("work");
    git_ok(
        &fixture.dir,
        &home,
        &["clone", &url_with(&base, WRITER, "guarded"), &work.to_string_lossy()],
    );
    write_file(&work, "a.txt", "a\n");
    git_ok(&work, &home, &["add", "."]);
    git_ok(&work, &home, &["commit", "-m", "A"]);
    git_ok(&work, &home, &["push", "origin", "main"]);

    // the reader may clone and not push
    let read = fixture.dir.join("read");
    git_ok(
        &fixture.dir,
        &home,
        &["clone", &url_with(&base, READER, "guarded"), &read.to_string_lossy()],
    );
    write_file(&read, "b.txt", "b\n");
    git_ok(&read, &home, &["add", "."]);
    git_ok(&read, &home, &["commit", "-m", "B"]);
    let (ok, out) = git_in(&read, &home, &["push", "origin", "main"]);
    assert!(!ok && out.contains("403"), "{out}");

    // a wrong password, no credentials, an unknown repository, a mirror
    let wrong = base.replacen("http://", &format!("http://{WRITER}:wrong@"), 1) + "/git/guarded.git";
    let (ok, out) = git_in(&fixture.dir, &home, &["ls-remote", &wrong]);
    assert!(
        !ok && (out.contains("401") || out.contains("Authentication failed")),
        "{out}"
    );
    let (ok, out) = git_in(&fixture.dir, &home, &["ls-remote", &format!("{base}/git/guarded.git")]);
    assert!(!ok, "{out}");
    let (ok, out) = git_in(&fixture.dir, &home, &["ls-remote", &url_with(&base, WRITER, "nosuch")]);
    assert!(!ok && out.contains("not found"), "{out}");
    let (ok, out) = git_in(&work, &home, &["push", &url_with(&base, WRITER, "mirror"), "main"]);
    assert!(!ok, "{out}");

    // an access token (a UserLoginKey) as the password
    let token = {
        let _entered = fixture.context.enter();
        crate::user::internal_login_user(WRITER).unwrap();
        let key = crate::user::get_login_key(Some(1.0)).unwrap();
        crate::user::logout().unwrap();
        key
    };
    let with_token = base.replacen("http://", &format!("http://{WRITER}:{token}@"), 1) + "/git/guarded.git";
    let out = git_ok(&fixture.dir, &home, &["ls-remote", &with_token]);
    assert!(out.contains("refs/heads/main"), "{out}");
}

/// A drive's members reach the repository the drive holds by the drive's own rule, without any AT_GIT_REPOSITORY
/// authorization: a viewer fetches and may not push, an editor pushes, a stranger finds nothing, and an app password
/// stands for the account password.
#[test]
fn a_drive_repository_takes_the_drive_members() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_in("drive-members", crate::context::framework_dir());
    define(&fixture, "DRIVE_D1", "db");
    let app_password = {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        for name in ["drive.owner", "drive.viewer", "drive.editor", "drive.stranger"] {
            user(&fixture.entity, name);
        }
        create(
            &fixture.entity,
            "moqui.resource.ResourceDrive",
            &[
                ("driveId", "D1"),
                ("driveName", "Specs"),
                ("driveKindEnumId", "RdkShared"),
                ("rootLocation", "git://DRIVE_D1/"),
                ("repositoryId", "DRIVE_D1"),
                ("ownerUserId", "drive.owner"),
            ],
        );
        for (id, member, permission) in [("M1", "drive.viewer", "RdpView"), ("M2", "drive.editor", "RdpEdit")] {
            let mut row = EntityValue::make(&fixture.entity, "moqui.resource.ResourceDriveMember").unwrap();
            for (field, value) in [
                ("driveMemberId", id),
                ("driveId", "D1"),
                ("userId", member),
                ("permissionEnumId", permission),
            ] {
                row.set(field, Value::string(value));
            }
            row.set(
                "fromDate",
                moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value,
            );
            row.create(&fixture.entity).unwrap();
        }
        crate::user::internal_login_user("drive.viewer").unwrap();
        let out = crate::service::sync(
            "org.moqui.impl.UserServices.create#UserAppPassword",
            &[("name".to_string(), Value::string("CI"))].into_iter().collect(),
        )
        .unwrap();
        crate::user::logout().unwrap();
        moqui_lib::object::to_plain_string(out.get("password").unwrap())
    };
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let work = fixture.dir.join("work");
    git_ok(
        &fixture.dir,
        &home,
        &[
            "clone",
            &url_with(&base, "drive.editor", "DRIVE_D1"),
            &work.to_string_lossy(),
        ],
    );
    write_file(&work, "spec.md", "the spec\n");
    git_ok(&work, &home, &["add", "."]);
    git_ok(&work, &home, &["commit", "-m", "Spec"]);
    git_ok(&work, &home, &["push", "origin", "main"]);
    assert!(head_of(&fixture, "DRIVE_D1", "refs/heads/main").is_some());

    let read = fixture.dir.join("read");
    git_ok(
        &fixture.dir,
        &home,
        &[
            "clone",
            &url_with(&base, "drive.viewer", "DRIVE_D1"),
            &read.to_string_lossy(),
        ],
    );
    assert!(read.join("spec.md").exists());
    write_file(&read, "more.md", "more\n");
    git_ok(&read, &home, &["add", "."]);
    git_ok(&read, &home, &["commit", "-m", "More"]);
    let (ok, out) = git_in(&read, &home, &["push", "origin", "main"]);
    assert!(!ok && out.contains("403"), "a viewer may not push: {out}");

    let (ok, out) = git_in(
        &fixture.dir,
        &home,
        &["ls-remote", &url_with(&base, "drive.stranger", "DRIVE_D1")],
    );
    assert!(!ok && out.contains("not found"), "{out}");

    let with_app_password =
        base.replacen("http://", &format!("http://drive.viewer:{app_password}@"), 1) + "/git/DRIVE_D1.git";
    let out = git_ok(&fixture.dir, &home, &["ls-remote", &with_app_password]);
    assert!(out.contains("refs/heads/main"), "{out}");
}

// ---- packs without the command line ----

fn commit_files(store: &MemoryStore, files: &[(&str, &str)], message: &str) -> gix::ObjectId {
    let edits: Vec<Edit> = files
        .iter()
        .map(|(path, text)| {
            let blob = store.write(Kind::Blob, text.as_bytes()).unwrap();
            Edit::Put(
                path.split('/').map(str::to_string).collect(),
                gix::objs::tree::EntryKind::Blob,
                blob,
            )
        })
        .collect();
    let author = Author {
        name: "Test".into(),
        email: "test@example.com".into(),
        user_id: None,
        seconds: None,
    };
    ops::commit(store, "main", &author, |_| Ok(Some((edits.clone(), message.into()))))
        .unwrap()
        .unwrap()
}

#[test]
fn a_planned_pack_holds_what_the_client_lacks() {
    let store = MemoryStore::default();
    let first = commit_files(&store, &[("a.txt", "a\n"), ("dir/b.txt", "b\n")], "one");
    let second = commit_files(&store, &[("a.txt", "a2\n")], "two");
    let all = plan::plan(&store, &[second], &[], &[], &Default::default(), &[]).unwrap();
    // two commits, the trees (root twice, dir once) and three blobs
    assert_eq!(all.commits, 2);
    assert_eq!(all.objects.len(), 2 + 3 + 3);
    let delta = plan::plan(&store, &[second], &[first], &[], &Default::default(), &[]).unwrap();
    // the new commit, its root tree and the new blob: dir/ and b.txt are under the client's tree
    assert_eq!(delta.commits, 1);
    assert_eq!(delta.objects.len(), 3);
    let mut pack = Vec::new();
    let deltas = pack_out::Deltas { ofs: true, thin: None };
    pack_out::write_pack(&store, &all.objects, deltas, None, &mut pack).unwrap();
    let read = pack_out::read_back(&pack);
    assert_eq!(read.len(), all.objects.len());
    for (id, kind) in read {
        assert_eq!(store.find(&id).unwrap().map(|(found, _)| found), Some(kind));
    }
}

#[test]
fn ssh_commands_name_a_service_and_a_repository() {
    use super::parse_ssh_command;
    assert_eq!(
        parse_ssh_command("git-upload-pack '/code.git'"),
        Some((GitService::UploadPack, "code".to_string()))
    );
    assert_eq!(
        parse_ssh_command("git receive-pack 'git/code'"),
        Some((GitService::ReceivePack, "code".to_string()))
    );
    assert_eq!(
        parse_ssh_command("git-upload-pack code.git"),
        Some((GitService::UploadPack, "code".to_string()))
    );
    assert_eq!(parse_ssh_command("git-upload-archive '/code.git'"), None);
    assert_eq!(parse_ssh_command("git-upload-pack '../etc/passwd'"), None);
    assert_eq!(parse_ssh_command("git-upload-packx 'code'"), None);
}

// ---- gitECA rules on pushes ----

const RULES: &str = r#"<gitecas>
    <giteca rule-name="NoWip" event="pre-receive">
        <condition><expression>newCommits.some(c => c.summary.startsWith('WIP'))</expression></condition>
        <actions><message error="true">WIP commits are not accepted</message></actions>
    </giteca>
    <giteca rule-name="Hello" event="pre-receive"><actions><message>Hello from moqui</message></actions></giteca>
    <giteca rule-name="ProtectMain" event="pre-ref-update" ref-pattern="refs/heads/main">
        <condition><expression>changeType == 'force'</expression></condition>
        <actions><message error="true">main cannot be forced</message></actions>
    </giteca>
    <giteca rule-name="RecordCommits" event="commit">
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="row"
                map="{testId: 'C' + commit.shortId, testMedium: commit.summary, value: refName, testLong: (commit.trailers.Task || []).join(',')}"/>
            <entity-create value-field="row"/>
        </actions>
    </giteca>
    <giteca rule-name="RecordTags" event="tag">
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="row"
                map="{testId: 'T' + tag.tagName, testMedium: tag.message ?? '', value: tag.annotated ? 'annotated' : 'light'}"/>
            <entity-create value-field="row"/>
        </actions>
    </giteca>
    <giteca rule-name="RecordMain" event="ref-update" default-branch-only="true">
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="row"
                map="{testId: 'R' + updateId, testMedium: changeType, testNumberInteger: newCommitIds.length}"/>
            <entity-create value-field="row"/>
        </actions>
    </giteca>
    <giteca rule-name="FailsOnPurpose" event="post-receive" priority="9">
        <actions><message error="true">post rule failed on purpose</message></actions>
    </giteca>
</gitecas>"#;

/// Wait until no update of the fixture is still to run.
pub(super) fn settled(fixture: &Fixture) -> Vec<EntityValue> {
    let started = std::time::Instant::now();
    loop {
        let rows = {
            let _entered = fixture.context.enter();
            let _authz = fixture.context.disable_authz();
            fixture
                .entity
                .find(crate::resource::git::events::GIT_UPDATE)
                .unwrap()
                .order_by("updateId", false)
                .list()
                .unwrap()
        };
        let busy = rows.iter().any(|row| {
            matches!(
                row.get("ecaStatus").and_then(Value::as_str),
                Some("recording" | "pending" | "running")
            )
        });
        if !busy {
            return rows.iter().cloned().collect();
        }
        assert!(started.elapsed().as_secs() < 30, "git updates did not settle: {rows:?}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

pub(super) fn test_rows(fixture: &Fixture) -> HashMap<String, EntityValue> {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    fixture
        .entity
        .find("moqui.test.TestEntity")
        .unwrap()
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get("testId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                row.clone(),
            )
        })
        .collect()
}

#[test]
fn git_eca_rules_check_pushes_and_see_their_commits_and_tags() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture("eca");
    crate::service::git_eca::install(fixture.context.factory(), RULES);
    define(&fixture, "eca", "db");
    let base = http_server(fixture.context.factory());
    let home = fixture.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let url = url_with(&base, WRITER, "eca");
    let work = fixture.dir.join("work");
    git_ok(&fixture.dir, &home, &["clone", &url, &work.to_string_lossy()]);

    // a pre-receive rule refuses a WIP commit; every rule's messages show
    write_file(&work, "a.txt", "a\n");
    git_ok(&work, &home, &["add", "."]);
    git_ok(&work, &home, &["commit", "-m", "WIP first try"]);
    let (ok, out) = git_in(&work, &home, &["push", "origin", "main"]);
    assert!(
        !ok && out.contains("WIP commits are not accepted") && out.contains("Hello from moqui"),
        "{out}"
    );
    assert_eq!(head_of(&fixture, "eca", "refs/heads/main"), None);

    // accepted: the commit rule sees each commit, its ref and its trailers
    git_ok(
        &work,
        &home,
        &["commit", "--amend", "-m", "First commit\n\nTask: ECA-100001"],
    );
    write_file(&work, "b.txt", "b\n");
    git_ok(&work, &home, &["add", "."]);
    git_ok(&work, &home, &["commit", "-m", "Second commit"]);
    let out = git_ok(&work, &home, &["push", "origin", "main"]);
    assert!(out.contains("Hello from moqui"), "{out}");
    git_ok(&work, &home, &["tag", "-a", "v1", "-m", "Version one"]);
    git_ok(&work, &home, &["tag", "light"]);
    git_ok(&work, &home, &["push", "origin", "--tags"]);
    let updates = settled(&fixture);
    let rows = test_rows(&fixture);
    let first = git_ok(&work, &home, &["rev-parse", "HEAD~1"]);
    let first = rows
        .get(&format!("C{}", &first.trim()[..10]))
        .expect("the first commit's row");
    assert_eq!(first.get("testMedium").and_then(Value::as_str), Some("First commit"));
    assert_eq!(first.get("value").and_then(Value::as_str), Some("refs/heads/main"));
    assert_eq!(first.get("testLong").and_then(Value::as_str), Some("ECA-100001"));
    assert_eq!(
        rows.keys().filter(|id| id.starts_with('C')).count(),
        2,
        "{:?}",
        rows.keys()
    );
    assert_eq!(rows["Tv1"].get("value").and_then(Value::as_str), Some("annotated"));
    assert_eq!(
        rows["Tv1"].get("testMedium").and_then(Value::as_str),
        Some("Version one")
    );
    assert_eq!(rows["Tlight"].get("value").and_then(Value::as_str), Some("light"));

    // the push log: the refused push, the branch push and the tag push; the failing rule is recorded and the others ran
    let field = |row: &EntityValue, name: &str| row.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let statuses: Vec<String> = updates.iter().map(|row| field(row, "ecaStatus")).collect();
    assert_eq!(statuses, ["none", "failed", "failed"], "{updates:?}");
    assert!(field(&updates[1], "ecaError").contains("post rule failed on purpose"));
    let main = rows
        .get(&format!("R{}", field(&updates[1], "updateId")))
        .expect("the ref-update row of main");
    assert_eq!(main.get("testMedium").and_then(Value::as_str), Some("create"));
    assert_eq!(
        moqui_lib::object::to_plain_string(main.get("testNumberInteger").unwrap_or(&Value::Null)),
        "2"
    );

    // a pre-ref-update rule protects main from a force push
    git_ok(&work, &home, &["commit", "--amend", "-m", "Second commit, rewritten"]);
    let (ok, out) = git_in(&work, &home, &["push", "--force", "origin", "main"]);
    assert!(!ok && out.contains("main cannot be forced"), "{out}");
}

#[test]
fn mirror_fetches_and_opted_in_writes_raise_events() {
    if !git_available() {
        eprintln!("skipped: a file:// fetch runs git-upload-pack, and git is not on the PATH");
        return;
    }
    let fixture = fixture("sources");
    crate::service::git_eca::install(
        fixture.context.factory(),
        r#"<gitecas>
            <giteca rule-name="FetchedCommits" event="commit" source="fetch">
                <actions>
                    <entity-make-value entity-name="moqui.test.TestEntity" value-field="row"
                        map="{testId: 'F' + commit.shortId, testMedium: commit.summary, value: source}"/>
                    <entity-create value-field="row"/>
                </actions>
            </giteca>
            <giteca rule-name="Writes" event="commit" source="write" repository-pattern="src.*">
                <actions>
                    <entity-make-value entity-name="moqui.test.TestEntity" value-field="row"
                        map="{testId: 'W' + commit.shortId, testMedium: commit.summary, value: userId ?? ''}"/>
                    <entity-create value-field="row"/>
                </actions>
            </giteca>
        </gitecas>"#,
    );
    define(&fixture, "src", "local");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let mut row = EntityValue::make(&fixture.entity, crate::resource::git::GIT_REPOSITORY).unwrap();
        row.set("repositoryId", Value::string("mirror"));
        row.set("repositoryKind", Value::string("remote"));
        row.set(
            "localPath",
            Value::string(fixture.dir.join("mirror.git").to_string_lossy()),
        );
        row.set(
            "remoteUrl",
            Value::string(format!("file://{}", fixture.dir.join("src.git").display())),
        );
        row.create(&fixture.entity).unwrap();
    }
    {
        let _entered = fixture.context.enter();
        crate::user::internal_login_user(WRITER).unwrap();
        let resources = fixture.context.factory().resource();
        resources.write_bytes("git://src/a.md", b"one\n").unwrap();
        resources.write_bytes("git://src/a.md", b"two\n").unwrap();
        crate::user::logout().unwrap();
    }
    // the in-memory test database commits whole tables, so the runs of two repositories must not overlap
    settled(&fixture);
    {
        let _entered = fixture.context.enter();
        crate::user::internal_login_user(WRITER).unwrap();
        fixture
            .context
            .factory()
            .resource()
            .git()
            .unwrap()
            .fetch("mirror")
            .unwrap();
        crate::user::logout().unwrap();
    }
    settled(&fixture);
    let rows = test_rows(&fixture);
    let writes: Vec<&EntityValue> = rows
        .iter()
        .filter(|(id, _)| id.starts_with('W'))
        .map(|(_, row)| row)
        .collect();
    assert_eq!(writes.len(), 2, "{:?}", rows.keys());
    assert!(writes
        .iter()
        .all(|row| row.get("value").and_then(Value::as_str) == Some(WRITER)));
    let fetched: Vec<&EntityValue> = rows
        .iter()
        .filter(|(id, _)| id.starts_with('F'))
        .map(|(_, row)| row)
        .collect();
    assert_eq!(fetched.len(), 2, "{:?}", rows.keys());
    assert!(fetched
        .iter()
        .all(|row| row.get("value").and_then(Value::as_str) == Some("fetch")));
}
