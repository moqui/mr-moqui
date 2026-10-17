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

//! Entity ECA actions, data-file service elements, and feed receive.
//!
//! Security scripts from `Security.eecas.xml` run as Rust checks. Other action
//! bodies run through `script::run_actions`. Feed receive submits the pending rows a commit
//! inserted to a service worker (`crate::search::feed`).

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use moqui_lib::object::{HostObject, Value};

use super::rule_actions::{condition_allows, publish_names, run_stored_actions};
use super::util::{has_error, take_errors_string};
use crate::context::ExecutionContext;
use crate::entity::load_feed::{self, DataFileService, EcaRule};
use crate::entity::EntityFacade;

/// Whether feed receive found search down. Kept with the facade, so it is the same on every thread
/// and one facade's state never shows on another.
#[derive(Default)]
struct SearchState {
    down: std::sync::atomic::AtomicBool,
}

/// True after [`schedule_feed_receive`] remembered at least one receive service for this facade.
pub fn search_not_up(facade: &EntityFacade) -> bool {
    facade.with_slot(SearchState::default, |state| {
        state.down.load(std::sync::atomic::Ordering::Relaxed)
    })
}

/// Run the ECA rules `load_feed::match_rules_with` matched for this write. `before` selects run-before rules.
///
/// As Java's `EntityEcaRule.runIfMatches`: the rule's context has the field values, `entityValue` (a value
/// scripts and called services share, so a field they set on it is set on the write for a run-before rule),
/// `originalValue` (the row before an update or delete, with `get-original-value`: read here for a before rule, and
/// `pre_row`, read by the write before it ran, for an after rule), and
/// `eecaOperation`. `get-entire-entity` fills unset fields from the row for an update or delete. With
/// `set-results`, non-primary-key fields the actions set in the rule's context are set on the values. A rule
/// is skipped while the message facade has errors unless it has `run-on-error` or is one of the Rust security
/// checks.
pub fn run_matched_eca(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    before: bool,
    rules: &[EcaRule],
    values: &mut BTreeMap<String, Value>,
    pre_row: Option<&crate::entity::EntityValue>,
) -> Result<(), Error> {
    if !rules.iter().any(|rule| rule.run_before == before) {
        return Ok(());
    }
    tracing::debug!(entity = entity_name, operation, before, "running matched entity eca");
    let update_or_delete = operation == "update" || operation == "delete";
    for rule in rules {
        if rule.run_before != before {
            continue;
        }
        // The security checks are checks, not business rules, so an earlier error does not skip them.
        if !rule.run_on_error && has_error() && !is_security_check(&rule.actions) {
            continue;
        }
        // Name the rule in a failure: an action error alone does not say where it came from.
        let in_rule = |err: Error| {
            Error::new(format!(
                "entity ECA {} on {operation} of {entity_name}: {}",
                rule.id,
                err.message()
            ))
        };
        let db_row = if update_or_delete && (rule.get_entire_entity || (before && rule.get_original_value)) {
            let keys = facade.pk_map(entity_name, values).map_err(in_rule)?;
            facade.find_by_pk(entity_name, &keys).map_err(in_rule)?
        } else {
            None
        };
        if rule.get_entire_entity {
            if let Some(row) = &db_row {
                for (name, value) in row.fields() {
                    values.entry(name.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        let original = match (&db_row, before && rule.get_original_value) {
            (Some(row), true) => entity_object(row.clone()),
            // an after rule gets the row as the write found it, which is what Java keeps of the old database values
            _ => match pre_row {
                Some(row) if !before && rule.get_original_value && update_or_delete => entity_object(row.clone()),
                _ => Value::Null,
            },
        };
        let mut current = facade.make_value(entity_name).map_err(in_rule)?;
        for (name, value) in values.iter() {
            current.set(name.clone(), value.clone());
        }
        let cell: std::sync::Arc<std::sync::Mutex<crate::entity::EntityValue>> =
            std::sync::Arc::new(std::sync::Mutex::new(current));

        // Every field of the entity is a name in the rule's context, null when not set, as the Java value's map is, so an
        // unset field does not read a variable of the same name from the caller.
        let mut published = values.clone();
        let names = facade
            .with_entity(entity_name, |entity, _, _| {
                Ok(entity.field_names().into_iter().map(str::to_string).collect::<Vec<_>>())
            })
            .map_err(in_rule)?;
        for name in names {
            published.entry(name).or_insert(Value::Null);
        }
        let _frame = publish_names(&published).map_err(to_lib).map_err(in_rule)?;
        let put = |name: &str, value: Value| {
            crate::script::set_context_name(name, value)
                .map_err(to_lib)
                .map_err(in_rule)
        };
        put("entityValue", Value::Object(HostObject(cell.clone())))?;
        put("originalValue", original)?;
        put("eecaOperation", Value::string(operation))?;
        if !condition_allows(&rule.condition).map_err(to_lib).map_err(in_rule)? {
            continue;
        }
        run_stored_actions(&rule.actions).map_err(to_lib).map_err(in_rule)?;
        if before {
            // Fields set on `entityValue` are fields of the write, as the Java value is the one written.
            let shared = cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (name, value) in shared.fields() {
                if values.get(name) != Some(value) {
                    values.insert(name.clone(), value.clone());
                }
            }
        }
        if rule.set_results {
            let set = crate::script::top_context_entries();
            let fields = facade
                .with_entity(entity_name, |entity, _, _| {
                    let pks = entity.pk_field_names();
                    Ok(entity
                        .field_names()
                        .into_iter()
                        .filter(|name| !pks.contains(name))
                        .map(str::to_string)
                        .collect::<Vec<_>>())
                })
                .map_err(in_rule)?;
            for name in fields {
                if let Some(value) = set.get(&name) {
                    values.insert(name, value.clone());
                }
            }
        }
    }
    Ok(())
}

/// A value scripts see as an entity value.
fn entity_object(value: crate::entity::EntityValue) -> Value {
    Value::Object(HostObject(std::sync::Arc::new(std::sync::Mutex::new(value))))
}

/// Run one data-file service element through [`crate::service::call::sync`].
///
/// `sync` reads the facade on the current execution context. A load that has not
/// entered a context returns an error and does not call the service.
pub fn run_data_file_service(facade: &EntityFacade, service: &DataFileService) -> Result<(), Error> {
    tracing::debug!(
        service = %service.name,
        location = %service.location,
        feed_off = facade.flags().disable_data_feed,
        "running data-file service"
    );
    let mut params = BTreeMap::new();
    for (name, value) in &service.parameters {
        params.insert(name.clone(), Value::string(value));
    }
    // The service runs in the entered context. Without one it cannot run, and saying it did would
    // lose the data-file element silently.
    if !crate::user::context_entered() {
        return Err(Error::new(format!(
            "data-file service {} cannot run without an execution context",
            service.name
        )));
    }
    let _guard = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let started = std::time::Instant::now();
    tracing::info!(
        "Calling service {} in data load from {} with {} parameters",
        service.name,
        service.location,
        service.parameters.len()
    );
    match super::call::sync(&service.name, &params) {
        Ok(_) => {
            tracing::info!(
                "Called service {} in data load in {}ms",
                service.name,
                started.elapsed().as_millis()
            );
            // As Java's `handleService`: errors the call left fail the element, and are cleared.
            match take_errors_string() {
                Some(errors) => Err(Error::new(format!("Error handling data load service call: {errors}"))),
                None => Ok(()),
            }
        }
        Err(err) => {
            tracing::warn!(
                "Service {} called in data load from {} failed after {}ms: {err}",
                service.name,
                service.location,
                started.elapsed().as_millis()
            );
            Err(to_lib(err))
        }
    }
}

/// The service that consumes `DataFeedPending` rows on a worker.
pub const PROCESS_PENDING_SERVICE: &str = "org.moqui.EntityServices.process#DataFeedPending";

/// After commit, hand the pending rows this transaction inserted to a service worker, which
/// assembles the documents and calls each feed's receive or delete service.
///
/// With no search facade installed, [`search_not_up`] reports it. The worker still runs feeds
/// whose receive service is not the search index. Rows it cannot deliver stay for the
/// `IndexDataFeedDocuments` drain. Without an entered context nothing is submitted.
pub fn schedule_feed_receive(facade: &EntityFacade, pending_ids: Vec<String>) -> Result<(), Error> {
    let search_up =
        ExecutionContext::with_current(|ec| crate::search::installed(ec.factory()).is_some()).unwrap_or(false);
    if !search_up && !load_feed::feed_service_names(facade).is_empty() {
        facade.with_slot(SearchState::default, |state| {
            state.down.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
    if pending_ids.is_empty() || !crate::user::context_entered() {
        return Ok(());
    }
    // A later search of this session that asks for read-your-writes waits for these rows.
    crate::search::fresh::remember_pending(&pending_ids);
    let mut params = BTreeMap::new();
    params.insert(
        "dataFeedPendingIds".to_string(),
        Value::List(pending_ids.into_iter().map(Value::String).collect()),
    );
    super::call::async_call(PROCESS_PENDING_SERVICE, &params).map_err(to_lib)
}

/// Whether a rule's actions call one of the `UserSecurityChecks` asserts (`Security.eecas.xml`). Those are checks, not
/// business rules, so they run even when the message facade already has errors.
fn is_security_check(actions: &str) -> bool {
    actions.contains("UserSecurityChecks.assertPrivilegedGroupMembership(")
        || actions.contains("UserSecurityChecks.assertSealedPermissionGrant(")
}

pub(super) fn to_lib(err: crate::context::Error) -> Error {
    Error::new(err.message())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use moqui_lib::object::{basic_convert, Value};

    use super::super::security_checks::{reset_sealed_grant, sealed_grant_entered, user_has_permission, user_in_group};
    use super::search_not_up;
    use crate::context::ExecutionContextFactory;
    use crate::entity::load_feed::{load_entity_data, LoadOptions};
    use crate::entity::{EntityFacade, EntityValue};
    use crate::user::{hash_password, login_user};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn sealed_permission_create_is_stored_when_authz_is_disabled() {
        let (facade, context, _runtime) = boot_security();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        seed_entity_allow(&facade);
        reset_sealed_grant();
        permission_row(&facade, "ADMIN", "ADMIN_PASSWORD")
            .create(&facade)
            .expect("create with authz disabled");
        assert!(sealed_grant_entered());
        assert_eq!(permission_count(&facade, "ADMIN", "ADMIN_PASSWORD"), 1);
    }

    #[test]
    fn sealed_permission_create_is_denied_without_the_permission() {
        let (facade, context, _runtime) = boot_security();
        let _entered = context.enter();
        {
            let _authz = context.disable_authz();
            seed_entity_allow(&facade);
            seed_user(&facade, "plain", "Secret-12");
        }
        login_user("plain", "Secret-12").expect("login");
        let seen = crate::user::current_username().expect("username");
        assert_eq!(
            seen.as_deref(),
            Some("plain"),
            "current_username cannot see the logged-in user"
        );
        reset_sealed_grant();
        let err = permission_row(&facade, "ADMIN", "ADMIN_PASSWORD")
            .create(&facade)
            .expect_err("sealed permission");
        let text = err.to_string();
        assert!(text.contains("authorized"), "{text}");
        assert!(text.contains("ADMIN_PASSWORD"), "{text}");
        assert!(sealed_grant_entered());
        assert_eq!(permission_count(&facade, "ADMIN", "ADMIN_PASSWORD"), 0);
    }

    #[test]
    fn sealed_permission_check_runs_while_the_message_facade_has_errors() {
        let (facade, context, _runtime) = boot_security();
        let _entered = context.enter();
        {
            let _authz = context.disable_authz();
            seed_entity_allow(&facade);
            seed_user(&facade, "plain", "Secret-12");
        }
        login_user("plain", "Secret-12").expect("login");
        context.message().add_error("an earlier error");
        reset_sealed_grant();
        let err = permission_row(&facade, "ADMIN", "ADMIN_PASSWORD")
            .create(&facade)
            .expect_err("sealed permission");
        assert!(err.to_string().contains("ADMIN_PASSWORD"), "{err}");
        assert!(sealed_grant_entered(), "the check is not skipped for the earlier error");
        assert_eq!(permission_count(&facade, "ADMIN", "ADMIN_PASSWORD"), 0);
    }

    #[test]
    fn data_file_runs_entity_auto_and_inline_service() {
        let service = write_temp(
            "inline",
            r#"<services>
                <service verb="copy" noun="Row">
                    <in-parameters><parameter name="seedId"/></in-parameters>
                    <actions>
                        <entity-find entity-name="moqui.test.TestEntity" list="found">
                            <econdition field-name="testId" from="seedId"/>
                        </entity-find>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "B"); made.set("testMedium", "made");</script>
                        <entity-create value-field="made"/>
                    </actions>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open_with(
            &crate::testing::framework_dir(),
            crate::testing::Options {
                entity_eca: false,
                data_feed: false,
                tape: None,
            },
        );
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        let failures = crate::service::lint(std::slice::from_ref(&service)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let data = write_temp(
            "data",
            r#"<entity-facade-xml>
                <moqui.test.TestEntity testId="E1" testMedium="entity"/>
                <create-moqui.test.TestEntity testId="A1" testMedium="auto"/>
                <copy-Row seedId="E1"/>
            </entity-facade-xml>"#,
        );
        load_entity_data(&facade, &data.display().to_string(), &LoadOptions::default()).unwrap();
        assert_eq!(field(&facade, "A1", "testMedium").as_deref(), Some("auto"));
        assert_eq!(field(&facade, "B", "testMedium").as_deref(), Some("made"));
    }

    #[test]
    fn feed_commit_keeps_pending_and_records_search_not_up() {
        let facade = crate::testing::open_with(
            &crate::testing::framework_dir(),
            crate::testing::Options {
                entity_eca: false,
                data_feed: true,
                tape: None,
            },
        );
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        create_row(
            &facade,
            "moqui.basic.EnumerationType",
            &[("enumTypeId", "DataFeedType")],
        );
        create_row(
            &facade,
            "moqui.basic.Enumeration",
            &[("enumId", "DTFDTP_RT_PUSH"), ("enumTypeId", "DataFeedType")],
        );
        create_row(
            &facade,
            "moqui.entity.document.DataDocument",
            &[("dataDocumentId", "DOC1"), ("primaryEntityName", "moqui.test.Foo")],
        );
        // A write is fed when it sets a document field.
        create_row(
            &facade,
            "moqui.entity.document.DataDocumentField",
            &[
                ("dataDocumentId", "DOC1"),
                ("fieldSeqId", "01"),
                ("fieldPath", "fooText"),
            ],
        );
        create_row(
            &facade,
            "moqui.entity.feed.DataFeed",
            &[
                ("dataFeedId", "FEED1"),
                ("dataFeedTypeEnumId", "DTFDTP_RT_PUSH"),
                (
                    "feedReceiveServiceName",
                    "org.moqui.search.SearchServices.index#DataDocuments",
                ),
                (
                    "feedDeleteServiceName",
                    "org.moqui.search.SearchServices.delete#DataDocument",
                ),
            ],
        );
        create_row(
            &facade,
            "moqui.entity.feed.DataFeedDocument",
            &[("dataFeedId", "FEED1"), ("dataDocumentId", "DOC1")],
        );
        facade.begin("moqui.test.Foo", None).unwrap();
        create_row(&facade, "moqui.test.Foo", &[("fooId", "F1"), ("fooText", "alpha")]);
        facade.commit("moqui.test.Foo").unwrap();
        let pending = facade
            .find("moqui.entity.feed.DataFeedPending")
            .unwrap()
            .list()
            .unwrap();
        assert!(
            pending.values().iter().any(|row| {
                text(row, "dataDocumentId") == "DOC1"
                    && text(row, "entityName") == "moqui.test.Foo"
                    && text(row, "primaryKey") == "F1"
                    && text(row, "operation") == "index"
            }),
            "{pending:?}"
        );
        assert!(search_not_up(&facade));
    }

    #[test]
    fn a_write_in_a_run_before_rule_keeps_the_outer_after_rule() {
        let facade = crate::testing::open_with(
            &crate::testing::framework_dir(),
            crate::testing::Options {
                entity_eca: true,
                data_feed: false,
                tape: None,
            },
        );
        // The before rule's write of another entity matches no rules of its own.
        crate::entity::load_feed::add_eca_rules(
            &facade,
            r#"<eecas>
                <eeca id="TestNestedBefore" entity="moqui.test.TestEntity" on-create="true" run-before="true">
                    <actions>
                        <entity-make-value entity-name="moqui.test.Foo" value-field="foo"/>
                        <script>foo.set("fooId", "BEFORE")</script>
                        <entity-create value-field="foo"/>
                    </actions>
                </eeca>
                <eeca id="TestOuterAfter" entity="moqui.test.TestEntity" on-create="true">
                    <actions>
                        <entity-make-value entity-name="moqui.test.Foo" value-field="foo"/>
                        <script>foo.set("fooId", "AFTER")</script>
                        <entity-create value-field="foo"/>
                    </actions>
                </eeca>
            </eecas>"#,
        )
        .unwrap();
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        create_row(&facade, "moqui.test.TestEntity", &[("testId", "T1")]);
        let foo_ids: Vec<String> = facade
            .find("moqui.test.Foo")
            .unwrap()
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| text(row, "fooId"))
            .collect();
        assert!(foo_ids.iter().any(|id| id == "BEFORE"), "{foo_ids:?}");
        assert!(foo_ids.iter().any(|id| id == "AFTER"), "{foo_ids:?}");
    }

    #[test]
    fn privileged_group_membership_is_denied_to_a_non_member() {
        let (facade, context, _runtime) = boot_security();
        let _entered = context.enter();
        {
            let _authz = context.disable_authz();
            seed_entity_allow(&facade);
            seed_user(&facade, "plain", "Secret-12");
        }
        login_user("plain", "Secret-12").expect("login");
        let mut row = facade.make_value("moqui.security.UserGroupMember").unwrap();
        row.set("userGroupId", Value::string("ADMIN"));
        row.set("userId", Value::string("U1"));
        row.set("fromDate", basic_convert(&Value::I64(0), "java.sql.Timestamp").value);
        let err = row.create(&facade).expect_err("privileged group");
        let text = err.to_string();
        assert!(
            text.contains("not authorized to change membership of group ADMIN"),
            "{text}"
        );
        let members = facade
            .find("moqui.security.UserGroupMember")
            .unwrap()
            .eq("userGroupId", Value::string("ADMIN"))
            .list()
            .unwrap();
        assert_eq!(members.len(), 0);
    }

    #[test]
    fn an_eeca_condition_checks_only_its_first_child() {
        let facade = crate::testing::open_with(
            &crate::testing::framework_dir(),
            crate::testing::Options {
                entity_eca: true,
                data_feed: false,
                tape: None,
            },
        );
        // As Java's `EntityEcaRule`: the second child is not part of the condition.
        crate::entity::load_feed::add_eca_rules(
            &facade,
            r#"<eecas>
                <eeca id="TestFirstConditionChild" entity="moqui.test.TestEntity" on-create="true">
                    <condition><expression>true</expression><expression>false</expression></condition>
                    <actions>
                        <entity-make-value entity-name="moqui.test.Foo" value-field="foo"/>
                        <script>foo.set("fooId", "FIRST_ONLY")</script>
                        <entity-create value-field="foo"/>
                    </actions>
                </eeca>
            </eecas>"#,
        )
        .unwrap();
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        create_row(&facade, "moqui.test.TestEntity", &[("testId", "T1")]);
        let found = facade
            .find("moqui.test.Foo")
            .unwrap()
            .eq("fooId", Value::string("FIRST_ONLY"))
            .one()
            .unwrap();
        assert!(found.is_some(), "the rule did not run");
    }

    #[test]
    fn everyone_is_in_all_users_without_a_lookup() {
        let (facade, context, _runtime) = boot_security();
        let _entered = context.enter();
        // No one is logged in, so any lookup would find no membership.
        assert!(user_in_group(&facade, "ALL_USERS").unwrap());
        let _authz = context.disable_authz();
        assert!(!user_in_group(&facade, "ADMIN").unwrap());
    }

    #[test]
    fn a_permission_granted_to_all_users_counts_only_with_a_membership_row() {
        let (facade, context, _runtime) = boot_security();
        let _entered = context.enter();
        {
            let _authz = context.disable_authz();
            seed_entity_allow(&facade);
            seed_user(&facade, "plain", "Secret-12");
            permission_row(&facade, "ALL_USERS", "ALL_USERS_PERM")
                .create(&facade)
                .unwrap();
        }
        login_user("plain", "Secret-12").expect("login");
        let _authz = context.disable_authz();
        // As Java's `UserPermissionCheck` view, which joins the user's own memberships.
        assert!(!user_has_permission(&facade, "ALL_USERS_PERM").unwrap());
        let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userGroupId", Value::string("ALL_USERS"));
        member.set("userId", Value::string("U1"));
        member.set("fromDate", basic_convert(&Value::I64(0), "java.sql.Timestamp").value);
        member.create(&facade).unwrap();
        assert!(user_has_permission(&facade, "ALL_USERS_PERM").unwrap());
    }

    fn boot_security() -> (EntityFacade, crate::context::ExecutionContext, TempDir) {
        let runtime = temp_dir("seca-runtime");
        let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
        let factory = {
            let _lock = crate::user::session::cookie_property_lock();
            ExecutionContextFactory::boot_with_framework(runtime.path(), &framework).expect("boot")
        };
        let facade = crate::testing::open_with(
            &crate::testing::framework_dir(),
            crate::testing::Options {
                entity_eca: true,
                data_feed: false,
                tape: None,
            },
        );
        factory.set_entity(facade.clone());
        let context = factory.new_context();
        (facade, context, runtime)
    }

    fn seed_entity_allow(facade: &EntityFacade) {
        create_row(
            facade,
            "moqui.security.ArtifactGroupMember",
            &[
                ("artifactGroupId", "SEALED_ENTITY"),
                ("artifactName", ".*"),
                ("artifactTypeEnumId", "AT_ENTITY"),
                ("nameIsPattern", "Y"),
            ],
        );
        create_row(
            facade,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "SEALED_ALLOW"),
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "SEALED_ENTITY"),
                ("authzTypeEnumId", "AUTHZT_ALLOW"),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
    }

    fn seed_user(facade: &EntityFacade, username: &str, password: &str) {
        let hashed = hash_password(password).expect("hash");
        let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
        row.set("userId", Value::string("U1"));
        row.set("username", Value::string(username));
        row.set("currentPassword", Value::string(hashed));
        row.set("passwordHashType", Value::string("argon2"));
        row.set("passwordSalt", Value::string(""));
        row.set("passwordBase64", Value::string("N"));
        row.set("disabled", Value::string("N"));
        row.set("requirePasswordChange", Value::string("N"));
        row.create(facade).unwrap();
    }

    fn permission_row(facade: &EntityFacade, group_id: &str, permission_id: &str) -> EntityValue {
        let mut row = facade.make_value("moqui.security.UserGroupPermission").unwrap();
        row.set("userGroupId", Value::string(group_id));
        row.set("userPermissionId", Value::string(permission_id));
        row.set("fromDate", basic_convert(&Value::I64(0), "java.sql.Timestamp").value);
        row
    }

    fn permission_count(facade: &EntityFacade, group_id: &str, permission_id: &str) -> usize {
        facade
            .find("moqui.security.UserGroupPermission")
            .unwrap()
            .eq("userGroupId", Value::string(group_id))
            .eq("userPermissionId", Value::string(permission_id))
            .list()
            .unwrap()
            .len()
    }

    fn field(facade: &EntityFacade, test_id: &str, name: &str) -> Option<String> {
        facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string(test_id))
            .one()
            .unwrap()
            .and_then(|row| row.get(name).and_then(Value::as_str).map(str::to_string))
    }

    fn create_row(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(facade).unwrap();
    }

    fn text(row: &EntityValue, field: &str) -> String {
        row.get(field).and_then(Value::as_str).unwrap_or("").to_string()
    }

    fn write_temp(label: &str, body: &str) -> PathBuf {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("moqui-cb-{label}-{seq}-{}.xml", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
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
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("moqui-{label}-{nanos}-{seq}"));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}
