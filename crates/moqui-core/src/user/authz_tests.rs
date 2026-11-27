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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use moqui_lib::object::Value;

use super::super::session;
use crate::context::{Authz, ExecutionContext, ExecutionContextFactory, Tarpit};
use crate::entity::{EntityFacade, EntityValue};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[test]
fn enter_marks_the_thread_until_the_guard_drops() {
    let context = ExecutionContextFactory::test_context();
    assert!(!super::context_entered());
    assert!(!ExecutionContext::is_entered());
    let outer = context.enter();
    assert!(super::context_entered());
    assert!(ExecutionContext::is_entered());
    let inner = context.enter();
    assert!(super::context_entered());
    drop(inner);
    assert!(super::context_entered());
    drop(outer);
    assert!(!super::context_entered());
    assert!(!ExecutionContext::is_entered());
}

/// A booted factory with the real conf, so artifact authorization is on, over an in-memory database.
struct World {
    _runtime: TempDir,
    _factory: std::sync::Arc<ExecutionContextFactory>,
    context: ExecutionContext,
    facade: EntityFacade,
}

fn world() -> World {
    world_with(crate::testing::open())
}

/// [`world`] over `facade`, such as one that records its SQL.
fn world_with(facade: EntityFacade) -> World {
    let runtime = temp_dir("authz-runtime");
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework");
    let factory = {
        let _lock = crate::user::session::cookie_property_lock();
        ExecutionContextFactory::boot_with_framework(runtime.path(), &framework).expect("boot")
    };
    factory.set_entity(facade.clone());
    let context = factory.new_context();
    World {
        _runtime: runtime,
        _factory: factory,
        context,
        facade,
    }
}

/// An authz record for `artifact_name`: the group member, the authz row, and the type and action.
fn grant(world: &World, id: &str, artifact_type: &str, artifact_name: &str, authz_type: &str, inherit: &str) {
    grant_member(
        world,
        id,
        artifact_type,
        artifact_name,
        authz_type,
        &[("nameIsPattern", "N"), ("inheritAuthz", inherit)],
    );
}

/// [`grant`] with the group member's other fields, such as `filterMap` or `nameIsPattern`.
fn grant_member(
    world: &World,
    id: &str,
    artifact_type: &str,
    artifact_name: &str,
    authz_type: &str,
    member: &[(&str, &str)],
) {
    let _guard = world.context.disable_authz();
    let mut fields = vec![
        ("artifactGroupId", id),
        ("artifactName", artifact_name),
        ("artifactTypeEnumId", artifact_type),
    ];
    fields.extend_from_slice(member);
    create(&world.facade, "moqui.security.ArtifactGroupMember", &fields);
    create(
        &world.facade,
        "moqui.security.ArtifactAuthz",
        &[
            ("artifactAuthzId", id),
            ("userGroupId", "ALL_USERS"),
            ("artifactGroupId", id),
            ("authzTypeEnumId", authz_type),
            ("authzActionEnumId", "AUTHZA_ALL"),
        ],
    );
}

/// Allow records for `artifact_name` (`AT_ENTITY`) for each of `actions` only (`AUTHZA_CREATE`), not inherited.
fn grant_actions(world: &World, id: &str, artifact_name: &str, actions: &[&str]) {
    let _guard = world.context.disable_authz();
    create(
        &world.facade,
        "moqui.security.ArtifactGroupMember",
        &[
            ("artifactGroupId", id),
            ("artifactName", artifact_name),
            ("artifactTypeEnumId", "AT_ENTITY"),
            ("nameIsPattern", "N"),
            ("inheritAuthz", "N"),
        ],
    );
    for action in actions {
        let authz_id = format!("{id}_{action}");
        create(
            &world.facade,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", authz_id.as_str()),
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", id),
                ("authzTypeEnumId", "AUTHZT_ALLOW"),
                ("authzActionEnumId", action),
            ],
        );
    }
}

fn create(facade: &EntityFacade, name: &str, fields: &[(&str, &str)]) {
    let mut value = facade.make_value(name).unwrap();
    for (field, text) in fields {
        value.set(*field, Value::string(*text));
    }
    value.create(facade).unwrap();
}

const ENTITY: &str = "moqui.test.TestEntity";

#[test]
fn a_second_context_entered_on_the_thread_is_the_current_one_until_it_drops() {
    let outer = ExecutionContextFactory::test_context();
    let inner = ExecutionContextFactory::test_context();
    let _outer = outer.enter();
    outer.message().add_message("outer");
    {
        let _inner = inner.enter();
        inner.message().add_message("inner");
        let seen = ExecutionContext::with_current(|ec| ec.message().messages().to_vec()).unwrap();
        assert_eq!(seen, ["inner"], "the innermost context answers");
    }
    let seen = ExecutionContext::with_current(|ec| ec.message().messages().to_vec()).unwrap();
    assert_eq!(seen, ["outer"], "the outer context is back");
}

#[test]
fn groups_loaded_while_the_generation_moves_are_not_kept() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let loaded = super::cached(
        |cache| cache.groups.clone(),
        || {
            // An authorization write lands while the groups are read.
            super::note_entity_write(Some("moqui.security.ArtifactAuthz"));
            Ok(std::sync::Arc::new(std::collections::HashSet::new()))
        },
        |cache, groups| cache.groups = Some(groups),
    )
    .unwrap();
    assert!(loaded.is_empty());
    assert!(
        super::with_cache(|cache| cache.groups.is_none()),
        "groups read under the old generation are not stored as current"
    );
}

#[test]
fn an_authorization_write_moves_the_generation_again_when_it_commits() {
    let world = world();
    let _entered = world.context.enter();
    let tx = world.facade.transactions_for("moqui.security.ArtifactAuthz").unwrap();
    assert!(tx.begin(None).unwrap());
    grant(&world, "GEN_COMMIT", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    // Membership caches still invalidate on commit, unlike Java's user authorization list.
    super::with_cache(|cache| cache.groups = Some(std::sync::Arc::new(std::collections::HashSet::new())));
    let written = super::generation();
    tx.commit().unwrap();
    assert!(super::generation() > written);
    assert!(
        super::with_cache(|cache| cache.groups.is_none()),
        "the commit drops what was read before it"
    );
}

#[test]
fn an_authorization_write_moves_the_generation_again_when_it_rolls_back() {
    let world = world();
    let _entered = world.context.enter();
    let tx = world.facade.transactions_for("moqui.security.ArtifactAuthz").unwrap();
    assert!(tx.begin(None).unwrap());
    grant(&world, "GEN_ROLLBACK", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    // The writer reloads and reads its own row, which the rollback then undoes.
    super::with_cache(|cache| cache.groups = Some(std::sync::Arc::new(std::collections::HashSet::new())));
    let written = super::generation();
    tx.rollback().unwrap();
    assert!(super::generation() > written);
    assert!(
        super::with_cache(|cache| cache.groups.is_none()),
        "the rollback drops what was read before it"
    );
}

#[test]
fn reads_and_writes_need_an_allow_record() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let row = test_row(&world.facade);
    let err = row.create(&world.facade).expect_err("create without an allow");
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::Unauthorized,
        "{err}"
    );
    let err = world
        .facade
        .find(ENTITY)
        .unwrap()
        .list()
        .expect_err("read without an allow");
    assert!(err.to_string().contains("not authorized"), "{err}");
    assert!(world.facade.find(ENTITY).unwrap().count().is_err());

    grant(&world, "READ_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    session::set_user("nobody", "nobody");
    world.facade.find(ENTITY).unwrap().list().expect("read after the allow");
    row.create(&world.facade).expect("the allow is for all actions");

    let guard = world.context.disable_authz();
    let found = world
        .facade
        .find(ENTITY)
        .unwrap()
        .eq("testId", Value::string("T-DENY"))
        .list()
        .unwrap();
    drop(guard);
    assert_eq!(found.len(), 1);
}

#[test]
fn an_iterator_is_authorized_as_a_list_is() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let err = world
        .facade
        .find(ENTITY)
        .unwrap()
        .iterator()
        .err()
        .expect("iterate without an allow");
    assert!(err.to_string().contains("not authorized"), "{err}");
    world
        .facade
        .find(ENTITY)
        .unwrap()
        .disable_authz()
        .iterator()
        .expect("a find that disables authorization")
        .close()
        .unwrap();
}

#[test]
fn a_dynamic_view_needs_view_authorization_on_each_member() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let text = |text: &str| Value::string(text);
    let map = |pairs: Vec<(&str, Value)>| {
        Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
    };
    let members = Value::List(vec![
        map(vec![
            ("entity-alias", text("FOO")),
            ("entity-name", text("moqui.test.Foo")),
        ]),
        map(vec![
            ("entity-alias", text("BAR")),
            ("entity-name", text("moqui.test.Bar")),
            ("join-from-alias", text("FOO")),
            ("key-map", map(vec![("fooId", text("fooId"))])),
        ]),
    ]);
    let parameters: std::collections::BTreeMap<String, Value> = [
        ("member-entity".to_string(), members),
        ("alias-all".to_string(), map(vec![("entity-alias", text("FOO"))])),
    ]
    .into_iter()
    .collect();
    let view = || world.facade.dynamic_view("dynamic.FooBarAuthz", &parameters);
    let err = view().expect_err("no member is allowed");
    assert!(err.to_string().contains("not authorized"), "{err}");
    grant(&world, "DV_FOO", "AT_ENTITY", "moqui.test.Foo", "AUTHZT_ALLOW", "N");
    session::set_user("nobody", "nobody");
    let err = view().expect_err("the joined member is not allowed");
    assert!(err.to_string().contains("moqui.test.Bar"), "{err}");
    grant(&world, "DV_BAR", "AT_ENTITY", "moqui.test.Bar", "AUTHZT_ALLOW", "N");
    session::set_user("nobody", "nobody");
    assert!(view().expect("both members are allowed").is_empty());
}

#[test]
fn deny_beats_allow_and_always_beats_deny() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    grant(&world, "ALLOW_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    grant(&world, "DENY_TEST", "AT_ENTITY", ENTITY, "AUTHZT_DENY", "N");
    let err = world.facade.find(ENTITY).unwrap().list().expect_err("deny wins");
    assert!(err.to_string().contains("deny record"), "{err}");
    grant(&world, "ALWAYS_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALWAYS", "N");
    session::set_user("nobody", "nobody");
    world.facade.find(ENTITY).unwrap().list().expect("always wins");
}

/// Rows created together are authorized as their creates are, by the entity's full name though the call gives its short
/// one: allowed, they are written; with a deny on the full name, none is.
#[test]
fn rows_created_together_are_authorized_by_the_full_name() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let rows = |ids: &[&str]| {
        ids.iter()
            .map(|id| {
                let mut row = world.facade.make_value(ENTITY).unwrap();
                row.set("testId", Value::string(*id));
                row
            })
            .collect::<Vec<_>>()
    };
    let count = || {
        let _guard = world.context.disable_authz();
        world.facade.find(ENTITY).unwrap().count().unwrap()
    };
    let err = world
        .facade
        .create_values("TestEntity", rows(&["N1"]))
        .expect_err("no allow");
    assert!(err.to_string().contains("not authorized"), "{err}");
    grant(&world, "ALLOW_ROWS", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    session::set_user("nobody", "nobody");
    world
        .facade
        .create_values("TestEntity", rows(&["A1", "A2"]))
        .expect("allowed");
    assert_eq!(count(), 2);
    grant(&world, "DENY_ROWS", "AT_ENTITY", ENTITY, "AUTHZT_DENY", "N");
    session::set_user("nobody", "nobody");
    let err = world
        .facade
        .create_values("TestEntity", rows(&["D1", "D2"]))
        .expect_err("deny wins");
    assert!(err.to_string().contains("deny record"), "{err}");
    assert_eq!(count(), 2, "nothing written");
}

/// A filterMap that is not a valid expression, so it cannot be evaluated.
const BAD_FILTER: &str = "[ this is not an expression";

fn filter_parameters() -> std::collections::BTreeMap<String, Value> {
    [("orderId".to_string(), Value::string("O1"))].into_iter().collect()
}

#[test]
fn a_deny_whose_filter_map_fails_denies_despite_an_allow() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let service = "test.Filtered#Row";
    grant(&world, "FILTER_ALLOW", "AT_SERVICE", service, "AUTHZT_ALLOW", "N");
    let parameters = filter_parameters();
    world
        .context
        .push_checked(
            "AT_SERVICE",
            service,
            "all",
            Authz::Required,
            Tarpit::Skip,
            Some(&parameters),
        )
        .expect("allowed before the deny");
    world.context.pop().unwrap();
    grant_member(
        &world,
        "FILTER_DENY",
        "AT_SERVICE",
        service,
        "AUTHZT_DENY",
        &[("filterMap", BAD_FILTER)],
    );
    session::set_user("nobody", "nobody");
    let err = world
        .context
        .push_checked(
            "AT_SERVICE",
            service,
            "all",
            Authz::Required,
            Tarpit::Skip,
            Some(&parameters),
        )
        .expect_err("the unevaluable deny applies");
    assert!(err.to_string().contains("deny record"), "{err}");
}

#[test]
fn a_deny_whose_name_pattern_is_invalid_denies() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let service = "test.Pattern#Row";
    grant(&world, "PATTERN_ALLOW", "AT_SERVICE", service, "AUTHZT_ALLOW", "N");
    grant_member(
        &world,
        "PATTERN_DENY",
        "AT_SERVICE",
        "test.(",
        "AUTHZT_DENY",
        &[("nameIsPattern", "Y")],
    );
    let err = world
        .context
        .push_checked("AT_SERVICE", service, "all", Authz::Required, Tarpit::Skip, None)
        .expect_err("the invalid deny pattern applies");
    assert!(err.to_string().contains("deny record"), "{err}");
}

#[test]
fn an_allow_whose_filter_map_fails_does_not_allow() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let service = "test.FilteredAllow#Row";
    grant_member(
        &world,
        "FILTER_BAD_ALLOW",
        "AT_SERVICE",
        service,
        "AUTHZT_ALLOW",
        &[("filterMap", BAD_FILTER)],
    );
    let parameters = filter_parameters();
    let err = world
        .context
        .push_checked(
            "AT_SERVICE",
            service,
            "all",
            Authz::Required,
            Tarpit::Skip,
            Some(&parameters),
        )
        .expect_err("the unevaluable allow does not apply");
    assert!(err.to_string().contains("not authorized"), "{err}");
    grant_member(
        &world,
        "FILTER_GOOD_ALLOW",
        "AT_SERVICE",
        service,
        "AUTHZT_ALLOW",
        &[("filterMap", "{orderId: 'O1'}")],
    );
    session::set_user("nobody", "nobody");
    world
        .context
        .push_checked(
            "AT_SERVICE",
            service,
            "all",
            Authz::Required,
            Tarpit::Skip,
            Some(&parameters),
        )
        .expect("a matching filterMap allows");
    world.context.pop().unwrap();
}

#[test]
fn an_inheritable_service_allow_covers_the_entity_operations_it_makes() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    grant(&world, "SVC_TEST", "AT_SERVICE", "test.Make#Row", "AUTHZT_ALLOW", "Y");
    // Without the service frame the entity is not allowed.
    assert!(test_row(&world.facade).create(&world.facade).is_err());
    world
        .context
        .push_checked(
            "AT_SERVICE",
            "test.Make#Row",
            "all",
            Authz::Required,
            Tarpit::Skip,
            None,
        )
        .expect("the service is allowed");
    test_row(&world.facade)
        .create(&world.facade)
        .expect("the entity write inherits the service's allow");
    world.facade.find(ENTITY).unwrap().list().expect("so does a read");
    world.context.pop().unwrap();
    assert!(test_row(&world.facade).create(&world.facade).is_err());

    // The same allow without inheritance covers only the service.
    grant(
        &world,
        "NOINH_TEST",
        "AT_SERVICE",
        "test.Other#Row",
        "AUTHZT_ALLOW",
        "N",
    );
    session::set_user("nobody", "nobody");
    world
        .context
        .push_checked(
            "AT_SERVICE",
            "test.Other#Row",
            "all",
            Authz::Required,
            Tarpit::Skip,
            None,
        )
        .expect("the service is allowed");
    assert!(world.facade.find(ENTITY).unwrap().list().is_err());
    world.context.pop().unwrap();
    // A deny below the stack keeps a plain allow from applying.
}

#[test]
fn a_deny_on_the_stack_stops_a_plain_allow_above_it() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    grant(&world, "DENYSVC", "AT_SERVICE", "test.Denied#Row", "AUTHZT_DENY", "N");
    // A denied frame is not pushed, so the deny is seen as the explicit-deny error.
    let err = world
        .context
        .push_checked(
            "AT_SERVICE",
            "test.Denied#Row",
            "all",
            Authz::Required,
            Tarpit::Skip,
            None,
        )
        .expect_err("denied");
    assert!(err.to_string().contains("deny record"), "{err}");
    // Not requiring authz pushes it anyway, carrying the deny.
    world
        .context
        .push_checked(
            "AT_SERVICE",
            "test.Denied#Row",
            "all",
            Authz::NotRequired,
            Tarpit::Skip,
            None,
        )
        .expect("pushed without needing authz");
    grant(&world, "ENT_ALLOW", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    session::set_user("nobody", "nobody");
    assert!(
        world.facade.find(ENTITY).unwrap().list().is_err(),
        "the allow does not apply under a deny frame"
    );
    world.context.pop().unwrap();
    world
        .facade
        .find(ENTITY)
        .unwrap()
        .list()
        .expect("allowed once the deny frame is gone");
}

#[test]
fn authorization_checks_stay_cached_until_the_user_changes_as_in_java() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let original = {
        let _guard = world.context.disable_authz();
        super::user_checks(&world.facade).unwrap()
    };
    assert!(original.is_empty());
    grant(&world, "LATE_DENY", "AT_SERVICE", "test.Cached#Row", "AUTHZT_DENY", "N");
    let retained = {
        let _guard = world.context.disable_authz();
        super::user_checks(&world.facade).unwrap()
    };
    assert!(
        std::sync::Arc::ptr_eq(&original, &retained),
        "seed writes must not reload the user's checks"
    );
    session::set_user("other", "other");
    let err = world
        .context
        .push_checked(
            "AT_SERVICE",
            "test.Cached#Row",
            "all",
            Authz::Required,
            Tarpit::Skip,
            None,
        )
        .expect_err("a new user loads and enforces the deny");
    assert!(err.to_string().contains("deny record"), "{err}");
}

#[test]
fn a_new_allow_is_seen_after_refreshing_the_user() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    assert!(world.facade.find(ENTITY).unwrap().list().is_err());
    grant(&world, "LATE_TEST", "AT_ENTITY", ENTITY, "AUTHZT_ALLOW", "N");
    assert!(
        world.facade.find(ENTITY).unwrap().list().is_err(),
        "the current user's checks stay cached"
    );
    session::set_user("nobody", "nobody");
    world
        .facade
        .find(ENTITY)
        .unwrap()
        .list()
        .expect("the cached checks were reloaded");
}

/// The check an upsert makes asks whether the user may create and update the entity without pushing a frame: a refusal
/// is an answer, not an `ArtifactAuthzFailure`.
#[test]
fn an_entity_permitted_check_records_no_failure() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    assert!(!super::entity_permitted(ENTITY, "create", true).unwrap());
    assert!(
        super::entity_permitted(ENTITY, "create", false).unwrap(),
        "authorize-skip covers it"
    );
    {
        let _guard = world.context.disable_authz();
        let failures = world
            .facade
            .find("moqui.security.ArtifactAuthzFailure")
            .unwrap()
            .eq("artifactName", Value::string(ENTITY))
            .count()
            .unwrap();
        assert_eq!(failures, 0);
    }
    grant_actions(&world, "PERMIT_TEST", ENTITY, &["AUTHZA_CREATE"]);
    session::set_user("nobody", "nobody");
    assert!(super::entity_permitted(ENTITY, "create", true).unwrap());
    assert!(!super::entity_permitted(ENTITY, "update", true).unwrap());
}

/// With authorization on, a create-or-update is one upsert where the user may both create and update the entity
/// (Parity-Diffs, atomic create-or-update). Where only one is allowed it looks for the row first, and the create or
/// the update it then makes is checked as before.
#[test]
fn create_or_update_is_one_upsert_where_create_and_update_are_both_allowed() {
    let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let world = world_with(crate::testing::open_with_tape(std::sync::Arc::clone(&tape)));
    let _entered = world.context.enter();
    let entity_sql = |tape: &std::sync::Mutex<Vec<String>>| -> Vec<String> {
        let mut tape = tape.lock().unwrap_or_else(|err| err.into_inner());
        std::mem::take(&mut *tape)
            .into_iter()
            .filter(|sql| sql.contains("\"test_entity\""))
            .collect()
    };
    let value = |id: &str| {
        let mut row = world.facade.make_value(ENTITY).unwrap();
        row.set("testId", Value::string(id));
        row.set("testMedium", Value::string("stored"));
        row
    };

    grant_actions(&world, "ONE_SIDE", ENTITY, &["AUTHZA_CREATE", "AUTHZA_VIEW"]);
    session::set_user("nobody", "nobody");
    entity_sql(&tape);
    value("T-ONE-SIDE")
        .create_or_update(&world.facade)
        .expect("a create is allowed");
    let sql = entity_sql(&tape);
    assert!(sql.first().is_some_and(|sql| sql.starts_with("SELECT")), "{sql:?}");
    assert!(sql.iter().any(|sql| sql.ends_with("DO NOTHING")), "{sql:?}");
    assert!(!sql.iter().any(|sql| sql.contains("DO UPDATE")), "{sql:?}");

    grant_actions(&world, "BOTH_SIDES", ENTITY, &["AUTHZA_UPDATE"]);
    session::set_user("nobody", "nobody");
    entity_sql(&tape);
    for _ in 0..2 {
        value("T-BOTH")
            .create_or_update(&world.facade)
            .expect("create and update are allowed");
    }
    let sql = entity_sql(&tape);
    assert_eq!(sql.len(), 2, "{sql:?}");
    assert!(
        sql.iter()
            .all(|sql| sql.starts_with("INSERT") && sql.contains("DO UPDATE SET")),
        "{sql:?}"
    );
}

#[test]
fn a_failed_check_leaves_an_authz_failure_row() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    assert!(world.facade.find(ENTITY).unwrap().list().is_err());
    let _guard = world.context.disable_authz();
    let failures = world
        .facade
        .find("moqui.security.ArtifactAuthzFailure")
        .unwrap()
        .eq("artifactName", Value::string(ENTITY))
        .list()
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures.values()[0].get("isDeny").and_then(Value::as_str), Some("N"));
}

#[test]
fn a_service_over_its_hit_limit_is_locked() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    grant(&world, "TARPIT_SVC", "AT_SERVICE", "test.Busy#Row", "AUTHZT_ALLOW", "N");
    {
        let _guard = world.context.disable_authz();
        create(
            &world.facade,
            "moqui.security.ArtifactTarpit",
            &[
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "TARPIT_SVC"),
                ("maxHitsCount", "2"),
                ("maxHitsDuration", "60"),
                ("tarpitDuration", "120"),
            ],
        );
    }
    for _ in 0..2 {
        world
            .context
            .push_checked(
                "AT_SERVICE",
                "test.Busy#Row",
                "all",
                Authz::Required,
                Tarpit::Count,
                None,
            )
            .expect("under the limit");
        world.context.pop().unwrap();
    }
    let err = world.context.push_checked(
        "AT_SERVICE",
        "test.Busy#Row",
        "all",
        Authz::Required,
        Tarpit::Count,
        None,
    );
    assert!(err.is_ok(), "the third hit exceeds the limit and sets the lock");
    world.context.pop().unwrap();
    let err = world
        .context
        .push_checked(
            "AT_SERVICE",
            "test.Busy#Row",
            "all",
            Authz::Required,
            Tarpit::Count,
            None,
        )
        .expect_err("locked");
    assert_eq!(err.kind(), crate::context::ErrorKind::Tarpit, "{err}");
    // The seconds go to the response as Retry-After, as Java's ArtifactTarpitException carries them.
    assert!(err.retry_after().is_some_and(|seconds| seconds > 0), "{err}");
}

/// SEC-WEB-14: with nobody signed in the tarpit counts and locks per client address (an IPv6 one by its /64), so one
/// client past the limit does not lock the artifact for every signed-out visitor.
#[test]
fn an_anonymous_tarpit_lock_is_the_client_addresss() {
    let subject = super::anonymous_tarpit_subject;
    assert_eq!(subject(""), "");
    assert!(subject("203.0.113.5").starts_with("_ANON_") && subject("203.0.113.5").len() == 22);
    assert_ne!(subject("203.0.113.5"), subject("203.0.113.6"));
    assert_eq!(subject("::ffff:203.0.113.5"), subject("203.0.113.5"));
    assert_eq!(subject("2001:db8:1:2:aaaa::1"), subject("2001:db8:1:2:bbbb::9"));
    assert_ne!(subject("2001:db8:1:2::1"), subject("2001:db8:1:3::1"));

    let world = world();
    let _entered = world.context.enter();
    let service = "test.Anonymous#Row";
    {
        let _guard = world.context.disable_authz();
        grant(&world, "TARPIT_ANON", "AT_SERVICE", service, "AUTHZT_ALLOW", "N");
        create(
            &world.facade,
            "moqui.security.ArtifactTarpit",
            &[
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "TARPIT_ANON"),
                ("maxHitsCount", "1"),
                ("maxHitsDuration", "60"),
                ("tarpitDuration", "120"),
            ],
        );
    }
    let hit = |client: &str| {
        let _web = crate::web::install(Default::default());
        let mut request = crate::web::WebRequest::new("POST", "/x");
        request.peer_ip = client.to_string();
        crate::web::set_request_info(&request);
        let result = world
            .context
            .push_checked("AT_SERVICE", service, "all", Authz::NotRequired, Tarpit::Count, None);
        if result.is_ok() {
            world.context.pop().unwrap();
        }
        result.err().map(|err| err.kind())
    };
    assert_eq!(hit("203.0.113.5"), None);
    assert_eq!(hit("203.0.113.5"), None, "the second hit passes the limit and sets the lock");
    assert_eq!(hit("203.0.113.5"), Some(crate::context::ErrorKind::Tarpit));
    assert_eq!(hit("203.0.113.6"), None, "another client is not locked");
}

/// plans/archive/TX-Cache.md: the tarpit's lock rows and the check views are read once, not on every counted hit or every new
/// session (a stateless client starts one per request); a lock written moves the authorization generation, so the next
/// hit reads the locks again and is refused.
#[test]
fn tarpit_locks_and_check_views_are_read_once_until_a_lock_is_written() {
    let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let world = world_with(crate::testing::open_with_tape(std::sync::Arc::clone(&tape)));
    let _entered = world.context.enter();
    let reads = |table: &str| {
        let needle = format!("\"{table}\"");
        let tape = tape.lock().unwrap_or_else(|err| err.into_inner());
        tape.iter()
            .filter(|sql| sql.starts_with("SELECT") && sql.contains(&needle))
            .count()
    };
    session::set_user("nobody", "nobody");
    grant(
        &world,
        "TARPIT_ONCE",
        "AT_SERVICE",
        "test.Once#Row",
        "AUTHZT_ALLOW",
        "N",
    );
    {
        let _guard = world.context.disable_authz();
        create(
            &world.facade,
            "moqui.security.ArtifactTarpit",
            &[
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "TARPIT_ONCE"),
                ("maxHitsCount", "3"),
                ("maxHitsDuration", "60"),
                ("tarpitDuration", "120"),
            ],
        );
    }
    let hit = || {
        let pushed = world.context.push_checked(
            "AT_SERVICE",
            "test.Once#Row",
            "all",
            Authz::Required,
            Tarpit::Count,
            None,
        );
        if pushed.is_ok() {
            world.context.pop().unwrap();
        }
        pushed
    };
    // The generation is a process static: an authorization write of another test on another thread moves it too, and
    // each such move may make a hit here read the locks again, the first hit's included (a load that a move overlapped
    // is not kept). Those are allowed for, counted from before the first hit; this thread's are none.
    let (generation_at, moved_at) = (super::generation(), super::moved_here());
    hit().expect("first hit");
    let (locks, checks) = (reads("artifact_tarpit_lock"), reads("artifact_tarpit"));
    assert!(locks >= 1, "the first hit reads the locks");
    // A new session (as a stateless client's next request) and two more hits read neither again.
    session::set_user("nobody", "nobody");
    hit().expect("second hit");
    hit().expect("third hit");
    let others = (super::generation() - generation_at) - (super::moved_here() - moved_at);
    let others = usize::try_from(others).unwrap_or(usize::MAX);
    assert!(
        reads("artifact_tarpit_lock") <= locks + others.min(2),
        "the locks are kept in memory: {} reads after {locks}, {others} moves by other threads",
        reads("artifact_tarpit_lock")
    );
    if others == 0 {
        assert_eq!(
            reads("artifact_tarpit"),
            checks,
            "the tarpit checks come from the entity cache"
        );
    }
    // The fourth hit is over the limit and writes a lock, which moves the generation: the next hit reads it.
    hit().expect("the hit that sets the lock");
    let err = hit().expect_err("locked");
    assert_eq!(err.kind(), crate::context::ErrorKind::Tarpit, "{err}");
    assert!(reads("artifact_tarpit_lock") > locks);
}

#[test]
fn a_hit_older_than_max_hits_duration_does_not_count() {
    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let service = "test.Window#Row";
    grant(&world, "TARPIT_WIN", "AT_SERVICE", service, "AUTHZT_ALLOW", "N");
    {
        let _guard = world.context.disable_authz();
        create(
            &world.facade,
            "moqui.security.ArtifactTarpit",
            &[
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "TARPIT_WIN"),
                ("maxHitsCount", "1"),
                ("maxHitsDuration", "60"),
                ("tarpitDuration", "120"),
            ],
        );
    }
    // One hit, two minutes old. maxHitsCount is 1, so a hit inside the 60s window would lock the next call.
    let key = format!("nobody@AT_SERVICE:{service}");
    let stale = crate::user::now_millis() - 120_000;
    world.context.factory().record_tarpit_hit(&key, stale, 60_000);
    world
        .context
        .push_checked("AT_SERVICE", service, "all", Authz::Required, Tarpit::Count, None)
        .expect("the stale hit is outside the window");
    world.context.pop().unwrap();
    {
        let _guard = world.context.disable_authz();
        let locks = world
            .facade
            .find("moqui.security.ArtifactTarpitLock")
            .unwrap()
            .eq("artifactName", Value::string(service))
            .list()
            .unwrap();
        assert!(locks.is_empty(), "the old hit must not set the lock");
    }
    world
        .context
        .push_checked("AT_SERVICE", service, "all", Authz::Required, Tarpit::Count, None)
        .expect("the next call inside the window still succeeds");
    world.context.pop().unwrap();
}

#[test]
fn login_works_with_authorization_on_and_no_entity_allows() {
    let world = world();
    let _entered = world.context.enter();
    {
        let _guard = world.context.disable_authz();
        let mut user = world.facade.make_value("moqui.security.UserAccount").unwrap();
        user.set("userId", Value::string("U1"));
        user.set("username", Value::string("ada"));
        user.set(
            "currentPassword",
            Value::string(super::super::password::hash_password("Secret1!").unwrap()),
        );
        user.set("passwordHashType", Value::string("argon2"));
        user.set("successiveFailedLogins", Value::I64(2));
        user.create(&world.facade).unwrap();
    }
    // Authorization is on and the caller is anonymous: the login must still read and update.
    super::super::login_user("ada", "Secret1!").expect("login");
    assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    let _guard = world.context.disable_authz();
    let user = world
        .facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("userId", Value::string("U1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(
        user.get("successiveFailedLogins")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("0")
    );
}

fn test_row(facade: &EntityFacade) -> EntityValue {
    let mut row = facade.make_value(ENTITY).unwrap();
    row.set("testId", Value::string("T-DENY"));
    row
}

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(label: &str) -> TempDir {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("moqui-{label}-{}-{nanos}-{seq}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir(path)
}

/// The security proof of OWASP ASVS 5.0.0 v5.0.0-16.3.4 (docs/security/proofs/V16-Security-Logging-and-Error-Handling.md),
/// in-process, the part of a security control that fails: an authorization record that cannot be evaluated (a
/// `filterMap` that is no expression, a name pattern that is no regular expression) is logged naming the record, by the
/// error-level line `authz.rs` writes for it, beside failing closed (`a_deny_whose_filter_map_fails_denies_despite_an_allow`,
/// `a_deny_whose_name_pattern_is_invalid_denies`); a record that evaluates logs nothing of the kind. The unexpected
/// errors and a backend TLS failure are `an_unexpected_error_and_a_backend_tls_failure_are_logged_at_error`
/// (web/tests.rs).
#[test]
fn an_authz_record_that_does_not_evaluate_is_logged_at_error_naming_it() {
    use std::sync::{Arc, Mutex};

    let world = world();
    let _entered = world.context.enter();
    session::set_user("nobody", "nobody");
    let parameters = filter_parameters();
    let check = |service: &str| {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let allowed = tracing::subscriber::with_default(crate::testing::LogSubscriber(lines.clone()), || {
            let pushed = world.context.push_checked(
                "AT_SERVICE",
                service,
                "all",
                Authz::Required,
                Tarpit::Skip,
                Some(&parameters),
            );
            if pushed.is_ok() {
                world.context.pop().unwrap();
            }
            pushed.is_ok()
        });
        let lines = lines.lock().unwrap().clone();
        let failed: Vec<String> = lines
            .into_iter()
            .filter(|line| line.contains("authz record did not evaluate"))
            .collect();
        (allowed, failed)
    };
    grant_member(
        &world,
        "LOGGED_GOOD_ALLOW",
        "AT_SERVICE",
        "test.LoggedGood#Row",
        "AUTHZT_ALLOW",
        &[("filterMap", "{orderId: 'O1'}")],
    );
    session::set_user("nobody", "nobody");
    assert_eq!(
        check("test.LoggedGood#Row"),
        (true, Vec::new()),
        "a record that evaluates"
    );

    grant_member(
        &world,
        "LOGGED_BAD_ALLOW",
        "AT_SERVICE",
        "test.LoggedBad#Row",
        "AUTHZT_ALLOW",
        &[("filterMap", BAD_FILTER)],
    );
    grant_member(
        &world,
        "LOGGED_BAD_DENY",
        "AT_SERVICE",
        "test.Logged(",
        "AUTHZT_DENY",
        &[("nameIsPattern", "Y")],
    );
    session::set_user("nobody", "nobody");
    let (allowed, failed) = check("test.LoggedBad#Row");
    assert!(!allowed, "both fail closed");
    for id in ["LOGGED_BAD_ALLOW", "LOGGED_BAD_DENY"] {
        assert!(
            failed
                .iter()
                .any(|line| line.contains(&format!("artifact_authz_id={id}"))),
            "{id}: {failed:?}"
        );
    }
}
