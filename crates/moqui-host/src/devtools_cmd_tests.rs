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

use super::*;

#[test]
fn refs_with_a_flag_without_its_value_is_the_usage_error() {
    for flag in ["--server", "--java", "--project"] {
        let err = dispatch_refs(&["a#B".to_string(), flag.to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("moqui help refs"), "{flag}: {err}");
    }
}

#[test]
fn check_takes_the_server_either_way() {
    let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
    let parsed = parse_check(&args(&["--server=https://a.test", "a.xml"])).unwrap();
    assert_eq!(parsed.server.as_deref(), Some("https://a.test"));
    assert_eq!(parsed.paths, [PathBuf::from("a.xml")]);
    let parsed = parse_check(&args(&["--server", "https://b.test"])).unwrap();
    assert_eq!(parsed.server.as_deref(), Some("https://b.test"));
    assert!(parse_check(&args(&["--serverx"])).is_err());
    let parsed = parse_check(&args(&["--java", "/src/moqui", "a.xml"])).unwrap();
    assert_eq!(
        parsed.project,
        Some(ProjectArg {
            dir: PathBuf::from("/src/moqui"),
            java: true
        })
    );
    assert!(parse_check(&args(&["--java"])).is_err());
    let parsed = parse_check(&args(&["--project=/src/mr", "a.xml"])).unwrap();
    assert_eq!(
        parsed.project,
        Some(ProjectArg {
            dir: PathBuf::from("/src/mr"),
            java: false
        })
    );
    assert!(parse_check(&args(&["--project"])).is_err());
}

/// A Java Moqui checkout in `dir`: the framework (no component.xml, no conf) with an entity of its own, a view
/// whose member is no entity and a relationship to no entity, and a component whose screen and service use them.
fn java_checkout_at(dir: &Path) {
    let write = |path: &str, text: &str| {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("framework/build.gradle", "");
    write("framework/src/main/groovy/org/moqui/Moqui.groovy", "");
    write(
        "framework/entity/OlapEntities.xml",
        r#"<entities>
    <entity entity-name="JavaOnlyFact" package="moqui.olap">
        <field name="factId" type="id" is-pk="true"/>
        <relationship type="one" related="moqui.olap.NoSuchDimension"><key-map field-name="factId"/></relationship>
    </entity>
    <view-entity entity-name="BrokenView" package="moqui.olap">
        <member-entity entity-alias="F" entity-name="moqui.olap.NoSuchFact"/>
        <alias-all entity-alias="F"/>
    </view-entity>
</entities>"#,
    );
    write(
        "runtime/base-component/tools/component.xml",
        r#"<component name="tools"/>"#,
    );
    write(
        "runtime/component/fp-core/component.xml",
        r#"<component name="fp-core"/>"#,
    );
    write(
        "runtime/component/fp-core/entity/PicklistEntities.xml",
        r#"<entities>
    <entity entity-name="PicklistBatch" package="fp.picklist">
        <field name="batchId" type="id" is-pk="true"/><field name="statusId" type="id"/>
    </entity>
    <view-entity entity-name="BatchFact" package="fp.picklist">
        <member-entity entity-alias="B" entity-name="fp.picklist.PicklistBatch"/>
        <member-entity entity-alias="F" entity-name="moqui.olap.JavaOnlyFact" join-from-alias="B">
            <key-map field-name="batchId" related="factId"/></member-entity>
        <alias-all entity-alias="B"/>
    </view-entity>
</entities>"#,
    );
    write(
        "runtime/component/fp-core/screen/Batch/BatchDetails.xml",
        r#"<screen>
    <transition name="updateBatch">
        <actions><service-call name="update#fp.picklist.PicklistBatch" in-map="[batchId:batchId]"/></actions>
        <default-response url="."/>
    </transition>
    <widgets/>
</screen>"#,
    );
}

/// `moqui refs` on a Java Moqui checkout (no runtime/base-component/framework): found as one, booted with this
/// repository's framework conf, its framework's own entities indexed, and a view or relationship that does not
/// resolve left out of the model with the rest loaded.
#[test]
fn a_java_checkout_is_read_directly() {
    let dir = std::env::temp_dir().join(format!("moqui-java-checkout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    java_checkout_at(&dir);
    let checkout = java_checkout(&dir).expect("a Java checkout").root;
    // its runtime names the same checkout
    assert_eq!(
        java_checkout(&dir.join("runtime")).map(|found| found.root),
        Some(checkout.clone())
    );
    // an MR Moqui runtime is not one
    let repo_runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    assert!(java_checkout(&repo_runtime).is_none());
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let framework = repo.join("runtime/base-component/framework");
    assert_eq!(
        mr_framework_for(None, Some(&repo.join("target/release/moqui"))).map(|dir| std::fs::canonicalize(dir).unwrap()),
        Some(std::fs::canonicalize(&framework).unwrap())
    );
    assert_eq!(mr_framework_for(Some(framework.clone()), None), Some(framework.clone()));
    let factory = model_only_in(ModelDirs {
        runtime: checkout.join("runtime"),
        framework,
        framework_component: Some(checkout.join("framework")),
    });
    let factory = factory.expect("the model-only boot of a Java checkout");
    assert_eq!(
        factory.framework_component_dir(),
        std::fs::canonicalize(checkout.join("framework")).unwrap()
    );
    let names: Vec<&str> = factory
        .components()
        .iter()
        .map(|component| component.name.as_str())
        .collect();
    assert!(
        names.contains(&"framework") && names.contains(&"fp-core") && names.contains(&"tools"),
        "{names:?}"
    );
    let facade = factory.entity().expect("an entity facade");
    facade.with_model(|model| {
        assert!(model.entity("moqui.olap.JavaOnlyFact").is_ok());
        assert!(model.entity("fp.picklist.BatchFact").is_ok());
        assert!(model.entity("moqui.olap.BrokenView").is_err());
        // this repository's framework entities are not loaded for a Java checkout
        assert!(model.entity("moqui.basic.Enumeration").is_err());
        let problems = model.problems().join("\n");
        assert!(problems.contains("moqui.olap.BrokenView"), "{problems}");
        assert!(problems.contains("moqui.olap.NoSuchDimension"), "{problems}");
    });
    let index = MemoryIndex::build(&factory);
    let screen = "transition:component://fp-core/screen/Batch/BatchDetails.xml#updateBatch:any";
    let used: Vec<Option<String>> = index
        .refs(screen, Direction::Out)
        .into_iter()
        .map(|found| found.to_id)
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    // the entity-auto call resolves to the service derived from the entity
    assert_eq!(used, [Some("service:update#fp.picklist.PicklistBatch".to_string())]);
    let derived = index
        .describe("service:update#fp.picklist.PicklistBatch")
        .expect("derived");
    assert_eq!(derived.origin, "derived");
    assert_eq!(derived.parent.as_deref(), Some("entity:fp.picklist.PicklistBatch"));
    assert!(index.describe("entity:moqui.olap.JavaOnlyFact").is_some());
    // the framework's entity is used by the component's view, from its member
    assert!(index
        .refs("entity:moqui.olap.JavaOnlyFact", Direction::In)
        .iter()
        .any(|found| found.from == "view-member:fp.picklist.BatchFact@F"));
}
