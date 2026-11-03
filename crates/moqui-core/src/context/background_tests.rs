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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::BackgroundThreads;

#[test]
fn wait_returns_after_the_threads_end() {
    let threads = BackgroundThreads::default();
    let stop = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        let ended = Arc::clone(&ended);
        threads
            .spawn("bg-test", move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::thread::sleep(Duration::from_millis(50));
                ended.store(true, Ordering::SeqCst);
            })
            .unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    threads.wait(Duration::from_secs(5));
    assert!(ended.load(Ordering::SeqCst), "wait returned before the thread ended");
}

#[test]
fn wait_gives_up_at_the_limit_and_takes_no_more_threads() {
    let threads = BackgroundThreads::default();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        threads
            .spawn("bg-stuck", move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .unwrap();
    }
    let started = Instant::now();
    threads.wait(Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(2), "wait passed its limit");
    assert!(
        threads.spawn("bg-late", || {}).is_err(),
        "a thread started after the wait"
    );
    stop.store(true, Ordering::SeqCst);
}
