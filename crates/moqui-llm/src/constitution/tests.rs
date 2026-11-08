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

use std::path::PathBuf;

use serde_json::{json, Value};

use super::document::{self, UrlKind};
use super::edit::{self, Direction, Edit};
use super::{locale, origins, render};

fn shipped_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework/docs/Constitution.json")
}

fn shipped() -> Value {
    document::parse(&std::fs::read_to_string(shipped_path()).unwrap()).unwrap()
}

fn small() -> Value {
    json!({
        "schemaVersion": 1,
        "defaultLocale": "en",
        "title": {"en": "Rules", "es": "Reglas"},
        "provenance": {"en": "Ours"},
        "instructions": {"en": "Follow them.", "es": "Síguelas."},
        "x-note": {"kept": true},
        "rules": [
            {"id": "C01", "title": {"en": "One"}, "description": {"en": "First."}, "instructions": {"en": "Do one."},
             "references": [{"id": "C01-R01", "title": {"en": "Ref"}, "url": "https://example.org/a#b"}], "x-rule": 1},
            {"id": "C02", "title": {"en": "Two", "es": "Dos"}, "description": {"en": "Second."},
             "instructions": {"en": "Do two.", "es": "Haz dos."}, "references": []}
        ]
    })
}

#[test]
fn the_shipped_default_is_complete_in_english_spanish_and_chinese() {
    let doc = shipped();
    assert_eq!(document::validate(&doc), Vec::new());
    let ids: Vec<&str> = document::rules(&doc).iter().map(document::id_of).collect();
    let expected: Vec<String> = (1..=13).map(|n| format!("C{n:02}")).collect();
    assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>());
    let references: usize = document::rules(&doc).iter().map(|rule| document::references(rule).len()).sum();
    assert_eq!(references, 37);
    for wanted in ["en", "es", "zh"] {
        let rendered = render::prompt_section(&doc, wanted);
        assert!(rendered.fallbacks.is_empty(), "{wanted} falls back: {:?}", rendered.fallbacks);
        assert!(rendered.text.contains("get#ConstitutionReferences"));
        assert!(rendered.text.contains("C13 — "));
        // the measured sizes of §7.10, with room: never cut, but kept within a known budget
        assert!(rendered.text.chars().count() < 9_000, "{wanted} is {} characters", rendered.text.chars().count());
    }
}

#[test]
fn validation_names_each_field_and_reason() {
    let mut doc = small();
    doc["schemaVersion"] = json!(2);
    doc["rules"][1]["id"] = json!("C01");
    doc["rules"][0]["title"] = json!({"es": "Uno"});
    doc["rules"][0]["description"]["es"] = json!("");
    doc["rules"][0]["references"][0]["url"] = json!("http://example.org/");
    doc["title"]["EN"] = json!("Again");
    let codes: Vec<(String, &str)> = document::validate(&doc).into_iter().map(|e| (e.path, e.code)).collect();
    assert!(codes.contains(&("schemaVersion".to_string(), "schemaVersion")), "{codes:?}");
    assert!(codes.contains(&("rules[1].id".to_string(), "duplicateId")), "{codes:?}");
    assert!(codes.contains(&("rules[0].title".to_string(), "missingDefault")), "{codes:?}");
    assert!(codes.contains(&("rules[0].description.es".to_string(), "emptyText")), "{codes:?}");
    assert!(codes.contains(&("rules[0].references[0].url".to_string(), "urlScheme")), "{codes:?}");
    assert!(codes.iter().any(|(path, code)| path.starts_with("title.") && *code == "duplicateLocale"), "{codes:?}");
}

#[test]
fn reference_urls_are_https_or_a_plain_path() {
    assert_eq!(
        document::check_url("https://grok-the-rock.org/rules#r1"),
        Ok(UrlKind::External { origin: "https://grok-the-rock.org".to_string() })
    );
    assert_eq!(document::check_url("/apps/wiki/page"), Ok(UrlKind::Internal));
    for bad in ["//evil.example/x", "/a/../b", "https://u:p@example.org/", "javascript:alert(1)", "https://a b/", "/a\\b", ""] {
        assert!(document::check_url(bad).is_err(), "{bad} passed");
    }
}

#[test]
fn locales_normalize_and_fall_back_exact_then_language_then_default() {
    assert_eq!(locale::normalize("es-mx").as_deref(), Some("es_MX"));
    assert_eq!(locale::normalize("ZH-hans").as_deref(), Some("zh_Hans"));
    assert_eq!(locale::normalize("en_US"), Some("en_US".to_string()));
    assert_eq!(locale::normalize("english"), None);
    let map = json!({"en": "Rules", "es": "Reglas", "zh": "规则"});
    assert_eq!(locale::text(&map, "es_MX", "en"), Some(("Reglas", "es".to_string())));
    assert_eq!(locale::text(&map, "zh-Hans", "en"), Some(("规则", "zh".to_string())));
    assert_eq!(locale::text(&map, "fr", "en"), Some(("Rules", "en".to_string())));
}

#[test]
fn the_prompt_section_is_deterministic_and_reports_fallbacks() {
    let doc = small();
    let rendered = render::prompt_section(&doc, "es_ES");
    let expected = format!(
        "## Reglas\n\nSíguelas.\n\nC01 — One\nDo one.\n\nC02 — Dos\nHaz dos.\n\n{}\n",
        render::hint("es")
    );
    assert_eq!(rendered.text, expected);
    assert_eq!(rendered.locale, "es_ES");
    assert_eq!(rendered.fallbacks.get("rules[0].title").map(String::as_str), Some("en"));
    assert!(!rendered.fallbacks.contains_key("title"), "es for es_ES is the language asked for");
    assert_eq!(render::hint("fr"), render::hint("en"));
    assert_eq!(render::prompt_section(&doc, "es_ES"), rendered);
}

#[test]
fn edits_keep_ids_and_extension_keys() {
    let doc = small();
    let added = edit::apply(&doc, &Edit::AddRule { title: "Three".into(), description: "Third.".into(), instructions: "Do three.".into() }).unwrap();
    assert_eq!(added.new_id.as_deref(), Some("C03"));
    let removed = edit::apply(&added.document, &Edit::RemoveRule { rule_id: "C02".into() }).unwrap();
    let again = edit::apply(&removed.document, &Edit::AddRule { title: "Four".into(), description: "4".into(), instructions: "4".into() }).unwrap();
    assert_eq!(again.new_id.as_deref(), Some("C04"));
    let moved = edit::apply(&again.document, &Edit::MoveRule { rule_id: "C04".into(), direction: Direction::Up }).unwrap();
    let ids: Vec<&str> = document::rules(&moved.document).iter().map(document::id_of).collect();
    assert_eq!(ids, ["C01", "C04", "C03"]);
    let reference = edit::apply(&moved.document, &Edit::AddReference { rule_id: "C01".into(), title: "More".into(), url: "/x".into() }).unwrap();
    assert_eq!(reference.new_id.as_deref(), Some("C01-R02"));
    let translated = edit::apply(&reference.document, &Edit::SetRuleText {
        rule_id: "C01".into(),
        locale: "es".into(),
        title: Some("Uno".into()),
        description: None,
        instructions: None,
    })
    .unwrap();
    assert_eq!(translated.document["rules"][0]["title"]["es"], json!("Uno"));
    assert_eq!(translated.document["x-note"], json!({"kept": true}));
    assert_eq!(translated.document["rules"][0]["x-rule"], json!(1));
    assert_eq!(document::validate(&translated.document), Vec::new());
    // the default locale's text cannot be cleared; another locale's can
    let clear_default = Edit::SetDocumentText { locale: "en".into(), title: Some(String::new()), provenance: None, instructions: None };
    assert!(edit::apply(&doc, &clear_default).is_err());
    let clear_es = Edit::SetDocumentText { locale: "es".into(), title: Some(String::new()), provenance: None, instructions: None };
    assert_eq!(edit::apply(&doc, &clear_es).unwrap().document["title"], json!({"en": "Rules"}));
    assert!(Edit::from_parameters("renumber", &|_| None).is_err());
    assert!(edit::apply(&doc, &Edit::RemoveLocale { locale: "en".into() }).is_err());
}

#[test]
fn the_hash_ignores_key_order_and_the_written_text_reads_back() {
    let doc = small();
    let reordered: Value = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
    assert_eq!(document::hash(&doc), document::hash(&reordered));
    let text = document::to_pretty(&doc);
    assert!(text.find("\"schemaVersion\"").unwrap() < text.find("\"rules\"").unwrap());
    assert!(text.find("\"id\": \"C01\"").unwrap() < text.find("\"title\": {\n        \"en\": \"One\"").unwrap());
    assert_eq!(document::parse(&text).unwrap(), doc);
    let shipped = shipped();
    assert_eq!(document::parse(&document::to_pretty(&shipped)).unwrap(), shipped);
}

#[test]
fn trusted_origins_are_exact_https_origins() {
    assert_eq!(origins::normalize_origin("https://Grok-The-Rock.org/").as_deref(), Ok("https://grok-the-rock.org"));
    for bad in ["http://example.org", "https://*.example.org", "https://example.org:8443", "https://example.org/path", "https://u@example.org"] {
        assert!(origins::normalize_origin(bad).is_err(), "{bad} passed");
    }
    let dir = std::env::temp_dir().join(format!("constitution-origins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let policy = origins::load(&dir);
    assert!(!policy.customized && policy.allows("https://grok-the-rock.org"));
    assert!(origins::store(&dir, &["https://example.org".to_string()], "stale").is_err());
    let stored = origins::store(&dir, &["https://example.org/".to_string()], &policy.revision).unwrap();
    assert_eq!(origins::load(&dir), stored);
    assert!(stored.allows("https://example.org") && !stored.allows("https://grok-the-rock.org"));
    std::fs::write(origins::path(&dir), "{\"origins\": [\"http://bad\"]}").unwrap();
    let broken = origins::load(&dir);
    assert!(broken.origins.is_empty() && broken.problem.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
