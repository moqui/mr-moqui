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

//! A dataset field's lookup reads only a label entity and none of its secret fields (plans/Security-Review.md
//! SEC-TLS-82), on the in-memory facade.

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use super::{mappings, refusal};

/// A context on the in-memory facade whose conf is `conf`, with a `StatusItem` and two `UserAccount` rows whose
/// `disabled` is `N` and whose `currentPassword` is a hash.
fn setup(conf: &str) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        conf,
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
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

/// The finding's lookup: `UserAccount` keyed by `disabled`, showing `currentPassword`, gives no mappings, so no hash;
/// a `StatusItem` lookup and the `statusTypeId` form still label their ids.
#[test]
fn a_lookup_reads_only_a_label_entity() {
    let (ctx, _facade) = setup("");
    let _entered = ctx.enter();
    let password = config(
        json!({"lookup":{"entityName":"moqui.security.UserAccount","keyField":"disabled",
        "displayField":"currentPassword"}}),
    );
    assert_eq!(mappings(&ctx, "spec", "disabled", &password, &[json!("N")]), None);
    // by its short name too
    let short = config(json!({"lookup":{"entityName":"UserAccount","keyField":"userId","displayField":"username"}}));
    assert_eq!(mappings(&ctx, "spec", "userId", &short, &[json!("LookupA")]), None);

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

/// An entity the conf's `report@lookup-entities` names is read, but never by or for a password field, and a restricted
/// entity is refused whatever the conf names.
#[test]
fn a_listed_entity_still_refuses_secret_fields_and_restricted_entities() {
    let (ctx, _facade) = setup(
        r#"<moqui-conf><report lookup-entities="moqui.security.UserAccount, moqui.security.UserLoginKey"/></moqui-conf>"#,
    );
    let _entered = ctx.enter();
    let names = config(
        json!({"lookup":{"entityName":"moqui.security.UserAccount","keyField":"userId",
        "displayField":"username"}}),
    );
    assert_eq!(
        mappings(&ctx, "spec", "userId", &names, &[json!("LookupA")]),
        Some(vec![json!({"value":"LookupA","text":"lookup.a"})])
    );
    let shown =
        json!({"entityName":"moqui.security.UserAccount","keyField":"disabled","displayField":"currentPassword"});
    assert!(refusal(&ctx, &shown).unwrap().contains("currentPassword"));
    let keyed = json!({"entityName":"moqui.security.UserAccount","keyField":"passwordSalt","displayField":"userId"});
    assert!(refusal(&ctx, &keyed).unwrap().contains("passwordSalt"));
    // a condition on a hash would answer whether it matches
    let conditioned = json!({"entityName":"moqui.security.UserAccount","keyField":"userId","displayField":"username",
        "conditions":{"currentPassword":"hash-a"}});
    assert!(refusal(&ctx, &conditioned).unwrap().contains("currentPassword"));
    let restricted = json!({"entityName":"moqui.security.UserLoginKey","keyField":"userId","displayField":"loginKey"});
    assert!(refusal(&ctx, &restricted).unwrap().contains("not available"));
}

/// `save#Definition` refuses a dataset whose field lookup is refused, before anything is written, and saves one whose
/// lookup reads a label entity.
#[test]
fn a_dataset_save_refuses_a_bad_lookup() {
    let (ctx, facade) = setup("");
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
        facade
            .transactions_for("moqui.report.Definition")
            .unwrap()
            .run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
                crate::definitions::save_owned(&ctx, definition, "")
            })
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
