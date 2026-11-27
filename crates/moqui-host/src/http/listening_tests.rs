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

use super::*;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

/// A loop that ended, by itself or aborted, is finished; one still running is not, within a short wait.
#[test]
fn a_serve_task_is_finished_once_its_loop_ends_or_is_aborted() {
    let runtime = runtime();
    let (stop, _own) = watch::channel(false);
    let ended = ServeTask::spawn(runtime.handle(), stop, async {});
    assert!(ended.finished(Duration::from_secs(5)));
    let (stop, _own) = watch::channel(false);
    let running = ServeTask::spawn(runtime.handle(), stop, std::future::pending());
    assert!(!running.finished(Duration::from_millis(20)));
    running.task.abort();
    assert!(running.finished(Duration::from_secs(5)));
}

/// `begin_stop` tells the loop its own stop; dropping the task stops and aborts it, so what it holds (its listener)
/// goes at once.
#[test]
fn a_serve_task_stops_on_its_own_stop_and_aborts_when_dropped() {
    let runtime = runtime();
    let (stop, mut own) = watch::channel(false);
    let stopping = ServeTask::spawn(runtime.handle(), stop, async move {
        let _ = own.wait_for(|stopped| *stopped).await;
    });
    stopping.begin_stop();
    assert!(stopping.finished(Duration::from_secs(5)));
    // the loop holds this sender until it is dropped with the loop
    let (held, gone) = mpsc::sync_channel::<()>(1);
    let (stop, _own) = watch::channel(false);
    let dropped = ServeTask::spawn(runtime.handle(), stop, async move {
        let _held = held;
        std::future::pending::<()>().await;
    });
    drop(dropped);
    assert_eq!(
        gone.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

/// A loop takes connections only once `go` turns true; a stop first, or `go`'s sender dropped (a start that failed),
/// releases nothing.
#[test]
fn a_loop_is_released_by_go_and_never_after_a_stop_or_a_failed_start() {
    let runtime = runtime();
    runtime.block_on(async {
        let (go, go_rx) = watch::channel(false);
        let (_process, process_rx) = watch::channel(false);
        let (_own, own_rx) = watch::channel(false);
        let waiting = tokio::spawn(released(go_rx.clone(), process_rx.clone(), own_rx.clone()));
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        go.send(true).unwrap();
        assert!(waiting.await.unwrap());

        let (go, go_rx) = watch::channel(false);
        let (own, own_rx) = watch::channel(false);
        let stopped = tokio::spawn(released(go_rx, process_rx.clone(), own_rx));
        own.send(true).unwrap();
        assert!(!stopped.await.unwrap());
        drop(go);

        let (go, go_rx) = watch::channel(false);
        let (_kept, kept_rx) = watch::channel(false);
        let failed = tokio::spawn(released(go_rx, process_rx, kept_rx));
        drop(go);
        assert!(!failed.await.unwrap());
    });
}

/// Each phase of the process stop waits for `shutdown-timeout` and a margin: 30 s when unset, as long as the
/// connections take when 0, and a huge one saturates rather than overflow.
#[test]
fn the_process_stop_waits_for_the_shutdown_timeout_and_a_margin() {
    use crate::http::{conf_stop_wait, process_stop_wait, ListenLimits};
    let limits = |timeout: &str| {
        let conf = format!(r#"<web-protection><connections shutdown-timeout="{timeout}"/></web-protection>"#);
        ListenLimits::from_node(Some(&moqui_lib::node::MNode::parse_text("test", &conf).unwrap()))
    };
    assert_eq!(
        process_stop_wait(&ListenLimits::from_node(None)),
        Duration::from_secs(35)
    );
    assert_eq!(process_stop_wait(&limits("10")), Duration::from_secs(15));
    assert_eq!(process_stop_wait(&limits("0")), Duration::MAX);
    assert_eq!(process_stop_wait(&limits(&u64::MAX.to_string())), Duration::MAX);
    let context = moqui_core::context::ExecutionContextFactory::test_context();
    assert_eq!(conf_stop_wait(context.factory()), Duration::from_secs(35));
}
