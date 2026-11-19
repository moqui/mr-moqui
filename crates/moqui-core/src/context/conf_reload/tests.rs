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

use crate::context::conf_schema::CATALOG;
use super::{apply_conf, pending_restart, preview_conf, ConfReport, Outcome};
use crate::context::ExecutionContextFactory;

const RUNNING: &str = r#"<moqui-conf>
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

/// An element at the top of the conf the catalog does not know is a component's own and applies live; an appended
/// list compares as a sequence and needs a restart.
#[test]
fn a_components_element_applies_and_an_appended_list_needs_a_restart() {
    let factory = factory(RUNNING);
    let disk = disk(&factory);
    disk.append("future-facade").set_attribute("setting", "on");
    disk.first_named("entity-facade")
        .unwrap()
        .append("load-entity")
        .set_attribute("location", "component://b/entity/B.xml");
    let report = preview_conf(&factory, &disk).unwrap();
    let unknown = change(&report, "future-facade", None);
    assert_eq!((unknown.kind, unknown.outcome), ("added", Outcome::WouldApply));
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

/// Every element at the top of the default conf has a unit, so none is reported as needing a restart only because
/// the catalog does not know it.
#[test]
fn every_top_level_element_of_the_default_conf_has_a_unit() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/framework/conf/MoquiDefaultConf.xml"
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

/// A conf that still uses the retired properties, a `default-property` or an attribute with a `${}`, is refused whole
/// with each named (plans/archive/Conf-Paths.md, phase 7); nothing of it is applied.
#[test]
fn a_conf_with_a_retired_property_is_refused() {
    let factory = factory(RUNNING);
    let with_property = disk(&factory);
    let property = MNode::new("default-property");
    property.set_attribute("name", "webapp_http_port");
    property.set_attribute("value", "9999");
    with_property.insert_node(0, property);
    let err = apply_conf(&factory, &with_property).unwrap_err().to_string();
    assert!(err.contains("default-property name=\"webapp_http_port\""), "{err}");
    let with_expression = disk(&factory);
    with_expression
        .first_named("webapp-list")
        .and_then(|list| list.first_named("webapp"))
        .unwrap()
        .set_attribute("http-host", "${webapp_http_host}");
    let err = preview_conf(&factory, &with_expression).unwrap_err().to_string();
    assert!(
        err.contains("webapp-list/webapp[webroot]@http-host holds a ${}"),
        "{err}"
    );
    assert!(
        err.contains("moqui_conf__webapp_list__webapp__WEBROOT__http_host"),
        "{err}"
    );
    assert_eq!(factory.conf_generation(), 0, "nothing applied");
}

/// A report never shows a secret, though overrides and generated secrets are in the running conf: an attribute the
/// schema or its name marks secret is masked in a change's values and inside an element's text, and its change still
/// counts. A component's own element is live: its readers read the conf on each use.
#[test]
fn a_report_masks_secrets_and_a_components_element_is_live() {
    let factory = factory(r#"<moqui-conf><smtp-server password="old-pass"/></moqui-conf>"#);
    let disk = disk(&factory);
    disk.first_named("smtp-server")
        .unwrap()
        .set_attribute("password", "new-pass");
    let pass = MNode::new("pass");
    pass.set_attribute("backend-url", "https://authority.example");
    pass.set_attribute("client-secret", "pass-client-secret");
    disk.append_node(pass);
    let report = preview_conf(&factory, &disk).unwrap();
    let text = format!("{report:?}");
    for secret in ["old-pass", "new-pass", "pass-client-secret"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let password = change(&report, "smtp-server", Some("password"));
    assert_eq!(password.old.as_deref(), Some(super::view::MASKED));
    assert_eq!(password.new.as_deref(), Some(super::view::MASKED));
    let added = change(&report, "pass", None);
    assert_eq!(added.class, "live");
    assert!(added
        .new
        .as_deref()
        .unwrap_or_default()
        .contains("https://authority.example"));
    let applied = apply_conf(&factory, &disk).unwrap();
    assert!(applied.applied > 0);
    assert_eq!(
        factory
            .conf_value("pass@backend-url")
            .map(|found| found.value)
            .as_deref(),
        Some("https://authority.example")
    );
}

/// A setting a spec sets over the conf (`withConf`) on an element the conf does not have makes the element, and putting
/// it back takes the element away again, so the running conf is as before and a preview finds no change.
#[test]
fn a_put_back_removes_the_element_a_set_made() {
    let factory = factory(RUNNING);
    let before = factory.conf_text();
    super::set_value(&factory, "demo@test-login", Some("true"), false).unwrap();
    assert!(factory.conf_text().contains("test-login=\"true\""), "{}", factory.conf_text());
    super::set_value(&factory, "demo@test-login", None, false).unwrap();
    let after = factory.conf_text();
    assert!(!after.contains("<demo"), "{after}");
    assert_eq!(
        MNode::parse_text("after", &after).unwrap().write_string(),
        MNode::parse_text("before", &before).unwrap().write_string()
    );
    // an element the conf has keeps its place when its attribute is put back
    super::set_value(&factory, "screen-facade@check-interval-ms", Some("5"), false).unwrap();
    super::set_value(&factory, "screen-facade@check-interval-ms", None, false).unwrap();
    assert!(factory.conf_text().contains("<screen-facade"), "{}", factory.conf_text());
}
