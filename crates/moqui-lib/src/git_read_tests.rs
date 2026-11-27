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

//! The tests of a hardened git read (git_read.rs).

use super::*;

#[test]
fn a_read_refuses_every_transport_by_name() {
    let args = read_config();
    assert!(args.contains(&"protocol.allow=never".to_string()));
    assert!(args.contains(&"protocol.ext.allow=never".to_string()));
    assert!(args.contains(&"core.fsmonitor=false".to_string()));
    assert!(args.chunks(2).all(|pair| pair[0] == "-c"), "{args:?}");
}

/// A partial clone whose committed blob is missing, its promisor remote an `ext::` command that touches a marker: a
/// plain `git show` of the blob runs it, a hardened one does not.
#[cfg(unix)]
#[test]
fn a_read_never_fetches_from_a_planted_promisor_remote() {
    let base = std::env::temp_dir().join(format!("moqui-git-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let marker = base.join("RAN");
    let git = |args: &[&str], hardened: bool| {
        let mut command = Command::new("git");
        if hardened {
            isolate(&mut command);
        }
        command
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&base)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("HOME", &base)
            .env_remove("XDG_CONFIG_HOME");
        command.output()
    };
    let Ok(init) = git(&["init", "-q"], false) else {
        let _ = std::fs::remove_dir_all(&base);
        return; // no git here
    };
    assert!(init.status.success());
    std::fs::write(base.join("f.xml"), "<a/>\n").unwrap();
    assert!(git(&["add", "f.xml"], false).unwrap().status.success());
    assert!(git(&["commit", "-q", "-m", "base"], false).unwrap().status.success());
    let blob = String::from_utf8(git(&["rev-parse", "HEAD:f.xml"], false).unwrap().stdout).unwrap();
    let blob = blob.trim();
    std::fs::remove_file(base.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    let url = format!("ext::sh -c touch% {}% >&2", marker.display());
    for (key, value) in [
        ("core.repositoryformatversion", "1"),
        ("extensions.partialClone", "origin"),
        ("remote.origin.url", url.as_str()),
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", "blob:none"),
        ("protocol.ext.allow", "always"),
    ] {
        assert!(git(&["config", key, value], false).unwrap().status.success(), "{key}");
    }
    let hardened = git(&["show", "HEAD:./f.xml"], true).unwrap();
    assert!(!hardened.status.success());
    assert!(!marker.exists(), "a hardened read ran the promisor's command");
    // the plant is real: an ordinary read runs it
    let _ = git(&["show", "HEAD:./f.xml"], false);
    let planted = marker.exists();
    let _ = std::fs::remove_dir_all(&base);
    assert!(
        planted,
        "an ordinary read did not run the planted command, so the test proves nothing"
    );
}
