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

use std::os::fd::IntoRawFd as _;
use std::path::PathBuf;

use super::{bind_own, granted_group, inherited, on_signal, OnSignal};

fn scratch(label: &str) -> PathBuf {
    let dir = PathBuf::from(format!("/tmp/msk-unit-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn only_a_socket_of_roots_in_a_directory_of_roots_grants_its_group() {
    assert_eq!(granted_group(0, 977, 0, 0o755), Some(977));
    assert_eq!(granted_group(1000, 977, 0, 0o755), None, "the server bound it");
    assert_eq!(granted_group(0, 977, 1000, 0o755), None, "in the server's directory");
    assert_eq!(granted_group(0, 977, 0, 0o775), None, "a directory its group can write");
    assert_eq!(granted_group(0, 977, 0, 0o1777), None, "a directory anyone can write");
}

#[test]
fn the_server_replaces_only_a_dead_socket_of_its_own() {
    let dir = scratch("bind");
    let path = dir.join("op.sock");
    std::fs::write(&path, "not a socket").unwrap();
    assert!(bind_own(&path).unwrap_err().to_string().contains("not a socket"));
    std::fs::remove_file(&path).unwrap();
    let live = bind_own(&path).unwrap();
    assert!(bind_own(&path).unwrap_err().to_string().contains("another server"));
    drop(live);
    // left by a process that ended: replaced
    let again = bind_own(&path).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    drop(again);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_socket_handed_over_is_taken_once_and_closed_on_exec() {
    let dir = scratch("inherit");
    let path = dir.join("op.sock");
    let fd = std::os::unix::net::UnixListener::bind(&path).unwrap().into_raw_fd();
    std::env::set_var("MOQUI_OPERATOR_FD", fd.to_string());
    std::env::set_var("MOQUI_OPERATOR_PID", std::process::id().to_string());
    let listener = inherited().unwrap().expect("the socket handed over");
    assert_eq!(listener.local_addr().unwrap().as_pathname(), Some(path.as_path()));
    let flags = rustix::io::fcntl_getfd(&listener).unwrap();
    assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
    assert!(inherited().unwrap().is_none(), "taken once");
    // another process's: not taken
    std::env::set_var("MOQUI_OPERATOR_FD", "3");
    std::env::set_var("MOQUI_OPERATOR_PID", "1");
    assert!(inherited().unwrap().is_none());
    std::env::remove_var("MOQUI_OPERATOR_FD");
    std::env::remove_var("MOQUI_OPERATOR_PID");
    drop(listener);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a SIGTERM or SIGINT does (the operator socket's handler): sealed, the gate gives up; booting, the process ends;
/// once the listener has served, the first signal is the shutdown's and the process ends after it, and a second one
/// ends it at once. A restart's or a seal's shutdown is the listener having served with no signal yet: a signal then
/// is left to the shutdown and marks the stop, which the server reads before it runs again or waits sealed.
#[test]
fn a_signal_after_the_listener_stopped_is_the_shutdowns_and_a_second_ends_it() {
    assert_eq!(on_signal(false, false, false), OnSignal::Abandon, "sealed");
    assert_eq!(
        on_signal(false, true, false),
        OnSignal::Abandon,
        "sealed by a seal's stop"
    );
    assert_eq!(
        on_signal(true, false, false),
        OnSignal::Exit,
        "booting, before the listener opened"
    );
    // the listener stopped on this signal, or a restart's or seal's shutdown runs: the shutdown goes on
    assert_eq!(on_signal(true, true, false), OnSignal::Leave);
    // a second Ctrl-C or SIGTERM cuts a slow shutdown short
    assert_eq!(on_signal(true, true, true), OnSignal::ExitSecond);
}
