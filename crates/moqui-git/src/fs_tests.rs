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

//! A fetch that denies internal addresses (SEC-TLS-08): its https client resolves through `PublicOnly`, and its ssh
//! command is pinned to the address checked, the host's name kept for its host key.

use super::{deny_internal, pin_ssh, pinned_ssh_command, public_only_options, FsStore, DENY_INTERNAL};

fn repo(name: &str) -> (std::path::PathBuf, gix::Repository) {
    let dir = std::env::temp_dir().join(format!("moqui-git-fs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = FsStore::init(&dir, "main").unwrap();
    let repo = store.local();
    (dir, repo)
}

#[test]
fn the_guard_holds_for_its_life_on_its_thread() {
    let denied = || DENY_INTERNAL.with(std::cell::Cell::get);
    assert!(!denied());
    {
        let _outer = deny_internal();
        {
            let _inner = deny_internal();
            assert!(denied());
        }
        assert!(denied(), "the outer guard still holds");
        assert!(
            !std::thread::spawn(denied).join().unwrap(),
            "another thread's fetches are its own"
        );
    }
    assert!(!denied());
}

#[test]
fn an_https_fetch_gets_the_public_only_client_and_an_ssh_one_a_pinned_command() {
    let (dir, mut repo) = repo("pins");
    let https = public_only_options(&repo, "https://git.example.com/r.git").unwrap();
    let options = https
        .expect("options for an https remote")
        .downcast::<gix::protocol::transport::client::blocking_io::http::Options>()
        .expect("http options");
    assert!(options.backend.is_some(), "the client is configured");
    assert!(public_only_options(&repo, "git@git.example.com:org/r.git")
        .unwrap()
        .is_none());

    pin_ssh(&mut repo, "ssh://git@93.184.216.34/org/r.git").unwrap();
    let command = repo
        .config_snapshot()
        .string("core.sshCommand")
        .map(|value| value.to_string());
    assert_eq!(
        command.as_deref(),
        Some("ssh -o HostName=93.184.216.34 -o HostKeyAlias=93.184.216.34 -o CheckHostIP=no")
    );
    assert!(
        pin_ssh(&mut repo, "git@127.0.0.1:org/r.git").is_err(),
        "an internal address"
    );
    pin_ssh(&mut repo, "https://git.example.com/r.git").unwrap();
    assert!(pinned_ssh_command("a b", "93.184.216.34".parse().unwrap()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
