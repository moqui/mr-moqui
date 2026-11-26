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
        "mod-shift-F",
    ] {
        assert!(valid_keys(keys), "{keys}");
    }
    let long = format!("Mod-{}", "x".repeat(MAX_KEYS_LEN));
    for keys in ["", "Mod-", "Super-x", "Mod-Shift-", "Mod-a b", "Mod- ", long.as_str()] {
        assert!(!valid_keys(keys), "{keys}");
    }
}

#[test]
fn two_ways_of_writing_a_key_are_one_key_on_the_platform_that_presses_it() {
    // the order of the modifiers and the key's case do not matter
    assert!(same_keys("Shift-Mod-F", "Mod-Shift-f"));
    // Mod is Ctrl except on a Mac, where it is Cmd
    assert!(same_keys("Mod-e", "Ctrl-e"));
    assert!(same_keys("Mod-e", "Cmd-e"));
    assert!(same_keys("Control-e", "ctrl-E"));
    assert!(!same_keys("Ctrl-e", "Cmd-e"));
    assert!(!same_keys("Mod-e", "Mod-Shift-e"));
}

#[test]
fn a_preference_that_is_not_settings_is_the_defaults_and_bad_parts_are_left_out() {
    assert_eq!(EditorSettings::parse(None), EditorSettings::default());
    assert_eq!(EditorSettings::parse(Some("not json")), EditorSettings::default());
    assert_eq!(
        EditorSettings::default().tab_size,
        None,
        "CodeMirror's tab and indent unless the user sets one"
    );
    let settings = EditorSettings::parse(Some(
        r#"{"fontSize": 99, "tabSize": 2, "wrap": true, "keymap": "emacs",
            "keys": {"undo": "Mod-u", "nothing": "Mod-q", "save": "Hyper-s", "find": "mod-F"}}"#,
    ));
    assert_eq!(settings.font_size, 13);
    assert_eq!(settings.tab_size, Some(2));
    assert!(settings.wrap && !settings.whitespace);
    assert_eq!(settings.keymap, "default");
    // an unknown command, keys not in the editor's form, and a row the same as its default are dropped
    assert_eq!(
        settings.keys.into_iter().collect::<Vec<_>>(),
        vec![("undo".to_string(), "Mod-u".to_string())]
    );
    assert_eq!(EditorSettings::parse(Some(r#"{"tabSize": 40}"#)).tab_size, None);
}

#[test]
fn a_command_rebound_its_default_back_and_a_key_another_command_has_refused() {
    let mut settings = EditorSettings::default();
    settings.rebind("undo", "Mod-e").unwrap();
    assert_eq!(settings.keys.get("undo").map(String::as_str), Some("Mod-e"));
    // the old key of a rebound command is free, its new one is taken, however it is written
    settings.rebind("find", "Mod-z").unwrap();
    for taken in ["mod-e", "Ctrl-e", "Cmd-E"] {
        let refused = settings.rebind("save", taken).unwrap_err();
        assert!(refused.contains("Undo"), "{taken}: {refused}");
    }
    // a default key is taken too, unless its command was rebound away from it
    assert!(settings.rebind("redo", "Mod-s").unwrap_err().contains("Save"));
    assert!(settings.rebind("undo", "Mod-Shift-").is_err());
    assert!(settings
        .rebind("nothing", "Mod-q")
        .unwrap_err()
        .contains("no editor command"));
    assert!(settings.rebind("undo", &"x".repeat(200)).unwrap_err().len() < 200);
    // the default back is refused while another command has its keys
    assert!(settings.rebind("undo", "").unwrap_err().contains("Find in the file"));
    // empty, or its default written another way, takes the row out
    settings.rebind("find", "mod-F").unwrap();
    settings.rebind("undo", "").unwrap();
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
    for (index, command) in COMMANDS.iter().enumerate() {
        assert!(
            !COMMANDS[index + 1..]
                .iter()
                .any(|other| same_keys(other.keys, command.keys)),
            "{}",
            command.name
        );
    }
}
