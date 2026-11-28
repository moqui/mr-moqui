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

use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync::lock;
use russh::client::AuthResult;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::PrivateKey;
use russh::MethodKind;

use super::{ended, intake, now_off, now_running, running, serve_stream, upgrade, Configs, Ending};
use crate::ssh::live::{Access, SshLive};

/// A runtime directory of its own for a test's intake, whose SFTP root is made in it.
fn runtime_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("moqui-ssh-{label}-{}", std::process::id()))
}

/// A bind's sessions end with its stop: at once for the process stop or "end now", after the grace of a live stop,
/// which a later "end now" cuts short, and at once when the bind is dropped.
#[tokio::test]
async fn a_binds_sessions_end_with_its_stop() {
    async fn ends_within(stop: tokio::sync::watch::Receiver<Option<Ending>>, within: Duration) -> bool {
        tokio::time::timeout(within, ended(stop)).await.is_ok()
    }
    let (stop, sessions) = tokio::sync::watch::channel(None);
    assert!(
        !ends_within(sessions.clone(), Duration::from_millis(50)).await,
        "running"
    );
    upgrade(&stop, Ending::Now);
    assert!(ends_within(sessions, Duration::from_millis(50)).await);

    let (stop, sessions) = tokio::sync::watch::channel(None);
    upgrade(&stop, Ending::After(Duration::from_millis(200)));
    assert!(
        !ends_within(sessions.clone(), Duration::from_millis(100)).await,
        "in its grace"
    );
    assert!(
        ends_within(sessions.clone(), Duration::from_millis(400)).await,
        "after its grace"
    );
    let waiting = tokio::spawn(ends_within(sessions.clone(), Duration::from_millis(150)));
    tokio::time::sleep(Duration::from_millis(20)).await;
    upgrade(&stop, Ending::After(Duration::from_secs(60)));
    upgrade(&stop, Ending::Now);
    assert_eq!(
        *stop.borrow(),
        Some(Ending::Now),
        "a later grace does not replace the first; now does"
    );
    assert!(waiting.await.unwrap(), "end now cuts the grace short");

    let (stop, sessions) = tokio::sync::watch::channel(None::<Ending>);
    drop(stop);
    assert!(ends_within(sessions, Duration::from_millis(50)).await, "a bind dropped");
}

/// The edge node's tunnel streams are taken only while SSH runs, and when it is then off they end as the last bind's
/// stop ended its own, a grace cut short by "end now" (plans/Runtime-Config.md §3.4: off means closed); on again, the
/// streams that come have a stop of their own.
#[test]
fn the_edge_tunnels_open_while_ssh_runs_and_end_when_it_is_off() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    now_running(factory);
    assert!(!running(factory), "no server has started");
    let dir = runtime_dir("intake");
    let made = intake(factory, &dir, Arc::default(), &SshLive::default());
    now_running(factory);
    assert!(running(factory));
    let streams = lock(&made.tunnels)
        .open
        .as_ref()
        .map(tokio::sync::watch::Sender::subscribe)
        .unwrap();
    let grace = Ending::After(Duration::from_secs(5));
    lock(&made.tunnels).last_stop = Some(grace);
    now_off(factory);
    assert!(!running(factory));
    assert_eq!(*streams.borrow(), Some(grace));
    made.end_tunnels_now();
    assert_eq!(*streams.borrow(), Some(Ending::Now));
    now_running(factory);
    let again = lock(&made.tunnels).open.as_ref().map(|open| *open.borrow());
    assert_eq!(again, Some(None));
    let _ = std::fs::remove_dir_all(&dir);
}

struct Accept;

impl russh::client::Handler for Accept {
    type Error = russh::Error;
    async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// An edge node's tunnel stream is served as a connection the access mode applies to as it does to one a bind took
/// (plans/Runtime-Config.md §3.4): under `key` a client asking with nothing is offered a key alone, and a password is
/// refused before it is checked; under `open` the stream is offered all three.
#[tokio::test]
async fn a_tunnel_stream_takes_the_access_mode() {
    let context = ExecutionContextFactory::test_context();
    let factory = Arc::clone(context.factory());
    let dir = runtime_dir("tunnel");
    let key_mode = SshLive {
        access: Access::Key,
        ..SshLive::default()
    };
    let made = intake(&factory, &dir, Arc::default(), &key_mode);
    let host = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
    made.configs.store(Some(Arc::new(Configs::new(&host))));
    now_running(&factory);
    let peer: std::net::SocketAddr = "198.51.100.30:40000".parse().unwrap();
    let offered = |factory: Arc<ExecutionContextFactory>| async move {
        let (client, server) = tokio::io::duplex(64 * 1024);
        assert!(
            serve_stream(&factory, server, peer),
            "a tunnel stream is taken while SSH runs"
        );
        let config = Arc::new(russh::client::Config::default());
        let mut handle = russh::client::connect_stream(config, client, Accept).await.unwrap();
        let methods = match handle.authenticate_none("john.doe").await.unwrap() {
            AuthResult::Failure { remaining_methods, .. } => remaining_methods.to_vec(),
            AuthResult::Success => panic!("a login with nothing was taken"),
        };
        let password = handle.authenticate_password("john.doe", "moqui").await.unwrap();
        (methods, password.success())
    };
    let (methods, by_password) = offered(Arc::clone(&factory)).await;
    assert_eq!(methods, [MethodKind::PublicKey]);
    assert!(!by_password, "no password under key, refused before it is checked");
    made.apply(&SshLive::default());
    let (methods, _) = tokio::time::timeout(Duration::from_secs(10), offered(Arc::clone(&factory)))
        .await
        .unwrap();
    let open: &[MethodKind] = if cfg!(feature = "tui") {
        &[
            MethodKind::PublicKey,
            MethodKind::KeyboardInteractive,
            MethodKind::Password,
        ]
    } else {
        &[MethodKind::PublicKey, MethodKind::Password]
    };
    assert_eq!(methods, open);
    now_off(&factory);
    let (_, server) = tokio::io::duplex(1024);
    assert!(!serve_stream(&factory, server, peer), "none while SSH is off");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sign-in dialog's end follows whether it is offered: the access mode leaving `open`, or the terminal client
/// turned off (plans/Runtime-Config.md §3.4); offered again, the dialog may be started once more.
#[test]
fn the_dialog_ends_when_it_is_no_longer_offered() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = runtime_dir("dialog");
    let made = intake(factory, &dir, Arc::default(), &SshLive::default());
    let dialog = cfg!(feature = "tui");
    assert_eq!(
        *made.dialog_off.borrow(),
        !dialog,
        "offered under open with the terminal client"
    );
    made.apply(&SshLive {
        tui: false,
        ..SshLive::default()
    });
    assert!(*made.dialog_off.borrow(), "the terminal client off");
    made.apply(&SshLive::default());
    assert_eq!(*made.dialog_off.borrow(), !dialog);
    made.apply(&SshLive {
        access: Access::Login,
        ..SshLive::default()
    });
    assert!(*made.dialog_off.borrow(), "access login");
    let _ = std::fs::remove_dir_all(&dir);
}
