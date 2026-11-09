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
