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

use super::{route, script_engine, with_script_engine, EntryKind, Route, ScriptEngine};
use crate::script::text::hash_text;

fn route_of(kind: EntryKind, text: &str) -> Result<Route, crate::context::Error> {
    route(kind, text, hash_text(text))
}

#[test]
fn the_setting_names_parse_and_a_thread_setting_is_put_back() {
    assert_eq!(ScriptEngine::parse("small-oxide"), Some(ScriptEngine::SmallOxide));
    assert_eq!(ScriptEngine::parse(" oxide "), Some(ScriptEngine::Oxide));
    assert_eq!(ScriptEngine::parse("rquickjs"), Some(ScriptEngine::Rquickjs));
    assert_eq!(ScriptEngine::parse("boa"), None);
    let before = script_engine();
    with_script_engine(ScriptEngine::Oxide, || assert_eq!(script_engine(), ScriptEngine::Oxide));
    assert_eq!(script_engine(), before);
    let _ = std::panic::catch_unwind(|| with_script_engine(ScriptEngine::SmallOxide, || panic!("in the body")));
    assert_eq!(script_engine(), before, "put back after a panic");
}

#[test]
fn small_oxide_routes_covered_kinds_that_pass_the_check() -> Result<(), crate::context::Error> {
    with_script_engine(ScriptEngine::SmallOxide, || {
        assert_eq!(route_of(EntryKind::Condition, "a > 1")?, Route::Oxide);
        assert_eq!(route_of(EntryKind::Value, "a.b")?, Route::Oxide);
        assert_eq!(route_of(EntryKind::FieldValue, "a.b")?, Route::Oxide);
        assert_eq!(
            route_of(EntryKind::Condition, "a = 1")?,
            Route::QuickJs,
            "fails the check"
        );
        assert_eq!(
            route_of(EntryKind::Condition, "ec.user.userId")?,
            Route::Oxide,
            "a bound ec path"
        );
        assert_eq!(
            route_of(EntryKind::Condition, "ec.transaction.isTransactionInPlace()")?,
            Route::QuickJs,
            "an unbound ec path"
        );
        assert_eq!(route_of(EntryKind::Expand, "`${a}`")?, Route::Oxide);
        assert_eq!(route_of(EntryKind::Template, "a")?, Route::QuickJs, "not covered yet");
        assert_eq!(route_of(EntryKind::Actions, "a")?, Route::QuickJs);
        Ok(())
    })
}

/// A text that fell back twice on a thread goes straight to QuickJS after, and still gives its value.
#[test]
fn a_text_that_falls_back_twice_goes_to_quickjs() -> Result<(), crate::context::Error> {
    std::thread::spawn(|| {
        with_script_engine(ScriptEngine::SmallOxide, || -> Result<(), crate::context::Error> {
            crate::script::clear_context_stack();
            crate::script::push_context_frame();
            // a Set the stack holds is QuickJS's (`oxide::convert`)
            let mut set = indexmap::IndexMap::new();
            set.insert(
                crate::script::runtime::SET_KEY.to_string(),
                moqui_lib::object::Value::List(vec![moqui_lib::object::Value::I32(1)]),
            );
            crate::script::set_context_name("held", moqui_lib::object::Value::Map(set))?;
            let text = "held.size !== null";
            assert_eq!(route_of(EntryKind::Condition, text)?, Route::Oxide);
            let fallbacks = super::oxide_fallbacks();
            for _ in 0..3 {
                // the oxide run falls back, the answer is QuickJS's
                assert!(crate::script::eval_condition(text).is_ok());
            }
            assert_eq!(
                super::oxide_fallbacks(),
                fallbacks + 2,
                "the third run did not try oxide"
            );
            assert_eq!(route_of(EntryKind::Condition, text)?, Route::QuickJs);
            Ok(())
        })
    })
    .join()
    .expect("the thread")
}

/// With the difference check on, a read-only expression that ran on oxide runs on QuickJS too: a different answer is
/// counted (and logged), the oxide answer is the caller's, and an equal one is not counted.
#[test]
fn the_difference_check_runs_an_expression_on_both_engines() -> Result<(), crate::context::Error> {
    with_script_engine(ScriptEngine::SmallOxide, || {
        super::set_thread_difference_check(Some(true));
        crate::script::clear_context_stack();
        let before = super::oxide_differences();
        assert_eq!(crate::script::eval_value("1 + 1")?, moqui_lib::object::Value::I32(2));
        assert_eq!(super::oxide_differences(), before, "the same answer");
        // the engines' Date texts differ (QuickJS adds the zone's name), but a text that reads the clock is not run
        // twice: its answers would differ by the time between the runs
        let _oxide = crate::script::eval_value("String(new Date(0))")?;
        let _quickjs = with_script_engine(ScriptEngine::Rquickjs, || {
            crate::script::eval_value("String(new Date(0))")
        })?;
        assert_eq!(super::oxide_differences(), before);
        super::set_thread_difference_check(None);
        Ok(())
    })
}

#[test]
fn rquickjs_routes_everything_to_quickjs() -> Result<(), crate::context::Error> {
    with_script_engine(ScriptEngine::Rquickjs, || {
        assert_eq!(route_of(EntryKind::Condition, "a > 1")?, Route::QuickJs);
        Ok(())
    })
}

#[test]
fn oxide_fails_a_kind_it_does_not_cover_with_its_name() {
    with_script_engine(ScriptEngine::Oxide, || {
        assert_eq!(route_of(EntryKind::Condition, "a = 1").ok(), Some(Route::Oxide));
        let err = route_of(EntryKind::Template, "a").unwrap_err();
        assert!(err.message().contains("template"), "{}", err.message());
        let err = crate::script::run_script("1").unwrap_err();
        assert!(err.message().contains("run_script"), "{}", err.message());
    });
}

#[test]
fn a_census_row_counts_routes_reasons_and_fallbacks() {
    let text = "censusRowName.length > 0";
    let hash = hash_text(text);
    let verdict = crate::script::check::verdict(text, hash, crate::script::check::Profile::Expression, &|_| false);
    crate::script::census::record(EntryKind::Condition, text, hash, Route::Oxide, &verdict, None);
    crate::script::census::record(
        EntryKind::Condition,
        text,
        hash,
        Route::QuickJs,
        &verdict,
        Some("nested in oxide"),
    );
    crate::script::census::fallback(EntryKind::Condition, hash, "a host object");
    let line = crate::script::census::census_lines()
        .into_iter()
        .find(|line| line.contains("censusRowName"))
        .expect("the row");
    let row: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(row["kind"], "eval_condition");
    assert_eq!(row["runs"], 2);
    assert_eq!(row["routes"]["oxide"], 1);
    assert_eq!(row["why_quickjs"]["nested in oxide"], 1);
    assert_eq!(row["passes"], true);
    // a fallback is recorded only while the census is on (MOQUI_SCRIPT_CENSUS), which the unit tests leave off
    assert!(row["fallbacks"].is_object());
}
