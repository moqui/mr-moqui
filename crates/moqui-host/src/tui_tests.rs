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

use super::*;

#[test]
fn a_navigation_attachment_keeps_the_screen_and_saves_the_original_bytes() {
    struct DownloadTransport;
    impl Transport for DownloadTransport {
        fn send(&self, ask: Ask) -> Result<RenderOutput, LinkError> {
            let refused = ask.parameters.get("refused").is_some_and(|value| value == "Y");
            Ok(RenderOutput {
                status: if refused { 403 } else { 200 },
                headers: vec![
                    ("Content-Type".into(), "application/octet-stream".into()),
                    (
                        "Content-Disposition".into(),
                        "attachment; filename=\"revision.bin\"".into(),
                    ),
                ],
                body: if refused {
                    b"Forbidden".to_vec()
                } else if ask.parameters.contains_key("json") {
                    br#"{"screenUrl":"/not-a-screen-redirect"}"#.to_vec()
                } else {
                    vec![0, 255, 1, 128]
                },
            })
        }

        fn login(&self, _username: &str, _password: &str, _code: &str) -> Result<(), LinkError> {
            Ok(())
        }

        fn user(&self) -> Option<String> {
            Some("fixture".into())
        }
    }

    let link = Arc::new(Link::over(DownloadTransport));
    let model = ScreenModel::from_json(
        r#"{"v":1,"screen":{"path":"/apps/app/Files","pathList":["apps","app","Files"]},
            "c":[{"t":"link","id":"download","text":"Download","url":"/apps/app/Files/raw"}]}"#,
    )
    .unwrap();
    let mut session = moqui_tui::Session::new(model.clone());
    session.navigate("/apps/app/Files/raw?ref=abc123", true);
    session.run_now(&link);
    assert_eq!(session.model(), &model);
    assert!(session.nav().history().is_empty());
    assert_eq!(session.focus_id(), "download");
    assert!(!session.message_is_error());
    assert_eq!(
        session.take_downloads(),
        [RawBody {
            content_type: "application/octet-stream".into(),
            file_name: "revision.bin".into(),
            bytes: vec![0, 255, 1, 128],
        }]
    );
    session.navigate("/apps/app/Files/raw?json=Y", true);
    session.run_now(&link);
    assert_eq!(
        session.take_downloads()[0].bytes,
        br#"{"screenUrl":"/not-a-screen-redirect"}"#
    );
    assert_eq!(
        session.model(),
        &model,
        "an attached JSON file is not parsed as navigation metadata"
    );
    session.navigate("/apps/app/Files/raw?refused=Y", true);
    session.run_now(&link);
    assert!(session.take_downloads().is_empty());
    assert!(session.message_is_error());
    assert!(session.message().contains("403"));
    assert_eq!(session.model(), &model);
}

#[test]
fn context_free_quick_create_leaves_out_record_required_actions() {
    let data = serde_json::json!({"apps": [{"title": "Commons", "path": "/qapps/Commons", "providers": {
        "quickCreate": [
            {"label": "Task", "url": "/qapps/Commons/QuickCreate?kind=task", "requiresRecord": false},
            {"label": "Activity", "url": "/qapps/Commons/QuickCreate?kind=activity", "requiresRecord": true},
            {"label": "Note", "url": "/qapps/Commons/QuickCreate?kind=note"}
        ]
    }}]});
    let mut shell = ShellInfo::default();
    shell_values(&data, &mut shell);
    let choices: Vec<(&str, &str)> = shell
        .quick_create
        .iter()
        .map(|action| (action.label.as_str(), action.url.as_str()))
        .collect();
    assert_eq!(
        choices,
        [
            ("Task", "/qapps/Commons/QuickCreate?kind=task"),
            ("Note", "/qapps/Commons/QuickCreate?kind=note")
        ]
    );
}

#[test]
fn a_redirect_location_keeps_what_follows_its_host() {
    assert_eq!(
        local_path("https://erp.example.com/qapps/Order?id=1"),
        "/qapps/Order?id=1"
    );
    assert_eq!(local_path("https://erp.example.com"), "/");
    assert_eq!(local_path("/qapps/Order"), "/qapps/Order");
}

#[test]
fn a_relative_url_drops_the_last_segment_before_dot_dot() {
    assert_eq!(resolve_url("/apps/gallery/List", "../Detail"), "/apps/Detail");
    assert_eq!(
        resolve_url("/apps/gallery/Wizard/Options", "../Contact?contactId=1"),
        "/apps/gallery/Contact?contactId=1"
    );
    assert_eq!(
        resolve_url("/apps/gallery/List", "/apps/gallery/Detail?enumId=A"),
        "/apps/gallery/Detail?enumId=A"
    );
    assert_eq!(
        resolve_url("/apps/gallery/List?pageIndex=1", "detail"),
        "/apps/gallery/detail"
    );
    assert_eq!(resolve_url("/apps/gallery/List", "./Detail"), "/apps/gallery/Detail");
}

#[test]
fn a_failed_transition_is_never_a_navigation_and_keeps_its_errors() {
    let output = |status: u16, body: &str| RenderOutput {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    };
    let failed = outcome_from(&output(
        500,
        r#"{"errorCode":500,"errors":"Party is locked","screenUrl":"/x"}"#,
    ))
    .unwrap();
    assert!(failed.screen_url.is_empty());
    assert_eq!(failed.messages.errors, ["Party is locked"]);
    assert!(matches!(outcome_from(&output(401, "{}")), Err(LinkError::Auth(_))));
    let saved = outcome_from(&output(200, r#"{"messages":"Saved","screenUrl":"/y"}"#)).unwrap();
    assert_eq!(saved.messages.info, ["Saved"]);
    assert_eq!(saved.screen_url, "/y");
    let repost = screen_repost("/apps/x?a=%C3%A9&b=100%", &BTreeMap::new());
    assert_eq!(repost.parameters.get("a").map(String::as_str), Some("é"));
    assert_eq!(repost.parameters.get("b").map(String::as_str), Some("100%"));
}

/// The menu data's `disableLink` reaches the drawer: the item is kept, marked disabled, so the terminal shows it dimmed
/// and not as a link (it used to be a live link there).
#[test]
fn a_disabled_menu_entry_is_kept_and_marked() {
    let menu = serde_json::json!([{ "name": "Tools", "title": "Tools", "path": "/Tools", "subscreens": [
        { "name": "Open", "title": "Open", "path": "/Tools/Open" },
        { "name": "Load", "title": "Load testing", "path": "/Tools/Load", "disableLink": true }
    ]}]);
    let item = menu_item(&menu[0]).unwrap();
    assert!(!item.disabled);
    let children: Vec<(&str, bool)> = item
        .children
        .iter()
        .map(|child| (child.title.as_str(), child.disabled))
        .collect();
    assert_eq!(children, [("Open", false), ("Load testing", true)]);
}

/// A parent level names the page it is loaded for in `X-Moqui-Page-Path` (percent-encoded, visible ASCII alone), and
/// does not become the base of relative urls; the leaf names none.
#[test]
fn a_parent_level_names_its_page_and_leaves_the_base_at_the_leaf() {
    struct Recording(Arc<Mutex<Vec<(String, String)>>>);
    impl Transport for Recording {
        fn send(&self, ask: Ask) -> Result<RenderOutput, LinkError> {
            let path = ask.path.trim_end_matches(".tui").to_string();
            lock(&self.0).push((ask.path, ask.page));
            let body = serde_json::json!({ "v": 1, "screen": { "path": path, "pathList": [] }, "c": [] });
            Ok(RenderOutput {
                status: 200,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: body.to_string().into_bytes(),
            })
        }

        fn login(&self, _username: &str, _password: &str, _code: &str) -> Result<(), LinkError> {
            Ok(())
        }

        fn user(&self) -> Option<String> {
            Some("fixture".into())
        }
    }

    let asked = Arc::new(Mutex::new(Vec::new()));
    let link = Link::over(Recording(Arc::clone(&asked)));
    let none = BTreeMap::new();
    assert!(matches!(
        link.navigation("/apps/People/Employees/Grievances", &none),
        Reply::Level(Ok(_))
    ));
    let parent = link.parent("/apps/People/Employees.tui", "/apps/People/Employees/Grievances", &none);
    assert!(matches!(parent, Reply::Level(Ok(ref model)) if model.screen.path == "/apps/People/Employees"));
    assert_eq!(
        *lock(&asked),
        [
            ("/apps/People/Employees/Grievances".to_string(), String::new()),
            (
                "/apps/People/Employees.tui".to_string(),
                "/apps/People/Employees/Grievances".to_string()
            ),
        ]
    );
    assert_eq!(link.resolve("Detail"), "/apps/People/Employees/Detail");
    assert_eq!(
        page_path_header("/apps/x/Leave Requests+é"),
        "/apps/x/Leave%20Requests%2B%C3%A9"
    );
}
