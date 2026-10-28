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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use moqui_db::sql::Condition;
use moqui_lib::object::Value;

use super::super::session;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::script::{clear_context_stack, set_context_name};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

// not in a moqui package: the entity filters leave the framework's entities alone
const VIEW: &str = "test.product.ProductOrg";
const PRODUCT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<entities>
    <entity entity-name="Product" package="test.product">
        <field name="productId" type="id" is-pk="true"/>
        <field name="ownerPartyId" type="id"/>
    </entity>
    <view-entity entity-name="ProductOrg" package="test.product">
        <member-entity entity-alias="PROD" entity-name="test.product.Product"/>
        <alias name="productId" entity-alias="PROD" field="productId"/>
        <alias name="ownerPartyId" entity-alias="PROD" field="ownerPartyId"/>
    </view-entity>
</entities>
"#;

#[test]
fn apply_entity_filters_returns_the_same_condition() {
    let condition = Condition::equals("testId", Value::string("T1"));
    let filtered = super::apply_entity_filters("moqui.test.TestEntity", Some(condition.clone()));
    assert_eq!(filtered, Some(condition));
    assert!(super::apply_entity_filters("moqui.test.TestEntity", None).is_none());
}

#[test]
fn a_filter_map_joins_a_list_of_maps_with_their_own_comparisons() {
    let (facade, _model_dir) = product_facade();
    let map = |pairs: Vec<(&str, Value)>| -> std::collections::BTreeMap<String, Value> {
        pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
    };
    let as_map = |pairs: Vec<(&str, Value)>| {
        Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
    };
    // [_join:'or', _list:[[ownerPartyId:'ORG1'], [productId:'P%', _comp:'like']]]
    let filter = map(vec![
        ("_join", Value::string("or")),
        (
            "_list",
            Value::List(vec![
                as_map(vec![("ownerPartyId", Value::string("ORG1"))]),
                as_map(vec![
                    ("productId", Value::string("P%")),
                    ("_comp", Value::string("like")),
                ]),
            ]),
        ),
    ]);
    let condition = facade
        .with_entity("test.product.Product", |entity, _, _| {
            super::map_conditions(
                entity,
                "test.product.Product",
                super::Relation::Direct,
                &filter,
                (false, ""),
                false,
            )
        })
        .unwrap()
        .expect("a condition");
    let text = format!("{condition:?}");
    assert!(text.starts_with("Or"), "{text}");
    assert!(text.contains("ORG1") && text.contains("Like"), "{text}");
    assert!(!text.contains("_join") && !text.contains("_list"), "{text}");
    assert_eq!(super::comparison_enum("!=").unwrap(), "ENTCO_NOT_EQUALS");
    assert!(super::comparison_enum("sounds-like").is_err());
}

#[test]
fn view_find_applies_user_org_and_skips_active_org_until_set() {
    session::reset_session();
    clear_context_stack();
    let runtime = temp_dir("filter-runtime");
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = {
        let _lock = crate::user::session::cookie_property_lock();
        ExecutionContextFactory::boot_with_framework(runtime.path(), &framework).expect("boot")
    };
    let (facade, _model_dir) = product_facade();
    factory.set_entity(facade.clone());
    let context = factory.new_context();
    let _entered = context.enter();
    session::set_user("org-user", "org-user");
    {
        let _guard = context.disable_authz();
        seed(&facade);
    }
    set_context_name(
        "filterOrgIds",
        Value::List(vec![Value::string("ORG1"), Value::string("ORG2")]),
    )
    .unwrap();

    // The filters come from the record that allowed this frame.
    context
        .push_checked("AT_ENTITY", VIEW, "view", true, false, None)
        .expect("the ALL_USERS allow covers the view");
    let incoming = Condition::equals("productId", Value::string("P1"));
    let filtered = super::apply_entity_filters(VIEW, Some(incoming)).expect("user org filter");
    assert!(has_equals(&filtered, "productId", "P1"), "{filtered:?}");
    assert!(has_in(&filtered, "ownerPartyId", &["ORG1", "ORG2"]), "{filtered:?}");
    assert!(!has_equals(&filtered, "ownerPartyId", "ORG9"), "{filtered:?}");

    set_context_name("activeOrgId", Value::string("ORG9")).unwrap();
    let both = super::apply_entity_filters(VIEW, None).expect("both filters");
    assert!(has_in(&both, "ownerPartyId", &["ORG1", "ORG2"]), "{both:?}");
    assert!(has_equals(&both, "ownerPartyId", "ORG9"), "{both:?}");

    // A view not in the model, as a dynamic view is, gets the same filters on its members.
    let mut dynamic = facade.with_entity(VIEW, |entity, _, _| Ok(entity.clone())).unwrap();
    dynamic.full_name = "dynamic.ProductFilterView".to_string();
    let on_dynamic = super::apply_view_filters(&dynamic, None).expect("filters on the dynamic view");
    assert!(has_in(&on_dynamic, "ownerPartyId", &["ORG1", "ORG2"]), "{on_dynamic:?}");
    assert!(has_equals(&on_dynamic, "ownerPartyId", "ORG9"), "{on_dynamic:?}");

    // `getFindFiltersForUser` lists the filters behind those terms, by set
    let sets: Vec<String> = super::find_filters_for_user(VIEW)
        .unwrap()
        .iter()
        .map(|filter| super::text(filter, "entityFilterSetId"))
        .collect();
    assert!(sets.contains(&"TEST_USER_ORG".to_string()), "{sets:?}");
    assert!(sets.contains(&"TEST_ACTIVE_ORG".to_string()), "{sets:?}");
    assert!(super::find_filters_for_user("moqui.security.UserAccount")
        .unwrap()
        .is_empty());

    // the orgs may come from ec.user.context, as setup#UserOrganizationInfo puts them, instead of the context
    clear_context_stack();
    assert!(crate::script::eval_condition(
        "(ec.user.context.filterOrgIds = ['ORG7']) && ec.user.context.filterOrgIds[0] === 'ORG7' \
                && Object.keys(ec.user.context).includes('filterOrgIds') && ec.user.context.missing === undefined"
    )
    .unwrap());
    let from_user = super::apply_entity_filters(VIEW, None).expect("user org filter from the user context");
    assert!(has_in(&from_user, "ownerPartyId", &["ORG7"]), "{from_user:?}");
    context.user_context().clear();

    context.pop().unwrap();

    // The same filters on a real find through the view: ORG9 is out until the active org is set.
    {
        let _guard = context.disable_authz();
        for (id, owner) in [("P1", "ORG1"), ("P2", "ORG9"), ("P3", "ORG2")] {
            create(
                &facade,
                "test.product.Product",
                &[("productId", id), ("ownerPartyId", owner)],
            );
        }
    }
    clear_context_stack();
    set_context_name(
        "filterOrgIds",
        Value::List(vec![Value::string("ORG1"), Value::string("ORG2")]),
    )
    .unwrap();
    let ids = |facade: &EntityFacade| -> Vec<String> {
        let mut ids: Vec<String> = facade
            .find(VIEW)
            .unwrap()
            .list()
            .unwrap()
            .values()
            .iter()
            .filter_map(|row| row.get("productId").and_then(Value::as_str).map(str::to_string))
            .collect();
        ids.sort();
        ids
    };
    // applyCond on the active org set skips it while activeOrgId is unset.
    assert_eq!(ids(&facade), ["P1", "P3"]);
    // a one-row find by key, as an agent card reads its record (moqui_llm::tools::catalog::entity_record)
    let record = |id: &str| {
        facade
            .find(VIEW)
            .unwrap()
            .eq("productId", Value::string(id))
            .one()
            .unwrap()
    };
    assert!(record("P1").is_some(), "agent cards may show an authorized row");
    assert!(
        record("P2").is_none(),
        "agent cards must apply the same org filters as entity finds"
    );
    set_context_name("activeOrgId", Value::string("ORG1")).unwrap();
    assert_eq!(ids(&facade), ["P1"]);

    // The filter sets are kept for an authorization generation, and a write to a filter table moves it.
    {
        let _guard = context.disable_authz();
        facade
            .find("moqui.security.ArtifactAuthzFilter")
            .unwrap()
            .eq("artifactAuthzId", Value::string("TEST_ORG_AUTHZ"))
            .eq("entityFilterSetId", Value::string("TEST_ACTIVE_ORG"))
            .one()
            .unwrap()
            .expect("the active org link")
            .delete(&facade)
            .unwrap();
    }
    assert_eq!(ids(&facade), ["P1", "P3"]);

    clear_context_stack();
    session::reset_session();
}

fn seed(facade: &EntityFacade) {
    create(
        facade,
        "moqui.security.ArtifactGroupMember",
        &[
            ("artifactGroupId", "TEST_ORG"),
            ("artifactName", VIEW),
            ("artifactTypeEnumId", "AT_ENTITY"),
            ("nameIsPattern", "N"),
        ],
    );
    create(
        facade,
        "moqui.security.ArtifactAuthz",
        &[
            ("artifactAuthzId", "TEST_ORG_AUTHZ"),
            ("userGroupId", "ALL_USERS"),
            ("artifactGroupId", "TEST_ORG"),
            ("authzTypeEnumId", "AUTHZT_ALLOW"),
            ("authzActionEnumId", "AUTHZA_ALL"),
        ],
    );
    create(
        facade,
        "moqui.security.EntityFilterSet",
        &[
            ("entityFilterSetId", "TEST_USER_ORG"),
            ("description", "User Organization Filters"),
        ],
    );
    create(
        facade,
        "moqui.security.EntityFilter",
        &[
            ("entityFilterId", "TEST_USER_ORG_A"),
            ("entityFilterSetId", "TEST_USER_ORG"),
            ("entityName", "test.product.Product"),
            ("filterMap", "({ ownerPartyId: filterOrgIds })"),
        ],
    );
    // a filter on another entity whose map does not evaluate; finds on the product view do not run it
    create(
        facade,
        "moqui.security.EntityFilter",
        &[
            ("entityFilterId", "TEST_USER_ORG_BROKEN"),
            ("entityFilterSetId", "TEST_USER_ORG"),
            ("entityName", "test.party.Party"),
            ("filterMap", "[ownerPartyId:(['_NA_']+"),
        ],
    );
    create(
        facade,
        "moqui.security.EntityFilterSet",
        &[
            ("entityFilterSetId", "TEST_ACTIVE_ORG"),
            ("description", "Active Organization Filters"),
            ("applyCond", "activeOrgId"),
            ("allowMissingAlias", "Y"),
        ],
    );
    create(
        facade,
        "moqui.security.EntityFilter",
        &[
            ("entityFilterId", "TEST_ACTIVE_ORG_A"),
            ("entityFilterSetId", "TEST_ACTIVE_ORG"),
            ("entityName", "test.product.Product"),
            ("filterMap", "({ ownerPartyId: activeOrgId })"),
        ],
    );
    create(
        facade,
        "moqui.security.ArtifactAuthzFilter",
        &[
            ("artifactAuthzId", "TEST_ORG_AUTHZ"),
            ("entityFilterSetId", "TEST_USER_ORG"),
        ],
    );
    create(
        facade,
        "moqui.security.ArtifactAuthzFilter",
        &[
            ("artifactAuthzId", "TEST_ORG_AUTHZ"),
            ("entityFilterSetId", "TEST_ACTIVE_ORG"),
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

fn has_equals(condition: &Condition, field: &str, expected: &str) -> bool {
    match condition {
        Condition::Equals(field_ref, value) => field_ref.name == field && value.as_str() == Some(expected),
        Condition::And(parts) | Condition::Or(parts) => parts.iter().any(|part| has_equals(part, field, expected)),
        Condition::Not(inner) => has_equals(inner, field, expected),
        _ => false,
    }
}

fn has_in(condition: &Condition, field: &str, expected: &[&str]) -> bool {
    match condition {
        Condition::In(field_ref, values) => {
            field_ref.name == field
                && values.len() == expected.len()
                && values
                    .iter()
                    .zip(expected)
                    .all(|(value, want)| value.as_str() == Some(*want))
        }
        Condition::And(parts) | Condition::Or(parts) => parts.iter().any(|part| has_in(part, field, expected)),
        Condition::Not(inner) => has_in(inner, field, expected),
        _ => false,
    }
}

fn product_facade() -> (EntityFacade, TempDir) {
    let root = temp_dir("product-fw");
    let entity_dir = root.path().join("entity");
    let conf_dir = root.path().join("conf");
    std::fs::create_dir_all(&entity_dir).unwrap();
    std::fs::create_dir_all(&conf_dir).unwrap();
    let real = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    std::os::unix::fs::symlink(
        real.join("conf/MoquiDefaultConf.xml"),
        conf_dir.join("MoquiDefaultConf.xml"),
    )
    .unwrap();
    for entry in std::fs::read_dir(real.join("entity")).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_file() {
            continue;
        }
        std::os::unix::fs::symlink(entry.path(), entity_dir.join(entry.file_name())).unwrap();
    }
    std::fs::write(entity_dir.join("TestProductAuthz.xml"), PRODUCT_XML).unwrap();
    (open_mem(root.path()), root)
}

fn open_mem(framework_dir: &Path) -> EntityFacade {
    crate::testing::open_at(framework_dir)
}

struct TempDir(PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
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
