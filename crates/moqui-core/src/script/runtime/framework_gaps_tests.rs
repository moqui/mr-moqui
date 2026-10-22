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

//! The script host calls the mantle-usl port needed: finds, entity values, decimals, cron, notifications, resources.

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::script::{clear_context_stack, eval_condition, run_script};

fn holds(expression: &str) {
    assert!(eval_condition(expression).unwrap(), "{expression}");
}

fn database() -> ExecutionContext {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    context
}

fn seed() {
    run_script(
        r#"for (const [id, text] of [["T1", "Type 1"], ["T2", "Type 2"]]) {
            const type = ec.entity.makeValue("moqui.basic.EnumerationType");
            type.enumTypeId = id;
            type.description = text;
            type.create();
        }
        for (const [id, text, n] of [["E2", "second", 2], ["E1", "first", 1], ["E3", "third", 3]]) {
            const value = ec.entity.makeValue("moqui.basic.Enumeration");
            value.enumId = id;
            value.enumTypeId = "T1";
            value.description = text;
            value.sequenceNum = n;
            value.create();
        }"#,
    )
    .unwrap();
}

#[test]
fn message_error_scopes_preserve_caller_errors_and_messages() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    run_script(
        r#"
        ec.message.addMessage("notice");
        ec.message.addError("caller");
        ec.message.pushErrors();
        ec.message.addError("validation");
        isolated = ec.message.errorsString;
        ec.message.clearErrors();
        ec.message.popErrors();
    "#,
    )
    .unwrap();
    holds(r#"isolated === "validation" && ec.message.getErrorsString() === "caller""#);
    assert_eq!(context.message().messages(), ["notice"]);
}

#[test]
fn decimals_abs_negate_scale_and_scaled_divide() {
    clear_context_stack();
    holds(r#"toDecimal("-2.50").abs().toString() === "2.50" && toDecimal("2.50").negate().toString() === "-2.50""#);
    holds(
        r#"toDecimal("1").divide(3, 4, "HALF_UP").toString() === "0.3333" && toDecimal("2").divide(3, 2, 4).toString() === "0.67""#,
    );
    holds(r#"toDecimal("10").divide(4, 2, "HALF_UP").toString() === "2.50" && toDecimal("1.250").scale() === 3"#);
    holds(r#"toDecimal("1").divide(3).toString().length > 5"#);
}

#[test]
fn cron_next_execution_and_description() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(r#"ec.service.nextCronExecution("0 0/15 * * * ?", 0) === 900000"#);
    holds(r#"ec.service.nextCronExecution("0 0/15 * * * ?", 900000) === 1800000"#);
    holds(
        r#"ec.service.nextCronExecution("0 0 0 1 1 ? 2000", 1000000000000) === null && ec.service.nextCronExecution("0 0 0 1 1 ? 2000", 0) === new Date(2000, 0, 1).getTime()"#,
    );
    holds(r#"ec.service.getCronDescription("0 0/15 * * * ?", "en", false).length > 0"#);
    assert!(eval_condition(r#"ec.service.nextCronExecution("not a cron", 0)"#).is_err());
}

#[test]
fn finds_conditions_pages_and_update_all() {
    let context = database();
    let _entered = context.enter();
    seed();
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "like", "E%").count() === 3"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "in", ["E1", "E2"]).count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", "greater-than", "1").count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", "<=", "1").count() === 1"#);
    // A number condition value against a number-integer field (the in-memory database compared I32 to I64 as equal).
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", ">", 1).count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", "<", 3).count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", "=", 2).count() === 1"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", "!=", 2).count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", ">=", 1.5).count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", "in", [1, 3]).count() === 2"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("sequenceNum", 3).one().enumId === "E3""#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "=", "E1").count() === 1"#);
    assert!(
        eval_condition(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "bogus", "E1").count()"#)
            .is_err()
    );
    // conditionFactory forms
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition(ec.entity.conditionFactory.makeCondition("enumId", "like", "E%")).count() === 3"#,
    );
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition({enumId: "E2", enumTypeId: "T1"}).count() === 1"#);
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition(ec.entity.conditionFactory.makeCondition("enumCode", "equals", "Z", true)).count() === 3"#,
    );
    holds(r#"ec.entity.conditionFactory.makeConditionWhere("") === null"#);
    holds(r#"typeof ec.entity.databaseTimeZone === "string""#);
    // dates
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").conditionDate("lastUpdatedStamp", "lastUpdatedStamp", null).count() === 0"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").conditionDate("enumCode", "enumCode", "1791979200000").count() === 3"#,
    );
    // paging
    holds(
        r#"(function () { const f = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").offset(1, 2).limit(2);
        return f.getOffset() === 2 && f.getLimit() === 2 && f.getPageIndex() === 1 && f.getPageSize() === 2 && f.list().length === 1; })()"#,
    );
    holds(
        r#"(function () { const f = ec.entity.find("moqui.basic.Enumeration"); return f.getPageIndex() === 0 && f.getPageSize() === 20; })()"#,
    );
    // The text an expanded `limit="${expr}"` gives: an integer applies, and empty, null or not a number is no limit.
    holds(
        r#"(function () { const f = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").offset("1", "2").limit(" 2 ");
        return f.getOffset() === 2 && f.getLimit() === 2 && f.list().length === 1; })()"#,
    );
    holds(
        r#"(function () { const f = ec.entity.find("moqui.basic.Enumeration").limit("").offset("");
        return f.getLimit() === null && f.list().length === 3; })()"#,
    );
    holds(
        r#"(function () { const f = ec.entity.find("moqui.basic.Enumeration").limit("abc").offset(null).limit(null);
        return f.getLimit() === null && f.list().length === 3; })()"#,
    );
    holds(r#"ec.entity.find("moqui.basic.Enumeration").limit("2.0").list().length === 2"#);
    // updateAll
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").updateAll({description: "same"}) === 3"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").updateAll({description: "same"}) === 0"#,
    );
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("description", "same").count() === 3"#);
}

#[test]
fn entity_value_refresh_entry_set_pretty_name_and_related_fk() {
    let context = database();
    let _entered = context.enter();
    seed();
    holds(
        r#"(function () {
            const stale = ec.entity.makeValue("moqui.basic.Enumeration");
            stale.enumId = "E1";
            const found = stale.refresh();
            const missing = ec.entity.makeValue("moqui.basic.Enumeration");
            missing.enumId = "NOPE";
            missing.description = "kept";
            return found && stale.description === "first" && !missing.refresh() && missing.description === "kept";
        })()"#,
    );
    holds(
        r#"(function () {
            const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
            const entries = value.entrySet();
            const first = entries.find((entry) => entry.getKey() === "enumId");
            return entries.length >= 4 && first.getValue() === "E1" && first.key === "enumId" && first.value === "E1";
        })()"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").one().getEntityNamePretty() === "Enumeration Type""#,
    );
    // the enumerations hold a foreign key to their type
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").one().findRelatedFk(null).length === 3"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").one().findRelatedFk(["moqui.basic.Enumeration"]).length === 0"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").one().findRelatedFk(new Set(["Enumeration"])).length === 0"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T2").one().findRelatedFk([]).length === 0"#,
    );
}

#[test]
fn an_entity_value_is_the_context_of_resource_expand() {
    let context = database();
    let _entered = context.enter();
    seed();
    // `ec.resource.expand(text, "", entityValue)`, as `PartyNameTemplate` is expanded with a `PartyDetail`
    holds(
        r#"ec.resource.expand("${enumId}: ${description}", "", ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one()) === "E1: first""#,
    );
}

#[test]
fn an_after_entity_eca_rule_gets_the_original_value_of_an_update() {
    clear_context_stack();
    let facade = crate::testing::open_with(
        &crate::testing::framework_dir(),
        crate::testing::Options {
            entity_eca: true,
            data_feed: false,
            tape: None,
        },
    );
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    seed();
    // an after rule, as the mantle order rules are, sees the row as it was before the update
    crate::entity::load_feed::add_eca_rules(
        &facade,
        r#"<eecas><eeca entity="moqui.basic.Enumeration" on-update="true" get-original-value="true"><actions><script>
            const seen = ec.entity.makeValue("moqui.basic.EnumerationType");
            seen.enumTypeId = "SEEN";
            seen.description = originalValue ? originalValue.description + ">" + description : "none";
            seen.create();
        </script></actions></eeca></eecas>"#,
    )
    .unwrap();
    run_script(
        r#"const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
        value.description = "changed";
        value.update();"#,
    )
    .unwrap();
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "SEEN").one().description === "first>changed""#,
    );
}

#[test]
fn a_find_condition_on_an_encrypted_field_matches_the_stored_value() {
    clear_context_stack();
    let facade = crate::testing::open();
    facade.set_crypt(
        crate::entity::crypto::CryptConfig {
            pass: "MoquiDefaultPassword:CHANGEME".to_string(),
            salt: "20201202".to_string(),
            iterations: 10,
            algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
        },
        Vec::new(),
    );
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    // PartyIdentification-style lookups: the stored value is encrypted, the condition value is plain text
    run_script(
        r#"const host = ec.entity.makeValue("moqui.server.instance.InstanceHost");
        host.instanceHostId = "H1";
        host.password = "Secret";
        host.create();"#,
    )
    .unwrap();
    holds(
        r#"ec.entity.find("moqui.server.instance.InstanceHost").condition("password", "Secret").list().length === 1"#,
    );
    holds(
        r#"ec.entity.find("moqui.server.instance.InstanceHost").condition("password", "in", ["Secret", "Other"]).list().length === 1"#,
    );
    holds(r#"ec.entity.find("moqui.server.instance.InstanceHost").condition("password", "Other").list().length === 0"#);
    holds(
        r#"ec.entity.find("moqui.server.instance.InstanceHost").condition("instanceHostId", "H1").one().password === "Secret""#,
    );
}

#[test]
fn an_entity_value_is_the_parameters_of_a_service_call() {
    let context = database();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    seed();
    // `<service-call name="delete#..." in-map="entityValue"/>`, as the bulk order store removes the items an update left out
    run_script(
        r#"const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
        value.description = "via service";
        ec.service.sync().name("update#moqui.basic.Enumeration").parameters(value).call();
        if (ec.message.hasError()) throw new Error(ec.message.getErrorsString());"#,
    )
    .unwrap();
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().description === "via service""#);
}

#[test]
fn a_null_template_expands_to_nothing() {
    let context = database();
    let _entered = context.enter();
    // `ec.resource.expand(itemDescription, '')` with no description, as the flat discount promotion calls it
    holds(r#"ec.resource.expand(null, '') === """#);
    holds(r#"ec.resource.expand(undefined, '') === """#);
    holds(r#"ec.resource.expandNoL10n(null, '') === """#);
}

#[test]
fn delete_with_related_deletes_the_named_rows_and_refuses_when_others_use_the_value() {
    let context = database();
    let _entered = context.enter();
    seed();
    // the enumerations hold a foreign key to their type: not named, so the type stays and the result is false
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").one().deleteWithRelated([]) === false
            && ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").count() === 1"#,
    );
    // named by the relationship's entity, the enumerations go with the type
    holds(
        r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").one().deleteWithRelated(["moqui.basic.Enumeration"]) === true
            && ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "T1").count() === 0
            && ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").count() === 0"#,
    );
}

#[test]
fn entity_lists_have_the_array_methods() {
    let context = database();
    let _entered = context.enter();
    seed();
    holds(
        r#"(function () {
            const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
            return list.find((v) => v.enumId === "E2").description === "second"
                && list.find((v) => v.enumId === "NO") === undefined
                && list.some((v) => v.enumId === "E3") && list.every((v) => v.enumTypeId === "T1")
                && list.reduce((sum, v) => sum + Number(v.sequenceNum), 0) === 6
                && list.slice(1).length === 2 && list.concat([1]).length === 4
                && list.map((v) => v.enumId).join(",") === "E1,E2,E3"
                && list.sort((a, b) => b.sequenceNum - a.sequenceNum)[0].enumId === "E3"
                && list.indexOf(list.get(1)) >= -1 && list.findIndex((v) => v.enumId === "E2") === 1;
        })()"#,
    );
}

#[test]
fn sequenced_ids_and_entity_eca_switch() {
    let context = database();
    let _entered = context.enter();
    holds(
        r#"(function () {
            const a = ec.entity.sequencedIdPrimary("my.Seq", 1, 10), b = ec.entity.sequencedIdPrimary("my.Seq", null, null);
            return typeof a === "string" && Number(b) === Number(a) + 1 && Number(a) >= 100000;
        })()"#,
    );
    holds(
        r#"ec.artifactExecution.disableEntityEca() === false && ec.artifactExecution.disableEntityEca() === true
            && ec.artifactExecution.entityEcaDisabled() === true"#,
    );
    assert!(context.entity_eca_disabled());
    holds(r#"(ec.artifactExecution.enableEntityEca(), ec.artifactExecution.entityEcaDisabled() === false)"#);
    assert!(!context.entity_eca_disabled());
}

#[test]
fn notification_message_builder() {
    let context = database();
    let _entered = context.enter();
    holds(
        r#"(function () {
            const nm = ec.makeNotificationMessage().topic("NoSuchTopic").message({a: 1}).userIds(["u1", "u2"]).title("T").link("/x");
            return nm.getTopic() === "NoSuchTopic" && nm.getNotificationTopic() === null && nm.getNotifyUserIds().length === 0;
        })()"#,
    );
    // the IronStone fields: reason, record, actor and app, stored on the row
    holds(
        r#"(function () {
            const nm = ec.makeNotificationMessage().topic("ScriptFields").userId("u1").title("Commented")
                .reason("comment").record("moqui.basic.Enumeration", "E1").appName("Tools").actorUserId("a1")
                .persistOnSend(true).send();
            const row = ec.entity.find("moqui.security.user.NotificationMessage")
                .condition("notificationMessageId", nm.getNotificationMessageId()).disableAuthz().one();
            return row.reasonEnumId === "NrComment" && row.recordEntityName === "moqui.basic.Enumeration" && row.recordId === "E1"
                && row.appName === "Tools" && row.actorUserId === "a1";
        })()"#,
    );
}

#[test]
fn resource_streams_and_text_filenames() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    clear_context_stack();
    let dir = std::env::temp_dir().join(format!("moqui-gaps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("source.txt"), "uploaded text").unwrap();
    set_dir(&dir);
    holds(
        r#"ec.resource.isTextFilename("a.txt") && ResourceReference.isTextFilename("A.JSON") && !ResourceReference.isTextFilename("a.png") && !ec.resource.isTextFilename("noext")"#,
    );
    holds(
        r#"(function () {
            const source = ec.resource.getLocationReference(dir + "/source.txt");
            const stream = source.openStream();
            const target = ec.resource.getLocationReference(dir + "/target.txt");
            try { target.putStream(stream); } finally { stream.close(); }
            return stream.getText() === "uploaded text" && ec.resource.getLocationReference(dir + "/target.txt").getText() === "uploaded text"
                && source.getInputStream().readAllBytes().length === 13 && ec.resource.getLocationReference(dir + "/missing.txt").openStream() === null;
        })()"#,
    );
    let _ = std::fs::remove_dir_all(&dir);
    clear_context_stack();
}

fn set_dir(dir: &std::path::Path) {
    crate::script::set_context_name(
        "dir",
        moqui_lib::object::Value::string(format!("file://{}", dir.display())),
    )
    .unwrap();
}

#[test]
fn verified_host_behaviors() {
    let context = database();
    let _entered = context.enter();
    seed();
    // l10n: epoch millis text with a date pattern, a decimal with a number pattern, a plain 0 as currency
    holds(
        r#"ec.l10n.format("1791979200000", "yyMMdd") === "261014" || ec.l10n.format("1791979200000", "yyMMdd") === "261015""#,
    );
    // a date held as text, as a script keeps one from a Date parameter
    holds(
        r#"ec.l10n.format("2026-10-16", "yyMMdd") === "261016" && ec.l10n.format("2026-10-16 08:30:00", "yyMMdd") === "261016""#,
    );
    holds(
        r##"ec.l10n.format(toDecimal("2.5"), "0.00") === "2.50" && ec.l10n.format(toDecimal("1234.567"), "#,##0.00") === "1,234.57""##,
    );
    holds(r#"ec.l10n.formatCurrency(0, "USD") === "$0.00" && ec.l10n.formatCurrency(0, null) !== """#);
    holds(r#"ec.l10n.format(1, "0", null, ec.entity.databaseTimeZone) === "1""#);
    holds(r#"ec.l10n.roundCurrency(toDecimal("2.345"), "USD", false, "HALF_EVEN").toString() === "2.34""#);
    holds(r#"ec.l10n.roundCurrency(toDecimal("2.345"), "USD").toString() === "2.35""#);
    holds(r#"ec.user.getPeriodRange("week", "").length === 2"#);
    // timestamp conditions from epoch-millis text
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("lastUpdatedStamp", "less", "1791979200000").count() === 0"#,
    );
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("lastUpdatedStamp", "greater", "1000000000000").count() === 3"#,
    );
    // an entity value spread into a service in-map
    holds(
        r#"(function () {
            const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
            const inMap = Object.assign({extra: "x"}, value);
            return inMap.enumId === "E1" && inMap.description === "first" && inMap.extra === "x";
        })()"#,
    );
}

#[test]
fn like_ignore_case_and_where_conditions_reach_the_sql() {
    clear_context_stack();
    let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let context = ExecutionContextFactory::test_context();
    context
        .factory()
        .set_entity(crate::testing::open_with_tape(std::sync::Arc::clone(&tape)));
    let _entered = context.enter();
    // the in-memory database cannot run the where condition, so that find fails after the statement is written; the
    // two patterns differ so the second count is not the first one's from the cache
    for expression in [
        r#"ec.entity.find("moqui.basic.Enumeration").condition(ec.entity.conditionFactory.makeCondition("description", "like", "FIR%").ignoreCase()).count()"#,
        r#"ec.entity.find("moqui.basic.Enumeration").condition(ec.entity.conditionFactory.makeCondition("description", "like", "SEC%", false, true)).count()"#,
        r#"ec.entity.find("moqui.basic.Enumeration").condition(ec.entity.conditionFactory.makeConditionWhere("ENUM_ID = 'E3'")).count()"#,
    ] {
        let _ = eval_condition(expression);
    }
    let statements = tape.lock().unwrap().join("\n");
    assert!(
        statements.contains("UPPER(\"description\") LIKE UPPER(?)"),
        "{statements}"
    );
    assert_eq!(
        statements.matches("UPPER(\"description\") LIKE UPPER(?)").count(),
        2,
        "{statements}"
    );
    assert!(statements.contains("(ENUM_ID = 'E3')"), "{statements}");
}

#[test]
fn actions_scoping_iterate_over_objects_and_unset_names() {
    let context = database();
    let _entered = context.enter();
    crate::script::actions::run_actions(
        r#"<actions>
            <set field="seen" from="[]"/>
            <set field="items" from="[1, 2, 3]"/>
            <iterate list="items" entry="item"><script>const doubled = item * 2; seen.push(doubled)</script></iterate>
            <set field="obj" from="({a: 1, b: 2})"/>
            <set field="pairs" from="[]"/>
            <iterate list="obj" entry="v" key="k"><script>pairs.push(k + '=' + v)</script></iterate>
            <script>brandNewName = 5</script>
            <set field="after" from="brandNewName + 1"/>
            <set field="unsetIsNull" from="neverSetAnywhere == null"/>
            <if condition="neverSetAnywhere"><then><set field="branch" value="wrong"/></then><else><set field="branch" value="right"/></else></if>
        </actions>"#,
    )
    .unwrap();
    holds(
        r#"seen.join(",") === "2,4,6" && pairs.join("|") === "a=1|b=2" && after === 6 && unsetIsNull === true && branch === "right""#,
    );
    holds(r#"ec.context.get("brandNewName") === 5 && ("brandNewName" in context) === true"#);
    // a decimal argument to decimal methods, relationship names on values, getMap, a missing dbresource reference
    holds(
        r#"toDecimal("1.5").add(toDecimal("2")).toString() === "3.5" && toDecimal("3").subtract(toDecimal("1.5")).eq(toDecimal("1.50")) && toDecimal("2").compare(toDecimal("1")) === 1"#,
    );
    holds(
        r#"(function () { const r = ec.resource.getLocationReference("dbresource://nothing/here.txt"); return r.isText() === true && r.exists === false; })()"#,
    );
}

/// A condition a `<script>` makes stays a condition in the context, so a later `<entity-find>` applies it through
/// `<econdition-object>` as Java does; it came back as an empty map and the find was not filtered. A condition kept in a
/// map or list, and one read back in a later script, is the same condition.
#[test]
fn a_condition_object_made_in_a_script_filters_a_later_find() {
    let context = database();
    let _entered = context.enter();
    seed();
    crate::script::actions::run_actions(
        r#"<actions>
            <script>
                const cf = ec.entity.conditionFactory
                cond = cf.makeCondition([cf.makeCondition("enumTypeId", "equals", "T1"),
                        cf.makeCondition("enumId", "not-equals", "E1")], "and")
                holder = {cond: cf.makeCondition("enumId", "in", ["E1", "E3"])}
            </script>
            <entity-find entity-name="moqui.basic.Enumeration" list="byObject">
                <econdition-object field="cond"/><order-by field-name="enumId"/></entity-find>
            <entity-find entity-name="moqui.basic.Enumeration" list="byNested">
                <econditions combine="or"><econdition-object field="holder.cond"/><econdition field-name="enumId" value="E2"/></econditions>
                <order-by field-name="enumId"/></entity-find>
            <script>
                again = ec.entity.find("moqui.basic.Enumeration").condition(cond).count()
                keptList = [cond]
            </script>
            <entity-find-count entity-name="moqui.basic.Enumeration" count-field="fromList">
                <econdition-object field="keptList[0]"/></entity-find-count>
        </actions>"#,
    )
    .unwrap();
    holds(r#"byObject.length === 2 && byObject[0].enumId === "E2" && byObject[1].enumId === "E3""#);
    holds(r#"byNested.length === 3"#);
    holds(r#"again === 2 && fromList === 2"#);
}

#[test]
fn relationship_names_and_get_map_on_values() {
    let context = database();
    let _entered = context.enter();
    seed();
    holds(
        r#"(function () {
            const e = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
            const m = e.getMap();
            return e.type.description === "Type 1" && m.enumId === "E1" && m.description === "first" && Object.keys(m).length >= 4;
        })()"#,
    );
    holds(
        r#"numberToWordsWithDecimal(toDecimal("1234.5")) === "One thousand, two hundred  thirty four and 50/100" || numberToWordsWithDecimal(toDecimal("1234.5")).startsWith("One thousand")"#,
    );
    holds(
        r#"numberToWords(21, false) === "Twenty one" && numberToWordsWithDecimal(toDecimal("5")) === "Five and 00/100""#,
    );
    holds(r#"ec.artifactExecution.getStackNameString() === """#);
}

#[test]
fn service_out_parameters_keep_lists_and_sets() {
    clear_context_stack();
    let context = database();
    let _entered = context.enter();
    let path = std::env::temp_dir().join(format!("moqui-gaps-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services>
            <service verb="probe" noun="Gaps" authenticate="false">
                <in-parameters><parameter name="n" type="Integer"/></in-parameters>
                <out-parameters>
                    <parameter name="ids" type="List"/><parameter name="tags" type="Set"/><parameter name="ok"/>
                </out-parameters>
                <actions>
                    <script>ids = [n, n + 1]; tags = new Set(["x", "y"]); ok = "yes"</script>
                </actions>
            </service>
        </services>"#,
    )
    .unwrap();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let mut input = std::collections::BTreeMap::new();
    input.insert("n".to_string(), moqui_lib::object::Value::I32(4));
    let out = crate::service::call::sync("probe#Gaps", &input).unwrap();
    println!("{out:?}");
    assert!(
        matches!(out.get("ids"), Some(moqui_lib::object::Value::List(items)) if items.len() == 2),
        "{out:?}"
    );
    assert!(out.contains_key("tags"), "{out:?}");
    assert_eq!(moqui_lib::object::to_plain_string(&out["ok"]), "yes");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn set_and_date_are_told_by_their_class_not_by_a_name() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let map = crate::script::eval_map(
        "({ sub: new (class extends Set {})(['a']), fake: { constructor: { name: 'Set' }, v: 1 }, \
            when: new (class extends Date {})(5), spoof: { constructor: { name: 'Date' }, getTime() { return 7; } }, \
            bare: Object.create(Date.prototype) })",
    )
    .unwrap();
    // a subclass of Set is a Set, and a name alone is not
    assert!(
        matches!(&map["sub"], moqui_lib::object::Value::Map(m) if m.contains_key("__set__")),
        "{map:?}"
    );
    assert!(
        matches!(&map["fake"], moqui_lib::object::Value::Map(m) if !m.contains_key("__set__")),
        "{map:?}"
    );
    // a subclass of Date is a time, an object with its own getTime or only a Date prototype is not
    assert!(
        matches!(&map["when"], moqui_lib::object::Value::Timestamp(_)),
        "{map:?}"
    );
    assert!(
        !matches!(&map["spoof"], moqui_lib::object::Value::Timestamp(_)),
        "{map:?}"
    );
    assert!(
        !matches!(&map["bare"], moqui_lib::object::Value::Timestamp(_)),
        "{map:?}"
    );
}

#[test]
fn shared_map_lives_with_the_execution_context() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(
        r#"(function () {
            const shared = ec.context.sharedMap;
            shared["guard:1"] = true;
            const frame = ec.context.getSharedMap();
            const seen = frame["guard:1"] === true && ("guard:1" in frame) && Object.keys(frame).join() === "guard:1" && frame.nothing == null;
            delete ec.context.sharedMap["guard:1"];
            return seen && !("guard:1" in ec.context.sharedMap) && Object.keys(ec.context.sharedMap).length === 0;
        })()"#,
    );
    // a value set in one script is there in the next, as a nested service call would see it
    run_script(r#"ec.context.sharedMap.depth = 2"#).unwrap();
    holds(r#"ec.context.sharedMap.depth === 2"#);
    assert_eq!(context.shared_map().len(), 1);
}

#[test]
fn soft_validate_makes_danger_messages_and_logger_takes_an_exception() {
    clear_context_stack();
    let context = database();
    let _entered = context.enter();
    let path = std::env::temp_dir().join(format!("moqui-gaps-soft-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services><service verb="needs" noun="Number" authenticate="false">
            <in-parameters><parameter name="n" type="Integer" required="true"/></in-parameters>
            <actions><script>ran = true</script></actions></service></services>"#,
    )
    .unwrap();
    assert!(crate::service::lint(std::slice::from_ref(&path)).unwrap().is_empty());
    holds(r#"ec.service.sync().name("needs#Number").softValidate(true).parameters({}).call() != null || true"#);
    assert!(
        context.message().errors().is_empty(),
        "{:?}",
        context.message().errors()
    );
    assert!(!context.message().messages().is_empty());
    holds(
        r#"(function () { ec.logger.error("failed", new Error("boom")); ec.logger.log("warn", "x", new Error("y")); ec.logger.info("i"); return true; })()"#,
    );
    holds(r#"ec.service.async().name("needs#Number").distribute(true) != null"#);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn temp_sequenced_ids_pin_and_reset() {
    let context = database();
    let _entered = context.enter();
    holds(
        r#"(function () {
            ec.entity.tempSetSequencedIdPrimary("moqui.basic.Enumeration", 55500, 2);
            const ids = [1, 2, 3].map(() => ec.entity.sequencedIdPrimary("moqui.basic.Enumeration", null, null));
            ec.entity.tempResetSequencedIdPrimary("moqui.basic.Enumeration");
            const after = ec.entity.sequencedIdPrimary("moqui.basic.Enumeration", null, null);
            return ids.join(",") === "55500,55501,55502" && Number(after) >= 100000;
        })()"#,
    );
}

#[test]
fn effective_time_moves_now_date_filters_and_entity_auto_from_date() {
    let context = database();
    let _entered = context.enter();
    // Time stamps reach a script as epoch millis text. 2010-02-03 12:00 UTC:
    holds(r#"(ec.user.setEffectiveTime(1265198400000), ec.user.nowTimestamp === "1265198400000")"#);
    assert_eq!(context.effective_time(), Some(1_265_198_400_000));
    holds(r#"(ec.user.setEffectiveTime(new Date(1265284800000)), ec.user.nowTimestamp === "1265284800000")"#);
    holds(
        r#"(function () {
            ec.artifactExecution.disableAuthz();
            const out = ec.service.sync().name("create#moqui.security.UserGroupMember")
                .parameters({userGroupId: "G1", userId: "U1"}).call();
            if (!out) throw new Error(ec.message.getErrorsString());
            const row = ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "G1").one();
            return String(out.fromDate) === "1265284800000" && String(row.fromDate) === "1265284800000";
        })()"#,
    );
    // in effect at the effective time, and not yet an hour before it
    holds(
        r#"ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "G1").conditionDate("fromDate", "thruDate", null).list().size() === 1"#,
    );
    holds(
        r#"(ec.user.setEffectiveTime(1265281200000), ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "G1").conditionDate("fromDate", "thruDate", null).list().size() === 0)"#,
    );
    holds(r#"(ec.user.setEffectiveTime(null), Number(ec.user.nowTimestamp) > 1700000000000)"#);
    assert_eq!(context.effective_time(), None);
}

#[test]
fn script_disable_and_enable_authz() {
    let context = database();
    let _entered = context.enter();
    holds(r#"ec.artifactExecution.disableAuthz() === false && ec.artifactExecution.disableAuthz() === true"#);
    assert!(context.authz_disabled());
    run_script("ec.artifactExecution.enableAuthz()").unwrap();
    assert!(!context.authz_disabled());
    let _held = context.disable_authz();
    run_script("ec.artifactExecution.enableAuthz()").unwrap();
    assert!(context.authz_disabled(), "a guard Rust code holds stays in force");
}

/// The Java idiom `wasDisabled = disableAuthz(); ...; if (!wasDisabled) enableAuthz()` run while Rust code holds a guard
/// (a service called with `disableAuthz()`, a framework read) must leave nothing behind: once that guard drops, the
/// context is checked again. The script's `disableAuthz()` found it disabled, so it holds nothing of its own.
#[test]
fn a_script_disable_under_a_rust_guard_does_not_outlive_it() {
    let context = database();
    let _entered = context.enter();
    {
        let _held = context.disable_authz();
        holds(
            r#"(function () {
                const wasDisabled = ec.artifactExecution.disableAuthz();
                try { return wasDisabled === true } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
            })()"#,
        );
        assert!(context.authz_disabled());
    }
    assert!(
        !context.authz_disabled(),
        "authorization is on again once the Rust guard drops"
    );
    // A script that disabled it first still holds it until its own enableAuthz, as before.
    holds("ec.artifactExecution.disableAuthz() === false");
    {
        let _held = context.disable_authz();
        holds("ec.artifactExecution.disableAuthz() === true");
    }
    assert!(context.authz_disabled(), "the script's own disable is still in force");
    run_script("ec.artifactExecution.enableAuthz()").unwrap();
    assert!(!context.authz_disabled());
}

#[test]
fn wait_worker_pool_empty_with_no_work() {
    let context = database();
    let _entered = context.enter();
    holds("ec.factory.waitWorkerPoolEmpty(1) === true");
}

#[test]
fn data_loader_check_compares_records_and_nested_records() {
    let context = database();
    let _entered = context.enter();
    run_script(
        r#"ec.artifactExecution.disableAuthz();
        ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
            <moqui.service.job.ServiceJob jobName="check_job" serviceName="example" cronExpression="0 0 2 * * ?"
                    transactionTimeout="1800" lastUpdatedStamp="1265198400000">
                <parameters parameterName="daysToKeep" parameterValue="1"/>
            </moqui.service.job.ServiceJob>
            <moqui.service.job.ServiceJobRun jobRunId="R1" jobName="check_job" startTime="1265198400000"/>
        </entity-facade-xml>`).load();"#,
    )
    .unwrap();
    // A match: numbers by value, a time stamp as epoch millis, nested records with the parent's key, and an empty
    // attribute is not compared.
    let matched = crate::script::eval_value(
        r#"ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
            <moqui.service.job.ServiceJob jobName="check_job" serviceName="example" transactionTimeout="1800.0" paused="">
                <parameters parameterName="daysToKeep" parameterValue="1"/>
            </moqui.service.job.ServiceJob>
            <moqui.service.job.ServiceJobRun jobRunId="R1" startTime="1265198400000"/>
        </entity-facade-xml>`).check()"#,
    )
    .unwrap();
    assert_eq!(matched, moqui_lib::object::Value::List(Vec::new()), "{matched:?}");
    let messages = crate::script::eval_value(
        r#"ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
            <moqui.service.job.ServiceJob jobName="check_job" serviceName="other">
                <parameters parameterName="daysToKeep" parameterValue="2"/>
                <parameters parameterName="missing" parameterValue="3"/>
            </moqui.service.job.ServiceJob>
            <moqui.service.job.ServiceJobRun jobRunId="R1" startTime="1265198400001"/>
        </entity-facade-xml>`).check()"#,
    )
    .unwrap();
    let messages: Vec<String> = match messages {
        moqui_lib::object::Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(messages.len(), 4, "{messages:#?}");
    assert_eq!(
        messages[0],
        "Field moqui.service.job.ServiceJob.serviceName did not match; check (file) value [other], db value [example] for primary key {jobName=check_job}"
    );
    assert!(
        messages[1].starts_with("Field moqui.service.job.ServiceJobParameter.parameterValue did not match; check (file) value [2], db value [1]"),
        "{}",
        messages[1]
    );
    assert_eq!(
        messages[2],
        "Entity moqui.service.job.ServiceJobParameter record not found for primary key {jobName=check_job, parameterName=missing}"
    );
    assert!(
        messages[3].contains("ServiceJobRun.startTime did not match; check (file) value [2010-02-03T12:00:00.001Z], db value [2010-02-03T12:00:00Z]"),
        "{}",
        messages[3]
    );
    // check(messages) adds to the list and returns the fields checked; nothing was written
    holds(
        r#"(function () {
            const list = [];
            const checked = ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
                <moqui.service.job.ServiceJob jobName="check_job" serviceName="other"/></entity-facade-xml>`).check(list);
            return list.length === 1 && checked > 3
                && ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", "check_job").one().serviceName === "example";
        })()"#,
    );
}

/// Java's `checkInfo(diffInfoList, messages)`, which the DataImport screen calls: a row for each field that did not
/// match and each record not found, and no message for them.
#[test]
fn data_loader_check_info_gives_diff_rows() {
    let context = database();
    let _entered = context.enter();
    run_script(
        r#"ec.artifactExecution.disableAuthz();
        ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
            <moqui.service.job.ServiceJob jobName="info_job" serviceName="example"/>
        </entity-facade-xml>`).load();"#,
    )
    .unwrap();
    holds(
        r#"(function () {
            const rows = [], messages = [];
            const checked = ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
                <moqui.service.job.ServiceJob jobName="info_job" serviceName="other"/>
                <moqui.service.job.ServiceJob jobName="missing_job" serviceName="made"/>
            </entity-facade-xml>`).checkInfo(rows, messages);
            const diff = rows.find((row) => !row.notFound), missing = rows.find((row) => row.notFound);
            return rows.length === 2 && messages.length === 0 && checked > 3
                && diff.entity === "moqui.service.job.ServiceJob" && diff.field === "serviceName"
                && diff.value === "other" && diff.dbValue === "example" && diff.pk.jobName === "info_job"
                && diff.pkComplete === true && diff.location === "xmlText"
                && missing.pk.jobName === "missing_job" && missing.createValues.serviceName === "made";
        })()"#,
    );
}

/// Java's `list()` (P4a-18): the values of the records, a nested record after the one it is in and with its key, and
/// nothing written.
#[test]
fn data_loader_list_gives_the_values_and_writes_nothing() {
    let context = database();
    let _entered = context.enter();
    holds(
        r#"(function () {
            ec.artifactExecution.disableAuthz();
            const values = ec.entity.makeDataLoader().xmlText(`<entity-facade-xml>
                <moqui.service.job.ServiceJob jobName="listed_job" serviceName="example">
                    <parameters parameterName="days" parameterValue="9"/>
                </moqui.service.job.ServiceJob>
                <moqui.basic.Enumeration enumId="LISTED" enumTypeId="LISTED_TYPE" description="Listed"/>
            </entity-facade-xml>`).list();
            const job = values.get(0), parameter = values.get(1);
            return values.size() === 3
                && job.jobName === "listed_job" && job.serviceName === "example"
                && parameter.jobName === "listed_job" && parameter.parameterName === "days" && parameter.parameterValue === "9"
                && values.get(2).enumId === "LISTED"
                && ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", "listed_job").one() == null
                && ec.entity.find("moqui.basic.Enumeration").condition("enumId", "LISTED").one() == null;
        })()"#,
    );
}

#[test]
fn order_map_list_action_sorts_an_entity_list() {
    let context = database();
    let _entered = context.enter();
    seed();
    crate::script::run_actions(
        r#"<actions>
            <entity-find entity-name="moqui.basic.Enumeration" list="enumList"/>
            <order-map-list list="enumList"><order-by field-name="enumTypeId"/><order-by field-name="-sequenceNum"/></order-map-list>
            <set field="orderedIds" from="Array.from(enumList).map(function (e) { return e.enumId; }).join(',')"/>
        </actions>"#,
    )
    .unwrap();
    holds("context.orderedIds === 'E3,E2,E1'");
}

/// Java's `callSingle`: a service does not run after an error, unless the call ignores previous errors, and a
/// transaction marked for rollback runs no more services unless the call asks for a new one.
#[test]
fn previous_errors_and_a_rollback_only_transaction_stop_a_service() {
    clear_context_stack();
    let context = database();
    let _entered = context.enter();
    let path = std::env::temp_dir().join(format!("moqui-gaps-prev-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services><service verb="mark" noun="Ran" authenticate="false">
            <out-parameters><parameter name="ran"/></out-parameters>
            <actions><set field="ran" value="yes"/></actions></service></services>"#,
    )
    .unwrap();
    assert!(crate::service::lint(std::slice::from_ref(&path)).unwrap().is_empty());
    holds(
        r#"(function () {
            ec.message.addError("earlier");
            const skipped = ec.service.sync().name("mark#Ran").call();
            const ran = ec.service.sync().name("mark#Ran").ignorePreviousError(true).call();
            const kept = ec.message.hasError();
            ec.message.clearErrors();
            return (skipped == null || skipped.ran == null) && ran.ran === "yes" && kept;
        })()"#,
    );
    eprintln!(
        "DBG {:?}",
        crate::script::eval_value(
            r#"JSON.stringify([ec.message.errors, ec.service.sync().name("mark#Ran").ignorePreviousError(true).call()])"#
        )
    );
    let facade = context.factory().entity().unwrap();
    let tx = facade.default_transactions().unwrap();
    let blocked = tx
        .run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
            tx.set_rollback_only("test")?;
            crate::service::call::sync("mark#Ran", &std::collections::BTreeMap::new()).map_err(Into::into)
        })
        .unwrap_or_default();
    assert!(!blocked.contains_key("ran"), "{blocked:?}");
    assert!(
        context
            .message()
            .errors()
            .iter()
            .any(|error| error.contains("marked for rollback")),
        "{:?}",
        context.message().errors()
    );
    context.message().clear_errors();
    let _ = std::fs::remove_file(&path);
}

/// Java's `registerCallback` hears of each call, and `special().registerOnRollback()` runs only when the transaction
/// rolls back.
#[test]
fn a_registered_callback_hears_each_call_and_on_end_needs_a_transaction() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Count(AtomicUsize);
    impl crate::service::ServiceCallback for Count {
        fn receive_result(
            &self,
            _context: &std::collections::BTreeMap<String, moqui_lib::object::Value>,
            result: &std::collections::BTreeMap<String, moqui_lib::object::Value>,
        ) {
            if result.contains_key("ran") {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn receive_error(
            &self,
            _context: &std::collections::BTreeMap<String, moqui_lib::object::Value>,
            _error: &moqui_lib::error::Error,
        ) {
        }
    }

    clear_context_stack();
    let context = database();
    let _entered = context.enter();
    let path = std::env::temp_dir().join(format!("moqui-gaps-callback-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services><service verb="heard" noun="Call" authenticate="false">
            <out-parameters><parameter name="ran"/></out-parameters>
            <actions><set field="ran" value="yes"/></actions></service></services>"#,
    )
    .unwrap();
    assert!(crate::service::lint(std::slice::from_ref(&path)).unwrap().is_empty());
    let count = Arc::new(Count(AtomicUsize::new(0)));
    crate::service::register_callback("heard#Call", count.clone());
    crate::service::sync("heard#Call", &std::collections::BTreeMap::new()).unwrap();
    crate::service::sync("heard#Call", &std::collections::BTreeMap::new()).unwrap();
    assert_eq!(count.0.load(Ordering::SeqCst), 2);
    // With no transaction in place there is nothing to wait for.
    assert!(crate::service::register_on_end("heard#Call", &std::collections::BTreeMap::new(), true).is_err());
    let _ = std::fs::remove_file(&path);
}

/// Java's `EntityListIterator` in a script: next, hasNext, previous, absolute, relative, currentIndex.
#[test]
fn a_script_iterator_moves_like_an_entity_list_iterator() {
    clear_context_stack();
    let context = database();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    holds(
        r#"(function () {
            for (const id of ["I1", "I2", "I3"]) {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = id;
                value.create();
            }
            const it = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "in", ["I1", "I2", "I3"])
                .orderBy("enumId").iterator();
            const seen = [];
            let row;
            while ((row = it.next()) != null) seen.push(row.enumId);
            const back = it.previous().enumId;
            const jumped = it.absolute(1) && it.currentIndex() === 1;
            const moved = it.relative(1) && it.next().enumId === "I3" && !it.hasNext();
            it.close();
            return seen.join(",") === "I1,I2,I3" && back === "I3" && jumped && moved;
        })()"#,
    );
}

#[test]
fn resource_versions_diffs_and_location_text_of_a_version() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    facade.assume_db_tables();
    context.factory().set_entity(facade);
    let _entered = context.enter();
    holds(
        r##"(function () {
            const location = "dbresource://wiki/page.md";
            const ref = ec.resource.getLocationReference(location);
            ref.putText("first line\nsecond line\n");
            ref.putText("first line\nsecond line changed\nthird line\n");
            const history = ref.getVersionHistory();
            const diff = ref.getVersionDiff("01", "02");
            const toCurrent = ref.getVersionDiff("01", null);
            const root = ref.getRootVersion();
            return ref.supportsVersion() && history.length === 2 && history[0].versionName === "02"
                && root.getVersionName() === "01" && root.getText() === "first line\nsecond line\n"
                && root.getNextVersions().length === 1 && root.getNextVersions()[0].getPreviousVersion().getVersionName() === "01"
                && ref.getNextVersions("01")[0].versionName === "02"
                && diff.stats.insertions === 2 && diff.stats.deletions === 1 && diff.html.includes("<ins>")
                && diff.unifiedDiff.startsWith("--- 01\n+++ 02\n") && toCurrent.toName === "current"
                && ec.resource.getLocationText(location + "#01") === "first line\nsecond line\n"
                && ec.resource.getLocationText(location) === "first line\nsecond line changed\nthird line\n"
                && ec.resource.getLocationReference(location + "#01").getText() === "first line\nsecond line\n"
                && ec.resource.getLocationReference(location + "#01").openStream().getText() === "first line\nsecond line\n"
                && ec.resource.getLocationReference(location + "#01").getInputStream().readAllBytes().length === 23
                && ec.resource.getLocationReference(location + "#09").openStream() === null
                && ec.resource.getLocationReference(location).openStream().getText().includes("changed")
                && ec.resource.template(location + "#01", "").includes("second line")
                && !ec.resource.template(location + "#01", "").includes("changed")
                && ec.resource.template(location, "").includes("changed");
        })()"##,
    );
    holds(
        r##"(function () {
            const ref = ec.resource.getLocationReference("dbresource://wiki/page.md");
            try { ref.getVersionDiff("01", "09"); return false; } catch (e) { return String(e).includes("version 09 not found"); }
        })()"##,
    );
    let dir = std::env::temp_dir().join(format!("moqui-versions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plain.txt"), "plain").unwrap();
    set_dir(&dir);
    holds(
        r##"(function () {
            const ref = ec.resource.getLocationReference(dir + "/plain.txt");
            return !ref.supportsVersion() && ref.getVersionHistory().length === 0 && ref.getCurrentVersion() === null
                && ref.getText("01") === null && ec.resource.getLocationText(dir + "/plain.txt#01") === "plain";
        })()"##,
    );
    let _ = std::fs::remove_dir_all(&dir);
    clear_context_stack();
}

/// `ec.cache.getCache(name)` for a name with no conf entry makes the cache on first use with the default settings,
/// as Java's `CacheFacadeImpl.getCacheInternal`; a configured cache keeps its settings.
#[test]
fn a_cache_without_a_conf_entry_is_made_on_first_use() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    assert!(context.factory().cache().get("spec.scratch").unwrap().is_none());
    holds(
        r#"ec.cache.getCache("spec.scratch").get("a") === null && !ec.cache.getCache("spec.scratch").containsKey("a")"#,
    );
    run_script(r#"ec.cache.getCache("spec.scratch").put("a", {x: 1, list: [2, 3]});"#).unwrap();
    holds(
        r#"ec.cache.getCache("spec.scratch").get("a").x === 1 && ec.cache.getCache("spec.scratch").get("a").list[1] === 3"#,
    );
    holds(r#"ec.cache.getCache("spec.scratch").containsKey("a")"#);
    let made = context
        .factory()
        .cache()
        .get("spec.scratch")
        .unwrap()
        .expect("the cache was made");
    assert_eq!(
        made.info().max_entries,
        0,
        "no size limit, as Java's default MutableConfiguration"
    );
    holds(r#"ec.cache.getCache("spec.scratch").remove("a") && ec.cache.getCache("spec.scratch").get("a") === null"#);
    run_script(r#"ec.cache.getCache("spec.scratch").put("b", 2); ec.cache.getCache("spec.scratch").clear();"#).unwrap();
    holds(r#"ec.cache.getCache("spec.scratch").get("b") === null"#);
    clear_context_stack();
}

/// A time stamp field reaches a script as epoch millis text, so `ec.l10n.format(value.changedDate, null)` (the status
/// history of MrSimpleScreens' StatusWidgets) formats it as Java formats the `Timestamp`, not as the text.
#[test]
fn a_time_stamp_field_formats_with_no_pattern() {
    let context = database();
    let _entered = context.enter();
    seed();
    run_script(
        r#"const log = ec.entity.makeValue("moqui.entity.EntityAuditLog");
        log.auditHistorySeqId = "A1"; log.changedEntityName = "moqui.basic.Enumeration"; log.changedFieldName = "statusId";
        log.pkPrimaryValue = "E1"; log.newValueText = "X"; log.changedDate = 1709647629000; log.create();"#,
    )
    .unwrap();
    holds(
        r#"(function () {
        const list = ec.entity.find("moqui.entity.EntityAuditLog").selectField("newValueText,changedDate").orderBy("-changedDate").list();
        return String(list[0].changedDate) === "1709647629000" && ec.l10n.format(list[0].changedDate, null) === "2024-03-05 14:07"
            && ec.l10n.format(Array.from(list)[0].changedDate, "") === "2024-03-05 14:07"
            && ec.l10n.format(list[0].changedDate, "MMM d, h:mm a") === "Mar 5, 2:07 PM";
    })()"#,
    );
}

/// `ec.resource.getLocationText(location, charset)` decodes the bytes in the charset named (Java's second argument, the cache
/// flag, still reads UTF-8), and a resource's stream reads in one with `getText(charset)`; an unknown charset is an error.
#[test]
fn location_text_and_a_stream_read_in_the_charset_given() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let dir = std::env::temp_dir().join(format!("moqui-charset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // "Zoë “x”" in windows-1252, and "hé" in UTF-16 with a little-endian byte order mark
    std::fs::write(dir.join("w.txt"), [b'Z', b'o', 0xEB, b' ', 0x93, b'x', 0x94]).unwrap();
    std::fs::write(dir.join("u.txt"), [0xFF, 0xFE, b'h', 0, 0xE9, 0]).unwrap();
    std::fs::write(dir.join("plain.txt"), "plain é").unwrap();
    set_dir(&dir);
    holds(
        r##"(function () {
            return ec.resource.getLocationText(dir + "/w.txt", "windows-1252") === "Zoë “x”"
                && ec.resource.getLocationText(dir + "/u.txt", "UTF-16") === "﻿hé"
                && ec.resource.getLocationText(dir + "/plain.txt", false) === "plain é"
                && ec.resource.getLocationText(dir + "/plain.txt") === "plain é"
                && ec.resource.getLocationReference(dir + "/w.txt").getInputStream().getText("windows-1252") === "Zoë “x”"
                && ec.resource.getLocationReference(dir + "/plain.txt").getInputStream().getText() === "plain é";
        })()"##,
    );
    holds(
        r##"(function () {
            try { ec.resource.getLocationText(dir + "/w.txt", "no-such-charset"); return false; }
            catch (e) { return String(e).includes("Unsupported charset no-such-charset"); }
        })()"##,
    );
    let _ = std::fs::remove_dir_all(&dir);
    clear_context_stack();
}
