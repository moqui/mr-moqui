use super::*;

fn colors(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect()
}

fn map(pairs: Vec<(&str, Value)>) -> IndexMap<String, Value> {
    pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
}

/// No policy, or a blank one, allows everything, as the shell did before there were policies.
#[test]
fn no_policy_allows_every_appearance_preference() {
    for text in [None, Some(""), Some("  ")] {
        let policy = AppearancePolicy::parse(text);
        assert_eq!(policy, AppearancePolicy::default());
        for (key, _) in APPEARANCE_KEYS {
            assert_eq!(policy.denial(key), None, "{key}");
        }
        assert!(policy.all_locked_keys().is_empty());
    }
}

/// The mode group is a boolean permission unless the theme names a forced light, dark or automatic mode.
#[test]
fn appearance_mode_survives_serialization_and_service_normalization() {
    for mode in ["true", "false", "\"light\"", "\"dark\"", "\"auto\""] {
        let policy = AppearancePolicy::parse(Some(&format!(r#"{{"mode": {mode}}}"#)));
        let expected: serde_json::Value = serde_json::from_str(mode).unwrap();
        assert_eq!(moqui_lib::json::to_json(&policy.as_value())["mode"], expected);
        let finished = finish_appearance(map(vec![("policy", policy.as_value())]));
        let normalized = AppearancePolicy::from_value(finished.get("policy").unwrap());
        assert_eq!(normalized.mode, policy.mode, "mode={mode}");
        assert_eq!(normalized.all_locked_keys(), policy.all_locked_keys(), "mode={mode}");
        assert_eq!(
            moqui_lib::json::to_json(finished.get("policy").unwrap())["mode"],
            expected
        );
    }
}

/// A policy that is not a JSON object is not trusted: everything is locked, including a component's own `THEME_*` key,
/// and a key that is not an appearance preference stays writable.
#[test]
fn a_malformed_policy_locks_everything() {
    for text in ["{not json", "[1, 2]", "true"] {
        let policy = AppearancePolicy::parse(Some(text));
        assert!(policy.locked, "{text}");
        assert!(policy.denial("THEME_COLOR_SCHEME").is_some());
        assert!(policy.denial("QUASAR_DARK").is_some());
        assert!(policy.denial("THEME_SOMETHING_ELSE").is_some());
        assert_eq!(policy.denial("QUASAR_LEFT_OPEN"), None);
    }
}

/// A group set false locks its keys, a forced mode locks the mode, and lockedKeys names more; the page reads the result as
/// a flag per group and every locked key.
#[test]
fn a_policy_locks_its_groups_mode_and_named_keys() {
    let policy = AppearancePolicy::parse(Some(
        r#"{"accent": false, "fonts": "false", "mode": "dark", "lockedKeys": ["THEME_RADIUS", "NOT_APPEARANCE"]}"#,
    ));
    assert_eq!(policy.mode, "dark");
    for key in ["THEME_PRIMARY_COLOR", "THEME_PRIMARY_COLOR_DARK", "THEME_FONT_BODY", "THEME_FONT_MONO", "THEME_RADIUS"] {
        assert!(policy.denial(key).is_some(), "{key}");
    }
    assert!(policy.denial("QUASAR_DARK").is_some_and(|reason| reason.contains("dark mode")));
    for key in ["THEME_COLOR_SCHEME", "THEME_MOTION", "SHELL_DENSITY"] {
        assert_eq!(policy.denial(key), None, "{key}");
    }
    let Value::Map(value) = policy.as_value() else { panic!("not a map") };
    assert_eq!(value.get("accent"), Some(&Value::Bool(false)));
    assert_eq!(value.get("scheme"), Some(&Value::Bool(true)));
    assert_eq!(value.get("appearance"), Some(&Value::Bool(true)));
    assert_eq!(value.get("mode"), Some(&Value::string("dark")));
    let Some(Value::List(keys)) = value.get("lockedKeys") else { panic!("no lockedKeys") };
    let keys: Vec<String> = keys.iter().map(to_plain_string).collect();
    assert!(keys.contains(&"QUASAR_DARK".to_string()) && keys.contains(&"THEME_RADIUS".to_string()), "{keys:?}");
    assert!(!keys.contains(&"NOT_APPEARANCE".to_string()), "{keys:?}");
    // what the page and an appearance service read back is the same policy
    assert_eq!(AppearancePolicy::from_value(&policy.as_value()).all_locked_keys(), policy.all_locked_keys());
}

/// The scheme's sheet keeps the transition's format: colors by name, brand as `--q-*`, the dark extras, a bad value left
/// out, then the extra CSS.
#[test]
fn a_scheme_sheet_is_its_tokens_then_its_extra_css() {
    let scheme = ColorScheme {
        id: "S".to_string(),
        light: colors(&[("page-bg", "#fff"), ("primary", "#123456"), ("text", "red; } body { display:none")]),
        dark: colors(&[("link", "#9cf"), ("page-bg", "#000"), ("panel-bg", "#111"), ("card-shadow", "0 1px 2px rgb(0 0 0 / 40%)")]),
        extra_css: "body { --m-x:1 }".to_string(),
        ..ColorScheme::default()
    };
    assert_eq!(
        scheme.css(),
        "/* color scheme S */\n\
         body:not(.body--dark) { --m-page-bg:#fff; --q-primary:#123456 !important; }\n\
         body.body--dark { --m-link:var(--m-accent-dark-link, #9cf); --m-page-bg:#000; --q-dark-page:#000 !important; \
         --m-panel-bg:#111; --q-dark:#111 !important; --m-card-shadow:0 1px 2px rgb(0 0 0 / 40%); }\n\
         body { --m-x:1 }\n"
    );
    assert!(!is_token_name("Bad"));
    assert!(!is_token_name(&"a".repeat(42)));
    assert!(!is_token_value("url('x')"));
}

/// The page's hash, so a version from the server and one from the page agree.
#[test]
fn the_text_hash_is_djb2_xor_in_base_36() {
    assert_eq!(text_hash(""), "45h");
    assert_eq!(text_hash("a"), "3t1g");
}

/// After the services: a locked value is dropped, the default scheme counts only when it is the theme's, the user's own
/// scheme wins, and the scheme's default font is linked when the user has none.
#[test]
fn the_finished_appearance_follows_policy_default_scheme_and_fonts() {
    let scheme = |id: &str, custom: bool, fonts: Vec<(&str, Value)>| {
        Value::Map(map(vec![
            ("id", Value::string(id)),
            ("custom", Value::Bool(custom)),
            ("version", Value::string("v1")),
            ("fonts", Value::Map(map(fonts))),
        ]))
    };
    let font = |id: &str, sheet: &str| {
        Value::Map(map(vec![("id", Value::string(id)), ("stylesheet", Value::string(sheet))]))
    };
    let base = || {
        map(vec![
            (
                "colorSchemes",
                Value::List(vec![
                    scheme("PLAIN", false, vec![]),
                    scheme("LAB", true, vec![("body", Value::string("INTER")), ("mono", Value::string("JB"))]),
                ]),
            ),
            ("fonts", Value::List(vec![font("INTER", "/libs/inter.css"), font("JB", "/libs/jb.css"), font("SERIF", "")])),
            ("defaultSchemeId", Value::string("LAB")),
            ("policy", AppearancePolicy::parse(Some(r#"{"accent": false}"#)).as_value()),
            (
                "values",
                Value::Map(map(vec![
                    ("THEME_PRIMARY_COLOR", Value::string("#f00")),
                    ("THEME_FONT_MONO", Value::string("SERIF")),
                ])),
            ),
        ])
    };
    let done = finish_appearance(base());
    assert_eq!(done.get("schemeId"), Some(&Value::string("LAB")));
    let Some(Value::Map(values)) = done.get("values") else { panic!("no values") };
    assert!(!values.contains_key("THEME_PRIMARY_COLOR"), "a locked value is dropped: {values:?}");
    let Some(Value::List(links)) = done.get("links") else { panic!("no links") };
    let hrefs: Vec<String> = links
        .iter()
        .map(|link| match link {
            Value::Map(link) => text_of(link, "href"),
            _ => String::new(),
        })
        .collect();
    assert_eq!(hrefs, ["/apps/colorSchemeCss?colorSchemeId=LAB&v=v1", "/libs/inter.css"], "the user's mono font has no sheet");

    let mut own = base();
    own.insert("values".to_string(), Value::Map(map(vec![("THEME_COLOR_SCHEME", Value::string("PLAIN"))])));
    own.insert("defaultSchemeId".to_string(), Value::string("ELSEWHERE"));
    let done = finish_appearance(own);
    assert_eq!(done.get("schemeId"), Some(&Value::string("PLAIN")));
    assert_eq!(done.get("defaultSchemeId"), Some(&Value::string("")));
    assert_eq!(done.get("links"), Some(&Value::List(Vec::new())));
}

#[test]
fn a_url_component_is_encoded_as_the_page_encodes_it() {
    assert_eq!(url_component("A_B-1.c"), "A_B-1.c");
    assert_eq!(url_component("a b&c"), "a%20b%26c");
}
