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

//! The data grid's save of `UserAccount` rows, on the in-memory facade: an existing account's identity fields are
//! refused, a new account's are not (plans/archive/Security-Review.md SEC-TLS-81).

use std::collections::BTreeMap;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;

use super::{check_change, definition, save, Change, Changes};
use crate::audit::Actor;

const ACCOUNT: &str = "moqui.security.UserAccount";

fn setup() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let mut row = facade.make_value(ACCOUNT).unwrap();
        for (name, value) in [
            ("userId", "GridTarget"),
            ("username", "grid.target"),
            ("emailAddress", "target@grid.invalid"),
        ] {
            row.set(name, Value::string(value));
        }
        row.create(&facade).unwrap();
    }
    (ctx, facade)
}

fn change(pk: &[(&str, &str)], fields: &[(&str, &str)]) -> Change {
    let map = |pairs: &[(&str, &str)]| -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
            .collect()
    };
    Change {
        pk: map(pk),
        stamp: None,
        fields: map(fields),
    }
}

/// An update of an account's email address, username or disabled flag is refused with a field error and nothing is
/// written; a new account's row may set them.
#[test]
fn the_grid_refuses_an_existing_accounts_identity_fields() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let changes = Changes {
        updates: vec![change(
            &[("userId", "GridTarget")],
            &[
                ("emailAddress", "taken@grid.invalid"),
                ("disabled", "Y"),
                ("userFullName", "Grid"),
            ],
        )],
        ..Changes::default()
    };
    let outcome = save(&facade, ACCOUNT, &changes, &Actor::default()).unwrap();
    assert!(!outcome.saved);
    let errors = &outcome.refused[0].field_errors;
    assert!(errors["emailAddress"].contains("account screens"), "{errors:?}");
    assert!(errors.contains_key("disabled"), "{errors:?}");
    assert!(!errors.contains_key("userFullName"), "{errors:?}");
    let stored = facade
        .find(ACCOUNT)
        .unwrap()
        .eq("userId", Value::string("GridTarget"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(stored.get("emailAddress"), Some(&Value::string("target@grid.invalid")));

    let entity = definition(&facade, ACCOUNT).unwrap();
    let pk_names = vec!["userId".to_string()];
    let created = change(
        &[],
        &[
            ("userId", "GridNew"),
            ("username", "grid.new"),
            ("emailAddress", "new@grid.invalid"),
        ],
    );
    assert!(check_change(&facade, &entity, &pk_names, "create", 0, &created)
        .unwrap()
        .is_none());
}
