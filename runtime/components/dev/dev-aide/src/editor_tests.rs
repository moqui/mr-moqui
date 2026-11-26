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
fn keys_in_codemirror_form() {
    for keys in [
        "Mod-p",
        "Mod-Shift-f",
        "Ctrl-Space",
        "F2",
        "Mod--",
        "Ctrl-Alt-[",
        "Mod-/",
    ] {
        assert!(valid_keys(keys), "{keys}");
    }
    for keys in ["", "Mod-", "Super-x", "Mod-Shift-", "Mod-a b", "Mod- "] {
        assert!(!valid_keys(keys), "{keys}");
    }
}

#[test]
fn a_preference_that_is_not_settings_is_the_defaults_and_bad_parts_are_left_out() {
    assert_eq!(EditorSettings::parse(None), EditorSettings::default());
    assert_eq!(EditorSettings::parse(Some("not json")), EditorSettings::default());
    let settings = EditorSettings::parse(Some(
        r#"{"fontSize": 99, "tabSize": 2, "wrap": true, "keymap": "emacs",
            "keys": {"gotoFile": "Mod-e", "nothing": "Mod-q", "save": "Hyper-s", "find": "Mod-f"}}"#,
    ));
    assert_eq!(settings.font_size, 13);
    assert_eq!(settings.tab_size, 2);
    assert!(settings.wrap && !settings.whitespace);
    assert_eq!(settings.keymap, "default");
    // an unknown command, keys not in the editor's form, and a row the same as its default are dropped
    assert_eq!(
        settings.keys.into_iter().collect::<Vec<_>>(),
        vec![("gotoFile".to_string(), "Mod-e".to_string())]
    );
}

#[test]
fn a_command_rebound_its_default_back_and_a_key_another_command_has_refused() {
    let mut settings = EditorSettings::default();
    settings.rebind("gotoFile", "Mod-e").unwrap();
    assert_eq!(settings.keys.get("gotoFile").map(String::as_str), Some("Mod-e"));
    // the old key of a rebound command is free, its new one is taken
    settings.rebind("find", "Mod-p").unwrap();
    let taken = settings.rebind("save", "mod-e").unwrap_err();
    assert!(taken.contains("Go to file"), "{taken}");
    // a default key is taken too, unless its command was rebound away from it
    assert!(settings.rebind("gotoArtifact", "Mod-s").unwrap_err().contains("Save"));
    assert!(settings.rebind("gotoFile", "Mod-Shift-").is_err());
    assert!(settings
        .rebind("nothing", "Mod-q")
        .unwrap_err()
        .contains("no editor command"));
    // empty, or its default, takes the row out
    settings.rebind("gotoFile", "").unwrap();
    settings.rebind("find", "Mod-f").unwrap();
    assert!(settings.keys.is_empty());
}

#[test]
fn the_answer_has_every_command_with_its_keys_now() {
    let mut settings = EditorSettings::default();
    settings.rebind("save", "Alt-s").unwrap();
    let out = settings.answer();
    let Some(Value::List(commands)) = out.get("commands") else {
        panic!("no commands: {out:?}")
    };
    assert_eq!(commands.len(), COMMANDS.len());
    let json = moqui_lib::json::to_json(&Value::List(commands.clone()));
    let save = json
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["command"] == "save"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(save["keys"], "Alt-s");
    assert_eq!(save["defaultKeys"], "Mod-s");
    // the default keys are distinct, so no default is refused as another's
    let mut defaults: Vec<&str> = COMMANDS.iter().map(|command| command.keys).collect();
    defaults.sort_unstable();
    defaults.dedup();
    assert_eq!(defaults.len(), COMMANDS.len());
}
