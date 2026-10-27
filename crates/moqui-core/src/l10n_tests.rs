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

use moqui_lib::object::Value;

use jiff::tz::TimeZone;

use super::{
    basic_convert, compare, convert_checked, format_currency, format_value, localize, localize_in, parse_timestamp,
    server_time_zone, server_time_zone_name, time_zone,
};
use crate::context::ExecutionContextFactory;

fn entity_context() -> crate::context::ExecutionContext {
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    context
}

fn message(context: &crate::context::ExecutionContext, original: &str, locale: &str, localized: &str) {
    let facade = context.factory().entity().expect("facade");
    let mut row = facade.make_value("moqui.basic.LocalizedMessage").expect("value");
    row.set("original", Value::string(original));
    row.set("locale", Value::string(locale));
    row.set("localized", Value::string(localized));
    row.create(&facade).expect("create");
}

#[test]
fn localize_walks_the_locale_chain_then_default() {
    let context = entity_context();
    let _entered = context.enter();
    message(&context, "Hello", "default", "Hello (default)");
    message(&context, "Hello", "fr", "Bonjour");
    message(&context, "Hello", "fr_CA", "Allo");
    message(&context, "Thing##a", "default", "Thing A");
    assert_eq!(localize_in("Hello", "fr_CA").unwrap(), "Allo");
    assert_eq!(localize_in("Hello", "fr_FR").unwrap(), "Bonjour");
    assert_eq!(localize_in("Hello", "de_DE").unwrap(), "Hello (default)");
    assert_eq!(localize_in("No Row", "de_DE").unwrap(), "No Row");
    assert_eq!(localize_in("Thing##b", "en_US").unwrap(), "Thing");
    assert_eq!(localize_in("", "en_US").unwrap(), "");
    assert_eq!(localize("Hello").unwrap(), "Hello (default)");
}

#[test]
fn a_text_over_255_characters_is_returned_as_it_is() {
    let context = entity_context();
    let _entered = context.enter();
    message(&context, &"x".repeat(255), "default", "two hundred fifty-five");
    assert_eq!(localize(&"x".repeat(255)).unwrap(), "two hundred fifty-five");
    let long = format!("{}##suffix", "y".repeat(300));
    assert_eq!(localize(&long).unwrap(), long, "unchanged, the ## suffix too");
    assert_eq!(localize_in(&"é".repeat(256), "fr").unwrap(), "é".repeat(256));
}

#[test]
fn epoch_millis_text_is_a_time_stamp_of_this_century() {
    assert_eq!(super::epoch_millis_text("1709647629000"), Some(1_709_647_629_000));
    assert_eq!(super::epoch_millis_text(" 1000000000000 "), Some(1_000_000_000_000));
    for text in [
        "",
        "100050",
        "999999999999",
        "4102444800000",
        "17096476290001",
        "-1709647629000",
        "1709647629000.5",
        "CT-REQ",
    ] {
        assert_eq!(super::epoch_millis_text(text), None, "{text}");
    }
}

#[test]
fn a_date_pattern_is_told_from_a_number_pattern() {
    for pattern in [
        "h:mm a",
        "HH:mm",
        "EEE",
        "mm:ss",
        "yyyy-MM-dd",
        "a",
        "w",
        "z",
        "d MMM 'at' h",
        "Y-ww",
    ] {
        assert!(super::is_date_pattern(pattern), "{pattern}");
    }
    for pattern in [
        "#,##0.00",
        "0",
        "0.###E0",
        "#,##0 'units'",
        "00000",
        "",
        "'hello'",
        "0.0%",
    ] {
        assert!(!super::is_date_pattern(pattern), "{pattern}");
    }
}

#[test]
fn format_covers_numbers_dates_and_currency() {
    let context = entity_context();
    let _entered = context.enter();
    // With no session zone the server's is used, as Java's JVM default, so the test names one.
    crate::user::session::set_time_zone(Some("UTC"));
    let amount = Value::Decimal("1234.125".parse().unwrap());
    assert_eq!(format_value(&amount, None).unwrap(), "1,234.125");
    assert_eq!(format_value(&amount, Some("#,##0.00")).unwrap(), "1,234.12");
    assert_eq!(format_value(&Value::Null, None).unwrap(), "");
    assert_eq!(format_value(&Value::string("as is"), Some("0")).unwrap(), "as is");
    let stamp = Value::Timestamp("2024-03-05T14:07:09Z".parse().unwrap());
    assert_eq!(format_value(&stamp, None).unwrap(), "2024-03-05 14:07");
    assert_eq!(format_currency(&amount, Some("USD"), None, false).unwrap(), "$1,234.12");
    assert_eq!(format_currency(&amount, Some("USD"), None, true).unwrap(), "1,234.12");
    assert_eq!(
        format_currency(&Value::string(""), Some("USD"), None, false).unwrap(),
        ""
    );
    assert_eq!(
        format_currency(&Value::I32(1234), Some("JPY"), None, false).unwrap(),
        "\u{a5}1,234"
    );
}

#[test]
fn parse_by_pattern_number_by_locale_and_round_by_the_currency() {
    use std::str::FromStr;
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    crate::user::session::set_time_zone(Some("America/New_York"));
    // The pattern's offset wins; without one the session zone.
    let with_offset = super::parse_timestamp_format("2024-03-05T10:20:30.000-0800", Some("yyyy-MM-dd'T'HH:mm:ss.SSSZ"));
    let Some(Value::Timestamp(stamp)) = with_offset else {
        panic!("{with_offset:?}")
    };
    assert_eq!(stamp.to_string(), "2024-03-05T18:20:30Z");
    let local = super::parse_timestamp_format("20240305 0930", Some("yyyyMMdd HHmm"));
    let Some(Value::Timestamp(stamp)) = local else {
        panic!("{local:?}")
    };
    assert_eq!(stamp.to_string(), "2024-03-05T14:30:00Z");
    assert!(matches!(
        super::parse_date_format("03/05/2024", None),
        Some(Value::Date(_))
    ));

    crate::user::session::set_locale(Some("de_DE"));
    assert_eq!(
        super::parse_number("1.234,5"),
        Some(Value::Decimal(moqui_lib::format::Decimal::from_str("1234.5").unwrap()))
    );
    crate::user::session::set_locale(None);
    let amount = Value::Decimal(moqui_lib::format::Decimal::from_str("1.23456").unwrap());
    assert_eq!(
        moqui_lib::object::to_plain_string(&super::round_currency(&amount, "BHD", false).unwrap()),
        "1.235"
    );
    assert_eq!(
        moqui_lib::object::to_plain_string(&super::round_currency(&amount, "USD", false).unwrap()),
        "1.23"
    );
    let half = Value::Decimal(moqui_lib::format::Decimal::from_str("2.125").unwrap());
    assert_eq!(
        moqui_lib::object::to_plain_string(&super::round_currency(&half, "USD", false).unwrap()),
        "2.13"
    );
    crate::user::session::set_time_zone(None);
}

#[test]
fn the_request_locale_is_the_fallback_when_the_session_has_none() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let _web = crate::web::install(Default::default());
    crate::web::set_request_locale(Some("fr-CA,fr;q=0.8,en;q=0.5"));
    assert_eq!(super::locale(), "fr_CA");
    crate::user::session::set_locale(Some("de"));
    assert_eq!(super::locale(), "de");
    crate::user::session::set_locale(None);
}

/// Timestamp text without an offset is read in the session time zone, Java's `ec.user.getTimeZone()`: the user's, else
/// the server's, which a test with no start has as UTC (MoquiDefaultConf's `default_time_zone`). A zone the session
/// names that is not known is the server's too. Offsets, millis and dates are the same in any zone.
#[test]
fn timestamp_text_converts_in_the_session_zone_else_the_servers() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let stamp = |text: &str| Value::Timestamp(text.parse::<jiff::Timestamp>().unwrap());
    let text = Value::string("2016-11-01 08:30");
    assert_eq!(time_zone(), server_time_zone());
    assert_eq!(server_time_zone(), TimeZone::UTC);
    assert_eq!(server_time_zone_name(), "UTC");
    assert_eq!(
        basic_convert(&text, "java.sql.Timestamp").value,
        stamp("2016-11-01T08:30:00Z")
    );
    crate::user::session::set_time_zone(Some("America/Chicago"));
    assert_eq!(
        basic_convert(&text, "java.sql.Timestamp").value,
        stamp("2016-11-01T13:30:00Z")
    );
    assert_eq!(convert_checked(&text, "Timestamp"), Some(stamp("2016-11-01T13:30:00Z")));
    assert!(compare(&text, "equals", &stamp("2016-11-01T13:30:00Z"), "Timestamp").unwrap());
    assert_eq!(parse_timestamp("2016-11-01"), Some(stamp("2016-11-01T05:00:00Z")));
    assert_eq!(
        basic_convert(&Value::string("2016-11-01T08:30:00Z"), "Timestamp").value,
        stamp("2016-11-01T08:30:00Z")
    );
    assert_eq!(
        basic_convert(&Value::string("2016-11-01"), "java.sql.Date").value,
        Value::Date(jiff::civil::date(2016, 11, 1))
    );
    crate::user::session::set_time_zone(Some("Not/AZone"));
    assert_eq!(time_zone(), server_time_zone());
    crate::user::session::set_time_zone(None);
}

fn dollar_placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        found.push(&rest[start..start + 2 + end + 1]);
        rest = &after[end + 1..];
    }
    found
}

fn has_word(text: &str, word: &str) -> bool {
    text.split(|ch: char| !ch.is_alphabetic()).any(|part| part == word)
}

/// The Spanish pack is data the loader can read: one row per original and locale, placeholders intact,
/// and the neutral catalog stays free of the Spain-only words.
#[test]
fn spanish_catalog_files_match_the_message_keys() {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use moqui_lib::node::MNode;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/component/mr-l10n/data/es");
    let mut seen: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut files = 0u32;
    let locked = [
        "Add",
        "Create",
        "Delete",
        "Description",
        "Find",
        "Name",
        "Status",
        "Type",
        "Update",
    ];
    for entry in std::fs::read_dir(&root).expect("l10n data directory") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("xml") {
            continue;
        }
        files += 1;
        let node = MNode::parse_file(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert!(node.has_name("entity-facade-xml"), "{}", path.display());
        assert_eq!(node.attribute("type").as_deref(), Some("es"), "{}", path.display());
        for child in node.children() {
            assert!(
                child.has_name("moqui.basic.LocalizedMessage"),
                "{} {}",
                path.display(),
                child.name()
            );
            let original = child.attribute("original").expect("original");
            let locale = child.attribute("locale").expect("locale");
            let localized = child.attribute("localized").expect("localized");
            assert!(matches!(locale.as_str(), "es" | "es_ES" | "es_MX"), "{locale}");
            assert!(original.chars().count() <= 255, "{original}");
            assert!(
                localized.chars().count() <= 255 && !localized.trim().is_empty(),
                "{original}"
            );
            // UserAuthcOtpMessage is a key. Its Spanish value is the template, which introduces ${code}.
            // A mantle name or message key is the same shape: the original has no ${}, and the Spanish
            // value is the template. A label whose original already has placeholders keeps them in order.
            let original_placeholders = dollar_placeholders(&original);
            if original == "UserAuthcOtpMessage" {
                assert!(localized.contains("${code}"), "{localized}");
            } else if !original_placeholders.is_empty() {
                assert_eq!(
                    original_placeholders,
                    dollar_placeholders(&localized),
                    "{original}"
                );
            }
            let key = (original.clone(), locale.clone());
            assert!(
                seen.insert(key, localized.clone()).is_none(),
                "duplicate {original} {locale}"
            );
            if locale == "es" {
                // A missing row returns the original, so a copy of it is not stored.
                assert_ne!(localized, original, "identity row {original}");
                assert!(!locked.contains(&original.as_str()), "{original}");
                for word in ["ordenador", "fichero", "ficheros", "vosotros", "móvil"] {
                    assert!(!has_word(&localized, word), "{original} -> {localized}");
                }
            }
        }
    }
    assert!(files >= 47, "catalog files {files}");
    let es = |original: &str| seen.get(&(original.to_string(), "es".to_string())).cloned();
    assert_eq!(es("Save").as_deref(), Some("Guardar"));
    assert_eq!(es("Invoice").as_deref(), Some("Factura"));
    assert_eq!(es("Order").as_deref(), Some("Orden"));
    assert_eq!(es("Shipment").as_deref(), Some("Envío"));
    assert_eq!(es("Germany").as_deref(), Some("Alemania"));
    assert_eq!(es("United States").as_deref(), Some("Estados Unidos"));
    assert_eq!(es("Active").as_deref(), Some("Activo"));
    assert_eq!(es("File").as_deref(), Some("Archivo"));
    assert_eq!(es("Settings").as_deref(), Some("Configuración"));
    assert_eq!(es("Mobile").as_deref(), Some("Celular"));
    assert_eq!(
        es("Value entered ({0}) could not be converted to a {1}{2,choice,0#|1# using format [}{3}{2,choice,0#|1#]}")
            .as_deref(),
        Some(
            "El valor introducido ({0}) no se pudo convertir a {1}{2,choice,0#|1# con el formato [}{3}{2,choice,0#|1#]}"
        )
    );
    assert_eq!(
        seen.get(&("File".to_string(), "es_ES".to_string())).map(String::as_str),
        Some("Fichero")
    );
    assert_eq!(
        seen.get(&("Settings".to_string(), "es_ES".to_string()))
            .map(String::as_str),
        Some("Ajustes")
    );
    assert_eq!(
        seen.get(&("Mobile".to_string(), "es_ES".to_string()))
            .map(String::as_str),
        Some("Móvil")
    );
    // es_MX falls back to es, so a copy of the es text is not stored.
    assert!(!seen.contains_key(&("File".to_string(), "es_MX".to_string())));
    for ((original, locale), localized) in &seen {
        if locale == "es_MX" {
            assert_ne!(
                seen.get(&(original.clone(), "es".to_string())).map(String::as_str),
                Some(localized.as_str()),
                "{original}"
            );
        }
    }
}
