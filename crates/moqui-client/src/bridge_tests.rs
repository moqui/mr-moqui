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

use std::sync::atomic::AtomicBool;

use super::*;

/// A socket that notes what is sent on it, and when it is dropped (closed).
struct Fake {
    sent: Arc<Mutex<Vec<String>>>,
    dropped: Arc<AtomicBool>,
}

impl PageSocket for Fake {
    fn send(&self, text: String) {
        self.sent.lock().unwrap().push(text);
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct Opened {
    id: u64,
    sent: Arc<Mutex<Vec<String>>>,
    dropped: Arc<AtomicBool>,
}

fn open(sockets: &Sockets, page: &str) -> Opened {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let dropped = Arc::new(AtomicBool::new(false));
    let fake = Fake {
        sent: Arc::clone(&sent),
        dropped: Arc::clone(&dropped),
    };
    let id = sockets
        .open(page, |_| Ok(Box::new(fake) as Box<dyn PageSocket>))
        .unwrap();
    Opened { id, sent, dropped }
}

#[test]
fn only_its_page_sends_on_a_socket() {
    let sockets = Sockets::default();
    let a = open(&sockets, "main");
    let b = open(&sockets, "remote-1");
    assert_ne!(a.id, b.id);
    assert_eq!(
        sockets.send("remote-1", a.id, "x".to_string(), None),
        Err("No such socket".to_string())
    );
    assert_eq!(
        sockets.send("main", 999, "x".to_string(), None),
        Err("No such socket".to_string())
    );
    sockets.send("main", a.id, "hello".to_string(), None).unwrap();
    assert_eq!(*a.sent.lock().unwrap(), vec!["hello".to_string()]);
    assert!(b.sent.lock().unwrap().is_empty());
    // a client's sockets carry text only
    assert_eq!(
        sockets.send("main", a.id, String::new(), Some(vec![1, 2])),
        Err("This socket carries text only".to_string())
    );
}

#[test]
fn a_page_closes_only_its_own_sockets() {
    let sockets = Sockets::default();
    let a = open(&sockets, "main");
    let b = open(&sockets, "remote-1");
    let c = open(&sockets, "remote-1");
    // another page's close, and the close of another page's sockets, leave it open
    sockets.close("remote-1", a.id);
    sockets.close_for("remote-2");
    assert!(!a.dropped.load(Ordering::SeqCst));
    assert!(sockets.send("main", a.id, "still".to_string(), None).is_ok());
    sockets.close_for("remote-1");
    assert!(b.dropped.load(Ordering::SeqCst) && c.dropped.load(Ordering::SeqCst));
    assert!(!a.dropped.load(Ordering::SeqCst));
    sockets.close("main", a.id);
    assert!(a.dropped.load(Ordering::SeqCst));
    assert!(sockets.send("main", a.id, "gone".to_string(), None).is_err());
}

#[test]
fn a_socket_read_for_a_page_that_has_since_gone_is_not_held() {
    let sockets = Sockets::default();
    let before = sockets.generation("main");
    // the page loads again (or the account changes) between the account read and the socket's start
    sockets.close_for("main");
    let refused = sockets.open_since("main", before, |_| panic!("not started for a page that went"));
    assert!(refused.is_err());
    let now = sockets.generation("main");
    sockets.close_all();
    assert!(sockets
        .open_since("main", now, |_| panic!("not started after a switch"))
        .is_err());
    // another page's close does not touch this one's
    let now = sockets.generation("main");
    sockets.close_for("remote-1");
    assert_eq!(sockets.generation("main"), now);
}

#[test]
fn a_socket_that_does_not_start_is_not_held() {
    let sockets = Sockets::default();
    let failed = sockets.open("main", |_| Err("refused".to_string()));
    assert_eq!(failed, Err("refused".to_string()));
    let a = open(&sockets, "main");
    sockets.close_all();
    assert!(a.dropped.load(Ordering::SeqCst));
}

#[test]
fn the_page_hears_each_event_as_its_kind() {
    assert_eq!(event_json(&Event::Open), json!({ "kind": "open" }));
    assert_eq!(
        event_json(&Event::Text("t".to_string())),
        json!({ "kind": "text", "text": "t" })
    );
    assert_eq!(event_json(&Event::Closed), json!({ "kind": "closed" }));
    assert_eq!(event_json(&Event::Revoked), json!({ "kind": "revoked" }));
    // the terminal's bytes in base64, and a close with its reason
    assert_eq!(
        event_json(&Event::Binary(vec![0x1b, b'['])),
        json!({ "kind": "binary", "data": "G1s=" })
    );
    assert_eq!(
        event_json(&Event::Ended("opened elsewhere".to_string())),
        json!({ "kind": "closed", "reason": "opened elsewhere" })
    );
    assert_eq!(decode_binary("G1s=").unwrap(), vec![0x1b, b'[']);
    assert!(decode_binary("not base64!").is_err());
}
