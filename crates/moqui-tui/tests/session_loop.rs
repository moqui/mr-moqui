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

//! The session loop of an SSH channel: the frames it writes, replayed on a small terminal here, show what Revue's
//! pipeline draws, and the loop runs keys, answers, sizes and the close over a channel.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use moqui_tui::{
    gallery_link, run_session, AppView, Depth, FakeLink, Key, KeyPress, RawBody, Screen, ScreenLink, ScreenModel,
    SessionEvent, SessionOptions, Theme, ENTER, GALLERY_LIST_PATH, LEAVE, MOUSE_OFF, MOUSE_ON,
};

/// The part of a terminal the encoder speaks to: cursor moves, clears, SGR (ignored), OSC 8, and characters.
struct Term {
    width: usize,
    cells: Vec<Vec<char>>,
    links: Vec<Vec<Option<String>>>,
    x: usize,
    y: usize,
    link: Option<String>,
}

impl Term {
    fn new(width: u16, height: u16) -> Self {
        let (width, height) = (usize::from(width), usize::from(height));
        Self {
            width,
            cells: vec![vec![' '; width]; height],
            links: vec![vec![None; width]; height],
            x: 0,
            y: 0,
            link: None,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let text = String::from_utf8(bytes.to_vec()).expect("frames are UTF-8");
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                self.put(ch);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    let mut body = String::new();
                    for next in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&next) {
                            self.csi(&body, next);
                            break;
                        }
                        body.push(next);
                    }
                }
                Some(']') => {
                    let mut body = String::new();
                    while let Some(next) = chars.next() {
                        if next == '\x07' {
                            break;
                        }
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                        body.push(next);
                    }
                    // The background query (OSC 11) and a copy to the clipboard (OSC 52) draw nothing.
                    if body == "11;?" || body.starts_with("52;c;") {
                        continue;
                    }
                    let url = body.strip_prefix("8;;").expect("only OSC 8, 11 and 52 are written");
                    self.link = (!url.is_empty()).then(|| url.to_string());
                }
                other => panic!("unexpected escape {other:?}"),
            }
        }
    }

    fn csi(&mut self, body: &str, last: char) {
        match last {
            'H' => {
                let mut parts = body.split(';').map(|part| part.parse::<usize>().unwrap_or(1));
                self.y = parts.next().unwrap_or(1) - 1;
                self.x = parts.next().unwrap_or(1) - 1;
            }
            'J' => {
                for row in &mut self.cells {
                    row.fill(' ');
                }
                for row in &mut self.links {
                    row.fill(None);
                }
            }
            'm' | 'h' | 'l' => {}
            other => panic!("unexpected CSI {body}{other}"),
        }
    }

    fn put(&mut self, ch: char) {
        let width = moqui_tui::display_width(&ch.to_string()).max(1);
        assert!(
            self.x + width <= self.width,
            "{ch} written past the edge at {},{}",
            self.x,
            self.y
        );
        self.cells[self.y][self.x] = ch;
        self.links[self.y][self.x] = self.link.clone();
        if width == 2 {
            self.cells[self.y][self.x + 1] = '\0';
        }
        self.x += width;
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                row.iter()
                    .filter(|ch| **ch != '\0')
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }
}

fn list_view() -> AppView {
    let link = gallery_link();
    let model = link.level(GALLERY_LIST_PATH, &BTreeMap::new()).unwrap();
    let mut view = AppView::new(model, link);
    view.set_clock("Tue 14:32");
    view
}

fn frame(screen: &mut Screen, view: &AppView) -> Vec<u8> {
    let mut bytes = Vec::new();
    screen.draw(view, &mut bytes).unwrap();
    bytes
}

#[test]
fn frames_replayed_show_what_the_pipeline_draws() {
    let mut view = list_view();
    let mut screen = Screen::new(100, 40, Depth::TrueColor);
    let mut term = Term::new(100, 40);
    let first = frame(&mut screen, &view);
    term.feed(&first);
    assert_eq!(term.text(), view.screen_text(100, 40).trim_end());

    // Nothing changed: nothing is written, linked cells included.
    assert!(frame(&mut screen, &view).is_empty());

    // A key moves the focus: only the cells that changed are written, and the replay still matches.
    view.on_key(&KeyPress::plain(Key::Tab));
    let second = frame(&mut screen, &view);
    assert!(!second.is_empty());
    assert!(second.len() * 4 < first.len(), "{} of {}", second.len(), first.len());
    term.feed(&second);
    assert_eq!(term.text(), view.screen_text(100, 40).trim_end());

    // A new size is drawn whole after a clear.
    view.session_mut().resize(180, 50);
    screen.resize(180, 50);
    let mut term = Term::new(180, 50);
    term.feed(&frame(&mut screen, &view));
    assert_eq!(term.text(), view.screen_text(180, 50).trim_end());
}

/// The parameters of each SGR sequence in a frame (cursor moves left out, which hold the same digits).
fn sgr(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    text.split("\x1b[")
        .skip(1)
        .filter_map(|part| {
            let end = part.find(|ch: char| ('\x40'..='\x7e').contains(&ch))?;
            (part[end..].starts_with('m')).then(|| part[..end].to_string())
        })
        .collect()
}

#[test]
fn colors_follow_the_terminal_depth() {
    let view = list_view();
    let looks = |depth| sgr(&frame(&mut Screen::new(100, 40, depth), &view));
    let has = |looks: &[String], part: &str| looks.iter().any(|look| look.contains(part));
    let true_color = looks(Depth::TrueColor);
    assert!(has(&true_color, "38;2;"));
    let indexed = looks(Depth::Ansi256);
    assert!(has(&indexed, "38;5;") && !has(&indexed, "38;2;"));
    let sixteen = looks(Depth::Ansi16);
    assert!(!has(&sixteen, "8;5;") && !has(&sixteen, "8;2;"));
    assert!(sixteen.iter().any(|look| look
        .split(';')
        .any(|p| matches!(p.parse::<u8>(), Ok(30..=37 | 90..=97)))));
    let mono = looks(Depth::Mono);
    assert!(!has(&mono, "38;") && !has(&mono, "48;"));
}

/// ASCII (`--ascii`, `TERM=linux`) writes no byte above 0x7F: borders, glyphs and marks all have stand-ins.
#[test]
fn ascii_writes_only_ascii() {
    let mut view = list_view();
    let mut theme = view.theme().clone();
    theme.ascii = true;
    view.set_theme(theme);
    let bytes = frame(&mut Screen::new(100, 40, Depth::Mono), &view);
    // The list's own data has CJK and emoji, which stay as the server sent them; no mark of the client's is left.
    let wide: String = String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|ch| !ch.is_ascii() && !"東京🎉👨👩👧".contains(*ch))
        .collect();
    assert!(wide.is_empty(), "{wide:?}");
}

#[test]
fn a_wide_glyph_is_written_once_and_links_by_url() {
    let model = ScreenModel::from_json(
        r#"{ "v": 1, "screen": { "path": "/apps/app/Wide", "title": "Wide" }, "c": [
          { "t": "text", "id": "t", "text": "東京 office" },
          { "t": "link", "id": "out", "text": "Docs", "url": "https://moqui.org/docs" } ] }"#,
    )
    .unwrap();
    let mut view = AppView::new(model, FakeLink::from_json(&[]).unwrap());
    view.set_clock("Tue 14:32");
    let mut screen = Screen::new(100, 30, Depth::TrueColor);
    let bytes = frame(&mut screen, &view);
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert_eq!(text.matches('東').count(), 1);
    let mut term = Term::new(100, 30);
    term.feed(&bytes);
    assert!(term.text().contains("東京 office"), "{}", term.text());
    let linked = term
        .links
        .iter()
        .flatten()
        .filter(|link| link.as_deref() == Some("https://moqui.org/docs"))
        .count();
    assert!(linked >= 4, "{linked} cells link to the docs");
}

/// The bytes a session wrote, shared with the test.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }

    /// The screen as the terminal shows it, once `want` is on it.
    fn wait_for(&self, want: &str, width: u16, height: u16) -> String {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let bytes = self.bytes();
            let body = bytes.strip_prefix(ENTER).unwrap_or(&bytes);
            let body = body.strip_suffix(LEAVE).unwrap_or(body);
            let mut term = Term::new(width, height);
            term.feed(body);
            let text = term.text();
            if text.contains(want) {
                return text;
            }
            assert!(Instant::now() < until, "{want:?} never shown:\n{text}");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

struct Running {
    tx: mpsc::Sender<SessionEvent>,
    out: Shared,
    done: thread::JoinHandle<std::io::Result<()>>,
}

fn start(link: FakeLink, path: &str, columns: u16, rows: u16) -> Running {
    start_with(link, path, columns, rows, false)
}

fn start_with(link: FakeLink, path: &str, columns: u16, rows: u16, log: bool) -> Running {
    let (tx, rx) = mpsc::channel();
    let replies = tx.clone();
    let out = Shared::default();
    let mut writer = out.clone();
    let options = SessionOptions {
        path: path.to_string(),
        theme: Theme::default(),
        columns,
        rows,
        notes: None,
        log,
        login_name: None,
    };
    let done = thread::spawn(move || run_session(link, options, &rx, &replies, &mut writer));
    Running { tx, out, done }
}

/// The caller's log lines (an SSH user who may read the log) show in the pane Alt+L opens.
#[test]
fn the_log_pane_shows_the_lines_the_caller_sends() {
    let running = start_with(gallery_link(), GALLERY_LIST_PATH, 100, 40, true);
    running.out.wait_for("Open detail", 100, 40);
    running
        .tx
        .send(SessionEvent::Log(
            "2026-10-21 02:44:00.000 WARN  [x] a::b - A warning for the pane".to_string(),
        ))
        .unwrap();
    running.tx.send(SessionEvent::Input(b"\x1bl".to_vec())).unwrap();
    running.out.wait_for("A warning for the pane", 100, 40);
    running.tx.send(SessionEvent::Close).unwrap();
    running.done.join().unwrap().unwrap();
}

#[test]
fn a_session_runs_keys_answers_sizes_and_the_close_over_a_channel() {
    let link = gallery_link();
    let running = start(link.clone(), GALLERY_LIST_PATH, 100, 40);
    running.out.wait_for("Open detail", 100, 40);
    assert!(running.out.bytes().starts_with(ENTER));

    // Tab to the detail link, in two reads, and Enter: the detail screen comes from a worker thread.
    running.tx.send(SessionEvent::Input(b"\t\t".to_vec())).unwrap();
    running.tx.send(SessionEvent::Input(b"\t\t\r".to_vec())).unwrap();
    running.out.wait_for("Detail", 100, 40);

    // A window change redraws whole at the new size.
    running
        .tx
        .send(SessionEvent::Resize { columns: 180, rows: 50 })
        .unwrap();
    let before = running.out.bytes().len();
    let until = Instant::now() + Duration::from_secs(10);
    while running.out.bytes()[before..].windows(4).all(|w| w != b"\x1b[2J") {
        assert!(Instant::now() < until, "no redraw after the resize");
        thread::sleep(Duration::from_millis(20));
    }

    running.tx.send(SessionEvent::Close).unwrap();
    running.done.join().unwrap().unwrap();
    assert!(running.out.bytes().ends_with(LEAVE));
}

#[test]
fn a_lone_escape_acts_after_its_wait_and_quit_ends_the_session() {
    let running = start(gallery_link(), GALLERY_LIST_PATH, 100, 40);
    running.out.wait_for("Open detail", 100, 40);
    // F1 opens the help; a lone ESC, with nothing after it, closes it once its wait is over.
    running.tx.send(SessionEvent::Input(b"\x1bOP".to_vec())).unwrap();
    running.out.wait_for("Keys", 100, 40);
    let shown = running.out.bytes().len();
    running.tx.send(SessionEvent::Input(b"\x1b".to_vec())).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let bytes = running.out.bytes();
        let mut term = Term::new(100, 40);
        term.feed(bytes.strip_prefix(ENTER).unwrap());
        if bytes.len() > shown && !term.text().contains("Keys") {
            break;
        }
        assert!(Instant::now() < until, "the help stayed open:\n{}", term.text());
        thread::sleep(Duration::from_millis(20));
    }
    // Ctrl+Q quits, and the session ends by itself, its terminal put back.
    running.tx.send(SessionEvent::Input(b"\x11".to_vec())).unwrap();
    running.done.join().unwrap().unwrap();
    assert!(running.out.bytes().ends_with(LEAVE));
}

#[test]
fn downloads_are_refused_in_a_remote_session() {
    let model = r#"{ "v": 1, "screen": { "path": "/apps/app/Things", "title": "Things" }, "c": [
      { "t": "formList", "id": "Things", "name": "Things", "columns": [{ "title": "Name" }],
        "rows": [ { "id": "T1", "cells": ["Anvil"] } ],
        "exports": [ { "kind": "csv", "label": "CSV", "url": "/apps/app/Things?renderMode=csv" } ] } ] }"#;
    let link = FakeLink::from_json(&[model]).unwrap();
    link.answer_raw(
        "/apps/app/Things",
        RawBody {
            content_type: "text/csv".to_string(),
            file_name: "Things.csv".to_string(),
            bytes: b"name\nAnvil\n".to_vec(),
        },
    );
    let running = start(link, "/apps/app/Things", 100, 40);
    running.out.wait_for("Anvil", 100, 40);
    // F10 on the list, the export, and Enter: the file is not saved on the server.
    running.tx.send(SessionEvent::Input(b"\x1b[21~".to_vec())).unwrap();
    running.out.wait_for("Export CSV", 100, 40);
    for _ in 0..4 {
        running.tx.send(SessionEvent::Input(b"\x1b[B".to_vec())).unwrap();
    }
    running.tx.send(SessionEvent::Input(b"\r".to_vec())).unwrap();
    running.out.wait_for("downloads are off", 100, 40);
    running.tx.send(SessionEvent::Close).unwrap();
    running.done.join().unwrap().unwrap();
}

/// The mouse over the channel: its modes go on after the screen starts and off with the rest; a drag across list rows,
/// as SGR bytes, comes back as one OSC 52 copy; a click on a link follows it.
#[test]
fn a_drag_and_a_click_arrive_as_sgr_reports() {
    let running = start(gallery_link(), GALLERY_LIST_PATH, 100, 40);
    let text = running.out.wait_for("Open detail", 100, 40);
    let has = |bytes: &[u8], part: &[u8]| bytes.windows(part.len()).any(|window| window == part);
    assert!(
        has(&running.out.bytes(), MOUSE_ON),
        "the mouse is on until the user turns it off"
    );
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.iter().position(|line| line.contains("ENUM_00")).unwrap() + 1;
    let report = format!("\x1b[<0;4;{first}M\x1b[<32;10;{}M\x1b[<0;10;{}m", first + 1, first + 1);
    running.tx.send(SessionEvent::Input(report.into_bytes())).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !has(&running.out.bytes(), b"\x1b]52;c;") {
        assert!(Instant::now() < until, "no OSC 52 after the drag");
        thread::sleep(Duration::from_millis(20));
    }
    running.out.wait_for("Copied", 100, 40);
    let open = lines.iter().position(|line| line.contains("Open detail")).unwrap() + 1;
    // A report split across two reads.
    running
        .tx
        .send(SessionEvent::Input(format!("\x1b[<0;3;{open}").into_bytes()))
        .unwrap();
    running
        .tx
        .send(SessionEvent::Input(format!("M\x1b[<0;3;{open}m").into_bytes()))
        .unwrap();
    running.out.wait_for("Enumeration Detail", 100, 40);
    running.tx.send(SessionEvent::Close).unwrap();
    running.done.join().unwrap().unwrap();
    assert!(running.out.bytes().ends_with(LEAVE));
    assert!(LEAVE.starts_with(MOUSE_OFF));
}
