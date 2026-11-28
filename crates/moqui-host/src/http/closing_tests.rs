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

/// A request that came over a listener ends on that listener's closing, and not on the process-wide stop; one that
/// came over none (in process, down the edge link) ends on the process-wide stop.
#[test]
fn a_request_closes_with_its_listener_or_else_with_the_process() {
    let (process, process_rx) = watch::channel(false);
    let (listener, listener_rx) = watch::channel(false);
    let mut over = axum::http::Extensions::new();
    over.insert(OnListener {
        name: "HTTP",
        closing: listener_rx,
    });
    let none = axum::http::Extensions::new();

    let on_listener = closing_of(&over, &process_rx);
    let in_process = closing_of(&none, &process_rx);
    listener.send(true).unwrap();
    assert!(*on_listener.borrow());
    assert!(!*in_process.borrow(), "another listener's closing is not the process's");

    process.send(true).unwrap();
    assert!(*in_process.borrow());
    assert_eq!(over.get::<OnListener>().map(|on| on.name), Some("HTTP"));
}

/// A receiver taken after its listener began closing has seen nothing yet, so a handler that waits for a change
/// ends at once rather than waiting for another.
#[test]
fn a_receiver_taken_after_the_closing_sees_it_as_a_change() {
    let (listener, listener_rx) = watch::channel(false);
    let mut over = axum::http::Extensions::new();
    over.insert(OnListener {
        name: "HTTPS",
        closing: listener_rx,
    });
    listener.send(true).unwrap();
    let (_process, process_rx) = watch::channel(false);
    let closing = closing_of(&over, &process_rx);
    assert!(closing.has_changed().unwrap());
}

/// `shutdown-timeout` gives the grace: a limit, or as long as the connections take when it is 0.
#[test]
fn the_grace_of_the_shutdown_timeout() {
    assert_eq!(
        Grace::of(Some(Duration::from_secs(30))),
        Grace::Upto(Duration::from_secs(30))
    );
    assert_eq!(Grace::of(None), Grace::Unbounded);
}
