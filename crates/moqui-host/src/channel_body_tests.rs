use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::Body as _;
use tokio::sync::mpsc;

use super::ChannelBody;

#[test]
fn an_error_after_data_waits_one_poll_so_the_data_is_flushed_first() {
    let (tx, rx) = mpsc::channel(4);
    tx.try_send(Ok(Bytes::from_static(b"part"))).unwrap();
    tx.try_send(Err(std::io::Error::other("failed"))).unwrap();
    let mut body = ChannelBody::new(rx);
    let waker = futures_util::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut poll = || Pin::new(&mut body).poll_frame(&mut cx);
    let Poll::Ready(Some(Ok(frame))) = poll() else {
        panic!("the data first");
    };
    assert_eq!(frame.into_data().unwrap(), Bytes::from_static(b"part"));
    assert!(poll().is_pending(), "the error waits a poll");
    assert!(matches!(poll(), Poll::Ready(Some(Err(_)))));
    drop(tx);
    assert!(matches!(poll(), Poll::Ready(None)));
}
