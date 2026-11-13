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
use std::sync::Arc;

use super::*;
use moqui_core::context::ExecutionContextFactory;

/// An MR Moqui clone in a fresh directory: this repository's framework (linked) and a component with one entity.
fn mr_clone(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("moqui-project-cache-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let framework = std::fs::canonicalize(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/framework"
    ))
    .unwrap();
    std::fs::create_dir_all(root.join("runtime/components/base")).unwrap();
    std::os::unix::fs::symlink(framework, root.join("runtime/components/base/framework")).unwrap();
    // only the sets the manifest lists load
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime/components/components.xml"),
        root.join("runtime/components/components.xml"),
    )
    .unwrap();
    let component = root.join("runtime/components/addon/proj-a");
    std::fs::create_dir_all(component.join("entity")).unwrap();
    std::fs::write(component.join("component.xml"), r#"<component name="proj-a"/>"#).unwrap();
    std::fs::write(component.join("MoquiConf.xml"), "<moqui-conf/>").unwrap();
    std::fs::write(
        component.join("entity/ProjEntities.xml"),
        r#"<entities><entity entity-name="ProjThing" package="proj"><field name="thingId" type="id" is-pk="true"/></entity></entities>"#,
    )
    .unwrap();
    std::fs::canonicalize(root).unwrap()
}

fn clean(root: &Path) {
    forget(root);
    let _ = std::fs::remove_dir_all(root);
}

/// A production instance reads no other project, and its one answer does not tell a project from a missing path.
#[test]
fn a_production_instance_answers_only_its_own_runtime() {
    let root = mr_clone("production");
    let ctx = ExecutionContextFactory::test_context();
    let own = Arc::clone(ctx.factory());
    moqui_core::web::TEST_PRODUCTION.with(|production| production.set(Some(true)));
    let project = requested(&root.display().to_string(), false, &own, false)
        .err()
        .unwrap();
    let missing = requested("/no/such/moqui/project", false, &own, false).err().unwrap();
    let java = requested(&root.display().to_string(), true, &own, false).err().unwrap();
    assert_eq!(project.to_string(), NOT_AVAILABLE);
    assert_eq!(missing.to_string(), NOT_AVAILABLE);
    assert_eq!(java.to_string(), NOT_AVAILABLE);
    assert!(!kept().contains(&root), "nothing of it was read");
    assert!(requested("", false, &own, false).unwrap().is_none());
    let own_runtime = std::fs::canonicalize(own.runtime_dir()).unwrap();
    assert!(requested(&own_runtime.display().to_string(), false, &own, false)
        .unwrap()
        .is_none());
    moqui_core::web::TEST_PRODUCTION.with(|production| production.set(None));
    clean(&root);
}

/// A project that does not boot again (a conf saved half-edited) still answers from the index read before, and boots
/// again once the conf is fixed.
#[test]
fn a_failed_boot_again_keeps_the_index_read_before() {
    let root = mr_clone("reboot");
    let project = super::super::detect(&root).unwrap();
    let dirs = project.model_dirs(|| None).unwrap();
    let first = open(project.clone(), dirs.clone(), false).unwrap();
    assert!(first.describe("entity:proj.ProjThing").is_some());
    let summary = first.summary();
    assert!(
        summary.files > 0 && summary.components.contains(&"proj-a".to_string()),
        "{summary:?}"
    );

    let conf = root.join("runtime/components/addon/proj-a/MoquiConf.xml");
    std::fs::write(&conf, "<moqui-conf><unclosed").unwrap();
    std::thread::sleep(RECHECK + Duration::from_millis(100));
    let kept = open(project.clone(), dirs.clone(), false).unwrap();
    assert!(Arc::ptr_eq(&first, &kept), "the index read before answers");
    assert!(kept.describe("entity:proj.ProjThing").is_some());

    std::fs::write(&conf, "<moqui-conf/>").unwrap();
    std::thread::sleep(RECHECK + Duration::from_millis(100));
    let again = open(project, dirs, false).unwrap();
    assert!(!Arc::ptr_eq(&first, &again), "booted again once the conf reads");
    assert!(again.describe("entity:proj.ProjThing").is_some());
    clean(&root);
}

/// An allowed project is read on a production instance too (the row is the admin's decision), into its own index, and
/// its stamp follows its files: the same files give the same stamp, a changed file a new one.
#[test]
fn an_allowed_project_is_read_anywhere_and_its_stamp_follows_its_files() {
    let root = mr_clone("allowed");
    let ctx = ExecutionContextFactory::test_context();
    let own = Arc::clone(ctx.factory());
    moqui_core::web::TEST_PRODUCTION.with(|production| production.set(Some(true)));
    let project = allowed(&root.display().to_string(), false, &own, false).unwrap();
    moqui_core::web::TEST_PRODUCTION.with(|production| production.set(None));
    assert!(project.describe("entity:proj.ProjThing").is_some());
    let first = project.stamp();
    assert!(
        first.files > 0 && first.hash.len() == 24 && first.built_at > 0,
        "{first:?}"
    );
    assert_eq!(project.stamp(), first);

    let entities = root.join("runtime/components/addon/proj-a/entity/ProjEntities.xml");
    std::fs::write(
        &entities,
        r#"<entities><entity entity-name="ProjOther" package="proj"><field name="otherId" type="id" is-pk="true"/></entity></entities>"#,
    )
    .unwrap();
    std::thread::sleep(RECHECK + Duration::from_millis(100));
    let again = allowed(&root.display().to_string(), false, &own, false).unwrap();
    assert!(again.describe("entity:proj.ProjOther").is_some());
    assert!(again.describe("entity:proj.ProjThing").is_none(), "a miss is a miss");
    let changed = again.stamp();
    assert_ne!(changed.hash, first.hash);
    assert_eq!(changed.files, first.files);

    for refused in [
        "relative/dir",
        "/no/such/moqui/project",
        &format!("{}/../x", root.display()),
    ] {
        assert!(allowed(refused, false, &own, false).is_err(), "{refused}");
    }
    clean(&root);
}

/// A project's data records are read with its entity model: a nested record is of its relationship's related entity by
/// the full name, also when the framework has an entity of the same short name (`Enumeration`).
#[test]
fn a_projects_nested_record_is_of_the_related_entity_by_its_full_name() {
    let root = mr_clone("records");
    let component = root.join("runtime/components/addon/proj-a");
    std::fs::write(
        component.join("entity/HolderEntities.xml"),
        r#"<entities>
    <entity entity-name="Holder" package="proj">
        <field name="holderId" type="id" is-pk="true"/>
        <relationship type="many" related="proj.other.Enumeration"><key-map field-name="holderId"/></relationship>
    </entity>
    <entity entity-name="Enumeration" package="proj.other">
        <field name="holderId" type="id" is-pk="true"/>
        <field name="itemSeqId" type="id" is-pk="true"/>
    </entity>
</entities>"#,
    )
    .unwrap();
    std::fs::create_dir_all(component.join("data")).unwrap();
    std::fs::write(
        component.join("data/HolderData.xml"),
        r#"<entity-facade-xml type="seed">
    <proj.Holder holderId="H1"><proj.other.Enumeration itemSeqId="01"/></proj.Holder>
</entity-facade-xml>"#,
    )
    .unwrap();
    let project = super::super::detect(&root).unwrap();
    let dirs = project.model_dirs(|| None).unwrap();
    let index = open(project, dirs, false).unwrap();
    assert!(index.describe("record:proj.Holder:H1").is_some());
    assert!(
        index.describe("record:proj.other.Enumeration:H1,01").is_some(),
        "{:?}",
        index
            .find("H1", &[], &[], 20)
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>()
    );
    clean(&root);
}
