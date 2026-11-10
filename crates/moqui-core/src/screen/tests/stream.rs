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

//! A screen's response sent as it renders (docs/ui/Screen-Output.md): a page past the threshold commits with its
//! session headers, a small one is sent whole, a failure after the head ends the body abnormally; and a template
//! writes as it renders, so a screen it renders keeps its place in the template's text.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{request, World};
use crate::screen::{serve_at, serve_at_to};
use crate::web::stream::OutputSink;
use crate::web::WebResponse;

/// A temporary screen tree.
struct Site {
    dir: PathBuf,
    root: String,
}

impl Site {
    /// `Site.xml` as `root`, with `files` (a path under the site and its text) beside it.
    fn new(name: &str, root: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-stream-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Site")).unwrap();
        std::fs::write(dir.join("Site.xml"), root.replace("SITE", &dir.display().to_string())).unwrap();
        for (path, text) in files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text.replace("SITE", &dir.display().to_string())).unwrap();
        }
        let root = dir.join("Site.xml").to_string_lossy().into_owned();
        Self { dir, root }
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What the test sink was given; shared with the test, as the host's sink is with its connection.
#[derive(Default)]
struct Seen {
    head: Option<WebResponse>,
    body: Vec<u8>,
    finished: bool,
    aborted: Option<String>,
}

struct Sink(Arc<Mutex<Seen>>);

impl OutputSink for Sink {
    fn head(&mut self, head: WebResponse) -> std::io::Result<()> {
        self.0.lock().unwrap().head = Some(head);
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap().body.extend_from_slice(bytes);
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn finish(&mut self) {
        self.0.lock().unwrap().finished = true;
    }

    fn abort(&mut self, reason: &str) {
        self.0.lock().unwrap().aborted = Some(reason.to_string());
    }
}

/// The conf with a small threshold, so a test page passes it.
const SMALL_THRESHOLD: &str = r#"<moqui-conf><screen-facade stream-threshold="2048"/></moqui-conf>"#;

/// A page of `rows` labels, then, with `refused`, a section whose actions are refused, a security error that fails the
/// render (a section's other errors are written where the section was).
fn rows_screen(rows: usize, refused: bool) -> String {
    let tail = if refused {
        r#"<section name="Late"><actions><script>const refusal = new Error("refused late"); refusal.moquiErrorKind = "Unauthorized"; throw refusal</script></actions>
            <widgets><label text="secret"/></widgets></section>"#
    } else {
        ""
    };
    format!(
        r#"<screen require-authentication="false">
    <actions><script>rows = Array.from({{length: {rows}}}, (_, i) => i)</script></actions>
    <widgets>
        <section-iterate name="Rows" list="rows" entry="row"><widgets><label text="row ${{row}}"/></widgets></section-iterate>
        {tail}
    </widgets>
</screen>"#
    )
}

fn html_request(path: &str) -> crate::web::WebRequest {
    let mut page = request("GET", path);
    page.set_header("accept", "text/html");
    page
}

#[test]
fn a_page_past_the_threshold_sends_its_head_with_the_session_and_then_its_body() {
    let world = World::open(SMALL_THRESHOLD);
    let screen = rows_screen(400, false);
    let site = Site::new("big", &screen, &[]);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let sink = Box::new(Sink(Arc::clone(&seen)));
    let _ignored = serve_at_to(&world.factory, html_request("/"), &site.root, Some(sink));
    let seen = seen.lock().unwrap();
    let head = seen
        .head
        .as_ref()
        .expect("the head went once the page passed the threshold");
    assert_eq!(head.status, 200);
    assert!(head
        .header("content-type")
        .is_some_and(|kind| kind.starts_with("text/html")));
    // the session made for the request, and its token, are in the head, as in a whole response
    assert!(
        head.headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("set-cookie") && value.starts_with("moqui.session=")),
        "{:?}",
        head.headers
    );
    assert!(head.header("moquiSessionToken").is_some_and(|token| !token.is_empty()));
    // the conf's screen-render headers too
    assert!(head.header("x-content-type-options").is_some(), "{:?}", head.headers);
    assert!(head.http_log.session.is_some());
    let body = String::from_utf8_lossy(&seen.body);
    assert!(body.contains("row 0") && body.contains("row 399"), "{body}");
    assert!(seen.finished);
    assert!(seen.aborted.is_none());
}

#[test]
fn a_page_under_the_threshold_is_sent_whole_and_the_sink_gets_nothing() {
    let world = World::open(SMALL_THRESHOLD);
    let screen = rows_screen(3, false);
    let site = Site::new("small", &screen, &[]);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let sink = Box::new(Sink(Arc::clone(&seen)));
    let page = serve_at_to(&world.factory, html_request("/"), &site.root, Some(sink));
    let html = page.body_text();
    assert_eq!(page.status, 200, "{html}");
    assert!(html.contains("row 2"), "{html}");
    assert!(page.header("Set-Cookie").is_some());
    let seen = seen.lock().unwrap();
    assert!(seen.head.is_none() && seen.body.is_empty() && !seen.finished);
}

#[test]
fn a_failure_after_the_head_writes_where_it_stopped_and_ends_the_body_abnormally() {
    let world = World::open(SMALL_THRESHOLD);
    let screen = rows_screen(400, true);
    let site = Site::new("fails", &screen, &[]);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let sink = Box::new(Sink(Arc::clone(&seen)));
    let _ignored = serve_at_to(&world.factory, html_request("/"), &site.root, Some(sink));
    let seen = seen.lock().unwrap();
    assert!(seen.head.as_ref().is_some_and(|head| head.status == 200));
    let body = String::from_utf8_lossy(&seen.body);
    assert!(body.contains("row 399"), "{body}");
    assert!(body.contains("Error rendering page"), "{body}");
    assert!(!body.contains("secret"), "{body}");
    assert!(seen.aborted.is_some());
    assert!(!seen.finished);

    // without a sink the same failure is the error page, with its status
    let page = serve_at(&world.factory, html_request("/"), &site.root);
    assert_ne!(page.status, 200, "{}", page.body_text());
}

#[test]
fn a_template_writes_as_it_renders_so_the_screens_it_renders_keep_their_place() {
    let world = World::open("");
    let root = r#"<screen require-authentication="false">
    <subscreens default-item="Home"/>
    <widgets>
        <render-mode><text type="html" template="true"><![CDATA[<div id="inline">${sri.renderSubscreen()}</div>]]></text></render-mode>
    </widgets>
</screen>"#;
    let home = r#"<screen require-authentication="false">
    <widgets>
        <render-mode><text type="html" location="SITE/Site/Wrap.html.fancy"/></render-mode>
    </widgets>
</screen>"#;
    let wrap = r#"<section id="file">${sri.renderText("SITE/Site/Inner.html.fancy", "true")}</section><p>after</p>"#;
    let inner = r#"<span id="inner">${"in" + "ner"}</span>"#;
    let site = Site::new(
        "templates",
        root,
        &[
            ("Site/Home.xml", home),
            ("Site/Wrap.html.fancy", wrap),
            ("Site/Inner.html.fancy", inner),
        ],
    );
    let page = serve_at(&world.factory, html_request("/"), &site.root);
    let html = page.body_text();
    assert_eq!(page.status, 200, "{html}");
    let at = |needle: &str| html.find(needle).unwrap_or_else(|| panic!("{needle} in {html}"));
    assert!(at("<div id=\"inline\">") < at("<section id=\"file\">"), "{html}");
    assert!(
        at("<section id=\"file\">") < at("<span id=\"inner\">inner</span>"),
        "{html}"
    );
    assert!(at("<span id=\"inner\">") < at("</section><p>after</p>"), "{html}");
    assert!(at("<p>after</p>") < at("</div>"), "{html}");
}

/// Three enumerations, `S1` to `S3`, for the lists below.
fn enumerations(world: &World) {
    for id in ["S1", "S2", "S3"] {
        let mut row = world.facade.make_value("moqui.basic.Enumeration").unwrap();
        row.set("enumId", moqui_lib::object::Value::string(id));
        row.set("description", moqui_lib::object::Value::string(format!("enum {id}")));
        row.create(&world.facade).unwrap();
    }
}

/// A form-list over a find, with row actions that read whether a row follows.
const FORM_LIST: &str = r#"<screen require-authentication="false">
    <widgets>
        <form-list name="Enums" list="enums" skip-form="true">
            <entity-find entity-name="moqui.basic.Enumeration" list="enums">
                <econdition field-name="enumId" operator="like" value="S%"/><order-by field-name="enumId"/>
            </entity-find>
            <row-actions><set field="mark" from="enumId + (enums_has_next ? '+' : '.')"/></row-actions>
            <field name="enumId"><default-field><display/></default-field></field>
            <field name="mark"><default-field><display/></default-field></field>
        </form-list>
    </widgets>
</screen>"#;

fn render(world: &World, root: &str, mode: &str) -> String {
    let output = crate::screen::make_render(&world.factory)
        .root_screen(root.to_string())
        .screen_path("/")
        .render_mode(mode)
        .render()
        .unwrap();
    assert_eq!(output.status, 200, "{}", output.body_text());
    output.body_text()
}

#[test]
fn a_csv_or_text_form_list_reads_its_rows_one_at_a_time_as_html_reads_them_together() {
    let world = World::open("");
    enumerations(&world);
    let site = Site::new("form-list", FORM_LIST, &[]);
    // csv, text and xml read the rows from a cursor, each made as it is written; html makes them all first
    let csv = render(&world, &site.root, "csv");
    let rows: Vec<&str> = csv.lines().skip(1).filter(|line| !line.is_empty()).collect();
    assert_eq!(rows, ["S1,S1+", "S2,S2+", "S3,S3."], "{csv}");
    let text = render(&world, &site.root, "text");
    assert!(text.contains("S1+") && text.contains("S3."), "{text}");
    let xml = render(&world, &site.root, "xml");
    assert_eq!(xml.matches("<EnumsEntry").count(), 3, "{xml}");
    let html = render(&world, &site.root, "html");
    assert!(
        html.contains("S1+") && html.contains("S2+") && html.contains("S3."),
        "{html}"
    );
}

#[test]
fn a_section_iterates_a_cursor_a_row_at_a_time() {
    let world = World::open("");
    enumerations(&world);
    let screen = r#"<screen require-authentication="false">
    <actions>
        <entity-find entity-name="moqui.basic.Enumeration" list="enums">
            <econdition field-name="enumId" operator="like" value="S%"/><order-by field-name="enumId"/><use-iterator/>
        </entity-find>
    </actions>
    <widgets>
        <section-iterate name="Each" list="enums" entry="item"><widgets>
            <label text="${item.enumId}${item_has_next ? ',' : '.'}"/>
        </widgets></section-iterate>
    </widgets>
</screen>"#;
    let site = Site::new("section", screen, &[]);
    let text = render(&world, &site.root, "text");
    let compact: String = text.split_whitespace().collect();
    assert!(compact.contains("S1,S2,S3."), "{text}");
}
