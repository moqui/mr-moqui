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

//! The A2A stream's writer (HL-027, HL-014).

use std::time::{Duration, Instant};

use super::*;

/// A client that stops reading ends the stream within the send stall, so its thread and slot come back; the frames
/// it did take are the ones the channel held.
#[test]
fn a_reader_that_stops_reading_ends_the_stream_within_the_stall() {
    // the client took nothing: its channel holds two frames, then is full
    let (tx, rx) = mpsc::channel::<Chunk>(2);
    let (_stopping, shutdown) = tokio::sync::watch::channel(false);
    let written = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&written);
    let write: Box<dyn FnOnce(a2a::Sink<'_>, a2a::Stop<'_>) + Send> = Box::new(move |sink, stop| {
        // a stream that always has another frame, as a long answer does
        while !stop() && sink(Some("event: x\ndata: {}\n\n")) {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let started = Instant::now();
    write_stream(write, &tx, &shutdown, Duration::from_millis(300));
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(300) && took < Duration::from_secs(2),
        "{took:?}"
    );
    assert_eq!(written.load(Ordering::SeqCst), 2);
    drop(rx);
}

/// A client that closed ends the stream at once, by the stop check, with nothing more sent.
#[test]
fn a_closed_client_ends_the_stream_at_once() {
    let (tx, rx) = mpsc::channel::<Chunk>(2);
    drop(rx);
    let (_stopping, shutdown) = tokio::sync::watch::channel(false);
    let write: Box<dyn FnOnce(a2a::Sink<'_>, a2a::Stop<'_>) + Send> = Box::new(|sink, stop| {
        assert!(stop());
        assert!(!sink(Some("event: x\ndata: {}\n\n")));
    });
    let started = Instant::now();
    write_stream(write, &tx, &shutdown, Duration::from_secs(60));
    assert!(started.elapsed() < Duration::from_millis(100));
}
