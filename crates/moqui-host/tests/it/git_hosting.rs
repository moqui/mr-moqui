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
//! SSH server with a `UserSshKey`, by the git command line (skipped when git, or for SSH ssh, is not installed). The
//! protocol itself, over each storage, is `moqui-git/src/server/tests.rs`.

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

fn data(user_key: &PrivateKey) -> String {
    let public = user_key.public_key().to_openssh().unwrap();
    let fingerprint = user_key.public_key().fingerprint(HashAlg::Sha256).to_string();
    format!(
        r#"<entity-facade-xml type="seed">
    <moqui.resource.GitRepository repositoryId="hosted" repositoryKind="local" description="Hosted on disk"/>
    <moqui.resource.GitRepository repositoryId="hosted-db" repositoryKind="db" description="Hosted in the database"/>
    <moqui.security.UserSshKey sshKeyId="JOHN_GIT_KEY" userId="EX_JOHN_DOE" publicKey="{public}"
        fingerprint="{fingerprint}" fromDate="1265184000000"/>
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
    let result = moqui_core::service::sync(name, &parameters).unwrap_or_default();
    let errors = moqui_core::context::ExecutionContext::with_current(|ctx| {
        let text = ctx.message().errors().join("\n");
        ctx.message().clear();
        text
    })
    .unwrap_or_default();
    (result, errors)
}

#[test]
fn repositories_are_cloned_and_pushed_over_http_and_ssh() {
    if !git_available("git") {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let user_key = PrivateKey::from(Ed25519Keypair::from_seed(&USER_SEED));
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties(
        "git-hosting",
        Some(&data(&user_key)),
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

    // the read services, as john.doe; a screen's inherited authorization covers AT_SERVICE, AT_GIT_REPOSITORY is
    // checked in the core tests
    {
        let factory = Arc::clone(gate.server.factory());
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let _authz = context.disable_authz();
        let (refs, errors) = sync(
            "org.moqui.impl.GitServices.get#GitRefs",
            &[("repositoryId", Value::string("hosted"))],
        );
        assert!(errors.is_empty(), "{errors}");
        let Some(Value::List(list)) = refs.get("refList") else {
            panic!("{refs:?}")
        };
        assert_eq!(list.len(), 1);
        let (log, errors) = sync(
            "org.moqui.impl.GitServices.get#GitLog",
            &[("repositoryId", Value::string("hosted-db"))],
        );
        assert!(errors.is_empty(), "{errors}");
        let Some(Value::List(commits)) = log.get("commitList") else {
            panic!("{log:?}")
        };
        assert_eq!(commits.len(), 2);
        let (urls, _) = sync(
            "org.moqui.impl.GitServices.get#GitCloneUrls",
            &[("repositoryId", Value::string("hosted"))],
        );
        assert!(urls
            .get("httpsUrl")
            .and_then(Value::as_str)
            .is_some_and(|url| url.ends_with("/git/hosted.git")));

        // a collection of the database repository (nothing is old enough to delete; the rows are repacked), then
        // every row decodes, and it still clones
        let (gc, errors) = sync(
            "org.moqui.impl.GitServices.gc#GitRepositories",
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
            "org.moqui.impl.GitServices.fsck#GitRepository",
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
    let check = work.join("ssh-check");
    git_ok(&work, &home, Some(&ssh), &["clone", &url, &check.to_string_lossy()]);
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
}
