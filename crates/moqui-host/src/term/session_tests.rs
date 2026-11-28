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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use super::{spawn, Attached, Joining, Kind, Out, Program, Spec, HIGH_WATER};
use crate::term::registry::Registry;
use crate::term::Ended;

/// A program the test drives: what it writes is the session's output, what the session writes to it is read back.
struct Fake {
    program: DuplexStream,
    sizes: Arc<Mutex<Vec<(u16, u16)>>>,
    redraws: Arc<Mutex<usize>>,
    /// Dropped, the program has ended.
    _done: tokio::sync::oneshot::Sender<()>,
}

fn session(kind: Kind, replay: usize, detached_for: Duration) -> (Arc<Registry>, Arc<super::Session>, Fake) {
    let registry = Arc::new(Registry::new(detached_for));
    let (ours, program) = tokio::io::duplex(1 << 20);
    let (output, input) = tokio::io::split(ours);
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let redraws = Arc::new(Mutex::new(0));
    let (seen_sizes, seen_redraws) = (Arc::clone(&sizes), Arc::clone(&redraws));
    let (done, ended) = tokio::sync::oneshot::channel::<()>();
    let program_value = Program {
        output: Box::pin(output),
        input: Box::pin(input),
        resize: Box::new(move |cols, rows| seen_sizes.lock().unwrap().push((cols, rows))),
        redraw: (kind == Kind::Apps)
            .then(|| -> Box<dyn FnMut() + Send> { Box::new(move || *seen_redraws.lock().unwrap() += 1) }),
        hang_up: None,
        kill: None,
        ended: Box::pin(async move {
            let _ = ended.await;
            Ended { status: 0, note: None }
        }),
    };
    let spec = Spec {
        id: "s1".to_string(),
        username: "dev".to_string(),
        kind,
        profile: String::new(),
        title: "test".to_string(),
        cwd: String::new(),
        cols: 80,
        rows: 24,
        detached_for,
        replay,
        slot: None,
        on_end: None,
        held: false,
        agent: false,
        attend: None,
        adopted: false,
        count_from: 0,
    };
    let session = spawn(&registry, spec, program_value);
    (
        registry,
        session,
        Fake {
            program,
            sizes,
            redraws,
            _done: done,
        },
    )
}

/// The next messages until the bytes hold `marker`: the text messages, and the bytes.
async fn read_until(attached: &mut Attached, marker: &str) -> (Vec<String>, Vec<u8>) {
    let mut texts = Vec::new();
    let mut bytes = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !String::from_utf8_lossy(&bytes).contains(marker) {
        match tokio::time::timeout_at(deadline, attached.out.recv()).await {
            Ok(Some(Out::Bytes(more))) => bytes.extend(more),
            Ok(Some(Out::Text(text))) => texts.push(text),
            Ok(Some(Out::Close(reason))) => panic!("closed: {reason}"),
            Ok(Some(Out::Viewer(_))) => {}
            Ok(None) | Err(_) => panic!("no {marker:?} in {:?}", String::from_utf8_lossy(&bytes)),
        }
    }
    (texts, bytes)
}

#[tokio::test]
async fn a_client_that_comes_back_inside_the_ring_gets_nothing_twice() {
    let (_registry, session, mut fake) = session(Kind::Command, 4096, Duration::from_secs(60));
    fake.program.write_all(b"before\n").await.unwrap();
    let mut first = session.attach(None, 100, 30).await.unwrap();
    let (texts, bytes) = read_until(&mut first, "before").await;
    assert!(texts[0].contains(r#""reset":true"#), "{texts:?}");
    assert!(String::from_utf8_lossy(&bytes).ends_with("before\n"));
    fake.program.write_all(b"more\n").await.unwrap();
    read_until(&mut first, "more").await;
    session.detach(first.client).await;
    fake.program.write_all(b"while away\n").await.unwrap();
    // drawn: "before\n" and "more\n", 12 bytes
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut back = session.attach(Some(12), 100, 30).await.unwrap();
    let (texts, bytes) = read_until(&mut back, "while away").await;
    assert!(
        texts[0].contains(r#""reset":false"#) && texts[0].contains(r#""from":12"#),
        "{texts:?}"
    );
    assert_eq!(bytes, b"while away\n");
    // the client's input reaches the program
    assert!(session.write(b"ls\r".to_vec()).await);
    let mut typed = [0u8; 3];
    fake.program.read_exact(&mut typed).await.unwrap();
    assert_eq!(&typed, b"ls\r");
    assert_eq!(fake.sizes.lock().unwrap().last(), Some(&(100, 30)));
}

#[tokio::test]
async fn a_second_attach_takes_the_session_and_an_apps_session_draws_again() {
    let (_registry, session, fake) = session(Kind::Apps, 0, Duration::from_secs(60));
    let mut first = session.attach(None, 80, 24).await.unwrap();
    let mut second = session.attach(None, 90, 30).await.unwrap();
    let mut closed = None;
    while let Some(out) = first.out.recv().await {
        if let Out::Close(reason) = out {
            closed = Some(reason);
            break;
        }
    }
    assert_eq!(closed.as_deref(), Some("opened elsewhere"));
    assert!(matches!(second.out.recv().await, Some(Out::Text(text)) if text.contains(r#""type":"attached""#)));
    assert_eq!(
        *fake.redraws.lock().unwrap(),
        2,
        "each attach asks an apps session to draw again"
    );
    assert!(session.info().attached);
}

#[tokio::test]
async fn a_client_far_behind_stops_the_output_until_it_acknowledges() {
    let (_registry, session, mut fake) = session(Kind::Command, 4 << 20, Duration::from_secs(60));
    let mut client = session.attach(None, 80, 24).await.unwrap();
    let writer = tokio::spawn(async move {
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..32 {
            fake.program.write_all(&chunk).await.unwrap();
        }
        fake
    });
    let mut received = 0_u64;
    let quiet = Duration::from_millis(300);
    while let Ok(Some(out)) = tokio::time::timeout(quiet, client.out.recv()).await {
        if let Out::Bytes(bytes) = out {
            received += bytes.len() as u64;
        }
    }
    assert!(
        received > HIGH_WATER && received < 2 * HIGH_WATER,
        "paused near the high water: {received}"
    );
    session.ack(client.client, received).await;
    while received < 32 * 64 * 1024 {
        match tokio::time::timeout(Duration::from_secs(5), client.out.recv()).await {
            Ok(Some(Out::Bytes(bytes))) => {
                received += bytes.len() as u64;
                session.ack(client.client, received).await;
            }
            Ok(Some(_)) => {}
            other => panic!("the output stopped at {received}: {:?}", other.map(|out| out.is_some())),
        }
    }
    drop(writer.await.unwrap());
}

#[tokio::test]
async fn a_session_with_no_window_for_its_time_ends_and_leaves_the_registry() {
    let (registry, session, mut fake) = session(Kind::Apps, 0, Duration::from_millis(100));
    assert!(registry.find("s1", "dev").is_some());
    assert!(registry.find("s1", "other").is_none(), "another user's id is not there");
    // the program sees its input close, as a socket closing would end it
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), fake.program.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    drop(fake);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while registry.find("s1", "dev").is_some() {
        assert!(tokio::time::Instant::now() < deadline, "still listed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(session.info().ended.unwrap().1.contains("with no window"));
}

#[cfg(feature = "devtools")]
#[tokio::test]
async fn the_screen_reads_as_text_after_output_a_resize_and_the_alternate_screen() {
    use crate::term::screen::Scope;
    let (_registry, session, mut fake) = session(Kind::Command, 4096, Duration::from_secs(60));
    fake.program.write_all(b"one\r\ntwo\r\n$ ").await.unwrap();
    // made on this first read, from what a new window would be sent
    let mut read = None;
    for _ in 0..50 {
        read = session.screen(Scope::Last(3), false).await;
        if read
            .as_ref()
            .is_some_and(|read| read.lines.last().is_some_and(|line| line == "$"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let read = read.unwrap();
    assert_eq!(read.lines, ["one", "two", "$"]);
    assert_eq!((read.cols, read.rows), (80, 24));
    assert!(!read.rebuilt, "the whole output, at one size");
    // fed from then on, and following the window's size
    let client = session.attach(None, 100, 30).await.unwrap();
    fake.program.write_all(b"\x1b[?1049h\x1b[Hfull").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let read = session.screen(Scope::Visible, false).await.unwrap();
    assert_eq!((read.cols, read.rows), (100, 30));
    assert!(read.alternate);
    assert_eq!(read.lines[0], "full");
    session.detach(client.client).await;
    // an apps session keeps no ring, and no screen
    let (_registry, apps, _fake) = self::session(Kind::Apps, 0, Duration::from_secs(60));
    assert!(apps.screen(Scope::Visible, false).await.is_none());
}

fn joining(username: &str, observe: bool) -> Joining {
    Joining {
        observe,
        username: username.to_string(),
    }
}

/// The next close of `attached`, passing over what comes before it.
async fn closed(attached: &mut Attached) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, attached.out.recv()).await {
            Ok(Some(Out::Close(reason))) => return reason,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!("never closed"),
        }
    }
}

#[tokio::test]
async fn viewers_watch_beside_the_writer_and_a_second_writer_makes_the_first_a_viewer() {
    let (_registry, session, mut fake) = session(Kind::Command, 1 << 20, Duration::from_secs(60));
    let mut writer = session.attach(None, 100, 30).await.unwrap();
    let mut viewer = session.attach_as(None, 40, 10, joining("bob", true)).await.unwrap();
    assert!(viewer.viewer && !writer.viewer);
    fake.program.write_all(b"for both\n").await.unwrap();
    read_until(&mut writer, "for both").await;
    let (texts, _) = read_until(&mut viewer, "for both").await;
    assert!(
        texts[0].contains(r#""viewer":true"#) && texts[0].contains(r#""cols":100"#),
        "{texts:?}"
    );
    assert_eq!(session.info().viewers, ["bob"]);
    // a viewer's size is not the session's
    session.resize(viewer.client, 50, 20).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(fake.sizes.lock().unwrap().last(), Some(&(100, 30)));
    // the owner's second window types; the first watches on, told so, and still gets the output
    let mut second = session.attach(None, 90, 25).await.unwrap();
    let mut told = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !told {
        match tokio::time::timeout_at(deadline, writer.out.recv()).await {
            Ok(Some(Out::Viewer(true))) => told = true,
            Ok(Some(Out::Close(reason))) => panic!("closed: {reason}"),
            Ok(Some(_)) => {}
            _ => panic!("not told"),
        }
    }
    fake.program.write_all(b"after\n").await.unwrap();
    read_until(&mut writer, "after").await;
    read_until(&mut second, "after").await;
    assert_eq!(session.info().viewers.len(), 2);
    assert!(session.info().attached);
}

#[tokio::test]
async fn a_viewer_far_behind_is_let_go_and_a_revoked_share_closes_its_windows() {
    let (_registry, session, mut fake) = session(Kind::Command, 4 << 20, Duration::from_secs(60));
    session.share("bob", false);
    let mut slow = session.attach_as(None, 80, 24, joining("bob", true)).await.unwrap();
    // it never acknowledges: past the high water it is let go, and the program is never held up for it
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..12 {
        fake.program.write_all(&chunk).await.unwrap();
    }
    assert_eq!(closed(&mut slow).await, "behind");
    // one that joins now gets the whole replay, which is not held against it
    let mut late = session.attach_as(None, 80, 24, joining("bob", true)).await.unwrap();
    read_until(&mut late, "xxxx").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    fake.program.write_all(b"\nstill here\n").await.unwrap();
    read_until(&mut late, "still here").await;
    drop(late);
    let mut watching = session.attach_as(None, 80, 24, joining("bob", true)).await.unwrap();
    assert!(session.unshare("bob"));
    assert_eq!(closed(&mut watching).await, "the share was revoked");
    assert_eq!(session.share_of("bob"), None);
}

#[tokio::test]
async fn another_users_viewer_keeps_no_session_alive() {
    let (registry, session, mut fake) = session(Kind::Command, 4096, Duration::from_millis(150));
    let _viewer = session.attach_as(None, 80, 24, joining("bob", true)).await.unwrap();
    // the detach timeout closes the program's input, as with no window at all
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), fake.program.read_to_end(&mut rest))
        .await
        .expect("the input closed with only another user watching")
        .unwrap();
    drop(fake);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while session.info().ended.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "still running with only another user watching"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = registry;
    // an apps session has nothing to watch
    let (_registry, apps, _fake) = self::session(Kind::Apps, 0, Duration::from_secs(60));
    assert!(apps.attach_as(None, 80, 24, joining("dev", true)).await.is_none());
}
