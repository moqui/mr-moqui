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

use super::AppearancePolicy;

#[test]
fn no_policy_allows_the_rest_and_locks_the_accent() {
    let policy = AppearancePolicy::parse(None);
    assert_eq!(policy, AppearancePolicy::default());
    assert_eq!(policy.denial("QUASAR_DARK"), None);
    assert!(
        policy.denial("THEME_PRIMARY_COLOR").is_some(),
        "the theme sets the primary"
    );
    assert!(policy.denial("THEME_PRIMARY_COLOR_DARK").is_some());
    assert_eq!(policy.denial("THEME_COLOR_SCHEME"), None);
    assert_eq!(policy.denial("PAGE_SIZE"), None);
}

#[test]
fn an_empty_object_allows_the_rest_and_locks_the_accent() {
    assert_eq!(AppearancePolicy::parse(Some("{}")), AppearancePolicy::default());
    assert!(AppearancePolicy::parse(Some("{}"))
        .denial("THEME_PRIMARY_COLOR")
        .is_some());
}

#[test]
fn the_theme_may_allow_the_users_accent() {
    let policy = AppearancePolicy::parse(Some(r#"{"accent": true}"#));
    assert!(policy.accent);
    assert_eq!(policy.denial("THEME_PRIMARY_COLOR"), None);
    assert_eq!(policy.denial("THEME_PRIMARY_COLOR_DARK"), None);
}

#[test]
fn appearance_false_locks_every_appearance_preference() {
    let policy = AppearancePolicy::parse(Some(r#"{"appearance": false}"#));
    assert!(!policy.appearance);
    for key in [
        "QUASAR_DARK",
        "SHELL_DENSITY",
        "THEME_COLOR_SCHEME",
        "THEME_PRIMARY_COLOR",
        "THEME_FONT_BODY",
        "THEME_SIZE_SMALL",
        "THEME_MOTION",
        "THEME_COLOR_OVERRIDES",
        "THEME_SOMETHING_NEW",
    ] {
        assert!(policy.denial(key).is_some(), "{key} should be denied");
    }
    assert_eq!(policy.denial("SHELL_START_APP"), None);
    assert_eq!(policy.denial("QUASAR_LEFT_OPEN"), None);
}

#[test]
fn a_forced_mode_locks_only_the_mode() {
    let policy = AppearancePolicy::parse(Some(r#"{"mode": "dark"}"#));
    assert_eq!(policy.mode, "dark");
    assert!(policy.denial("QUASAR_DARK").unwrap().contains("dark"));
    assert_eq!(policy.denial("THEME_COLOR_SCHEME"), None);
    assert!(policy.denial("THEME_PRIMARY_COLOR").is_some());
}

#[test]
fn an_unknown_mode_is_not_forced() {
    assert_eq!(AppearancePolicy::parse(Some(r#"{"mode": "sepia"}"#)).mode, "");
}

#[test]
fn a_locked_control_is_denied_and_the_rest_allowed() {
    let policy = AppearancePolicy::parse(Some(
        r#"{"scheme": false, "accent": false, "fonts": false, "sizes": false, "motion": false, "density": false, "colors": false}"#,
    ));
    for key in [
        "THEME_COLOR_SCHEME",
        "THEME_PRIMARY_COLOR",
        "THEME_PRIMARY_COLOR_DARK",
        "THEME_FONT_HEADING",
        "THEME_SIZE_LARGE",
        "THEME_RADIUS",
        "SHELL_DENSITY",
        "THEME_COLOR_OVERRIDES",
    ] {
        assert!(policy.denial(key).is_some(), "{key} should be denied");
    }
    assert_eq!(policy.denial("QUASAR_DARK"), None);
    assert!(
        policy.denial("THEME_LINKS_UNDERLINE").is_some(),
        "motion false locks it"
    );
}

#[test]
fn a_policy_that_is_not_json_is_locked() {
    let policy = AppearancePolicy::parse(Some("not json"));
    assert!(!policy.appearance);
    assert!(policy.denial("QUASAR_DARK").is_some());
}

#[test]
fn a_policy_that_is_not_an_object_is_locked() {
    assert!(!AppearancePolicy::parse(Some("[]")).appearance);
    assert!(!AppearancePolicy::parse(Some("true")).appearance);
}

#[test]
fn the_page_gets_the_normalized_policy() {
    let json = AppearancePolicy::parse(Some(r#"{"mode": "light", "accent": true}"#)).as_json();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["appearance"], true);
    assert_eq!(value["mode"], "light");
    assert_eq!(value["accent"], true);
    assert_eq!(value["scheme"], true);
    let locked: serde_json::Value = serde_json::from_str(&AppearancePolicy::parse(Some("{}")).as_json()).unwrap();
    assert_eq!(locked["accent"], false);
}
