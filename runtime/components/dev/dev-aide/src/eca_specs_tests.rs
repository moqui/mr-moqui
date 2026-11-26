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
fn a_rules_files_spec_is_named_by_its_parts() {
    assert_eq!(rules_spec_name("service/MySite.secas.xml"), "MySiteSecas");
    assert_eq!(rules_spec_name("entity/my.Site.eecas.xml"), "MySiteEecas");
    assert_eq!(rules_spec_name("Up-App.gitecas.xml"), "UpAppGitecas");
}

#[test]
fn a_fixture_name_is_letters_digits_dashes_and_underscores() {
    assert_eq!(fixture_name(" widget-1_a ").unwrap(), "widget-1_a");
    for bad in ["", "../up", "a b", "x.json", &"n".repeat(65)] {
        assert!(fixture_name(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_rules_trigger_is_its_service_call_or_entity_operation() {
    assert_eq!(
        trigger_js("seca", "a.B.make#C", "", true).unwrap(),
        "const out = call(\"a.B.make#C\", fixture)"
    );
    assert_eq!(
        trigger_js("seca", "a.B.make#C", "", false).unwrap(),
        "ec.service.sync().name(\"a.B.make#C\").parameters(fixture).call()"
    );
    assert_eq!(
        trigger_js("eeca", "up.Widget", "create", true).unwrap(),
        "ec.entity.makeValue(\"up.Widget\").setAll(fixture).create()"
    );
    assert!(trigger_js("eeca", "up.Widget", "update", true)
        .unwrap()
        .contains("if (held.refresh()) held.setAll(fixture).update()"));
    assert!(trigger_js("eeca", "up.Widget", "delete", false)
        .unwrap()
        .ends_with("ec.entity.makeValue(\"up.Widget\").setAll(row.getPrimaryKeys()).delete()"));
    assert!(trigger_js("eeca", "up.Widget", "", true).is_err());
    assert!(trigger_js("emeca", "x", "", true).is_err());
    // a name is a string literal, whatever it holds
    assert_eq!(
        trigger_js("seca", "x\"); evil(\"", "", true).unwrap(),
        "const out = call(\"x\\\"); evil(\\\"\", fixture)"
    );
}

#[test]
fn a_rules_feature_runs_its_trigger_with_the_fixture_and_checks_what_it_did() {
    let rows = vec![json!({"entity": "up.Widget", "action": "create", "fields": [
        {"field": "widgetId", "after": "W-1"}, {"field": "note", "after": "set by the rule"}]})];
    let feature = trigger_feature(
        "the seca rule on up.WidgetServices.make#Widget against one",
        "component://my-site/test/fixtures/one.json",
        "const out = call(\"up.WidgetServices.make#Widget\", fixture)",
        &[json!("the site rule ran for W-1")],
        &rows,
    );
    assert!(
        feature.contains(
            "const fixture = JSON.parse(ec.resource.getLocationText(\"component://my-site/test/fixtures/one.json\"))"
        ),
        "{feature}"
    );
    assert!(
        feature.contains("messages.includes(\"the site rule ran for W-1\")"),
        "{feature}"
    );
    assert!(
        feature.contains("<up.Widget widgetId=\"W-1\" note=\"set by the rule\"/>"),
        "{feature}"
    );
    let nothing = trigger_feature("x", "component://s/test/fixtures/x.json", "", &[], &[]);
    assert!(nothing.contains("        messages.length == 0\n"), "{nothing}");
}

#[test]
fn a_gitecas_feature_checks_its_condition_only() {
    let feature = condition_feature(
        "the giteca rule's condition against push-1 (a spec does not replay a push)",
        "component://my-site/test/fixtures/push-1.json",
        "refName == 'refs/heads/main'",
        true,
    );
    assert!(
        feature.contains("ec.resource.condition(\"refName == 'refs/heads/main'\", null, fixture) == true"),
        "{feature}"
    );
    assert!(feature.contains("expect() {"), "{feature}");
}
