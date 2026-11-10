use std::time::Duration;

use moqui_core::web::stream::OutputSink;
use moqui_core::web::WebResponse;
use tokio::sync::{mpsc, oneshot};

use super::{ScreenSink, CHUNK, QUEUED};

fn open() -> (ScreenSink, oneshot::Receiver<WebResponse>, mpsc::Receiver<super::Chunk>) {
    let (head_tx, head_rx) = oneshot::channel();
    let (body_tx, body_rx) = mpsc::channel(QUEUED);
    (
        ScreenSink::new(head_tx, body_tx, Duration::from_millis(50)),
        head_rx,
        body_rx,
    )
}

#[test]
fn the_head_goes_once_and_the_body_in_chunks_until_it_is_finished() {
    let (mut sink, mut head_rx, mut body_rx) = open();
    sink.head(WebResponse::new(200, Vec::new())).unwrap();
    assert_eq!(head_rx.try_recv().unwrap().status, 200);
    assert!(sink.head(WebResponse::new(200, Vec::new())).is_err());
    // what was kept before the commit goes at once
    sink.write(b"start").unwrap();
    assert_eq!(&body_rx.try_recv().unwrap().unwrap()[..], b"start");
    sink.write(b"small").unwrap();
    assert!(body_rx.try_recv().is_err(), "a small write waits for more");
    sink.write(&vec![b'x'; CHUNK]).unwrap();
    let chunk = body_rx.try_recv().unwrap().unwrap();
    assert_eq!(chunk.len(), CHUNK + 5);
    sink.write(b"tail").unwrap();
    sink.finish();
    assert_eq!(&body_rx.try_recv().unwrap().unwrap()[..], b"tail");
    drop(sink);
    assert!(
        body_rx.try_recv().is_err(),
        "a finished body ends cleanly, with no error chunk"
    );
}

#[test]
fn an_abort_or_a_drop_without_finishing_ends_the_body_with_an_error() {
    let (mut sink, _head_rx, mut body_rx) = open();
    sink.head(WebResponse::new(200, Vec::new())).unwrap();
    sink.write(b"start").unwrap();
    sink.write(b"part").unwrap();
    sink.abort("the render failed");
    assert_eq!(&body_rx.try_recv().unwrap().unwrap()[..], b"start");
    // what was held goes, then the error ends the body
    assert_eq!(&body_rx.try_recv().unwrap().unwrap()[..], b"part");
    assert!(body_rx.try_recv().unwrap().is_err());

    let (mut sink, _head_rx, mut body_rx) = open();
    sink.head(WebResponse::new(200, Vec::new())).unwrap();
    drop(sink);
    assert!(body_rx.try_recv().unwrap().is_err());

    // with no head nothing was sent, and the handler sends the whole response instead
    let (sink, mut head_rx, mut body_rx) = open();
    drop(sink);
    assert!(head_rx.try_recv().is_err());
    assert!(body_rx.try_recv().is_err());
}

#[test]
fn a_client_that_stops_reading_fails_the_write_after_the_stall() {
    let (mut sink, _head_rx, _body_rx) = open();
    sink.head(WebResponse::new(200, Vec::new())).unwrap();
    let chunk = vec![b'x'; CHUNK];
    let failed = (0..=QUEUED).map(|_| sink.write(&chunk)).find(Result::is_err);
    let err = failed.expect("the queue fills and the write times out").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}
