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
    let runtime = temp_dir("authz-runtime");
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = {
        let _lock = crate::user::session::cookie_property_lock();
        ExecutionContextFactory::boot_with_framework(runtime.path(), &framework).expect("boot")
    };
    let facade = crate::testing::open();
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
            super::note_entity_write("moqui.security.ArtifactAuthz");
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
