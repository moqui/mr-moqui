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

//! A component placed in a component directory of a running server loads without a restart (plans/Component-Deploy.md,
//! Phase 2): its new entity's table and an `extend-entity` column made before the model is swapped in, its conf mount
//! and property applied, its service callable, its seed data loaded and its demo data not, and a row recording it. A
//! component with Rust code is refused, and one loaded already. A component a git deploy staged moves into place and
//! loads for a user with COMPONENT_DEPLOY, and is refused without it (the fetch itself needs a remote, checked live).
//! A component kept in the database whose files are missing at start is written out from its repository.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::deploy::load::{self, Mode};
use moqui_lib::object::{to_plain_string, Value};

const FILES: &[(&str, &str)] = &[
    ("component.xml", r#"<component name="hotdeploy" version="1.0"/>"#),
    (
        "MoquiConf.xml",
        r#"<moqui-conf>
    <default-property name="hotdeploy_title" value="Hot Deploy"/>
    <screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
        <subscreens-item name="hotdeploy" menu-title="${hotdeploy_title}" location="component://hotdeploy/screen/HotDeploy.xml"/>
    </screen></screen-facade>
</moqui-conf>"#,
    ),
    (
        "entity/HotEntities.xml",
        r#"<entities>
    <entity entity-name="HotThing" package="hot.deploy">
        <field name="thingId" type="id" is-pk="true"/>
        <field name="name" type="text-medium"/>
    </entity>
    <extend-entity entity-name="Enumeration" package="moqui.basic"><field name="hotNote" type="text-medium"/></extend-entity>
</entities>"#,
    ),
    (
        "service/hot/HotServices.xml",
        r#"<services>
    <service verb="count" noun="Things">
        <out-parameters><parameter name="count" type="Long"/></out-parameters>
        <actions><entity-find-count entity-name="hot.deploy.HotThing" count-field="count"/></actions>
    </service>
</services>"#,
    ),
    (
        "screen/HotDeploy.xml",
        r#"<screen xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/xml-screen-3.xsd">
    <widgets><label text="Hot deployed"/></widgets>
</screen>"#,
    ),
    (
        "data/HotSeedData.xml",
        r#"<entity-facade-xml type="seed">
    <hot.deploy.HotThing thingId="ONE" name="One"/>
    <moqui.basic.EnumerationType enumTypeId="HotType" description="Hot"/>
    <moqui.basic.Enumeration enumId="HOT_ENUM" enumTypeId="HotType" description="Hot" hotNote="extended"/>
</entity-facade-xml>"#,
    ),
    (
        "data/HotDemoData.xml",
        r#"<entity-facade-xml type="demo"><hot.deploy.HotThing thingId="DEMO" name="Demo"/></entity-facade-xml>"#,
    ),
];

fn place(runtime: &std::path::Path, component: &str, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = runtime.join("component").join(component).join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn in_context<T>(factory: &Arc<ExecutionContextFactory>, body: impl FnOnce() -> T) -> T {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    body()
}

fn field(factory: &Arc<ExecutionContextFactory>, entity: &str, key: (&str, &str), name: &str) -> Option<String> {
    let facade = factory.entity().unwrap();
    let row = facade
        .find(entity)
        .unwrap()
        .condition(moqui_db::sql::Condition::equals(key.0, Value::string(key.1)))
        .disable_authz()
        .use_cache(false)
        .one()
        .unwrap()?;
    row.get(name).map(to_plain_string)
}

#[test]
fn a_component_placed_on_a_running_server_loads_without_a_restart() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start("component-deploy");
    let factory = Arc::clone(gate.server.factory());
    place(&gate.runtime, "hotdeploy", FILES);

    let found = in_context(&factory, || load::scan(&factory)).unwrap();
    assert!(found.iter().any(|component| component.name == "hotdeploy"), "{found:?}");
    let preview = in_context(&factory, || load::preview(&factory, "hotdeploy")).unwrap();
    assert!(preview.refusals.is_empty(), "{:?}", preview.refusals);
    assert!(!preview.restart_needed, "{:#?}", preview.conf.changes);
    assert_eq!(preview.entities.added, ["hot.deploy.HotThing"]);
    assert!(preview
        .entities
        .changed
        .contains(&"moqui.basic.Enumeration".to_string()));
    assert_eq!(preview.data_types.get("seed"), Some(&1));
    assert_eq!(preview.data_types.get("demo"), Some(&1));

    let report = in_context(&factory, || load::load(&factory, "hotdeploy", Mode::Now, &[]));
    assert_eq!(report.status, "deployed", "{:#?}", report.problems);
    assert!(report.problems.is_empty(), "{:#?}", report.problems);
    assert!(factory
        .components()
        .iter()
        .any(|component| component.name == "hotdeploy"));
    // the seed data loaded, into the new table and the extended one; the demo data did not
    assert_eq!(
        field(&factory, "hot.deploy.HotThing", ("thingId", "ONE"), "name").as_deref(),
        Some("One")
    );
    assert_eq!(
        field(&factory, "hot.deploy.HotThing", ("thingId", "DEMO"), "name"),
        None
    );
    assert_eq!(
        field(&factory, "moqui.basic.Enumeration", ("enumId", "HOT_ENUM"), "hotNote").as_deref(),
        Some("extended")
    );
    // its service, its conf mount and its property
    let out = in_context(&factory, || {
        moqui_core::service::sync("hot.HotServices.count#Things", &BTreeMap::new()).unwrap()
    });
    assert_eq!(out.get("count").map(to_plain_string).as_deref(), Some("1"));
    let screens = format!("{:?}", moqui_core::screen::ScreenFacade::of(&factory).conf().screens);
    assert!(
        screens.contains("component://hotdeploy/screen/HotDeploy.xml"),
        "{screens}"
    );
    assert_eq!(
        moqui_lib::env::get_property("hotdeploy_title").as_deref(),
        Some("Hot Deploy")
    );
    assert_eq!(
        field(
            &factory,
            "moqui.server.DeployedComponent",
            ("componentName", "hotdeploy"),
            "status"
        )
        .as_deref(),
        Some("deployed")
    );

    // loaded once only; a component with Rust code waits for a build
    let again = in_context(&factory, || load::preview(&factory, "hotdeploy"));
    assert!(again.is_err());
    place(
        &gate.runtime,
        "hotnative",
        &[
            ("component.xml", r#"<component name="hotnative"/>"#),
            ("Cargo.toml", "[package]\nname = \"hotnative\"\n"),
        ],
    );
    let native = in_context(&factory, || load::preview(&factory, "hotnative")).unwrap();
    assert!(
        native.refusals.iter().any(|refusal| refusal.contains("Rust code")),
        "{:?}",
        native.refusals
    );
    moqui_lib::env::clear_property("hotdeploy_title");
}

/// The files a git deploy's preview staged move into `deployed-component` and load; without COMPONENT_DEPLOY the deploy
/// is refused. The fetch is not here: it needs a remote.
#[cfg(feature = "git")]
#[test]
fn a_staged_git_deploy_moves_into_place_and_loads() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties(
        "component-git-deploy",
        Some(
            r#"<entity-facade-xml type="seed"><moqui.security.UserAccount userId="DEPLOY_NOBODY" username="deploy.nobody" disabled="N"/></entity-facade-xml>"#,
        ),
        &[("component_deploy_enabled", "true")],
    );
    let factory = Arc::clone(gate.server.factory());
    let deployed = gate.runtime.join("deployed-component");
    let staged = deployed.join(".staging-hotgit");
    for (path, text) in [
        ("component.xml", r#"<component name="hotgit" version="1.0"/>"#),
        (
            "service/hotgit/GitServices.xml",
            r#"<services><service verb="say" noun="Hello"><out-parameters><parameter name="text"/></out-parameters>
                <actions><set field="text" value="hello"/></actions></service></services>"#,
        ),
    ] {
        let path = staged.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    in_context(&factory, || {
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.server.DeployedComponent").unwrap();
        row.set("componentName", Value::string("hotgit"));
        row.set("source", Value::string("git"));
        row.set("storage", Value::string("database"));
        row.set("location", Value::string(deployed.join("hotgit").display().to_string()));
        row.set("status", Value::string("staged"));
        row.create(&facade).unwrap();
    });
    let refused = as_user(&factory, "deploy.nobody", || {
        moqui_devkit::deploy::git::deploy(&factory, "hotgit", Mode::Now, &[])
    });
    assert_eq!(refused.status, "failed");
    assert!(
        refused.problems[0].contains("COMPONENT_DEPLOY"),
        "{:?}",
        refused.problems
    );
    assert!(staged.is_dir(), "a refused deploy leaves the staged files");
    let (report, out) = as_user(&factory, "john.doe", || {
        let report = moqui_devkit::deploy::git::deploy(&factory, "hotgit", Mode::Now, &[]);
        let out = moqui_core::service::sync("hotgit.GitServices.say#Hello", &BTreeMap::new());
        (report, out)
    });
    assert_eq!(report.status, "deployed", "{:#?}", report.problems);
    assert!(deployed.join("hotgit/component.xml").is_file());
    assert!(!staged.exists());
    assert_eq!(out.unwrap().get("text").map(to_plain_string).as_deref(), Some("hello"));
}

#[cfg(feature = "git")]
fn as_user<T>(factory: &Arc<ExecutionContextFactory>, username: &str, body: impl FnOnce() -> T) -> T {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    moqui_core::user::internal_login_user(username).unwrap();
    body()
}

/// At start a component kept in the database whose files are missing (a new container) is written out from its
/// repository, at the deployed commit, for the boot to load (plans/Component-Deploy.md, Phase 4).
#[cfg(feature = "git")]
#[test]
fn a_component_kept_in_the_database_is_written_out_when_its_files_are_missing() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start("component-restore");
    let factory = Arc::clone(gate.server.factory());
    let commit = as_user(&factory, "john.doe", || {
        let facade = factory.entity().unwrap();
        let mut repository = facade.make_value("moqui.resource.GitRepository").unwrap();
        repository.set("repositoryId", Value::string("component-hotdb"));
        repository.set("repositoryKind", Value::string("db"));
        repository.set("branch", Value::string("main"));
        repository.create(&facade).unwrap();
        let files: BTreeMap<String, Value> = [
            ("component.xml", r#"<component name="hotdb"/>"#),
            ("service/hotdb/DbServices.xml", "<services/>"),
        ]
        .into_iter()
        .map(|(path, text)| (path.to_string(), Value::string(text)))
        .collect();
        let parameters: BTreeMap<String, Value> = [
            ("repositoryId".to_string(), Value::string("component-hotdb")),
            ("message".to_string(), Value::string("first")),
            ("fileMap".to_string(), Value::Map(files.into_iter().collect())),
        ]
        .into_iter()
        .collect();
        let out = moqui_core::service::sync("org.moqui.impl.GitServices.commit#GitFiles", &parameters).unwrap();
        let commit = out.get("commitId").map(to_plain_string).unwrap();
        let mut row = facade.make_value("moqui.server.DeployedComponent").unwrap();
        row.set("componentName", Value::string("hotdb"));
        row.set("source", Value::string("git"));
        row.set("storage", Value::string("database"));
        row.set("gitRepositoryId", Value::string("component-hotdb"));
        row.set("commitId", Value::string(commit.as_str()));
        row.set("status", Value::string("deployed"));
        row.create(&facade).unwrap();
        commit
    });
    assert!(!commit.is_empty());
    let location = gate.runtime.join("deployed-component/hotdb");
    assert!(!location.exists());
    assert_eq!(moqui_devkit::deploy::git::restore(&factory), 1);
    assert_eq!(
        std::fs::read_to_string(location.join("component.xml")).unwrap(),
        r#"<component name="hotdb"/>"#
    );
    assert!(location.join("service/hotdb/DbServices.xml").is_file());
    // written once: a start with the files does nothing
    assert_eq!(moqui_devkit::deploy::git::restore(&factory), 0);
    let found = moqui_core::context::discover_again(&factory).unwrap();
    assert!(found.iter().any(|component| component.name == "hotdb"));
    // removed on another member: its files here go aside at start
    in_context(&factory, || {
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.server.DeployedComponent").unwrap();
        row.set("componentName", Value::string("hotdb"));
        row.set("status", Value::string("removed"));
        row.update(&facade).unwrap();
    });
    assert_eq!(moqui_devkit::deploy::git::restore(&factory), 1);
    assert!(!location.exists());
    assert!(gate.runtime.join("deployed-component/.removed").is_dir());
}
