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

//! The sealed start's host side (plans/Sealed-Secrets.md §3.3): the gate a server boot runs, the operator socket it
//! opens before anything else, the peer check of that socket, its commands, and the `moqui operator` client.
//!
//! The gate is the `moqui` binary's server's (`moqui`, `moqui serve`): [`require_gate`] turns it on, and a test of the
//! sealed start turns it on too. A program that embeds the host without it (the desktop app, the other tests) boots
//! unattended, as `moqui load` does ([`moqui_core::seal::open_unattended`]).

pub mod client;

#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
mod commands;
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
pub(crate) mod peer;
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
mod socket;

#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
pub use peer::{set_operator_gid_for_tests, set_service_uid_for_tests};
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
pub use server::require_gate;
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
pub(crate) use server::{hook, sealed_now, stop_on_seal};
#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
pub(crate) use socket::set_serving;

#[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
mod server {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, LazyLock, Mutex};

    use moqui_core::context::Error;
    use moqui_core::seal::{GateRequest, Mode, SealWatch, Start, UnsealHook};

    /// Whether this process's server boots run the operator gate.
    static GATE: AtomicBool = AtomicBool::new(false);

    /// The watches the gate keeps for the life of the process, one per runtime: each marks the server's stop as a
    /// seal's ([`crate::systemd_notify::mark_sealed_stop`]), even when the seal comes while the server boots.
    static WATCHES: LazyLock<Mutex<Vec<(PathBuf, SealWatch)>>> = LazyLock::new(Mutex::default);

    /// Run the operator gate in this process's server boots: the `moqui` binary's server, and a test of the sealed start.
    pub fn require_gate() {
        GATE.store(true, Ordering::SeqCst);
    }

    /// The unseal hook of a server boot, when the gate is on.
    pub(crate) fn hook() -> Option<UnsealHook> {
        GATE.load(Ordering::SeqCst).then(|| Arc::new(gate) as UnsealHook)
    }

    /// The gate: open the operator socket, then open the seal or wait there until an operator does.
    fn gate(request: &GateRequest<'_>) -> Result<(), Error> {
        if rustix::process::geteuid().is_root() {
            tracing::error!(
                "This server runs as root: every child and job of it is root too, and the operator socket's rule that \
                 the server's own user runs only status protects nothing. Run it as its own user (systemd/moqui.service)"
            );
        }
        let keeper = request.keeper()?;
        let path = super::socket::serve(request.runtime_dir, &keeper.conf().socket, Arc::clone(&keeper))?;
        {
            let mut watches = moqui_lib::sync::lock(&WATCHES);
            if !watches.iter().any(|(dir, _)| dir == request.runtime_dir) {
                // a seal also drops the TLS keys held in memory (S4); the listeners that loaded them close
                let watch = keeper.on_seal(Arc::new(|| {
                    crate::systemd_notify::mark_sealed_stop();
                    crate::tls::drop_held_keys();
                }));
                watches.push((request.runtime_dir.to_path_buf(), watch));
            }
        }
        // a managed instance dials its manager while sealed: no listener opens (plans/archive/Vault-Instances.md §4.4)
        crate::manage_link::dial::start_for_gate(request.runtime_dir, request.conf);
        match keeper.start(Mode::Operator)? {
            Start::Unsealed => {
                crate::manage_link::dial::after_unseal(request.runtime_dir, request.conf);
                crate::systemd_notify::notify("STATUS=unsealed");
                Ok(())
            }
            Start::Waiting(reason) => {
                tracing::warn!(
                    "Sealed: {reason}. No listener opens until then; the operator socket is {}",
                    path.display()
                );
                // a server waiting for a passphrase or shares is ready: the unit's start timeout must not end it
                let status = if reason.starts_with("auto-unseal failed") {
                    "sealed: auto-unseal failed"
                } else {
                    "sealed"
                };
                crate::systemd_notify::notify(&format!("READY=1\nSTATUS={status}"));
                keeper.wait_unsealed()?;
                // a first start's bootstrap bundle into the seal before the boot reads the datasource password
                crate::manage_link::dial::after_unseal(request.runtime_dir, request.conf);
                crate::systemd_notify::notify("STATUS=unsealed");
                Ok(())
            }
        }
    }

    /// Whether the seal of `runtime_dir` is closed now (an operator sealed it while the server booted): the listener
    /// then does not open.
    pub(crate) fn sealed_now(runtime_dir: &Path) -> bool {
        moqui_core::seal::keeper_of(runtime_dir).is_some_and(|keeper| !keeper.is_unsealed())
    }

    /// `stop`, which also fires when the seal of `runtime_dir` is closed (§3.5: `seal` closes the listeners), and the
    /// watch that keeps that callback while the listener runs. Registered before [`sealed_now`] is asked, so a seal
    /// between the two still stops the listener.
    pub(crate) fn stop_on_seal(
        runtime: &tokio::runtime::Runtime,
        runtime_dir: &Path,
        stop: tokio::sync::oneshot::Receiver<()>,
    ) -> (tokio::sync::oneshot::Receiver<()>, Option<SealWatch>) {
        let Some(keeper) = moqui_core::seal::keeper_of(runtime_dir) else {
            return (stop, None);
        };
        let (sealed_tx, sealed_rx) = tokio::sync::oneshot::channel::<()>();
        let sealed_tx = Mutex::new(Some(sealed_tx));
        let watch = keeper.on_seal(Arc::new(move || {
            if let Some(tx) = moqui_lib::sync::lock(&sealed_tx).take() {
                let _ = tx.send(());
            }
        }));
        let (either_tx, either_rx) = tokio::sync::oneshot::channel::<()>();
        runtime.spawn(async move {
            let mut sealed_rx = sealed_rx;
            tokio::select! {
                _ = stop => {}
                Ok(()) = &mut sealed_rx => {}
            }
            let _ = either_tx.send(());
        });
        (either_rx, Some(watch))
    }
}
