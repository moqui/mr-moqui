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

use super::log_filter::{Console, LogFilter};
use super::*;
use tracing::Level;

fn change<'a>(key: &'a str, value: &'a str, minutes: i64) -> Change<'a> {
    Change {
        key,
        value,
        minutes,
        reason: "checking a slow request",
        user: "admin",
    }
}

#[test]
fn each_type_parses_its_values_and_refuses_others() {
    let refused = |key: &str, value: &str| validate(&change(key, value, 60), 0).unwrap_err();
    let made = |key: &str, value: &str| validate(&change(key, value, 60), 0).unwrap().value;
    assert_eq!(made("logging@level", " DEBUG "), "debug");
    assert!(refused("logging@level", "trace").contains("target rule"));
    assert!(refused("logging@level", "loud").contains("not a log level"));
    assert_eq!(made(&catalog::target_key("moqui_core::service"), "trace"), "trace");
    assert!(refused(&catalog::target_key("bad target!"), "debug").contains("not a log target"));
    assert!(refused(&catalog::target_key("rsipstack::transport"), "debug").contains("WARN"));
    assert!(refused("telemetry/browser@sample-ratio", "1.5").contains("ratio"));
    assert_eq!(made("telemetry/browser@sample-ratio", "0.25"), "0.25");
    assert!(refused("telemetry/receiver@requests-per-minute", "0").contains("whole number"));
    assert_eq!(made("telemetry/receiver@enabled", "false"), "false");
    assert!(refused("telemetry/receiver@enabled", "no").contains("true or false"));
    assert!(refused("telemetry/receiver@concurrent-requests", "4").contains("not a setting"));
    assert!(refused("telemetry@enabled", "false").contains("not a setting"));
    if cfg!(feature = "telemetry") {
        assert_eq!(made("telemetry/operations@record", "declared"), "declared");
        assert!(refused("telemetry/operations@record", "all").contains("one of"));
        assert_eq!(
            made(
                "telemetry/metric[moqui.job.runs]@attributes",
                " moqui.job.name , outcome,"
            ),
            "moqui.job.name,outcome"
        );
        assert!(refused("telemetry/metric[moqui.job.runs]@attributes", "a b").contains("attribute name"));
    }
}

#[test]
fn a_change_needs_a_known_duration_and_a_reason_and_expires_within_a_day() {
    let refused = validate(&change("logging@level", "debug", 30), 0).unwrap_err();
    assert!(refused.contains("15, 60, 240 or 1440"), "{refused}");
    let mut unreasoned = change("logging@level", "debug", 60);
    unreasoned.reason = "  ";
    assert!(validate(&unreasoned, 0).unwrap_err().contains("reason"));
    let longest = validate(&change("logging@level", "debug", 1440), 1000).unwrap();
    assert_eq!(longest.expires_at, 1000 + MAX_MILLIS);
    assert_eq!(longest.set_by, "admin");
}

#[test]
fn a_remote_override_is_checked_against_this_catalog_and_the_cap() {
    let now = 1_000_000;
    let made = |key: &str, value: &str, expires_at: i64| Override {
        key: key.into(),
        value: value.into(),
        expires_at,
        set_by: "admin".into(),
        set_at: now,
        reason: "r".into(),
    };
    assert!(check_remote(&made("logging@level", "debug", now + MINUTE), now).is_ok());
    assert!(check_remote(&made("telemetry@nonesuch", "1", now + MINUTE), now).is_err());
    assert!(check_remote(&made("logging@level", "loud", now + MINUTE), now).is_err());
    assert!(check_remote(&made("logging@level", "debug", now - 1), now).is_err());
    assert!(check_remote(&made("logging@level", "debug", now + 2 * MAX_MILLIS), now).is_err());
    let round = made("logging@level", "debug", now + MINUTE);
    assert_eq!(Override::from_json(&round.to_json()), Some(round));
}

#[test]
fn a_conf_override_is_effective_and_the_on_disk_value_stays() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let key = "telemetry/receiver@requests-per-minute";
    let before = list(factory);
    let row = before.iter().find(|setting| setting.entry.key == key).unwrap();
    assert_eq!((row.on_disk.as_str(), row.source), ("600", Source::Default));
    assert!(row.active.is_none());

    let applied = set(factory, &change(key, "42", 15)).unwrap();
    let made = applied.made;
    assert_eq!((applied.on_disk.as_str(), applied.extended), ("600", false));
    assert_eq!(factory.conf_setting("telemetry/receiver", "requests-per-minute"), "42");
    assert_eq!(
        factory.conf_setting_on_disk("telemetry/receiver", "requests-per-minute"),
        ""
    );
    assert_eq!(effective(factory, key).unwrap(), "42");
    let row = list(factory)
        .into_iter()
        .find(|setting| setting.entry.key == key)
        .unwrap();
    assert_eq!((row.effective.as_str(), row.on_disk.as_str()), ("42", "600"));
    assert_eq!(row.active.as_ref(), Some(&made));

    // Extend: the same key again replaces the override with a new expiry.
    let extended = set(factory, &change(key, "42", 240)).unwrap();
    assert!(extended.extended);
    assert!(extended.made.expires_at > made.expires_at);
    assert_eq!(active(factory).len(), 1);

    assert_eq!(
        revert(factory, key, "admin").unwrap().map(|ended| ended.value),
        Some("42".into())
    );
    assert_eq!(factory.conf_setting("telemetry/receiver", "requests-per-minute"), "");
    assert_eq!(effective(factory, key).unwrap(), "600");
    assert!(
        revert(factory, key, "admin").unwrap().is_none(),
        "nothing left to revert"
    );
}

#[test]
fn a_cluster_message_applies_and_an_unknown_key_is_skipped() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let now = now_millis();
    let remote = |key: &str, value: &str| {
        json!({"op": "set", "override": {"key": key, "value": value, "expiresAt": now + 15 * MINUTE,
            "setBy": "admin", "setAt": now, "reason": "from a peer"}})
    };
    apply_message(factory, &remote("telemetry/browser@enabled", "true"));
    apply_message(factory, &remote("telemetry/browser@nonesuch", "true"));
    let overrides = active(factory);
    assert_eq!(overrides.keys().collect::<Vec<_>>(), ["telemetry/browser@enabled"]);
    assert_eq!(overrides["telemetry/browser@enabled"].set_by, "admin");
    assert_eq!(factory.conf_setting("telemetry/browser", "enabled"), "true");

    apply_message(factory, &json!({"op": "revertAll"}));
    assert!(active(factory).is_empty());
    assert_eq!(factory.conf_setting("telemetry/browser", "enabled"), "");
}

#[test]
fn an_override_reverts_at_its_expiry() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    // A peer's override carries its absolute expiry, so a short one comes this way.
    let now = now_millis();
    apply_message(
        factory,
        &json!({"op": "set", "override": {"key": "telemetry/receiver@body-limit", "value": "2048",
            "expiresAt": now + 200, "setBy": "admin", "setAt": now, "reason": "short"}}),
    );
    assert_eq!(factory.conf_setting("telemetry/receiver", "body-limit"), "2048");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while factory.conf_setting("telemetry/receiver", "body-limit") == "2048" && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(factory.conf_setting("telemetry/receiver", "body-limit"), "");
    assert!(lock(&registry(factory).state).overrides.is_empty());
}

#[test]
fn the_log_filter_takes_the_longest_target_rule_and_holds_rsipstack() {
    let mut filter = LogFilter::new(Level::INFO, Console::All);
    filter.targets = vec![
        ("moqui_core".into(), Level::WARN),
        ("moqui_core::service".into(), Level::DEBUG),
        ("rsipstack".into(), Level::TRACE),
    ];
    filter.sort_targets();
    assert!(filter.enabled("moqui_core::service::call", Level::DEBUG));
    assert!(!filter.enabled("moqui_core::service::call", Level::TRACE));
    assert!(!filter.enabled("moqui_core::entity", Level::INFO));
    assert_eq!(
        filter.least_for("moqui_core_extra"),
        Level::INFO,
        "a prefix matches at a path boundary"
    );
    assert!(filter.enabled("moqui_host::web", Level::INFO));
    assert!(
        !filter.enabled("rsipstack::transport", Level::INFO),
        "rsipstack stays at WARN"
    );
    assert!(filter.enabled("rsipstack::transport", Level::WARN));
    assert_eq!(filter.max_level(), Level::TRACE);
    assert!(filter.to_console(Level::TRACE));
    filter.console = Console::AtLeast(Level::WARN);
    assert!(!filter.to_console(Level::INFO));
    filter.console = Console::Off;
    assert!(!filter.to_console(Level::ERROR));
}

#[test]
fn logging_overrides_make_the_filter() {
    let at = |key: &str, value: &str| {
        let made = Override {
            key: key.into(),
            value: value.into(),
            expires_at: i64::MAX,
            set_by: "admin".into(),
            set_at: 0,
            reason: "r".into(),
        };
        (key.to_string(), made)
    };
    let overrides: BTreeMap<String, Override> = [
        at("logging@level", "warn"),
        at(&catalog::target_key("moqui_core::service"), "debug"),
    ]
    .into_iter()
    .collect();
    let filter = log_filter_of(&overrides);
    assert_eq!(filter.level, Level::WARN);
    assert_eq!(filter.targets, [("moqui_core::service".to_string(), Level::DEBUG)]);
    assert_eq!(filter.max_level(), Level::DEBUG);
}
