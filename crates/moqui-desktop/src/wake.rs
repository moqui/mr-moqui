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

//! The computer waking from sleep (plans/Mobile-Desktop.md section 4, configuration 1, Sleep and network changes).
//! Tauri tells a desktop app nothing of it (its `Suspended` and `Resumed` window events are mobile only, and
//! `RunEvent::Resumed` is the event loop's), so a thread wakes every [`TICK`] and compares the wall clock with the
//! monotonic one, which stands still while the computer sleeps (macOS and Linux). A gap of more than [`SLEEP_GAP`]
//! is a sleep, or the clock set forward; either way the node's `/health` is asked again and the answer logged. A
//! sidecar that died in the sleep is already on the splash page with its error, from its watch thread.
//!
//! Scheduled jobs after a long sleep: the framework's scheduler runs a job whose last cron occurrence is after its
//! last run on its next pass (moqui-core `service::jobs`, `readiness`), so the nightly backup missed in a sleep runs
//! once soon after the wake, not once for each night missed.

use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime};

use tauri::{AppHandle, Manager};

use crate::NodeState;

/// How often the watch looks.
const TICK: Duration = Duration::from_secs(30);
/// How far the wall clock may run ahead of the monotonic one in a tick before it is a sleep.
const SLEEP_GAP: Duration = Duration::from_secs(60);
/// How many times, and how far apart, `/health` is asked after a wake: the network may be slow to come back.
const HEALTH_TRIES: u32 = 5;
const HEALTH_WAIT: Duration = Duration::from_secs(3);

/// How long the computer slept in a tick in which the wall clock moved `wall` (none when it went back) and the
/// monotonic clock `monotonic`: the difference, when it is more than `gap`.
pub(crate) fn slept(wall: Option<Duration>, monotonic: Duration, gap: Duration) -> Option<Duration> {
    let ahead = wall?.checked_sub(monotonic)?;
    (ahead > gap).then_some(ahead)
}

/// Start the watch: each tick refreshes the tray's last backup, and a wake asks the node's `/health` again.
pub(crate) fn watch(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("moqui-desktop-wake".to_string())
        .spawn(move || loop {
            let (wall_before, before) = (SystemTime::now(), Instant::now());
            std::thread::sleep(TICK);
            let wall = SystemTime::now().duration_since(wall_before).ok();
            if let Some(gap) = slept(wall, before.elapsed(), SLEEP_GAP) {
                tracing::info!("The computer woke after about {} seconds asleep", gap.as_secs());
                check_node(&app);
            }
            crate::backup::refresh_last(&app);
        });
    if let Err(err) = spawned {
        tracing::warn!("No watch for the computer waking: {err}");
    }
}

/// Ask the node's `/health` until it answers ready or the tries run out, and log which.
fn check_node(app: &AppHandle) {
    // the node in process, with no port: its /health in process
    if let Some(local) = app.try_state::<crate::node::scheme::Local>() {
        if let Some(node) = local.node() {
            for _ in 0..HEALTH_TRIES {
                let ready = node
                    .request("GET", "/health", &[], Vec::new())
                    .ok()
                    .filter(|answer| answer.status == 200)
                    .and_then(|answer| answer.bytes().ok())
                    .is_some_and(|body| String::from_utf8_lossy(&body).contains("\"ready\":true"));
                if ready {
                    tracing::info!("After the wake the node in process answers ready");
                    return;
                }
                std::thread::sleep(HEALTH_WAIT);
            }
            tracing::warn!("After the wake the node in process does not answer /health ready");
        }
        return;
    }
    let Some(address) = node_address(app) else {
        return;
    };
    for _ in 0..HEALTH_TRIES {
        if crate::node::answers_ready(address) {
            tracing::info!("After the wake the node at {address} answers ready");
            return;
        }
        std::thread::sleep(HEALTH_WAIT);
    }
    tracing::warn!("After the wake the node at {address} does not answer /health ready");
}

fn node_address(app: &AppHandle) -> Option<SocketAddr> {
    let origin = app.state::<NodeState>().origin.get()?.clone();
    origin.socket_addrs(|| None).ok()?.into_iter().next()
}

#[cfg(test)]
#[path = "wake_tests.rs"]
mod tests;
