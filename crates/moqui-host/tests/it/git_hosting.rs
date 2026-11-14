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

//! The git server through the host on the seeded embedded Postgres: smart HTTP at `/git/<id>.git` (the request
//! streamed into and the response out of the git pool), and `git-upload-pack` / `git-receive-pack` over the built-in
//! SSH server with a `UserSshKey`, by the git command line (skipped when git, or for SSH ssh, is not installed). A
//! repository with a path is cloned at `/git/<path>.git`, over SSH by the path, and at its page's URL on a public-site
//! host. The protocol itself, over each storage, is `moqui-git/src/server/tests.rs`.

#![cfg(all(
    unix,
    feature = "embedded-postgres",
    feature = "axum",
    feature = "git",
    feature = "ssh"
))]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use base64::Engine;
use moqui_lib::object::Value;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{HashAlg, PrivateKey};

const HOST_SEED: [u8; 32] = [5; 32];
const USER_SEED: [u8; 32] = [6; 32];
/// The key of pub.user, an account in no group (only the automatic `ALL_USERS`), so without `SSH_GIT`.
const PUBLIC_SEED: [u8; 32] = [7; 32];

fn data(user_key: &PrivateKey) -> String {
    let public = user_key.public_key().to_openssh().unwrap();
    let fingerprint = user_key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let public_key = PrivateKey::from(Ed25519Keypair::from_seed(&PUBLIC_SEED));
    let pub_public = public_key.public_key().to_openssh().unwrap();
    let pub_fingerprint = public_key.public_key().fingerprint(HashAlg::Sha256).to_string();
    format!(
        r#"<entity-facade-xml type="seed">
    <moqui.resource.GitRepository repositoryId="hosted" repositoryKind="local" description="Hosted on disk"
        repositoryPath="team/hosted" repositoryPathKey="team/hosted"/>
    <moqui.resource.GitRepository repositoryId="hosted-db" repositoryKind="db" description="Hosted in the database"/>
    <moqui.resource.GitRepository repositoryId="mirror-db" repositoryKind="db" description="A push mirror's target"/>
    <moqui.security.UserSshKey sshKeyId="JOHN_GIT_KEY" userId="EX_JOHN_DOE" publicKey="{public}"
        fingerprint="{fingerprint}" fromDate="1265184000000"/>
    <!-- pub.user is in no group; every account may view pub-db, and nothing else -->
    <moqui.resource.GitRepository repositoryId="pub-db" repositoryKind="db" description="Anyone may view"/>
    <moqui.security.UserAccount userId="PUB_USER" username="pub.user" userFullName="Public User"/>
    <moqui.security.UserSshKey sshKeyId="PUB_GIT_KEY" userId="PUB_USER" publicKey="{pub_public}"
        fingerprint="{pub_fingerprint}" fromDate="1265184000000"/>
    <moqui.security.ArtifactGroup artifactGroupId="GIT_PUB_DB" description="pub-db"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="GIT_PUB_DB" artifactName="pub-db"
        artifactTypeEnumId="AT_GIT_REPOSITORY"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="GIT_PUB_DB_VIEW" userGroupId="ALL_USERS" artifactGroupId="GIT_PUB_DB"
        authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_VIEW"/>
</entity-facade-xml>"#
    )
}

fn git_available(program: &str) -> bool {
    Command::new(program)
        .arg(if program == "ssh" { "-V" } else { "--version" })
        .output()
        .is_ok_and(|output| output.status.success())
}

/// git isolated from the desktop: no config, credential helper or askpass dialog of the user running the test.
fn git(dir: &Path, home: &Path, ssh: Option<&str>, args: &[&str]) -> (bool, String) {
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
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("XDG_CONFIG_HOME")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS", "false")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .env_remove("SSH_AUTH_SOCK");
    if let Some(ssh) = ssh {
        command.env("GIT_SSH_COMMAND", ssh);
    }
    let output = command.output().expect("git runs");
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), text)
}

fn git_ok(dir: &Path, home: &Path, ssh: Option<&str>, args: &[&str]) -> String {
    let (ok, text) = git(dir, home, ssh, args);
    assert!(ok, "git {args:?} failed:\n{text}");
    text
}

fn commit(dir: &Path, home: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    git_ok(dir, home, None, &["add", "."]);
    git_ok(dir, home, None, &["commit", "-m", &format!("Write {name}")]);
}

fn sync(name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let called = moqui_core::service::sync(name, &parameters);
    let mut errors = moqui_core::context::ExecutionContext::with_current(|ctx| {
        let text = ctx.message().errors().join("\n");
        ctx.message().clear();
        text
    })
    .unwrap_or_default();
    // a call that fails outright is an error too, not an empty result
    let result = called.unwrap_or_else(|err| {
        errors.push_str(err.message());
        BTreeMap::new()
    });
    (result, errors)
}

/// The ssh command line of pub.user, its key written under `work`.
fn public_ssh(work: &Path) -> String {
    let key_file = work.join("id_public");
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&PUBLIC_SEED));
    std::fs::write(&key_file, key.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
    std::fs::set_permissions(
        &key_file,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    )
    .unwrap();
    format!(
        "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
         -o LogLevel=ERROR -F /dev/null",
        key_file.display()
    )
}

/// `ssh-server` `git-any-account` (plans/Dogfood.md F3): an account in no group, so without `SSH_GIT`, uses git over
/// SSH by its key. The repository's own check decides what it reaches, and SFTP and the shell stay closed. That it is
/// refused with the attribute off is in the test below, on its server.
#[test]
fn any_account_uses_git_over_ssh_by_key_when_the_conf_says_so() {
    if !crate::common::own_process() {
        return;
    }
    if !git_available("git") || !git_available("ssh") {
        eprintln!("skipped: git or ssh is not on the PATH");
        return;
    }
    let user_key = PrivateKey::from(Ed25519Keypair::from_seed(&USER_SEED));
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_conf_properties(
        "git-any-account",
        Some(&data(&user_key)),
        // sftp-drives off: with it (the default) every account's SFTP is the drives it may open, whatever this says
        r#"<ssh-server git-any-account="true"/><dav-server sftp-drives="false"/>"#,
        &[("ssh_port", "0"), ("ssh_host_key", &seed)],
    );
    let work = gate.runtime.join("tmp/git-any-account");
    let home = work.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let ssh_port = gate.server.ssh_address().expect("the SSH server listens").port();
    let ssh = public_ssh(&work);
    let at = |repository: &str| format!("ssh://pub.user@127.0.0.1:{ssh_port}/{repository}.git");

    // a repository every account may view is cloned (it is empty); one it may not view is as one that is not there
    let clone = work.join("pub");
    git_ok(
        &work,
        &home,
        Some(&ssh),
        &["clone", &at("pub-db"), &clone.to_string_lossy()],
    );
    assert!(clone.join(".git").is_dir());
    let (ok, out) = git(&work, &home, Some(&ssh), &["clone", &at("hosted-db"), "private"]);
    assert!(!ok && out.contains("not found"), "{out}");
    // view is not push
    commit(&clone, &home, "README.md", "# Mine\n");
    let (ok, out) = git(&clone, &home, Some(&ssh), &["push", "origin", "HEAD:refs/heads/main"]);
    assert!(!ok, "a viewer pushed: {out}");

    // git only: no SFTP subsystem and no shell command
    let ssh_args: Vec<String> = ssh.split_whitespace().skip(1).map(str::to_string).collect();
    let run = |extra: &[&str]| {
        Command::new("ssh")
            .args(&ssh_args)
            .args(["-p", &ssh_port.to_string(), "pub.user@127.0.0.1"])
            .args(extra)
            .env("HOME", &home)
            .env_remove("SSH_AUTH_SOCK")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    let sftp = run(&["-s", "sftp"]);
    assert!(!sftp.status.success(), "SFTP opened for an account without SSH_SFTP");
    let shell = run(&["status"]);
    let said = String::from_utf8_lossy(&shell.stdout);
    assert!(
        !shell.status.success() && !said.contains("moqui"),
        "a shell command ran: {said}"
    );
}

#[test]
fn repositories_are_cloned_and_pushed_over_http_and_ssh() {
    if !crate::common::own_process() {
        return;
    }
    if !git_available("git") {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let user_key = PrivateKey::from(Ed25519Keypair::from_seed(&USER_SEED));
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    // code.test is a public-site host: git at a page's URL is served there (its pages are never rendered here)
    let gate = crate::common::start_with_conf_properties(
        "git-hosting",
        Some(&data(&user_key)),
        r#"<webapp-list><webapp name="webroot"><public-site host="code.test" site-id="CODE"
            screen-location="component://example/screen/PublicLink.xml"><mount path="/" screen="Code"/></public-site>
        </webapp></webapp-list>"#,
        &[("ssh_port", "0"), ("ssh_host_key", &seed)],
    );
    let http = gate.server.address();
    let work = gate.runtime.join("tmp/git-hosting");
    let home = work.join("home");
    std::fs::create_dir_all(&home).unwrap();

    // HTTP, on the disk and the database kinds
    for id in ["hosted", "hosted-db"] {
        let url = format!("http://john.doe:moqui@127.0.0.1:{}/git/{id}.git", http.port());
        let first = work.join(format!("{id}-first"));
        git_ok(&work, &home, None, &["clone", &url, &first.to_string_lossy()]);
        commit(&first, &home, "README.md", "# Hosted\n");
        git_ok(&first, &home, None, &["push", "origin", "main"]);
        let second = work.join(format!("{id}-second"));
        git_ok(
            &work,
            &home,
            None,
            &["-c", "protocol.version=2", "clone", &url, &second.to_string_lossy()],
        );
        assert_eq!(std::fs::read_to_string(second.join("README.md")).unwrap(), "# Hosted\n");
        // a push big enough that git sends it in chunks (over http.postBuffer, 1 MiB)
        let big: String = (0..40_000)
            .map(|line| format!("line {line} of a large file\n"))
            .collect();
        commit(&second, &home, "large.txt", &big);
        git_ok(&second, &home, None, &["push", "origin", "main"]);
        git_ok(&first, &home, None, &["pull", "--ff-only", "origin", "main"]);
        assert_eq!(std::fs::read_to_string(first.join("large.txt")).unwrap(), big);
    }
    let (status, _, body) = crate::common::exchange(
        http,
        &crate::common::http_request("GET", "/git/hosted.git/info/refs?service=git-upload-pack", &[], ""),
    );
    assert_eq!(status, 401, "{body}");
    let (status, _, _) = crate::common::exchange(
        http,
        &crate::common::http_request("GET", "/git/hosted.git/HEAD", &[], ""),
    );
    assert_eq!(status, 404, "no dumb protocol");
    // with no credentials a path that is not one, a repository that is not there and one that is are one answer
    let anonymous = |path: &str| {
        let (status, headers, body) = crate::common::exchange(
            http,
            &crate::common::http_request(
                "GET",
                &format!("/git/{path}/info/refs?service=git-upload-pack"),
                &[],
                "",
            ),
        );
        let asks = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"));
        (status, asks, body)
    };
    let there = anonymous("hosted.git");
    assert_eq!((there.0, there.1), (401, true), "{}", there.2);
    for path in ["nosuch.git", "a/-b.git", "a/.b", "a%20b.git", "a/b/c/d/e/f/g/h/i.git"] {
        assert_eq!(anonymous(path), there, "{path}");
    }
    let (status, _, body) = crate::common::exchange(
        http,
        &crate::common::http_request("POST", "/git/a/-b.git/info/lfs/objects/batch", &[], "{}"),
    );
    assert_eq!(status, 401, "{body}");

    // the repository's path, on the public-site host at its page's URL, with and without `.git` (under /git/ it is
    // moqui-git's tests'); the same URL on another host is a page's, not git's
    let page_url = format!("http://john.doe:moqui@127.0.0.1:{}/team/hosted", http.port());
    let site = ["-c", "http.extraHeader=Host: code.test"];
    let clone = work.join("page");
    git_ok(
        &work,
        &home,
        None,
        &[site[0], site[1], "clone", &page_url, &clone.to_string_lossy()],
    );
    assert!(clone.join("large.txt").is_file());
    let listed = git_ok(
        &work,
        &home,
        None,
        &[site[0], site[1], "ls-remote", &format!("{page_url}.git")],
    );
    assert!(listed.contains("refs/heads/main"), "{listed}");
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("john.doe:moqui")
    );
    let page = |host: &str, path: &str| {
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAuthorization: {basic}\r\n\r\n");
        let (status, _, body) = crate::common::exchange(http, &request);
        (status, body)
    };
    let (status, body) = page("code.test:8080", "/team/hosted/info/refs?service=git-upload-pack");
    assert!(
        status == 200 && body.contains("# service=git-upload-pack"),
        "{status} {body}"
    );
    let (status, body) = page("localhost", "/team/hosted/info/refs?service=git-upload-pack");
    assert!(
        status != 200 || !body.contains("# service=git-upload-pack"),
        "{status} {body}"
    );
    // a framework route and a path with no git suffix stay what they were on the site host
    let (_, body) = page("code.test", "/rest/team/hosted/info/refs?service=git-upload-pack");
    assert!(!body.contains("# service=git-upload-pack"), "{body}");
    let (_, body) = page("code.test", "/team/hosted/info/refs");
    assert!(!body.contains("# service=git-upload-pack"), "{body}");

    // the read services, as john.doe; a screen's inherited authorization covers AT_SERVICE, AT_GIT_REPOSITORY is
    // checked in the core tests
    {
        let factory = Arc::clone(gate.server.factory());
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let _authz = context.disable_authz();
        let (refs, errors) = sync(
            "org.moqui.git.GitServices.get#GitRefs",
            &[("repositoryId", Value::string("hosted"))],
        );
        assert!(errors.is_empty(), "{errors}");
        let Some(Value::List(list)) = refs.get("refList") else {
            panic!("{refs:?}")
        };
        assert_eq!(list.len(), 1);
        let (log, errors) = sync(
            "org.moqui.git.GitServices.get#GitLog",
            &[("repositoryId", Value::string("hosted-db"))],
        );
        assert!(errors.is_empty(), "{errors}");
        let Some(Value::List(commits)) = log.get("commitList") else {
            panic!("{log:?}")
        };
        assert_eq!(commits.len(), 2);
        let (urls, _) = sync(
            "org.moqui.git.GitServices.get#GitCloneUrls",
            &[("repositoryId", Value::string("hosted"))],
        );
        assert!(urls
            .get("httpsUrl")
            .and_then(Value::as_str)
            .is_some_and(|url| url.ends_with("/git/team/hosted.git")));

        // a collection of the database repository (nothing is old enough to delete; the rows are repacked), then
        // every row decodes, and it still clones
        let (gc, errors) = sync(
            "org.moqui.git.GitServices.gc#GitRepositories",
            &[("repositoryId", Value::string("hosted-db"))],
        );
        assert!(errors.is_empty(), "{errors}");
        let Some(Value::List(results)) = gc.get("results") else {
            panic!("{gc:?}")
        };
        let Value::Map(result) = &results[0] else {
            panic!("{results:?}")
        };
        assert_eq!(result.get("deletedCount"), Some(&Value::I64(0)), "{result:?}");
        let (fsck, errors) = sync(
            "org.moqui.git.GitServices.fsck#GitRepository",
            &[("repositoryId", Value::string("hosted-db"))],
        );
        assert!(errors.is_empty(), "{errors}");
        assert!(
            matches!(fsck.get("checkedCount"), Some(Value::I64(count)) if *count > 5),
            "{fsck:?}"
        );
        moqui_core::user::logout().unwrap();
    }
    // Git LFS: a batch, an upload with the token it gives, and the download
    {
        let content: Vec<u8> = (0..300_000u32).map(|n| (n % 241) as u8).collect();
        let oid: String = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&content)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("john.doe:moqui")
        );
        let batch = format!(
            r#"{{"operation":"upload","transfers":["basic"],"objects":[{{"oid":"{oid}","size":{}}}]}}"#,
            content.len()
        );
        let (status, _, body) = crate::common::exchange(
            http,
            &crate::common::http_request(
                "POST",
                "/git/hosted-db.git/info/lfs/objects/batch",
                &[
                    ("Authorization", &basic),
                    ("Accept", "application/vnd.git-lfs+json"),
                    ("Content-Type", "application/vnd.git-lfs+json"),
                ],
                &batch,
            ),
        );
        assert_eq!(status, 200, "{body}");
        let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
        let upload = &reply["objects"][0]["actions"]["upload"];
        let href = upload["href"].as_str().unwrap();
        let path = &href[href.find("/git/").unwrap()..];
        let token = upload["header"]["Authorization"].as_str().unwrap();
        let mut request = format!(
            "PUT {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {token}\r\nContent-Length: {}\r\n\r\n",
            content.len()
        )
        .into_bytes();
        request.extend_from_slice(&content);
        let mut stream = std::net::TcpStream::connect(http).unwrap();
        std::io::Write::write_all(&mut stream, &request).unwrap();
        let mut response = Vec::new();
        std::io::Read::read_to_end(&mut stream, &mut response).unwrap();
        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"));
        let (status, _, downloaded) = crate::common::exchange_bytes(
            http,
            &crate::common::http_request(
                "GET",
                &format!("/git/hosted-db.git/info/lfs/objects/{oid}"),
                &[("Authorization", &basic)],
                "",
            ),
        );
        assert_eq!(status, 200);
        assert!(downloaded == content);
    }

    let after_gc = work.join("hosted-db-after-gc");
    let url = format!("http://john.doe:moqui@127.0.0.1:{}/git/hosted-db.git", http.port());
    git_ok(&work, &home, None, &["clone", &url, &after_gc.to_string_lossy()]);
    let fsck = git_ok(&after_gc, &home, None, &["fsck", "--strict", "--full"]);
    assert!(!fsck.contains("error"), "{fsck}");

    // SSH with john.doe's key, protocol v0 and v2
    if !git_available("ssh") {
        eprintln!("skipped the SSH part: ssh is not on the PATH");
        return;
    }
    let ssh_port = gate.server.ssh_address().expect("the SSH server listens").port();
    let key_file = work.join("id_ed25519");
    std::fs::write(&key_file, user_key.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
    std::fs::set_permissions(
        &key_file,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    )
    .unwrap();
    let ssh = format!(
        "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
         -o LogLevel=ERROR -F /dev/null",
        key_file.display()
    );
    let url = format!("ssh://john.doe@127.0.0.1:{ssh_port}/hosted.git");
    for version in ["0", "2"] {
        let clone = work.join(format!("ssh-v{version}"));
        git_ok(
            &work,
            &home,
            Some(&ssh),
            &[
                "-c",
                &format!("protocol.version={version}"),
                "clone",
                &url,
                &clone.to_string_lossy(),
            ],
        );
        assert_eq!(std::fs::read_to_string(clone.join("README.md")).unwrap(), "# Hosted\n");
        commit(&clone, &home, &format!("ssh-{version}.txt"), "over ssh\n");
        git_ok(&clone, &home, Some(&ssh), &["pull", "--rebase", "origin", "main"]);
        git_ok(&clone, &home, Some(&ssh), &["push", "origin", "main"]);
    }
    // git-lfs asks for the HTTPS endpoint and a token over SSH
    let ssh_args: Vec<String> = ssh.split_whitespace().skip(1).map(str::to_string).collect();
    let output = Command::new("ssh")
        .args(&ssh_args)
        .args([
            "-p",
            &ssh_port.to_string(),
            "john.doe@127.0.0.1",
            "git-lfs-authenticate",
            "hosted-db.git",
            "download",
        ])
        .env("HOME", &home)
        .env_remove("SSH_AUTH_SOCK")
        .output()
        .unwrap();
    let answer = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{answer} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        answer.contains("/git/hosted-db.git/info/lfs") && answer.contains("MoquiLfs"),
        "{answer}"
    );
    // by the repository's path, as by its id above
    let check = work.join("ssh-check");
    let by_path = format!("ssh://john.doe@127.0.0.1:{ssh_port}/team/hosted.git");
    git_ok(&work, &home, Some(&ssh), &["clone", &by_path, &check.to_string_lossy()]);
    assert!(check.join("ssh-0.txt").is_file() && check.join("ssh-2.txt").is_file());
    let (ok, out) = git(
        &work,
        &home,
        Some(&ssh),
        &[
            "clone",
            &format!("ssh://john.doe@127.0.0.1:{ssh_port}/nosuch.git"),
            "nosuch",
        ],
    );
    assert!(!ok && out.contains("not found"), "{out}");
    // an account without SSH_GIT is not served git, by key either, on a repository it may view (git-any-account off)
    let pub_ssh = public_ssh(&work);
    let pub_url = format!("ssh://pub.user@127.0.0.1:{ssh_port}/pub-db.git");
    let (ok, out) = git(&work, &home, Some(&pub_ssh), &["clone", &pub_url, "pub-refused"]);
    assert!(!ok, "{out}");

    // a push mirror of hosted to mirror-db over this SSH server, as john.doe with his key and the host key pinned;
    // then one to a repository that is not there, whose failure is kept on the row
    let host_key = PrivateKey::from(Ed25519Keypair::from_seed(&HOST_SEED));
    let factory = Arc::clone(gate.server.factory());
    let set_mirror = |path: &str| {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut remote =
            moqui_core::entity::EntityValue::make(&facade, "moqui.service.message.SystemMessageRemote").unwrap();
        remote.set("systemMessageRemoteId", Value::string("GIT_MIRROR"));
        remote.set(
            "sendUrl",
            Value::string(format!("ssh://john.doe@127.0.0.1:{ssh_port}/{path}")),
        );
        remote.set(
            "privateKey",
            Value::string(user_key.to_openssh(LineEnding::LF).unwrap().as_str()),
        );
        remote.set("publicKey", Value::string(user_key.public_key().to_openssh().unwrap()));
        remote.set(
            "sshHostKeyFingerprint",
            Value::string(host_key.public_key().fingerprint(HashAlg::Sha256).to_string()),
        );
        remote.create_or_update(&facade).unwrap();
        let mut repository = moqui_core::entity::EntityValue::make(&facade, "moqui.resource.GitRepository").unwrap();
        repository.set("repositoryId", Value::string("hosted"));
        repository.set("pushMirrorRemoteId", Value::string("GIT_MIRROR"));
        repository.set("pushMirrorRefs", Value::string("main:mirror"));
        repository.update(&facade).unwrap();
    };
    let push_mirror = || {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let (result, errors) = sync(
            "org.moqui.git.GitServices.push#GitMirror",
            &[("repositoryId", Value::string("hosted"))],
        );
        let row = factory
            .entity()
            .unwrap()
            .find("moqui.resource.GitRepository")
            .unwrap()
            .eq("repositoryId", Value::string("hosted"))
            .one()
            .unwrap()
            .unwrap();
        let error = row.get("lastPushMirrorError").cloned().unwrap_or(Value::Null);
        (result, errors, error)
    };
    set_mirror("mirror-db.git");
    let (result, errors, error) = push_mirror();
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(error, Value::Null);
    assert!(
        result
            .get("pushResult")
            .and_then(Value::as_str)
            .is_some_and(|text| text.starts_with("pushed 1 refs")),
        "{result:?}"
    );
    let head = git_ok(&check, &home, None, &["rev-parse", "HEAD"]);
    let mirror_url = format!("http://john.doe:moqui@127.0.0.1:{}/git/mirror-db.git", http.port());
    let listed = git_ok(&work, &home, None, &["ls-remote", &mirror_url, "refs/heads/*"]);
    assert_eq!(
        listed.trim(),
        format!("{}\trefs/heads/mirror", head.trim()),
        "only the mapped name: {listed}"
    );

    set_mirror("nosuch.git");
    let (_, errors, error) = push_mirror();
    assert!(!errors.is_empty());
    assert!(
        error.as_str().is_some_and(|text| text.contains("nosuch")),
        "the failure is kept on the row: {error:?} ({errors})"
    );
    set_mirror("mirror-db.git");
    let (result, errors, error) = push_mirror();
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(error, Value::Null, "a push that works clears the error");
    assert!(
        result
            .get("pushResult")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("1 already current")),
        "{result:?}"
    );

    // the framework's post-receive rule pushes the mirror after a push, async
    commit(&check, &home, "mirrored.txt", "after the mirror\n");
    git_ok(&check, &home, Some(&ssh), &["push", "origin", "main"]);
    let head = git_ok(&check, &home, None, &["rev-parse", "HEAD"]);
    let expected = format!("{}\trefs/heads/mirror", head.trim());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let listed = git_ok(&work, &home, None, &["ls-remote", &mirror_url, "refs/heads/mirror"]);
        if listed.trim() == expected {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            let updates = factory
                .entity()
                .unwrap()
                .find("moqui.resource.GitUpdate")
                .unwrap()
                .eq("repositoryId", Value::string("hosted"))
                .list()
                .unwrap();
            let updates: Vec<String> = updates
                .iter()
                .map(|row| {
                    format!(
                        "{:?} {:?} {:?}",
                        row.get("source"),
                        row.get("ecaStatus"),
                        row.get("ecaError")
                    )
                })
                .collect();
            panic!("the rule did not push the mirror: {listed}; updates: {updates:#?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // usage and delete on Postgres: the grouped read and the one-statement deletes (their rules are moqui-git's tests)
    let context = factory.new_context();
    let _entered = context.enter();
    // as an application's own code calls them: no screen is below to authorize the services
    let _authz = context.disable_authz();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let (usage, errors) = sync(
        "org.moqui.git.GitServices.get#GitRepositoryUsage",
        &[("repositoryIdList", Value::string("hosted-db,hosted"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let Some(Value::List(usages)) = usage.get("usageList") else {
        panic!("no usageList: {usage:?}");
    };
    let number = |usage: &Value, name: &str| match usage {
        Value::Map(map) => map
            .get(name)
            .map(moqui_lib::object::to_plain_string)
            .and_then(|text| text.parse::<i64>().ok())
            .unwrap_or(0),
        _ => 0,
    };
    assert_eq!(usages.len(), 2, "{usages:?}");
    // each holds the 40,000 line file, compressed
    assert!(
        number(&usages[0], "storedBytes") > 50_000 && number(&usages[0], "objectCount") >= 6,
        "{usages:?}"
    );
    assert!(number(&usages[1], "storedBytes") > 50_000, "{usages:?}");
    let (_, errors) = sync(
        "org.moqui.git.GitServices.delete#GitRepository",
        &[("repositoryId", Value::string("hosted-db"))],
    );
    assert!(errors.is_empty(), "{errors}");
    let left = {
        let entity = factory.entity().unwrap();
        let count = |name: &str| {
            entity
                .find(name)
                .unwrap()
                .eq("repositoryId", Value::string("hosted-db"))
                .count()
                .unwrap()
        };
        [
            count("moqui.resource.GitObject"),
            count("moqui.resource.GitRef"),
            count("moqui.resource.GitUpdate"),
            count("moqui.resource.GitRepository"),
        ]
    };
    assert_eq!(left, [0, 0, 0, 0]);
    let gone = format!("http://john.doe:moqui@127.0.0.1:{}/git/hosted-db.git", http.port());
    let (ok, out) = git(&work, &home, None, &["ls-remote", &gone]);
    assert!(!ok, "a deleted repository answers: {out}");
}
