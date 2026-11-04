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

use std::sync::Arc;

use moqui_lib::node::MNode;

use super::catalog::CATALOG;
use super::{apply_conf, pending_restart, preview_conf, ConfReport, Outcome};
use crate::context::ExecutionContextFactory;

const RUNNING: &str = r#"<moqui-conf>
    <default-property name="conf_reload_kept" value="kept"/>
    <webapp-list><webapp name="webroot" http-port="8080" http-host="localhost">
        <root-screen host=".*" location="component://webroot/screen/webroot.xml"/>
    </webapp></webapp-list>
    <screen-facade>
        <screen location="component://webroot/screen/webroot/apps.xml">
            <subscreens-item name="system" location="component://tools/screen/System.xml"/>
        </screen>
    </screen-facade>
    <entity-facade><load-entity location="component://a/entity/A.xml"/></entity-facade>
</moqui-conf>"#;

fn factory(conf: &str) -> Arc<ExecutionContextFactory> {
    let ec = ExecutionContextFactory::test_context_with_conf(conf);
    Arc::clone(ec.factory())
}

/// The conf of `factory` as a tree to change, as the files would make it.
fn disk(factory: &ExecutionContextFactory) -> MNode {
    MNode::parse_text("disk", &factory.conf_text()).unwrap()
}

fn change<'a>(report: &'a ConfReport, unit: &str, attribute: Option<&str>) -> &'a super::ConfChange {
    report
        .changes
        .iter()
        .find(|change| change.unit == unit && change.attribute.as_deref() == attribute)
        .unwrap_or_else(|| panic!("no change of {unit} {attribute:?} in {:#?}", report.changes))
}

#[test]
fn no_change_is_reported_for_the_same_conf() {
    let factory = factory(RUNNING);
    let report = apply_conf(&factory, &disk(&factory)).unwrap();
    assert!(report.changes.is_empty(), "{:#?}", report.changes);
    assert_eq!(factory.conf_generation(), 0);
}

/// A new mount is applied: the running conf takes it, the generation moves, and the screen facade reads it.
#[test]
fn a_new_mount_applies_and_the_screen_facade_reads_it() {
    let factory = factory(RUNNING);
    let before = crate::screen::ScreenFacade::of(&factory);
    let screens = before.conf();
    let apps = "component://webroot/screen/webroot/apps.xml";
    assert!(screens.screens.contains_key(apps));
    let disk = disk(&factory);
    let screen = disk
        .first_named("screen-facade")
        .and_then(|facade| facade.first_named("screen"))
        .unwrap();
    let item = screen.append("subscreens-item");
    item.set_attribute("name", "deployed");
    item.set_attribute("location", "component://deployed/screen/Deployed.xml");

    let preview = preview_conf(&factory, &disk).unwrap();
    let mount = format!("screen-facade/screen[{apps}]/subscreens-item[deployed]");
    assert_eq!(change(&preview, &mount, None).outcome, Outcome::WouldApply);
    assert_eq!(factory.conf_generation(), 0, "a preview changes nothing");

    let report = apply_conf(&factory, &disk).unwrap();
    let found = change(&report, &mount, None);
    assert_eq!(
        (found.kind, found.class, found.group),
        ("added", "apply", Some("screens"))
    );
    assert_eq!(found.outcome, Outcome::Applied);
    assert_eq!(factory.conf_generation(), 1);
    assert!(factory.conf_text().contains("component://deployed/screen/Deployed.xml"));
    let rendered = format!(
        "{:?}",
        crate::screen::ScreenFacade::of(&factory).conf().screens.get(apps)
    );
    assert!(rendered.contains("deployed"), "{rendered}");
    // applied once, the files and the running conf agree
    assert!(preview_conf(&factory, &disk).unwrap().changes.is_empty());
}

/// A changed port waits for a restart and stays out of the running conf, so the next reload reports it again; a
/// change beside it that can be live is applied.
#[test]
fn a_port_change_waits_for_a_restart_beside_a_live_change() {
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    let webapp = disk
        .first_named("webapp-list")
        .and_then(|list| list.first_named("webapp"))
        .unwrap();
    webapp.set_attribute("http-port", "8081");
    webapp.set_attribute("http-host", "example.com");
    let report = apply_conf(&factory, &disk).unwrap();
    let port = change(&report, "webapp-list/webapp[webroot]", Some("http-port"));
    assert_eq!((port.class, port.outcome), ("restart", Outcome::PendingRestart));
    let host = change(&report, "webapp-list/webapp[webroot]", Some("http-host"));
    assert_eq!((host.class, host.outcome), ("live", Outcome::Applied));
    assert_eq!(factory.webapp_setting("webroot", "http-host"), "example.com");
    assert_eq!(factory.webapp_setting("webroot", "http-port"), "8080");
    assert_eq!(pending_restart(&factory).len(), 1);
    let again = preview_conf(&factory, &disk).unwrap();
    assert_eq!(again.changes.len(), 1, "{:#?}", again.changes);
    assert_eq!(again.changes[0].attribute.as_deref(), Some("http-port"));
}

/// A new element with an attribute that needs a restart waits whole: a webapp with a port.
#[test]
fn an_added_element_waits_whole_when_part_of_it_needs_a_restart() {
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    let list = disk.first_named("webapp-list").unwrap();
    let webapp = list.append("webapp");
    webapp.set_attribute("name", "second");
    webapp.set_attribute("http-port", "9090");
    let screen = webapp.append("root-screen");
    screen.set_attribute("host", ".*");
    screen.set_attribute("location", "component://second/screen/Root.xml");
    let report = apply_conf(&factory, &disk).unwrap();
    assert!(
        report
            .changes
            .iter()
            .all(|change| change.outcome == Outcome::PendingRestart),
        "{:#?}",
        report.changes
    );
    assert!(!factory.conf_text().contains("second"));
}

/// An element at the top of the conf the catalog does not know needs a restart; an appended list compares as a
/// sequence.
#[test]
fn unknown_elements_and_appended_lists_need_a_restart() {
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    disk.append("future-facade").set_attribute("setting", "on");
    disk.first_named("entity-facade")
        .unwrap()
        .append("load-entity")
        .set_attribute("location", "component://b/entity/B.xml");
    let report = preview_conf(&factory, &disk).unwrap();
    let unknown = change(&report, "future-facade", None);
    assert_eq!((unknown.kind, unknown.outcome), ("added", Outcome::PendingRestart));
    let load = change(&report, "entity-facade/load-entity", None);
    assert_eq!((load.kind, load.class), ("changed", "restart"));
}

/// A setting a later conf overrides shows no change: the whole conf is merged again, so the runtime conf's value of a
/// mount a component adds is the one compared.
#[test]
fn a_setting_a_later_conf_overrides_is_no_change() {
    let base = MNode::parse_text("base", RUNNING).unwrap();
    let runtime = MNode::parse_text(
        "runtime",
        r#"<moqui-conf><screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
            <subscreens-item name="system" location="component://site/screen/System.xml"/>
        </screen></screen-facade></moqui-conf>"#,
    )
    .unwrap();
    let component = MNode::parse_text(
        "component",
        r#"<moqui-conf><screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
            <subscreens-item name="system" location="component://other/screen/System.xml"/>
        </screen></screen-facade></moqui-conf>"#,
    )
    .unwrap();
    crate::context::merge_conf(&base, &runtime);
    let factory = factory(&base.write_string());
    let disk = MNode::parse_text("disk", RUNNING).unwrap();
    crate::context::merge_conf(&disk, &component);
    crate::context::merge_conf(&disk, &runtime);
    let report = preview_conf(&factory, &disk).unwrap();
    assert!(
        !report
            .changes
            .iter()
            .any(|change| change.unit.contains("subscreens-item[system]")),
        "{:#?}",
        report.changes
    );
}

/// A new property is set as boot sets it, and a unit that reads it is reported; one the environment already set is
/// left as it is.
#[test]
fn a_new_property_is_set_and_its_readers_are_reported() {
    let name = format!("conf_reload_new_{}", std::process::id());
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    let property = MNode::new("default-property");
    property.set_attribute("name", name.as_str());
    property.set_attribute("value", "/qapps/Deployed");
    disk.insert_node(0, property);
    disk.first_named("webapp-list")
        .and_then(|list| list.first_named("webapp"))
        .unwrap()
        .append("error-screen")
        .set_attribute("screen-path", format!("${{{name}}}"));
    let report = apply_conf(&factory, &disk).unwrap();
    let set = change(&report, &format!("default-property[{name}]"), None);
    assert_eq!(set.outcome, Outcome::Applied, "{:#?}", report.changes);
    assert_eq!(moqui_lib::env::get_property(&name).as_deref(), Some("/qapps/Deployed"));
    assert!(report.changes.iter().any(|change| change.kind == "property"));

    let preset = format!("conf_reload_preset_{}", std::process::id());
    moqui_lib::env::set_property(&preset, "from-env".to_string());
    let again = MNode::parse_text("disk", &factory.conf_text()).unwrap();
    let property = MNode::new("default-property");
    property.set_attribute("name", preset.as_str());
    property.set_attribute("value", "from-conf");
    again.insert_node(0, property);
    let report = apply_conf(&factory, &again).unwrap();
    assert_eq!(
        change(&report, &format!("default-property[{preset}]"), None).outcome,
        Outcome::NoEffect
    );
    assert_eq!(moqui_lib::env::get_property(&preset).as_deref(), Some("from-env"));
    moqui_lib::env::clear_property(&name);
    moqui_lib::env::clear_property(&preset);
}

/// A property the process was set up with waits for a restart.
#[test]
fn a_fixed_property_waits_for_a_restart() {
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    let property = MNode::new("default-property");
    property.set_attribute("name", "webapp_http_port");
    property.set_attribute("value", "9999");
    disk.insert_node(0, property);
    let report = preview_conf(&factory, &disk).unwrap();
    assert_eq!(
        change(&report, "default-property[webapp_http_port]", None).outcome,
        Outcome::PendingRestart
    );
}

/// A changed value is set only while the property still has the value the conf set: once the environment changed it
/// the change has no effect and the environment's value stays; while it has the conf's value the change is applied.
#[test]
fn a_changed_property_value_yields_to_the_environment() {
    let name = format!("conf_reload_changed_{}", std::process::id());
    let conf = RUNNING.replacen(
        "<moqui-conf>",
        &format!("<moqui-conf><default-property name=\"{name}\" value=\"boot\"/>"),
        1,
    );
    let changed = |factory: &ExecutionContextFactory| {
        let disk = disk(factory);
        disk.children_named("default-property")
            .into_iter()
            .find(|node| node.attribute("name").as_deref() == Some(name.as_str()))
            .unwrap()
            .set_attribute("value", "changed");
        disk
    };
    let unit = format!("default-property[{name}]");

    let overridden = factory(&conf);
    overridden.conf_properties().insert(name.clone(), "boot".to_string());
    moqui_lib::env::set_property(&name, "from-env".to_string());
    let report = apply_conf(&overridden, &changed(&overridden)).unwrap();
    let found = change(&report, &unit, Some("value"));
    assert_eq!(found.outcome, Outcome::NoEffect, "{:#?}", report.changes);
    assert!(found.reason.contains("environment"), "{}", found.reason);
    assert_eq!(moqui_lib::env::get_property(&name).as_deref(), Some("from-env"));

    let from_conf = factory(&conf);
    from_conf.conf_properties().insert(name.clone(), "boot".to_string());
    moqui_lib::env::set_property(&name, "boot".to_string());
    let report = apply_conf(&from_conf, &changed(&from_conf)).unwrap();
    assert_eq!(
        change(&report, &unit, Some("value")).outcome,
        Outcome::Applied,
        "{:#?}",
        report.changes
    );
    assert_eq!(moqui_lib::env::get_property(&name).as_deref(), Some("changed"));
    moqui_lib::env::clear_property(&name);
}

/// A property the conf no longer sets waits for a restart, which clears it; the running conf keeps it until then.
#[test]
fn a_removed_property_waits_for_a_restart() {
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    assert!(disk.remove_where(|node| {
        node.name() == "default-property" && node.attribute("name").as_deref() == Some("conf_reload_kept")
    }));
    let report = apply_conf(&factory, &disk).unwrap();
    let removed = change(&report, "default-property[conf_reload_kept]", None);
    assert_eq!(
        (removed.kind, removed.class, removed.outcome),
        ("removed", "restart", Outcome::PendingRestart)
    );
    assert!(removed.reason.contains("next start"), "{}", removed.reason);
    assert!(factory.conf_text().contains("conf_reload_kept"));
}

/// A new secret property at its `CHANGEME` default is generated as boot generates one, and saved to the runtime's
/// `.env`, not the default.
#[test]
fn a_new_changeme_secret_is_generated_on_apply() {
    use crate::context::tests::{repo_env_stamp, workspace_root, EnvGuard, TempRuntime};
    let _lock = crate::user::session::cookie_property_lock();
    let root = workspace_root();
    let _env = EnvGuard::new(root.clone());
    let before = repo_env_stamp(&root);
    let temp = TempRuntime::new("conf-reload-secret");
    let factory =
        ExecutionContextFactory::boot_with_framework(temp.path(), root.join("runtime/base-component/framework"))
            .unwrap();
    let name = format!("conf_reload_new_pass_{}", std::process::id());
    let disk = disk(&factory);
    let property = MNode::new("default-property");
    property.set_attribute("name", name.as_str());
    property.set_attribute("value", "reload-secret-CHANGEME");
    disk.insert_node(0, property);
    let report = apply_conf(&factory, &disk).unwrap();
    assert_eq!(
        change(&report, &format!("default-property[{name}]"), None).outcome,
        Outcome::Applied,
        "{:#?}",
        report.changes
    );
    let value = moqui_lib::env::get_property(&name).unwrap_or_default();
    assert_eq!(value.len(), 44, "{value}");
    assert!(!value.contains("CHANGEME"));
    let env_text = std::fs::read_to_string(temp.path().join(".env")).unwrap();
    assert!(env_text.contains(&format!("{name}=")), "{env_text}");
    assert!(!env_text.contains("reload-secret-CHANGEME"));
    assert_eq!(repo_env_stamp(&root), before);
    moqui_lib::env::clear_property(&name);
    for name in factory.property_names() {
        moqui_lib::env::clear_property(name);
    }
}

/// Every element at the top of the default conf has a unit, so none is reported as needing a restart only because
/// the catalog does not know it.
#[test]
fn every_top_level_element_of_the_default_conf_has_a_unit() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/base-component/framework/conf/MoquiDefaultConf.xml"
    );
    let conf = MNode::parse_file(path).unwrap();
    for node in conf.children() {
        let name = node.name();
        assert!(
            CATALOG.iter().any(|unit| unit.name == name),
            "{name} is in MoquiDefaultConf.xml but not in the conf reload catalog"
        );
    }
}

#[test]
fn a_property_reference_is_found_whole() {
    assert!(super::diff::refers("${webapp_http_port}", "webapp_http_port"));
    assert!(super::diff::refers("${webapp_http_port ?: '8080'}", "webapp_http_port"));
    assert!(!super::diff::refers("${webapp_http_port_x}", "webapp_http_port"));
    assert!(!super::diff::refers("webapp_http_port", "webapp_http_port"));
}
