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

//! Host checks for the entity data loader, stored ECA rules, and `DataFeedPending`.

#![cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::DbDriver;
use moqui_core::entity::load_feed::{
    data_file_services, eca_rules, feed_service_names, load_entity_data, load_entity_xml_text, match_rules, LoadOptions,
};
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_lib::object::Value;

use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::db::entity_facade::open_entity_facade;

#[test]
fn load_data_stores_eca_and_inserts_pending() {
    let _lock = crate::common::shared();
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
        &framework,
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    facade
        .check_tables(&[
            "moqui.test.TestEntity",
            "moqui.test.Foo",
            "moqui.screen.form.DbForm",
            "moqui.basic.EnumerationType",
            "moqui.basic.Enumeration",
            "moqui.basic.LocalizedEntityField",
            "moqui.entity.document.DataDocument",
            "moqui.entity.document.DataDocumentField",
            "moqui.entity.document.DataDocumentCondition",
            "moqui.entity.feed.DataFeed",
            "moqui.entity.feed.DataFeedDocument",
            "moqui.entity.feed.DataFeedPending",
            "moqui.security.UserGroup",
            "moqui.security.UserGroupPermission",
            "moqui.security.UserGroupMember",
            "moqui.security.UserAccount",
            // A service frame reads the tarpit and authz checks, as Java does, though authz is disabled.
            "moqui.security.ArtifactTarpit",
            "moqui.security.ArtifactAuthz",
            "moqui.security.ArtifactGroupMember",
        ])
        .unwrap();

    // The data-file service element runs in an entered context, as a real load does.
    let factory_dir = dir.join("runtime");
    std::fs::create_dir_all(&factory_dir).unwrap();
    let factory = moqui_core::context::ExecutionContextFactory::boot_with_framework(&factory_dir, &framework).unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();

    let fix = dir.join("fixtures");
    std::fs::create_dir_all(&fix).unwrap();
    let xml = fix.join("rows.xml");
    std::fs::write(
        &xml,
        r#"<entity-facade-xml type="seed">
            <moqui.test.TestEntity testId="XML1" testMedium="from-xml"/>
            <create-moqui.test.TestEntity testId="SVC" testMedium="held"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    let loaded = load_entity_data(&facade, xml.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 1);
    assert_eq!(
        text_of(&facade, "moqui.test.TestEntity", "testId", "XML1", "testMedium"),
        Some("from-xml".to_string())
    );
    let held = data_file_services(&facade);
    assert!(held
        .iter()
        .any(|service| service.name == "create#moqui.test.TestEntity"));
    // The element is remembered and also ran, as the entered context lets it.
    assert_eq!(
        text_of(&facade, "moqui.test.TestEntity", "testId", "SVC", "testMedium"),
        Some("held".to_string())
    );

    let demo = fix.join("demo.xml");
    std::fs::write(
        &demo,
        r#"<entity-facade-xml type="demo"><moqui.test.TestEntity testId="DEMO" testMedium="no"/></entity-facade-xml>"#,
    )
    .unwrap();
    let seed_only = LoadOptions {
        data_types: vec!["seed".to_string()],
        ..LoadOptions::default()
    };
    let skipped = load_entity_data(&facade, demo.to_str().unwrap(), &seed_only).unwrap();
    assert_eq!(skipped.records, 0);
    assert!(row_missing(&facade, "moqui.test.TestEntity", "testId", "DEMO"));

    let csv_path = fix.join("rows.csv");
    std::fs::write(
        &csv_path,
        "moqui.test.TestEntity,seed\ntestId,testMedium\nCSV1,from-csv\n",
    )
    .unwrap();
    let csv_loaded = load_entity_data(&facade, csv_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(csv_loaded.records, 1);
    assert_eq!(
        text_of(&facade, "moqui.test.TestEntity", "testId", "CSV1", "testMedium"),
        Some("from-csv".to_string())
    );

    let once = fix.join("once.xml");
    std::fs::write(
        &once,
        r#"<entity-facade-xml type="seed"><moqui.test.TestEntity testId="OC1" testMedium="first"/></entity-facade-xml>"#,
    )
    .unwrap();
    assert_eq!(
        load_entity_data(&facade, once.to_str().unwrap(), &LoadOptions::default())
            .unwrap()
            .records,
        1
    );
    let again = fix.join("once-again.xml");
    std::fs::write(
        &again,
        r#"<entity-facade-xml type="seed"><moqui.test.TestEntity testId="OC1" testMedium="second"/></entity-facade-xml>"#,
    )
    .unwrap();
    let only_create = LoadOptions {
        only_create: true,
        ..LoadOptions::default()
    };
    assert_eq!(
        load_entity_data(&facade, again.to_str().unwrap(), &only_create)
            .unwrap()
            .records,
        0
    );
    assert_eq!(
        text_of(&facade, "moqui.test.TestEntity", "testId", "OC1", "testMedium"),
        Some("first".to_string())
    );

    let try_insert = LoadOptions {
        use_try_insert: true,
        ..LoadOptions::default()
    };
    assert_eq!(
        load_entity_data(&facade, again.to_str().unwrap(), &try_insert)
            .unwrap()
            .records,
        1
    );
    assert_eq!(
        text_of(&facade, "moqui.test.TestEntity", "testId", "OC1", "testMedium"),
        Some("second".to_string())
    );

    let nested = fix.join("nested.xml");
    std::fs::write(
        &nested,
        r#"<entity-facade-xml type="seed">
            <moqui.security.UserGroup userGroupId="G1" description="group">
                <permissions userPermissionId="PERM1" fromDate="0"/>
            </moqui.security.UserGroup>
        </entity-facade-xml>"#,
    )
    .unwrap();
    assert_eq!(
        load_entity_data(&facade, nested.to_str().unwrap(), &LoadOptions::default())
            .unwrap()
            .records,
        2
    );
    assert_eq!(
        text_of(&facade, "moqui.security.UserGroup", "userGroupId", "G1", "description"),
        Some("group".to_string())
    );
    let perms = facade
        .find("moqui.security.UserGroupPermission")
        .unwrap()
        .eq("userGroupId", Value::string("G1"))
        .eq("userPermissionId", Value::string("PERM1"))
        .list()
        .unwrap();
    assert_eq!(perms.len(), 1);

    let member = fix.join("member.xml");
    std::fs::write(
        &member,
        r#"<entity-facade-xml type="seed">
            <moqui.security.UserGroupMember userGroupId="GDUMMY" userId="U1" fromDate="0"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    let dummy = LoadOptions {
        dummy_fks: true,
        ..LoadOptions::default()
    };
    assert_eq!(
        load_entity_data(&facade, member.to_str().unwrap(), &dummy)
            .unwrap()
            .records,
        1
    );
    assert!(!facade.flags().dummy_fks);
    assert_eq!(
        text_of(
            &facade,
            "moqui.security.UserGroup",
            "userGroupId",
            "GDUMMY",
            "userGroupId"
        ),
        Some("GDUMMY".to_string())
    );

    let missing = fix.join("missing-group.xml");
    std::fs::write(
        &missing,
        r#"<entity-facade-xml type="seed">
            <moqui.security.UserGroupMember userGroupId="GMISSING" userId="U2" fromDate="0"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    // Without dummy foreign keys the database constraint decides, as in Java.
    let blocked = load_entity_data(&facade, missing.to_str().unwrap(), &LoadOptions::default()).unwrap_err();
    let blocked_text = format!("{blocked:?}");
    assert!(blocked_text.contains("usergroupmemberusergroup"), "{blocked_text}");
    let no_fk = LoadOptions {
        disable_fk_create: true,
        ..LoadOptions::default()
    };
    let db_err = load_entity_data(&facade, missing.to_str().unwrap(), &no_fk).unwrap_err();
    let db_text = format!("{db_err:?}");
    assert!(db_text.contains("usergroupmemberusergroup"), "{db_text}");
    assert!(!facade.flags().disable_fk_create);
    assert!(row_missing(
        &facade,
        "moqui.security.UserGroup",
        "userGroupId",
        "GMISSING"
    ));

    // As Java's `componentNameList`, the components narrow only the locations a load gathers; a location given is
    // read (here it is not there), not skipped.
    let other = LoadOptions {
        components: vec!["tools".to_string()],
        ..LoadOptions::default()
    };
    assert!(load_entity_data(&facade, "component://other/data/x.xml", &other).is_err());

    // A restricted load refuses a sensitive record by entity name, entity short alias, or nested relationship
    // name, and any service element, before anything of the tree is written.
    let restricted = LoadOptions {
        restrict_sensitive_entities: true,
        ..LoadOptions::default()
    };
    for (label, body) in [
        (
            "nested",
            r#"<moqui.security.UserAccount userId="RSTU" username="rstu"><groups userGroupId="ADMIN"/></moqui.security.UserAccount>"#,
        ),
        (
            "root",
            r#"<moqui.security.UserGroupMember userGroupId="ADMIN" userId="RSTU" fromDate="0"/>"#,
        ),
        (
            "alias",
            r#"<userGroupPermissions userGroupId="ADMIN" userPermissionId="X" fromDate="0"/>"#,
        ),
        (
            "service",
            r#"<create-moqui.test.TestEntity testId="RSVC" testMedium="no"/>"#,
        ),
    ] {
        let text = format!(r#"<entity-facade-xml type="seed">{body}</entity-facade-xml>"#);
        let err = load_entity_xml_text(&facade, "restricted", &text, &restricted).unwrap_err();
        assert!(err.to_string().contains("is not allowed"), "{label}: {err}");
    }
    assert!(row_missing(&facade, "moqui.security.UserAccount", "userId", "RSTU"));
    assert!(row_missing(&facade, "moqui.test.TestEntity", "testId", "RSVC"));
    let allowed = load_entity_xml_text(
        &facade,
        "restricted",
        r#"<entity-facade-xml type="seed"><moqui.test.TestEntity testId="RALLOW" testMedium="ok"/></entity-facade-xml>"#,
        &restricted,
    )
    .unwrap();
    assert_eq!(allowed.records, 1);

    let rules = eca_rules(&facade, "moqui.screen.form.DbForm");
    let form_rule = rules.iter().find(|rule| rule.id == "DbFormCache").expect("DbFormCache");
    assert!(form_rule.actions.contains("ec.cache"), "{}", form_rule.actions);
    assert!(form_rule.on_create && form_rule.on_update && form_rule.on_delete);
    let mut form = facade.make_value("moqui.screen.form.DbForm").unwrap();
    form.set("formId", Value::string("FORM1"));
    form.create(&facade)
        .unwrap_or_else(|err| panic!("create failed: {err:?}"));
    assert!(match_rules(&facade, "moqui.screen.form.DbForm", "create")
        .iter()
        .any(|id| id == "DbFormCache"));
    facade.set_disable_entity_eca(true);
    assert!(match_rules(&facade, "moqui.screen.form.DbForm", "update").is_empty());
    facade.set_disable_entity_eca(false);

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
    // A write is fed when it sets a document field, as Java's `dataFeedCheckAndRegister`.
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
    let pending = pending_for(&facade, "F1");
    assert_eq!(
        pending,
        vec![("DOC1".to_string(), "moqui.test.Foo".to_string(), "index".to_string())]
    );
    let services = feed_service_names(&facade);
    assert!(services
        .iter()
        .any(|service| service.receive_service == "org.moqui.search.SearchServices.index#DataDocuments"));
    assert!(services
        .iter()
        .any(|service| service.delete_service == "org.moqui.search.SearchServices.delete#DataDocument"));

    facade.begin("moqui.test.Foo", None).unwrap();
    create_row(&facade, "moqui.test.Foo", &[("fooId", "F2"), ("fooText", "beta")]);
    facade.rollback("moqui.test.Foo").unwrap();
    assert!(pending_for(&facade, "F2").is_empty());
    assert!(row_missing(&facade, "moqui.test.Foo", "fooId", "F2"));

    facade.set_disable_data_feed(true);
    facade.begin("moqui.test.Foo", None).unwrap();
    create_row(&facade, "moqui.test.Foo", &[("fooId", "F3"), ("fooText", "gamma")]);
    facade.commit("moqui.test.Foo").unwrap();
    assert!(pending_for(&facade, "F3").is_empty());
    assert_eq!(
        text_of(&facade, "moqui.test.Foo", "fooId", "F3", "fooText"),
        Some("gamma".to_string())
    );
    facade.set_disable_data_feed(false);

    drop(facade);
    drop(cluster);
}

fn create_row(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn text_of(facade: &EntityFacade, entity: &str, pk_field: &str, pk: &str, field: &str) -> Option<String> {
    facade
        .find(entity)
        .unwrap()
        .eq(pk_field, Value::string(pk))
        .one()
        .unwrap()
        .and_then(|row| row.get(field).and_then(Value::as_str).map(str::to_string))
}

fn row_missing(facade: &EntityFacade, entity: &str, pk_field: &str, pk: &str) -> bool {
    facade
        .find(entity)
        .unwrap()
        .eq(pk_field, Value::string(pk))
        .one()
        .unwrap()
        .is_none()
}

fn pending_for(facade: &EntityFacade, pk: &str) -> Vec<(String, String, String)> {
    facade
        .find("moqui.entity.feed.DataFeedPending")
        .unwrap()
        .eq("primaryKey", Value::string(pk))
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            (
                cell(row, "dataDocumentId"),
                cell(row, "entityName"),
                cell(row, "operation"),
            )
        })
        .collect()
}

fn cell(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).and_then(Value::as_str).unwrap_or("").to_string()
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_cluster_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("moqui-pg-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}
