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

//! A dataset field's lookup labels only the rows the frame's ids name and reads none of their secret fields
//! (plans/Security-Review.md SEC-TLS-82), on the in-memory facade.

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use super::{mappings, refusal};

/// A context on `facade`.
fn context_on(facade: &EntityFacade) -> ExecutionContext {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    ctx.factory().set_entity(facade.clone());
    ctx
}

/// A context on the in-memory facade, with a `StatusItem` and two `UserAccount` rows whose `disabled` is `N` and whose
/// `currentPassword` is a hash.
fn setup() -> (ExecutionContext, EntityFacade) {
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    let ctx = context_on(&facade);
    let rows: [(&str, &[(&str, &str)]); 4] = [
        (
            "moqui.basic.StatusType",
            &[("statusTypeId", "LookupSpec"), ("description", "Lookup spec")],
        ),
        (
            "moqui.basic.StatusItem",
            &[
                ("statusId", "LookupOpen"),
                ("statusTypeId", "LookupSpec"),
                ("description", "Open"),
            ],
        ),
        (
            "moqui.security.UserAccount",
            &[
                ("userId", "LookupA"),
                ("username", "lookup.a"),
                ("disabled", "N"),
                ("currentPassword", "hash-a"),
            ],
        ),
        (
            "moqui.security.UserAccount",
            &[
                ("userId", "LookupB"),
                ("username", "lookup.b"),
                ("disabled", "N"),
                ("currentPassword", "hash-b"),
            ],
        ),
    ];
    {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        for (entity, fields) in rows {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(*name, Value::string(*value));
            }
            row.create(&facade).unwrap();
        }
    }
    (ctx, facade)
}

fn config(value: Json) -> Map<String, Json> {
    value.as_object().unwrap().clone()
}

/// The finding's lookup, `UserAccount` keyed by `disabled` (not its primary key) showing `currentPassword`, gives no
/// mappings, so no hash; keyed by `userId` it still shows no password; a name by the primary key, a `StatusItem` lookup
/// and the `statusTypeId` form still label their ids.
#[test]
fn a_lookup_labels_only_the_rows_the_frame_names() {
    let (ctx, _facade) = setup();
    let _entered = ctx.enter();
    let password = config(
        json!({"lookup":{"entityName":"moqui.security.UserAccount","keyField":"disabled",
        "displayField":"currentPassword"}}),
    );
    assert_eq!(mappings(&ctx, "spec", "disabled", &password, &[json!("N")]), None);
    let by_key = config(
        json!({"lookup":{"entityName":"moqui.security.UserAccount","keyField":"userId",
        "displayField":"currentPassword"}}),
    );
    assert_eq!(mappings(&ctx, "spec", "userId", &by_key, &[json!("LookupA")]), None);
    // a name by the primary key (the shipped datasets' organization, facility and product names), by short name too
    let names = config(json!({"lookup":{"entityName":"UserAccount","keyField":"userId","displayField":"username"}}));
    assert_eq!(
        mappings(&ctx, "spec", "userId", &names, &[json!("LookupA")]),
        Some(vec![json!({"value":"LookupA","text":"lookup.a"})])
    );

    let expected = Some(vec![json!({"value":"LookupOpen","text":"Open"})]);
    let status = config(
        json!({"lookup":{"entityName":"moqui.basic.StatusItem","keyField":"statusId",
        "displayField":"description","conditions":{"statusTypeId":"LookupSpec"}}}),
    );
    assert_eq!(
        mappings(&ctx, "spec", "statusId", &status, &[json!("LookupOpen")]),
        expected
    );
    let form = config(json!({"statusTypeId":"LookupSpec"}));
    assert_eq!(
        mappings(&ctx, "spec", "statusId", &form, &[json!("LookupOpen")]),
        expected
    );
}

/// Each refusal: a key that is not the single primary-key field, a password or encrypted key, display or condition
/// field, a condition that is not a plain value, a restricted entity and a view over one.
#[test]
fn a_lookup_refuses_secret_fields_other_keys_and_restricted_entities() {
    let (ctx, _facade) = setup();
    let _entered = ctx.enter();
    let refused = |lookup: Json| refusal(&ctx, &lookup).unwrap_or_default();
    assert!(refused(
        json!({"entityName":"moqui.security.UserAccount","keyField":"passwordSalt","displayField":"userId"})
    )
    .contains("primary-key"));
    assert!(refused(
        json!({"entityName":"moqui.security.UserAccount","keyField":"emailAddress","displayField":"username"})
    )
    .contains("primary-key"));
    // a condition on a hash would answer whether it matches
    assert!(refused(
        json!({"entityName":"moqui.security.UserAccount","keyField":"userId","displayField":"username",
        "conditions":{"currentPassword":"hash-a"}})
    )
    .contains("currentPassword"));
    assert!(refused(
        json!({"entityName":"moqui.security.UserAccount","keyField":"userId","displayField":"username",
        "conditions":{"username":{"op":"like","value":"%"}}})
    )
    .contains("plain value"));
    // an encrypted field of an entity generic access reads
    assert!(refused(
        json!({"entityName":"moqui.agent.AgentPushConfig","keyField":"pushConfigId","displayField":"token"})
    )
    .contains("token"));
    assert!(
        refused(json!({"entityName":"moqui.security.UserLoginKey","keyField":"loginKey","displayField":"userId"}))
            .contains("not available")
    );
    // a view whose members include a restricted entity (UserGroupMember)
    assert!(refused(
        json!({"entityName":"moqui.security.UserAccountAndGroup","keyField":"userId","displayField":"username"})
    )
    .contains("not available"));
    assert!(refusal(
        &ctx,
        &json!({"entityName":"moqui.basic.Uom","keyField":"uomId","conditions":{"uomTypeId":"UT_CURRENCY_MEASURE"}})
    )
    .is_none());
}

/// `save#Definition` refuses a dataset whose field lookup is refused, before anything is written, and saves one whose
/// lookup labels by a primary key.
#[test]
fn a_dataset_save_refuses_a_bad_lookup() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    moqui_core::user::internal_login_user("lookup.a").unwrap();
    let _authz = ctx.disable_authz();
    let dataset = |lookup: Json| {
        json!({"id":"lookup-spec","type":"dataset","title":"Lookup spec",
            "body":{"documentType":"LookupSpec","fieldDefaults":{"disabled":{"lookup":lookup}}}})
    };
    let bad = dataset(json!({"entityName":"moqui.security.UserAccount","keyField":"disabled",
        "displayField":"currentPassword"}));
    // save#Definition runs in the service's transaction
    let save = |definition: &Json| {
        facade.transactions_for("moqui.report.Definition").unwrap().run(
            moqui_core::transaction::TransactionAttribute::UseOrBegin,
            None,
            || crate::definitions::save_owned(&ctx, definition, ""),
        )
    };
    let error = save(&bad).unwrap_err().to_string();
    assert!(error.contains("fieldDefaults.disabled.lookup is refused"), "{error}");
    let saved = || {
        facade
            .find("moqui.report.Definition")
            .unwrap()
            .eq("definitionId", Value::string("lookup-spec"))
            .count()
            .unwrap()
    };
    assert_eq!(saved(), 0);
    let good = dataset(json!({"entityName":"moqui.basic.StatusItem","keyField":"statusId"}));
    save(&good).unwrap();
    assert_eq!(saved(), 1);
}

/// Every lookup of the add-on components' shipped report definitions passes [`refusal`]: run with the directory that
/// holds mr-mantle and mr-boss, `MOQUI_ADDON_DIR=runtime/components/addon cargo test -p moqui-report shipped -- --ignored`.
#[test]
#[ignore = "needs the add-on components: MOQUI_ADDON_DIR"]
fn the_shipped_definitions_lookups_pass() {
    let addon = std::path::PathBuf::from(std::env::var("MOQUI_ADDON_DIR").expect("MOQUI_ADDON_DIR"));
    let components = ["mr-mantle", "mr-boss"];
    let dirs: Vec<std::path::PathBuf> = components.iter().map(|name| addon.join(name).join("entity")).collect();
    let facade = moqui_core::testing::open_groups_with_components("logging", &dirs);
    let ctx = context_on(&facade);
    let _entered = ctx.enter();
    fn lookups(value: &Json, path: &str, out: &mut Vec<(String, Json)>) {
        match value {
            Json::Object(map) => {
                for (name, value) in map {
                    if name == "lookup" && value.is_object() {
                        out.push((path.to_string(), value.clone()));
                    } else {
                        lookups(value, &format!("{path}.{name}"), out);
                    }
                }
            }
            Json::Array(items) => items.iter().for_each(|item| lookups(item, path, out)),
            _ => {}
        }
    }
    let mut checked = 0;
    let mut failed = Vec::new();
    for component in components {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(addon.join(component).join("report"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();
        for file in files {
            let value: Json = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
            let mut found = Vec::new();
            lookups(&value, "", &mut found);
            for (path, lookup) in found {
                checked += 1;
                if let Some(reason) = refusal(&ctx, &lookup) {
                    failed.push(format!("{}{path}: {reason}", file.display()));
                }
            }
        }
    }
    println!("checked {checked} shipped lookups");
    assert!(checked > 0, "no lookups found under {}", addon.display());
    assert!(failed.is_empty(), "{failed:#?}");
}
