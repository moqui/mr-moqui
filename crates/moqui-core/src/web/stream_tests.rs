use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use super::{abort, arm, committed, finish, flush, offer, on_head, OutputSink};
use crate::web::{self, WebResponse};

/// What a test sink was given.
#[derive(Default)]
struct Seen {
    head: Option<WebResponse>,
    body: Vec<u8>,
    flushes: usize,
    finished: bool,
    aborted: Option<String>,
}

struct TestSink {
    seen: Rc<RefCell<Seen>>,
    /// Writes fail as when the client went away.
    gone: bool,
}

impl OutputSink for TestSink {
    fn head(&mut self, head: WebResponse) -> std::io::Result<()> {
        self.seen.borrow_mut().head = Some(head);
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.gone {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone"));
        }
        self.seen.borrow_mut().body.extend_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.seen.borrow_mut().flushes += 1;
        Ok(())
    }

    fn finish(&mut self) {
        self.seen.borrow_mut().finished = true;
    }

    fn abort(&mut self, reason: &str) {
        self.seen.borrow_mut().aborted = Some(reason.to_string());
    }
}

/// A web slot with a test sink offered, and what the sink sees.
fn offered(gone: bool) -> (web::WebGuard, Rc<RefCell<Seen>>) {
    let guard = web::install(BTreeMap::new());
    let seen = Rc::new(RefCell::new(Seen::default()));
    offer(Box::new(TestSink {
        seen: Rc::clone(&seen),
        gone,
    }));
    (guard, seen)
}

#[test]
fn a_body_under_the_threshold_stays_whole_and_one_over_it_commits_once_with_its_head() {
    let (_guard, seen) = offered(false);
    assert!(arm(16));
    assert!(web::start_output("text/html;charset=utf-8"));
    assert!(web::write_output("<html>"));
    assert!(!committed());
    assert!(seen.borrow().head.is_none());
    web::add_header("X-Before", "kept");
    assert!(web::write_output("<body>0123456789"));
    assert!(committed());
    {
        let seen = seen.borrow();
        let head = seen.head.as_ref().expect("the head went at the threshold");
        assert_eq!(head.status, 200);
        assert_eq!(head.header("content-type"), Some("text/html;charset=utf-8"));
        assert_eq!(head.header("x-before"), Some("kept"));
        assert_eq!(seen.body, b"<html><body>0123456789");
    }
    // after the commit a write goes straight to the sink, a header is dropped, and the body cannot be rewound
    assert!(web::write_output("</body>"));
    web::add_header("X-After", "dropped");
    web::set_header("X-Before", Some("changed"));
    web::truncate_output(0);
    assert_eq!(seen.borrow().body, b"<html><body>0123456789</body>");
    assert!(web::output_body_len().is_some_and(|len| len == 0));
    finish();
    let seen = seen.borrow();
    assert!(seen.finished);
    assert!(seen.aborted.is_none());
    assert!(seen.head.as_ref().is_some_and(|head| head.header("x-after").is_none()));
}

#[test]
fn an_unarmed_sink_never_commits_and_the_response_is_taken_whole() {
    let (_guard, seen) = offered(false);
    web::start_output("text/plain");
    for _ in 0..100 {
        web::write_output("0123456789");
    }
    assert!(!committed());
    finish();
    assert!(seen.borrow().head.is_none());
    let (_, _, body, sent) = web::take_output();
    assert!(sent);
    assert_eq!(body.len(), 1000);
    // with no sink offered there is nothing to arm
    let _plain = web::install(BTreeMap::new());
    assert!(!arm(16));
}

#[test]
fn head_hooks_run_last_registered_first_and_finish_the_head() {
    let (_guard, seen) = offered(false);
    let order = Rc::new(RefCell::new(Vec::new()));
    for name in ["request", "render"] {
        let order = Rc::clone(&order);
        on_head(Box::new(move |head: &mut WebResponse| {
            order.borrow_mut().push(name);
            head.headers.push((format!("X-{name}"), "1".to_string()));
            Ok(())
        }));
    }
    arm(4);
    web::start_output("text/plain");
    web::write_output("12345");
    assert_eq!(*order.borrow(), ["render", "request"]);
    let seen = seen.borrow();
    let head = seen.head.as_ref().expect("committed");
    assert_eq!(head.header("x-render"), Some("1"));
    assert_eq!(head.header("x-request"), Some("1"));
}

#[test]
fn a_response_with_an_etag_stays_whole_so_it_can_be_answered_304() {
    let (_guard, seen) = offered(false);
    arm(4);
    web::set_header("ETag", Some("\"v1\""));
    web::start_output("text/plain");
    web::write_output("0123456789");
    web::write_output("0123456789");
    assert!(!committed());
    assert!(seen.borrow().head.is_none());
    assert_eq!(web::take_output().2.len(), 20);
}

#[test]
fn flush_commits_what_is_written_and_then_flushes_the_sink() {
    let (_guard, seen) = offered(false);
    arm(1024);
    // nothing written yet: nothing to commit
    flush();
    assert!(!committed());
    web::write_bytes(b"part");
    flush();
    assert!(committed());
    assert_eq!(seen.borrow().body, b"part");
    flush();
    assert_eq!(seen.borrow().flushes, 1);
}

#[test]
fn a_client_that_went_away_stops_the_writes_and_a_failure_aborts_the_body() {
    let (_guard, seen) = offered(true);
    arm(4);
    web::start_output("text/plain");
    assert!(!web::write_output("0123456789"));
    assert!(committed());
    assert!(!web::write_output("more"));
    finish();
    assert!(
        !seen.borrow().finished,
        "a failed response is not finished as if it were whole"
    );

    let (_guard, seen) = offered(false);
    arm(4);
    web::start_output("text/plain");
    web::write_output("0123456789");
    abort("the render failed");
    assert!(!web::write_output("after"));
    finish();
    let seen = seen.borrow();
    assert_eq!(seen.aborted.as_deref(), Some("the render failed"));
    assert!(!seen.finished);
}

#[test]
fn a_failed_head_hook_sends_nothing_and_aborts() {
    let (_guard, seen) = offered(false);
    on_head(Box::new(|_head: &mut WebResponse| {
        Err(crate::context::Error::new("no token"))
    }));
    arm(4);
    web::start_output("text/plain");
    assert!(!web::write_output("0123456789"));
    let seen = seen.borrow();
    assert!(seen.head.is_none());
    assert!(seen.aborted.as_deref().is_some_and(|why| why.contains("no token")));
}
