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

//! Scripted Gallery, Party and Tools screens through the in-process client: the Phase 2 and Phase 3 Exits of
//! plans/archive/Screen-TUI-Impl.md.
//!
//! One server: every Gallery screen renders a model with no error node in it, a login through the dialog, then the
//! keyboard flows on Gallery List (find, page, sort, open, a dialog submit) and on SimpleScreens Party, a Mantle app
//! (dependent drop-downs, create, edit and save a person, read back). Mantle is linked into this runtime only. Phase 3:
//! the Board and Calendar through their components, a dynamic container, the palette's search, an options transition
//! that fails, and a Tools task (find a user, add a group, change its membership, run a service job, read its run).

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

mod common;

use example as _;

use std::path::PathBuf;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_host::tui::{HostLink, Transport};
use moqui_lib::object::Value;
use moqui_tui::{
    frame, outline, AppView, FormList, Key, KeyPress, LinkError, Node, Overlay, ScreenLink, ScreenModel, Session,
};

const GALLERY: &[&str] = &[
    "Landing",
    "List",
    "Detail",
    "Record360",
    "Task",
    "Board",
    "Calendar",
    "Split",
    "Settings",
    "Wizard",
    "Dialog",
    "Report",
    "Panel",
    "Widgets",
    "Editor",
    "Message",
    "Views",
    "QuickNote",
    "QuickTask",
    "Chatter",
    "RecordFiles",
];

const SIMPLE_SCREENS: &str = r#"<screen-facade>
    <screen location="component://webroot/screen/webroot/apps.xml">
        <subscreens-item name="SimpleScreens" menu-title="Simple Screens"
            location="component://SimpleScreens/screen/SimpleScreens.xml"/>
    </screen>
</screen-facade>"#;

#[test]
fn gallery_and_party_screens() {
    let gate = start_gate();
    let factory = Arc::clone(gate.server.factory());
    grant_gate(&factory);
    log_in_through_the_dialog(&factory);
    log_in_with_a_code_and_from_an_address(&factory);
    let link = HostLink::as_user(&factory, "john.doe");
    render_gallery(&link);
    snapshot_gallery(&link);
    drive_gallery(&link);
    drive_row_dialog(&link);
    drive_party(&link);
    drive_components(&link);
    drive_shell(&link);
    drive_tools(&link);
    drive_push(&factory, &link);
    drive_views(&factory, &link);
}

/// The Gallery's Views list: its board first (`views="board,list,calendar"`), then `v` to the list and the calendar,
/// and back to the board, where a card moves to the next status through the board's transition.
fn drive_views(factory: &Arc<ExecutionContextFactory>, link: &HostLink) {
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().expect("entity facade");
        for (id, name, status, date) in [
            ("TUI_EX1", "Board card one", "EXST_IN_DESIGN", "2026-10-19 09:00:00"),
            ("TUI_EX2", "Board card two", "EXST_DEFINED", "2026-10-20 14:00:00"),
        ] {
            let mut example = facade.make_value("moqui.example.Example").expect("Example");
            example.set("exampleId", Value::string(id));
            example.set("exampleName", Value::string(name));
            example.set("statusId", Value::string(status));
            example.set("exampleTypeEnumId", Value::string("EXT_MADE_UP"));
            example.set("exampleDate", Value::string(date));
            example.create(&facade).expect(id);
        }
    }
    let mut view = open_screen(link, "/apps/gallery/Views");
    let board = "ExampleList~board";
    assert!(
        view.session().index().components.contains_key(board),
        "{:?}",
        view.session().index().components.keys().collect::<Vec<_>>()
    );
    let text = view.screen_text(180, 50);
    assert!(text.contains("Board card one") && text.contains("Defined"), "{text}");
    focus(&mut view, board);
    view.on_key(&press(Key::Char('v')));
    assert!(view.session().list("ExampleList").is_some(), "the list again");
    assert_eq!(view.session().focus_id(), "ExampleList");
    view.on_key(&press(Key::Char('v')));
    assert!(view.session().index().components.contains_key("ExampleList~calendar"));
    let text = view.screen_text(180, 50);
    assert!(
        text.contains("October 2026") && text.contains("Board card two"),
        "{text}"
    );
    view.on_key(&press(Key::Char('v')));
    view.run_requests();
    assert_eq!(view.session().focus_id(), board);
    // m, then Right to the next column, then Enter: the card's status moves as the browser's drag does.
    view.on_key(&press(Key::Char('m')));
    view.on_key(&press(Key::Right));
    view.on_key(&press(Key::Enter));
    view.run_requests();
    assert!(lone_error(view.session().model()).is_none());
    let moved = view.screen_text(180, 50);
    assert!(
        moved.contains("In Design · 0") && moved.contains("Defined · 2"),
        "{moved}"
    );
    assert!(
        view.session().index().components.contains_key(board),
        "the board stays after the move: {}",
        view.session().message()
    );
}

/// A notification for the user reaches an open view by the topic in this process: its title on the message line, and
/// the counters read again at once rather than in a minute.
fn drive_push(factory: &Arc<moqui_core::context::ExecutionContextFactory>, link: &HostLink) {
    let (tx, rx) = std::sync::mpsc::channel();
    let subscription = link
        .subscribe(Arc::new(move |push| {
            let _ = tx.send(push);
        }))
        .expect("the in-process link pushes");
    let mut view = open_screen(link, "/apps/system/ThreadList");
    view.session_mut().set_pushed(true);
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let authz = ctx.disable_authz();
    let mut note = moqui_core::notify::NotificationMessage::new();
    note.topic("TuiPush")
        .user_id("EX_JOHN_DOE")
        .title("Pushed to the terminal")
        .persist(false)
        .message_json("{}")
        .send()
        .unwrap();
    drop(authz);
    drop(entered);
    let push = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the push for john.doe");
    assert_eq!(push.title, "Pushed to the terminal");
    view.session_mut().on_push(&push);
    assert_eq!(view.session().message(), "Pushed to the terminal");
    view.run_requests();
    drop(subscription);
}

/// The Gallery's Board and Calendar are `.qvue` screens with `tui` twins: their components get their data from the
/// screens' own transitions. Widgets has a dynamic container, which loads with the screen.
fn drive_components(link: &HostLink) {
    for (screen, name, shows) in [("Board", "board", "Qualify"), ("Calendar", "calendar", "Mo")] {
        let mut view = open_screen(link, &format!("/apps/gallery/{screen}"));
        let id = view
            .session()
            .index()
            .components
            .iter()
            .find(|(_, info)| info.name == name)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| panic!("no {name} component on {screen}: {:?}", view.session().model().c));
        let state = view.session().component_state(&id).cloned().unwrap_or_default();
        assert!(state.error.is_empty(), "{screen}: {}", state.error);
        assert!(state.data.is_some(), "{screen}: no data");
        let text = view.screen_text(180, 50);
        assert!(text.contains(shows), "{text}");
    }
    let mut view = open_screen(link, "/apps/gallery/Board");
    let id = view
        .session()
        .index()
        .components
        .keys()
        .next()
        .cloned()
        .unwrap_or_default();
    focus(&mut view, &id);
    view.on_key(&press(Key::Char('m')));
    view.on_key(&press(Key::Right));
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().message().contains("moved"),
        "{}",
        view.session().message()
    );
    let view = open_screen(link, "/apps/gallery/Widgets");
    let mut loaded = false;
    for model in view.session().nav().models() {
        moqui_tui::walk(&model.c, &mut |node| {
            if let Node::DynamicContainer { c, .. } = node {
                let mut text = String::new();
                moqui_tui::walk(c, &mut |inner| {
                    if let Node::Text { text: shown, .. } = inner {
                        text.push_str(shown);
                    }
                });
                loaded |= text.contains("Loaded by dynamic-container");
            }
        });
    }
    let mut containers = Vec::new();
    for model in view.session().nav().models() {
        moqui_tui::walk(&model.c, &mut |node| {
            if let Node::DynamicContainer { .. } = node {
                containers.push(format!("{node:?}"));
            }
        });
    }
    assert!(
        loaded,
        "the dynamic container's body: {containers:?} {}",
        view.session().message()
    );
}

/// The palette asks the server's search; an options transition that fails leaves its message, and the drop-down.
fn drive_shell(link: &HostLink) {
    let mut view = open_screen(link, "/apps/gallery/List");
    view.on_key(&KeyPress::with_ctrl(Key::Char('k')));
    type_text(&mut view, "gall");
    view.session_mut()
        .on_time(std::time::Instant::now() + std::time::Duration::from_secs(1));
    view.run_requests();
    assert!(!view.session().message_is_error(), "{}", view.session().message());
    assert!(!view.session().palette_rows().is_empty(), "the palette found nothing");
    let failed = link.options("/apps/gallery/List/noSuchOptions", &std::collections::BTreeMap::new());
    assert!(failed.is_err(), "{failed:?}");
}

/// A Tools task by keyboard: find a user, add a group in its dialog, change that membership in the group list's row
/// editor, run a service job, and read its run.
fn drive_tools(link: &HostLink) {
    let mut view = open_screen(link, "/apps/system/Security/UserAccount/UserAccountList");
    view.on_key(&press(Key::Char('f')));
    assert!(view.session().find_open().is_some(), "{:?}", view.session().overlays());
    let username = view
        .session()
        .focus_order()
        .iter()
        .find(|item| item.key.ends_with(".username"))
        .map(|item| item.key.clone())
        .expect("a username find field");
    focus(&mut view, &username);
    type_text(&mut view, "john");
    view.on_key(&press(Key::Enter));
    assert_eq!(
        view.session().parameters().get("username").map(String::as_str),
        Some("john")
    );
    let list = "UserAccountList".to_string();
    focus(&mut view, &list);
    view.on_key(&press(Key::Right));
    assert!(
        view.session().path().ends_with("UserAccountDetail"),
        "{} {}",
        view.session().path(),
        view.session().message()
    );
    let groups = |view: &AppView<HostLink>| view.session().list("UserGroupList").map_or(0, |list| list.rows.len());
    let before = groups(&view);
    focus(&mut view, "AddGroupMemberDialog");
    view.on_key(&press(Key::Enter));
    let group = view
        .session()
        .focus_order()
        .iter()
        .find(|item| item.key.ends_with(".userGroupId"))
        .map(|item| item.key.clone())
        .expect("the group drop-down");
    focus(&mut view, &group);
    view.on_key(&press(Key::F(4)));
    let choices = view.session().dropdown_choices();
    let taken: Vec<String> = view
        .session()
        .list_page("UserGroupList")
        .iter()
        .map(|row| {
            row.cells
                .first()
                .map(|cell| cell.text().to_string())
                .unwrap_or_default()
        })
        .collect();
    let at = choices
        .iter()
        .position(|choice| !choice.value.is_empty() && !taken.iter().any(|text| text.contains(&choice.value)))
        .expect("a group the user is not in");
    for _ in 0..at {
        view.on_key(&press(Key::Down));
    }
    view.on_key(&press(Key::Enter));
    view.on_key(&KeyPress::with_ctrl(Key::Char('s')));
    assert!(!view.session().message_is_error(), "{}", view.session().message());
    assert_eq!(groups(&view), before + 1, "the group is added");
    // The membership's thru date, in its row's editor.
    focus(&mut view, "UserGroupList");
    view.on_key(&press(Key::Char('e')));
    assert!(
        view.session().row_editing().is_some(),
        "{:?}",
        view.session().overlays()
    );
    let thru = view
        .session()
        .focus_order()
        .iter()
        .find(|item| item.node.contains("thruDate"))
        .map(|item| item.key.clone())
        .expect("the thru date of the row");
    focus(&mut view, &thru);
    type_text(&mut view, "2099-12-31 00:00");
    view.on_key(&press(Key::Enter));
    assert!(!view.session().message_is_error(), "{}", view.session().message());
    assert!(view.session().row_editing().is_none());
    // Run a service job and read its run.
    let mut view = AppView::with_session(
        Session::start("/apps/system/ServiceJob/Jobs/ServiceJobDetail?jobName=clean_ServiceJobRun_daily"),
        link.clone(),
    );
    view.run_requests();
    assert!(
        view.session().path().ends_with("ServiceJobDetail"),
        "{} {}",
        view.session().path(),
        view.session().message()
    );
    let run = link_with_text(&view, "Run Job");
    focus(&mut view, &run);
    view.on_key(&press(Key::Enter));
    view.on_key(&press(Key::Char('y')));
    assert!(!view.session().message_is_error(), "{}", view.session().message());
    let runs = link_with_text(&view, "Job Runs");
    focus(&mut view, &runs);
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().path().ends_with("JobRunList"),
        "{}",
        view.session().path()
    );
    let list = view
        .session()
        .index()
        .lists
        .keys()
        .next()
        .cloned()
        .expect("the job runs");
    assert!(!view.session().list_page(&list).is_empty(), "a run of the job");
    focus(&mut view, &list);
    view.on_key(&press(Key::Right));
    assert!(
        view.session().path().ends_with("JobRunDetail"),
        "{}",
        view.session().path()
    );
    assert!(lone_error(view.session().model()).is_none());
    // A link to a transition that leaves a message: the message comes with the screen the transition goes back to.
    let mut view = AppView::with_session(
        Session::start("/apps/system/ThreadList/interruptThread?threadName=nobody"),
        link.clone(),
    );
    view.run_requests();
    assert!(
        view.session().path().ends_with("ThreadList"),
        "{}",
        view.session().path()
    );
    assert!(
        view.session().message().contains("not interrupted"),
        "{}",
        view.session().message()
    );
}

fn link_with_text(view: &AppView<HostLink>, text: &str) -> String {
    view.session()
        .index()
        .links
        .values()
        .find(|link| link.text == text)
        .map(|link| link.id.clone())
        .unwrap_or_else(|| panic!("no link {text}"))
}

/// A link with no user answers 401 to every call, so the client opens its login dialog; a wrong password stays there,
/// and the right one asks for the screen again.
fn log_in_through_the_dialog(factory: &Arc<ExecutionContextFactory>) {
    let link = HostLink::new(factory);
    let mut view = AppView::with_session(Session::start("/apps/gallery/List"), link);
    view.run_requests();
    assert!(
        matches!(view.session().top_overlay(), Some(Overlay::Login(_))),
        "{:?} {}",
        view.session().overlays(),
        view.session().message()
    );
    type_text(&mut view, "john.doe");
    view.on_key(&press(Key::Tab));
    type_text(&mut view, "wrong");
    view.on_key(&press(Key::Enter));
    let Some(Overlay::Login(login)) = view.session().top_overlay() else {
        panic!("a wrong password keeps the dialog: {:?}", view.session().overlays());
    };
    assert!(!login.error.is_empty());
    type_text(&mut view, "moqui");
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().top_overlay().is_none(),
        "{:?}",
        view.session().overlays()
    );
    assert!(
        view.session().path().ends_with("/gallery/List"),
        "{}",
        view.session().path()
    );
    assert_eq!(view.session().shell().user, "john.doe");
    assert!(!view.session().menu().is_empty(), "the drawer of the gallery");
}

/// The dialog's logins on the in-process link: an account with a second factor answers the code it wants for its
/// right password, a wrong code, and logs in with the right one (a single-use code here); an account with `ipAllowed`
/// is checked against the address the link was given, as the SSH peer is.
fn log_in_with_a_code_and_from_an_address(factory: &Arc<ExecutionContextFactory>) {
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().expect("entity facade");
        let john = facade
            .find("moqui.security.UserAccount")
            .expect("UserAccount")
            .eq("username", Value::string("john.doe"))
            .one()
            .expect("find john.doe")
            .expect("john.doe");
        for (id, ip) in [("CODE_USER", ""), ("IP_USER", "10.9.9.9")] {
            let mut account = facade.make_value("moqui.security.UserAccount").expect("UserAccount");
            for field in ["currentPassword", "passwordHashType", "passwordSalt", "passwordBase64"] {
                if let Some(value) = john.get(field) {
                    account.set(field, value.clone());
                }
            }
            account.set("userId", Value::string(id));
            account.set("username", Value::string(id.to_lowercase().replace('_', ".")));
            if !ip.is_empty() {
                account.set("ipAllowed", Value::string(ip));
            }
            account.create(&facade).unwrap_or_else(|err| panic!("{id}: {err}"));
        }
        let mut factor = facade
            .make_value("moqui.security.UserAuthcFactor")
            .expect("UserAuthcFactor");
        factor.set("factorId", Value::string("CODE_USER_1"));
        factor.set("userId", Value::string("CODE_USER"));
        factor.set("factorTypeEnumId", Value::string("UafSingleUse"));
        factor.set(
            "factorOption",
            Value::string(moqui_core::user::single_use_hash("246810").expect("hash")),
        );
        factor.set("fromDate", Value::I64(0));
        factor.create(&facade).expect("factor");
    }
    let link = HostLink::new(factory);
    match link.login("code.user", "moqui", "") {
        Err(LinkError::Code(ask)) => assert!(ask.text.starts_with("Enter the authentication code"), "{ask:?}"),
        other => panic!("a code is wanted: {other:?}"),
    }
    assert!(link.transport().user().is_none(), "no one logged in yet");
    match link.login("code.user", "moqui", "111111") {
        Err(LinkError::Code(ask)) => assert_eq!(ask.text, "Authentication code is not valid"),
        other => panic!("a wrong code: {other:?}"),
    }
    link.login("code.user", "moqui", "246810").expect("the code logs in");
    assert_eq!(link.transport().user().as_deref(), Some("code.user"));

    let refused = HostLink::new(factory)
        .from_address("127.0.0.1")
        .login("ip.user", "moqui", "");
    match refused {
        Err(LinkError::Remote(text)) => assert!(text.contains("not in allowed list"), "{text}"),
        other => panic!("refused from another address: {other:?}"),
    }
    HostLink::new(factory)
        .from_address("10.9.9.9")
        .login("ip.user", "moqui", "")
        .expect("allowed from its address");
}

fn start_gate() -> common::Gate {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let udm = std::fs::canonicalize(repo.join("runtime/component/mr-mantle-udm")).expect("mantle-udm");
    let usl = std::fs::canonicalize(repo.join("runtime/component/mr-mantle-usl")).expect("mantle-usl");
    let screens = std::fs::canonicalize(repo.join("runtime/component/MrSimpleScreens")).expect("SimpleScreens");
    let links = [
        (udm.to_str().expect("udm path"), "component/mantle-udm"),
        (usl.to_str().expect("usl path"), "component/mantle-usl"),
        (screens.to_str().expect("screens path"), "component/SimpleScreens"),
    ];
    // The Editor screen reads the EXAMPLE wiki space. That file is demo data, so the seed load leaves it out.
    let wiki = std::fs::read_to_string(repo.join("runtime/base-component/example/data/ExampleWikiData.xml"))
        .expect("example wiki data");
    common::start_with_links("tui-screens", SIMPLE_SCREENS, Some(&wiki), Some(&["demo"]), &links)
}

fn render_gallery(link: &HostLink) {
    let mut failures = Vec::new();
    for name in GALLERY {
        let path = format!("/apps/gallery/{name}");
        match link.level(&path, &std::collections::BTreeMap::new()) {
            Ok(model) => {
                if let Some(text) = lone_error(&model) {
                    failures.push(format!("{path} rendered an error: {text}"));
                }
            }
            Err(err) => failures.push(format!("{path}: {err}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every Gallery screen at 180x50 as text, compared with its golden in `tests/tui_snapshots/` (`UPDATE_GOLDEN=1`
/// rewrites them), so a change of look shows in the diff as the screenshots do for qvt. List, Detail and Widgets also in
/// ASCII; and every screen in monochrome writes no color. The clock is fixed, and the date of today, which some
/// screens show (in each form they write it, the Panel's `toLocaleDateString` as M/D/YYYY), is written as `<today>`;
/// the screens whose sample data is dated back from today write those days as `<today-N>` (the Panel's overdue items,
/// the Report's hit bins of the last week, as dates and as the chart's MM-DD labels), so the goldens hold on any day.
/// The Calendar, whose events are days of this month, has no golden.
fn snapshot_gallery(link: &HostLink) {
    let today = jiff::Zoned::now().date();
    let stamps: Vec<(String, String)> = vec![
        (today.strftime("%Y-%m-%d").to_string(), "<today>".to_string()),
        (today.strftime("%a %b %-d").to_string(), "<today>".to_string()),
        (today.strftime("%B %Y").to_string(), "<today>".to_string()),
        (today.strftime("%-m/%-d/%Y").to_string(), "<today>".to_string()),
    ];
    // the days back from today a screen's sample data is dated by, in the forms it writes them
    let days_back = |days: i64, short: bool| -> Vec<(String, String)> {
        let mut out = Vec::new();
        for back in 1..=days {
            let day = today.checked_sub(jiff::Span::new().days(back)).expect("a day back");
            out.push((day.strftime("%Y-%m-%d").to_string(), format!("<today-{back}>")));
        }
        if short {
            for back in 0..=days {
                let day = today.checked_sub(jiff::Span::new().days(back)).expect("a day back");
                let mask = if back == 0 { "<today>".to_string() } else { format!("<today-{back}>") };
                out.push((format!(" {} ", day.strftime("%m-%d")), format!(" {mask} ")));
            }
        }
        out
    };
    let mut failures = Vec::new();
    for name in GALLERY {
        let stamps: Vec<(String, String)> = match *name {
            "Panel" => stamps.iter().cloned().chain(days_back(1, false)).collect(),
            "Report" => stamps.iter().cloned().chain(days_back(7, true)).collect(),
            _ => stamps.clone(),
        };
        let path = format!("/apps/gallery/{name}");
        let mut view = AppView::with_session(Session::start(&path), link.clone());
        view.run_requests();
        view.set_clock("Tue 14:32");
        // The Calendar's events are days of this month, so it has no golden; drive_components reads it.
        let text = masked(&view.screen_text(180, 50), &stamps);
        if *name != "Calendar" {
            if let Err(err) = golden(&format!("gallery-{name}-180x50"), &text) {
                failures.push(err);
            }
        }
        let mut bytes = Vec::new();
        moqui_tui::Screen::new(180, 50, moqui_tui::Depth::Mono)
            .draw(&view, &mut bytes)
            .unwrap();
        // The parameters of each SGR (`ESC [ … m`); a cursor move (`ESC [ 38 ; 1 H`) is not one.
        let written = String::from_utf8_lossy(&bytes);
        let colored = written.split("\x1b[").skip(1).any(|part| {
            let end = part
                .find(|ch: char| ('\x40'..='\x7e').contains(&ch))
                .unwrap_or(part.len());
            part[end..].starts_with('m')
                && part[..end]
                    .split(';')
                    .any(|p| matches!(p, "38" | "48") || p.starts_with('3') && p.len() == 2 && p != "39")
        });
        if colored {
            failures.push(format!("{path} wrote a color in monochrome"));
        }
        if ["List", "Detail", "Widgets"].contains(name) {
            let mut theme = view.theme().clone();
            theme.ascii = true;
            view.set_theme(theme);
            let text = masked(&view.screen_text(180, 50), &stamps);
            if let Err(err) = golden(&format!("gallery-{name}-180x50-ascii"), &text) {
                failures.push(err);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Today's date as `<today>` (and the other `stamps` as theirs), and a time after it (`<today> 02:46`, a record stamped
/// while the data loaded) as `hh:mm`.
fn masked(text: &str, stamps: &[(String, String)]) -> String {
    let mut text = text.to_string();
    for (stamp, mask) in stamps {
        text = text.replace(stamp.as_str(), mask);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("<today> ") {
        let (head, tail) = rest.split_at(at + "<today> ".len());
        out.push_str(head);
        let time = tail.as_bytes();
        if time.len() >= 5 && time[2] == b':' && [0, 1, 3, 4].iter().all(|i| time[*i].is_ascii_digit()) {
            out.push_str("hh:mm");
            rest = &tail[5..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The text against its golden file, or why not.
fn golden(name: &str, actual: &str) -> Result<(), String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/tui_snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshots dir")).unwrap();
        std::fs::write(&path, actual).unwrap();
        return Ok(());
    }
    let expected =
        std::fs::read_to_string(&path).map_err(|err| format!("read {}: {err}. Set UPDATE_GOLDEN=1", path.display()))?;
    if expected == actual {
        return Ok(());
    }
    let first = expected
        .lines()
        .zip(actual.lines())
        .position(|(a, b)| a != b)
        .unwrap_or(0);
    Err(format!(
        "{} differs from line {}:\n- {}\n+ {}",
        path.display(),
        first + 1,
        expected.lines().nth(first).unwrap_or(""),
        actual.lines().nth(first).unwrap_or("")
    ))
}

/// The first error node anywhere in the document.
fn lone_error(model: &ScreenModel) -> Option<String> {
    let mut found = None;
    moqui_tui::walk(&model.c, &mut |node| {
        if let Node::Error { text, .. } = node {
            found.get_or_insert_with(|| text.clone());
        }
    });
    found
}

fn drive_gallery(link: &HostLink) {
    let mut view = open_screen(link, "/apps/gallery/List");
    paint_frame(&mut view);
    page_enumerations(&mut view);
    sort_enumerations(&mut view);
    toggle_find(&mut view);
    open_detail(&mut view);
    view.on_key(&press(Key::F(3)));
    assert!(
        view.session().path().contains("gallery/List"),
        "{} {}",
        view.session().path(),
        view.session().message()
    );
    submit_new_enum(&mut view);
    find_enumerations(&mut view);
}

fn find_enumerations(view: &mut AppView<HostLink>) {
    focus(view, "EnumFind.description");
    type_text(view, "Screen");
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().path().contains("gallery/List"),
        "{}",
        view.session().message()
    );
    assert_eq!(
        view.session().parameters().get("description").map(String::as_str),
        Some("Screen"),
        "{:?}",
        view.session().parameters()
    );
    let rows = view.session().list_page("EnumList");
    assert!(!rows.is_empty(), "the find matched nothing");
    assert!(
        rows.iter().all(|row| row
            .cells
            .iter()
            .any(|cell| cell.text().to_lowercase().contains("screen"))),
        "a row without the find text"
    );
}

fn page_enumerations(view: &mut AppView<HostLink>) {
    let count = find_list(&view.session().model().c, "EnumList")
        .map(list_count)
        .unwrap_or(0);
    assert!(
        count > 20,
        "EnumList count {count}; {}",
        list_summary(view.session().model())
    );
    let before = first_cell(view);
    focus(view, "EnumList");
    view.on_key(&press(Key::F(8)));
    assert_eq!(view.session().page_index("EnumList"), 1, "{}", view.session().message());
    assert_ne!(before, first_cell(view), "page did not change the first row");
    // Tall enough that the footer is on screen with the header above it.
    let wide = view.screen_text(180, 120);
    assert!(wide.contains("Page 2/"), "{wide}");
    let narrow = view.screen_text(100, 120);
    assert!(narrow.contains("Page 2/"), "{narrow}");
}

fn sort_enumerations(view: &mut AppView<HostLink>) {
    focus(view, "EnumList");
    view.on_key(&press(Key::Char('s')));
    assert_eq!(view.session().order_by(), "enumId", "{}", view.session().message());
    assert_eq!(
        view.session().page_index("EnumList"),
        0,
        "a sort starts at the first page"
    );
}

fn toggle_find(view: &mut AppView<HostLink>) {
    focus(view, "EnumList");
    view.on_key(&press(Key::Char('f')));
    assert_eq!(view.session().find_open(), Some("EnumList"));
    let text = view.screen_text(100, 40);
    assert!(text.contains(" Find "), "{text}");
    view.on_key(&press(Key::Escape));
    assert_eq!(view.session().find_open(), None);
}

fn open_detail(view: &mut AppView<HostLink>) {
    focus(view, "EnumList");
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().path().contains("/Detail"),
        "{} {}",
        view.session().path(),
        view.session().message()
    );
    assert_side_by_side(view.session().model(), frame(180).canvas);
    assert_stacked(view.session().model(), frame(100).canvas);
}

fn submit_new_enum(view: &mut AppView<HostLink>) {
    focus(view, "NewEnumDialog");
    view.on_key(&press(Key::Enter));
    focus(view, "NewEnumForm.description");
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().message().contains("nothing is written"),
        "{}",
        view.session().message()
    );
    assert!(
        view.session().dialogs_open().is_empty(),
        "{:?}",
        view.session().dialogs_open()
    );
    assert!(
        view.session().path().contains("gallery/List"),
        "{}",
        view.session().path()
    );
}

fn drive_party(link: &HostLink) {
    let mut view = open_screen(link, "/apps/SimpleScreens/Party/FindParty");
    paint_frame(&mut view);
    assert_stacked(view.session().model(), frame(180).canvas);
    note_search(&mut view);
    view = open_screen(link, "/apps/SimpleScreens/Party/FindParty");
    check_country_states(&mut view);
    create_person(&mut view, link);
}

/// A search the index cannot answer leaves the screen with its message, not a crash.
fn note_search(view: &mut AppView<HostLink>) {
    assert!(
        focus_at(view, "OuterSearchForm.anyField"),
        "field missing at {}",
        view.session().focus_id()
    );
    type_text(view, "zzznomatch");
    view.on_key(&press(Key::Enter));
    assert!(view.session().path().contains("FindParty"), "{}", view.session().path());
    assert_eq!(
        view.session().parameters().get("anyField").map(String::as_str),
        Some("zzznomatch")
    );
    let shown = view.session().list_page("PartyListForm").len();
    assert!(
        shown == 0 || lone_error(view.session().model()).is_some() || !view.session().message().is_empty(),
        "{shown} rows for a search that matches nothing"
    );
}

fn check_country_states(view: &mut AppView<HostLink>) {
    focus(view, "CreatePersonCustDialog");
    view.on_key(&press(Key::Enter));
    focus(view, "CreatePersonCustForm.stateProvinceGeoId");
    choose_value(view, "CA");
    assert_eq!(view.session().buffer("CreatePersonCustForm.stateProvinceGeoId"), "CA");
    focus(view, "CreatePersonCustForm.countryGeoId");
    choose_value(view, "CAN");
    assert_ne!(view.session().buffer("CreatePersonCustForm.stateProvinceGeoId"), "CA");
    focus(view, "CreatePersonCustForm.stateProvinceGeoId");
    view.on_key(&press(Key::F(4)));
    let choices = view.session().dropdown_choices();
    assert!(choices.iter().any(|choice| choice.value == "ON"), "{choices:?}");
    assert!(choices.iter().all(|choice| choice.value != "CA"), "{choices:?}");
    view.on_key(&press(Key::Escape));
    view.on_key(&press(Key::Escape));
    // The dialog's edits ask before they go.
    if view.session().confirm_choice().is_some() {
        view.on_key(&press(Key::Char('d')));
        view.on_key(&press(Key::Escape));
    }
    assert!(
        view.session().dialogs_open().is_empty(),
        "{:?}",
        view.session().dialogs_open()
    );
}

fn create_person(view: &mut AppView<HostLink>, link: &HostLink) {
    focus(view, "CreatePersonDialog");
    view.on_key(&press(Key::Enter));
    // An empty required name is marked here, and nothing is sent.
    focus(view, "CreatePersonForm.firstName");
    view.on_key(&press(Key::Enter));
    let marked = view.session().marks().keys().cloned().collect::<Vec<_>>();
    assert!(!marked.is_empty(), "no mark for the empty required names");
    assert!(view.session().message_is_error(), "{}", view.session().message());
    focus(view, "CreatePersonForm.firstName");
    type_text(view, "Tui");
    view.on_key(&press(Key::Tab));
    assert_eq!(view.session().focus_id(), "CreatePersonForm.lastName");
    type_text(view, "Gate");
    view.on_key(&press(Key::Enter));
    assert!(
        view.session().path().contains("/EditParty"),
        "{} {}",
        view.session().path(),
        view.session().message()
    );
    let party_id = view
        .session()
        .parameters()
        .get("partyId")
        .cloned()
        .unwrap_or_else(|| panic!("no partyId in {:?}", view.session().parameters()));
    paint_frame(view);
    assert_side_by_side(view.session().model(), frame(180).canvas);
    assert_stacked(view.session().model(), frame(100).canvas);
    focus(view, "EditPersonForm.firstName");
    type_text(view, "x");
    view.on_key(&press(Key::Enter));
    assert!(
        !view.session().message_is_error(),
        "save failed: {} mark {:?}",
        view.session().message(),
        view.session().mark("EditPersonForm.firstName")
    );
    assert!(!view.session().any_dirty(), "the save left edits");
    // Read the record back from the server: the edit was saved.
    let again = link
        .level(
            "/apps/SimpleScreens/Party/EditParty",
            &std::collections::BTreeMap::from([("partyId".to_string(), party_id)]),
        )
        .expect("EditParty again");
    let mut first_name = String::new();
    moqui_tui::walk(&again.c, &mut |node| {
        if let Node::Field { id, w, .. } = node {
            if id == "EditPersonForm.firstName" {
                first_name = w.value().to_string();
            }
        }
    });
    assert_eq!(first_name, "Tuix");
}

/// The screen at `path` as the client opens it: the shell data (drawer, theme) and the level.
/// Gallery Dialog's Edit per row, a container-dialog in a list cell: F10 on the second row offers that row's dialog,
/// whose form holds the row's values and posts its own update.
fn drive_row_dialog(link: &HostLink) {
    let mut view = open_screen(link, "/apps/gallery/Dialog");
    focus(&mut view, "EnumList");
    view.on_key(&press(Key::Down));
    assert_eq!(view.session().row_at("EnumList"), 1);
    let second = view.session().list("EnumList").expect("EnumList").rows[1].clone();
    let enum_id = second.cells[0].text().to_string();
    view.on_key(&press(Key::F(10)));
    let Some(Overlay::Menu(menu)) = view.session().top_overlay() else {
        panic!("no actions: {:?}", view.session().overlays());
    };
    let at = menu
        .items
        .iter()
        .position(|item| item.label == format!("Edit {enum_id}…"))
        .unwrap_or_else(|| panic!("no Edit {enum_id}: {:?}", menu.items));
    for _ in 0..at {
        view.on_key(&press(Key::Down));
    }
    view.on_key(&press(Key::Enter));
    let open = view.session().dialogs_open().join(",");
    assert!(open.starts_with("EditEnumDialog"), "{open}");
    assert_eq!(view.session().focus_kind(), Some(moqui_tui::FocusKind::Text));
    type_text(&mut view, " (edited)");
    view.on_key(&press(Key::Enter));
    assert!(
        view.session()
            .message()
            .contains(&format!("Enumeration {enum_id} updated")),
        "{}",
        view.session().message()
    );
}

fn open_screen(link: &HostLink, path: &str) -> AppView<HostLink> {
    let mut view = AppView::with_session(Session::start(path), link.clone());
    view.run_requests();
    assert!(
        view.session().path().ends_with(path.rsplit('/').next().unwrap_or(path)),
        "{path}: {} {}",
        view.session().path(),
        view.session().message()
    );
    assert!(!view.session().menu().is_empty(), "menu {path}");
    view
}

fn paint_frame(view: &mut AppView<HostLink>) {
    let narrow = view.screen_text(100, 40);
    let leaf = view.session().crumbs().last().cloned().unwrap_or_default();
    assert!(narrow.contains(&leaf), "{leaf}: {narrow}");
    let wide = view.screen_text(180, 50);
    assert!(wide.contains('│'), "{wide}");
    let title = drawer_title(view);
    assert!(!title.is_empty(), "menu has no title");
    assert!(wide.contains(&title), "missing {title}\n{wide}");
}

fn drawer_title(view: &AppView<HostLink>) -> String {
    let mut titles = Vec::new();
    for item in view.session().menu() {
        if !item.title.is_empty() {
            titles.push(item.title.clone());
        }
        for child in &item.children {
            if !child.title.is_empty() {
                titles.push(child.title.clone());
            }
        }
    }
    titles
        .into_iter()
        .min_by_key(|title| title.chars().count())
        .unwrap_or_default()
}

fn focus(view: &mut AppView<HostLink>, id: &str) {
    assert!(
        focus_at(view, id),
        "focus is {}, wanted {id}",
        view.session().focus_id()
    );
}

fn focus_at(view: &mut AppView<HostLink>, id: &str) -> bool {
    if view.session().focus_id() == id {
        return true;
    }
    for _ in 0..200 {
        view.on_key(&press(Key::Tab));
        if view.session().focus_id() == id {
            return true;
        }
    }
    false
}

fn type_text(view: &mut AppView<HostLink>, text: &str) {
    for ch in text.chars() {
        view.on_key(&press(Key::Char(ch)));
    }
}

fn choose_value(view: &mut AppView<HostLink>, value: &str) {
    view.on_key(&press(Key::F(4)));
    let choices = view.session().dropdown_choices();
    let at = view.session().dropdown().map_or(0, |drop| drop.at);
    let Some(to) = choices.iter().position(|choice| choice.value == value) else {
        let shown: Vec<_> = choices.iter().take(8).map(|choice| choice.value.as_str()).collect();
        panic!(
            "no {value} among {} choices, first {shown:?}; {}",
            choices.len(),
            field_debug(view)
        );
    };
    let key = if to >= at { Key::Down } else { Key::Up };
    for _ in 0..at.abs_diff(to) {
        view.on_key(&press(key.clone()));
    }
    view.on_key(&press(Key::Enter));
}

fn field_debug(view: &AppView<HostLink>) -> String {
    let id = view.session().focus_id().to_string();
    let buffer = view.session().buffer(&id).to_string();
    let mut found = String::new();
    describe_field(&view.session().model().c, &id, &mut found);
    if found.is_empty() {
        format!("focus {id} buffer {buffer}")
    } else {
        format!("{found} buffer {buffer}")
    }
}

fn describe_field(nodes: &[Node], id: &str, found: &mut String) {
    for node in nodes {
        if let Node::Field { id: field_id, w, .. } = node {
            if field_id == id {
                *found = format!("{field_id} {w:?}");
            }
        }
        describe_field(node.children(), id, found);
    }
}

fn press(key: Key) -> KeyPress {
    KeyPress::plain(key)
}

fn first_cell(view: &AppView<HostLink>) -> String {
    view.session()
        .list_page("EnumList")
        .first()
        .and_then(|row| row.cells.first())
        .map(|cell| cell.text().to_string())
        .unwrap_or_default()
}

fn list_count(list: &FormList) -> u32 {
    if list.paging.count > 0 {
        list.paging.count
    } else {
        u32::try_from(list.rows.len()).unwrap_or(u32::MAX)
    }
}

fn list_summary(model: &ScreenModel) -> String {
    let mut lines = Vec::new();
    summarize_lists(&model.c, &mut lines);
    if lines.is_empty() {
        "no form-list".to_string()
    } else {
        lines.join("; ")
    }
}

fn summarize_lists(nodes: &[Node], lines: &mut Vec<String>) {
    for node in nodes {
        if let Node::FormList { list } = node {
            lines.push(format!(
                "{} count {} size {} rows {}",
                list.id,
                list.paging.count,
                list.paging.size,
                list.rows.len()
            ));
        }
        summarize_lists(node.children(), lines);
    }
}

fn find_list<'a>(nodes: &'a [Node], id: &str) -> Option<&'a FormList> {
    for node in nodes {
        if let Node::FormList { list } = node {
            if list.id == id {
                return Some(list);
            }
        }
        if let Some(found) = find_list(node.children(), id) {
            return Some(found);
        }
    }
    None
}

fn assert_side_by_side(model: &ScreenModel, canvas: u16) {
    let xs = column_xs(model, canvas);
    assert!(xs.len() >= 2, "columns {xs:?}\n{}", outline(model, canvas));
}

fn assert_stacked(model: &ScreenModel, canvas: u16) {
    let xs = column_xs(model, canvas);
    assert!(xs.len() < 2, "columns {xs:?}\n{}", outline(model, canvas));
}

fn column_xs(model: &ScreenModel, canvas: u16) -> Vec<u16> {
    let mut xs = Vec::new();
    for line in outline(model, canvas).lines() {
        let Some(rest) = line.trim().strip_prefix("col x=") else {
            continue;
        };
        let Some(token) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(x) = token.parse::<u16>() else {
            continue;
        };
        if !xs.contains(&x) {
            xs.push(x);
        }
    }
    xs
}

fn grant_gate(factory: &Arc<ExecutionContextFactory>) {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().expect("entity facade");
    let mut group = facade
        .make_value("moqui.security.ArtifactGroup")
        .expect("ArtifactGroup");
    group.set("artifactGroupId", Value::string("TUI_GATE"));
    group.set("description", Value::string("Screen TUI gate"));
    group.create(&facade).expect("TUI_GATE");
    grant_member(&facade, ".*", "AT_SERVICE");
    grant_member(&facade, "component://SimpleScreens/.*", "AT_XML_SCREEN");
    grant_member(&facade, "component://SimpleScreens/.*", "AT_XML_SCREEN_TRANS");
    let mut authz = facade
        .make_value("moqui.security.ArtifactAuthz")
        .expect("ArtifactAuthz");
    authz.set("artifactAuthzId", Value::string("TUI_GATE_ADMIN"));
    authz.set("userGroupId", Value::string("ADMIN"));
    authz.set("artifactGroupId", Value::string("TUI_GATE"));
    authz.set("authzTypeEnumId", Value::string("AUTHZT_ALWAYS"));
    authz.set("authzActionEnumId", Value::string("AUTHZA_ALL"));
    authz.create(&facade).expect("TUI_GATE_ADMIN");
    grant_state(&facade, "CA", "California", "USA");
    grant_state(&facade, "ON", "Ontario", "CAN");
    grant_enum(&facade, "PartyType", "Party Type", "PtyPerson", "Person");
    let mut na = facade.make_value("mantle.party.Party").expect("Party");
    na.set("partyId", Value::string("_NA_"));
    na.set("pseudoId", Value::string("_NA_"));
    na.set("partyTypeEnumId", Value::string("PtyPerson"));
    na.set("ownerPartyId", Value::string("_NA_"));
    na.create(&facade).unwrap_or_else(|err| panic!("_NA_ party: {err}"));
}

fn grant_enum(
    facade: &moqui_core::entity::EntityFacade,
    type_id: &str,
    type_desc: &str,
    enum_id: &str,
    enum_desc: &str,
) {
    let mut kind = facade
        .make_value("moqui.basic.EnumerationType")
        .expect("EnumerationType");
    kind.set("enumTypeId", Value::string(type_id));
    kind.set("description", Value::string(type_desc));
    kind.create(facade)
        .unwrap_or_else(|err| panic!("enum type {type_id}: {err}"));
    let mut item = facade.make_value("moqui.basic.Enumeration").expect("Enumeration");
    item.set("enumId", Value::string(enum_id));
    item.set("enumTypeId", Value::string(type_id));
    item.set("description", Value::string(enum_desc));
    item.create(facade)
        .unwrap_or_else(|err| panic!("enum {enum_id}: {err}"));
}

fn grant_member(facade: &moqui_core::entity::EntityFacade, name: &str, artifact_type: &str) {
    let mut member = facade
        .make_value("moqui.security.ArtifactGroupMember")
        .expect("ArtifactGroupMember");
    member.set("artifactGroupId", Value::string("TUI_GATE"));
    member.set("artifactName", Value::string(name));
    member.set("artifactTypeEnumId", Value::string(artifact_type));
    member.set("nameIsPattern", Value::string("Y"));
    member.set("inheritAuthz", Value::string("Y"));
    member
        .create(facade)
        .unwrap_or_else(|err| panic!("{artifact_type} {name}: {err}"));
}

fn grant_state(facade: &moqui_core::entity::EntityFacade, geo_id: &str, geo_name: &str, parent: &str) {
    let mut geo = facade.make_value("moqui.basic.Geo").expect("Geo");
    geo.set("geoId", Value::string(geo_id));
    geo.set("geoTypeEnumId", Value::string("GEOT_STATE"));
    geo.set("geoName", Value::string(geo_name));
    geo.set("geoCodeAlpha2", Value::string(geo_id));
    geo.create(facade).unwrap_or_else(|err| panic!("geo {geo_id}: {err}"));
    let mut assoc = facade.make_value("moqui.basic.GeoAssoc").expect("GeoAssoc");
    assoc.set("geoId", Value::string(parent));
    assoc.set("toGeoId", Value::string(geo_id));
    assoc.set("geoAssocTypeEnumId", Value::string("GAT_REGIONS"));
    assoc
        .create(facade)
        .unwrap_or_else(|err| panic!("assoc {parent}->{geo_id}: {err}"));
}
