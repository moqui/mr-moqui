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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use moqui_lib::env;
use moqui_lib::node::MNode;

use super::{
    apply_conf, on_apply, pending_restart, preview_conf, set_class, set_with, set_with_holding, Applying, ConfReport,
    Inputs, Kept, Outcome, SetOutcome,
};
use crate::context::conf_env::{self, RetiredValue, Source};
use crate::context::conf_schema::{self, Class, Group, CATALOG};
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
    assert!(
        factory.conf_text().contains("test-login=\"true\""),
        "{}",
        factory.conf_text()
    );
    super::set_value(&factory, "demo@test-login", None, false).unwrap();
    // a spec's end puts it back once more (restoreProperties): that resolve makes no element either
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
    assert!(
        factory.conf_text().contains("<screen-facade"),
        "{}",
        factory.conf_text()
    );
}

/// A runtime directory of its own for a test that keeps settings in `.env`. Writing it also writes the process's loaded
/// `.env` table, which a merge with no `.env` file reads (the view's tests): a test puts back what it kept there, and
/// names elements no other test looks at.
fn runtime(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-conf-set-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn no_environment(_: &str) -> Option<String> {
    None
}

fn inputs<'a>(
    runtime: &Path,
    environment: &'a dyn Fn(&str) -> Option<String>,
    retired: &'a [RetiredValue],
) -> Inputs<'a> {
    Inputs {
        environment,
        retired,
        env_file: runtime.join(".env"),
        secrets_in_env: true,
    }
}

fn dotenv(runtime: &Path) -> String {
    std::fs::read_to_string(runtime.join(".env")).unwrap_or_default()
}

fn value(factory: &ExecutionContextFactory, selector: &str) -> Option<String> {
    factory.conf_value(selector).map(|found| found.value)
}

/// A hook on `group` that counts its runs and notes whether the set's lock was free in each.
fn counting(factory: &ExecutionContextFactory, group: Group) -> (Arc<AtomicUsize>, Arc<AtomicBool>) {
    let (count, free) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(true)));
    let (counted, unlocked) = (Arc::clone(&count), Arc::clone(&free));
    on_apply(factory, group, move |factory| {
        counted.fetch_add(1, Ordering::SeqCst);
        let lock_free = factory.extension(|_| Applying::default()).0.try_lock().is_ok();
        unlocked.fetch_and(lock_free, Ordering::SeqCst);
    });
    (count, free)
}

const RATE_LIMIT: &str = "webapp-list/webapp[webroot]/web-protection/rate-limit";

/// A setting set while running takes effect as a reload does (plans/Runtime-Config.md §3.2): its group's hook runs,
/// once, after the set has let go of its lock, so a hook may set the conf itself.
#[test]
fn a_set_runs_its_groups_hook_once_after_the_lock() {
    let factory = factory(RUNNING);
    let runtime = runtime("hook-once");
    let (count, free) = counting(&factory, Group::WebProtection);
    let selector = format!("{RATE_LIMIT}[rc1-once]@limit");
    let outcomes = set_with(
        &factory,
        &[(selector.as_str(), Some("5"))],
        true,
        &inputs(&runtime, &no_environment, &[]),
    )
    .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(free.load(Ordering::SeqCst), "the hook ran with the set's lock held");
    let outcome = &outcomes[0];
    assert_eq!(outcome.selector, selector);
    assert_eq!(outcome.class, Class::Apply(Group::WebProtection));
    assert_eq!((outcome.kept, outcome.outranked_by), (Kept::DotEnv, None));
    assert_eq!(outcome.previous, None);
    assert!(outcome.problems.is_empty(), "{:?}", outcome.problems);
    assert!(outcome.note().contains("runtime/.env") && outcome.note().contains("applies now"));
    assert_eq!(value(&factory, &selector).as_deref(), Some("5"));
    assert!(
        dotenv(&runtime).contains(&format!("{}=5", outcome.env_name)),
        "{}",
        dotenv(&runtime)
    );
    set_with(
        &factory,
        &[(selector.as_str(), None)],
        true,
        &inputs(&runtime, &no_environment, &[]),
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A set that holds a group runs none of its hooks, and the other groups' as ever: the listener services apply their
/// settings and then ask the listener registry for the one pass that applies them (plans/Runtime-Config.md §3.6).
#[test]
fn a_set_holding_a_group_runs_the_other_groups_hooks_only() {
    let factory = factory(RUNNING);
    let runtime = runtime("holding");
    let (listeners, _) = counting(&factory, Group::Listeners);
    let (protection, _) = counting(&factory, Group::WebProtection);
    let port = "webapp-list/webapp[webroot]@http-listen-port";
    let limit = format!("{RATE_LIMIT}[rc2-held]@limit");
    let given = inputs(&runtime, &no_environment, &[]);
    let outcomes = set_with_holding(
        &factory,
        &[(port, Some("8181")), (limit.as_str(), Some("5"))],
        true,
        &given,
        Some(Group::Listeners),
    )
    .unwrap();
    assert_eq!(outcomes[0].class, Class::Apply(Group::Listeners));
    assert_eq!(
        listeners.load(Ordering::SeqCst),
        0,
        "the held group's hooks did not run"
    );
    assert_eq!(protection.load(Ordering::SeqCst), 1);
    assert_eq!(value(&factory, port).as_deref(), Some("8181"));
    set_with(&factory, &[(port, None), (limit.as_str(), None)], true, &given).unwrap();
    assert_eq!(
        listeners.load(Ordering::SeqCst),
        1,
        "a set that holds nothing runs them"
    );
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A snapshot of the running conf reads every setting as of when it was taken: a set after it is not in it.
#[test]
fn a_conf_snapshot_keeps_the_settings_of_when_it_was_taken() {
    let factory = factory(RUNNING);
    let runtime = runtime("snapshot");
    let given = inputs(&runtime, &no_environment, &[]);
    let (port, bind) = (
        "webapp-list/webapp[webroot]@http-listen-port",
        "webapp-list/webapp[webroot]@http-bind",
    );
    set_with(
        &factory,
        &[(port, Some("8181")), (bind, Some("127.0.0.1"))],
        true,
        &given,
    )
    .unwrap();
    let before = factory.conf_snapshot();
    set_with(
        &factory,
        &[(port, Some("8282")), (bind, Some("127.0.0.2"))],
        true,
        &given,
    )
    .unwrap();
    assert_eq!(before.value(port).as_deref(), Some("8181"));
    assert_eq!(before.value(bind).as_deref(), Some("127.0.0.1"));
    assert_eq!(factory.conf_snapshot().value(port).as_deref(), Some("8282"));
    assert_eq!(before.value("not a selector"), None);
    set_with(&factory, &[(port, None), (bind, None)], true, &given).unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// Several settings set together run each group's hook once, at the end; one refused (a retired value, an attribute
/// named twice, a renamed attribute) refuses the whole list before anything is written.
#[test]
fn set_many_runs_the_hook_once_and_one_refused_value_refuses_the_list() {
    const RETIRED: &[RetiredValue] = &[("sip-server@rc1-port", &["", "off"], "sip-server@rc1-enabled", "")];
    let factory = factory(RUNNING);
    let runtime = runtime("many");
    let (count, _) = counting(&factory, Group::WebProtection);
    let limit = format!("{RATE_LIMIT}[rc1-many]@limit");
    let window = format!("{RATE_LIMIT}[rc1-many]@window-seconds");
    let given = inputs(&runtime, &no_environment, RETIRED);
    let outcomes = set_with(
        &factory,
        &[(limit.as_str(), Some("5")), (window.as_str(), Some("60"))],
        true,
        &given,
    )
    .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(value(&factory, &window).as_deref(), Some("60"));

    let (generation, kept) = (factory.conf_generation(), dotenv(&runtime));
    let err = set_with(
        &factory,
        &[(limit.as_str(), Some("6")), ("sip-server@rc1-port", Some(" OFF"))],
        true,
        &given,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("sip-server@rc1-port")
            && err.contains("moqui_conf__sip_server__rc1_port")
            && err.contains("moqui_conf__sip_server__rc1_enabled"),
        "{err}"
    );
    let err = set_with(
        &factory,
        &[(limit.as_str(), Some("7")), (limit.as_str(), Some("8"))],
        true,
        &given,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("given twice"), "{err}");
    // a renamed attribute is refused as the boot refuses it, naming what replaced it (plans/Runtime-Config.md §3.4)
    let err = set_with(
        &factory,
        &[(limit.as_str(), Some("9")), ("ssh-server@login-dialog", Some("false"))],
        true,
        &given,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("ssh-server@access") && err.contains("access=\"login\""),
        "{err}"
    );
    assert_eq!(factory.conf_generation(), generation, "nothing applied");
    assert_eq!(dotenv(&runtime), kept, "nothing kept");
    assert_eq!(value(&factory, &limit).as_deref(), Some("5"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    set_with(
        &factory,
        &[(limit.as_str(), None), (window.as_str(), None)],
        true,
        &given,
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A setting read once at boot is set and kept but waits for the restart: its outcome says so, and no hook runs.
#[test]
fn a_restart_class_setting_is_reported_with_its_class_and_runs_no_hook() {
    let factory = factory(RUNNING);
    let runtime = runtime("restart");
    let counts: Vec<Arc<AtomicUsize>> = [
        Group::Screens,
        Group::Derived,
        Group::Caches,
        Group::WebProtection,
        Group::Listeners,
    ]
    .into_iter()
    .map(|group| counting(&factory, group).0)
    .collect();
    let given = inputs(&runtime, &no_environment, &[]);
    let outcome = set_with(&factory, &[("sip-server@rc1-restart", Some("5060"))], true, &given)
        .unwrap()
        .remove(0);
    assert!(matches!(outcome.class, Class::Restart(_)), "{:?}", outcome.class);
    assert_eq!(outcome.class, conf_schema::attribute_class(&["sip-server"], "port"));
    assert!(
        outcome.note().contains("applies at the next start"),
        "{}",
        outcome.note()
    );
    assert!(counts.iter().all(|count| count.load(Ordering::SeqCst) == 0));
    set_with(&factory, &[("sip-server@rc1-restart", None)], true, &given).unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// `.env` does not outrank the OS environment, so a persisted setting the environment names is kept as a process
/// property, which does outrank it, in readers and in a reload, until the restart (plans/Runtime-Config.md §3.6).
#[test]
fn a_setting_the_environment_names_is_kept_until_the_restart_and_holds_in_a_reload() {
    let conf = r#"<moqui-conf><rc1-env-set port="1"/></moqui-conf>"#;
    let factory = factory(conf);
    let runtime = runtime("environment");
    let name = "moqui_conf__rc1_env_set__port";
    let environment = |asked: &str| (asked == name).then(|| "2".to_string());
    let outcome = set_with(
        &factory,
        &[("rc1-env-set@port", Some("3"))],
        true,
        &inputs(&runtime, &environment, &[]),
    )
    .unwrap()
    .remove(0);
    assert_eq!((outcome.kept, outcome.outranked_by), (Kept::UntilRestart, None));
    assert_eq!(outcome.previous.as_deref(), Some("1"));
    assert!(outcome.note().contains("until it restarts") && outcome.note().contains(name));
    assert_eq!(env::get_property(name).as_deref(), Some("3"));
    assert!(!dotenv(&runtime).contains(name), "{}", dotenv(&runtime));
    assert_eq!(value(&factory, "rc1-env-set@port").as_deref(), Some("3"));
    // a reload merges the files again with the environment's names, a process property above the environment
    let disk = disk(&factory);
    disk.first_named("rc1-env-set").unwrap().set_attribute("port", "1");
    let names = vec![
        (name.to_string(), env::get_property(name).unwrap(), Source::Property),
        (name.to_string(), "2".to_string(), Source::Environment),
    ];
    let _ = conf_env::apply_from(&disk, names);
    let report = apply_conf(&factory, &disk).unwrap();
    assert!(report.changes.is_empty(), "{:#?}", report.changes);
    assert_eq!(value(&factory, "rc1-env-set@port").as_deref(), Some("3"));
    env::clear_property(name);
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A persisted setting clears a process property of its name, which would shadow it; a `-D` the boot took is named as
/// what sets it again at the next start.
#[test]
fn a_persisted_set_clears_a_stale_property_and_names_a_boot_property_over_it() {
    let factory = factory(RUNNING);
    let runtime = runtime("stale");
    let (selector, name) = ("rc1-stale@port", "moqui_conf__rc1_stale__port");
    env::set_property(name, "stale");
    conf_env::keep_boot_overrides(
        &factory,
        vec![conf_env::Override {
            name: name.to_string(),
            source: Source::Property,
            selector: Some(selector.to_string()),
            secret: false,
            outcome: conf_env::Outcome::Set,
        }],
    );
    let given = inputs(&runtime, &no_environment, &[]);
    let outcome = set_with(&factory, &[(selector, Some("new"))], true, &given)
        .unwrap()
        .remove(0);
    assert_eq!(
        (outcome.kept, outcome.outranked_by),
        (Kept::DotEnv, Some(Source::Property))
    );
    assert!(outcome.note().contains("-D"), "{}", outcome.note());
    assert_eq!(env::get_property(name), None);
    assert!(
        dotenv(&runtime).contains(&format!("{name}=new")),
        "{}",
        dotenv(&runtime)
    );
    assert_eq!(
        conf_env::conf_value_now(&factory, selector)
            .map(|found| found.value)
            .as_deref(),
        Some("new")
    );
    set_with(&factory, &[(selector, None)], true, &given).unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// What a persisted set reports as kept before is `.env`'s line, read from the file: the table loaded at start may be
/// another runtime's, or stale.
#[test]
fn the_dotenv_value_reported_as_before_is_read_from_the_file() {
    let factory = factory(RUNNING);
    let runtime = runtime("before");
    let (selector, name) = ("rc1-before@port", "moqui_conf__rc1_before__port");
    std::fs::write(runtime.join(".env"), format!("{name}=from-the-file\n")).unwrap();
    env::load_dotenv(&format!("{name}=from-the-table"));
    let given = inputs(&runtime, &no_environment, &[]);
    let outcome = set_with(&factory, &[(selector, Some("set"))], true, &given)
        .unwrap()
        .remove(0);
    assert_eq!(outcome.before_kept.as_deref(), Some("from-the-file"));
    assert_eq!(outcome.previous, None);
    let back = set_with(&factory, &[(selector, None)], true, &given).unwrap().remove(0);
    assert_eq!(back.before_kept.as_deref(), Some("set"));
    assert_eq!(back.previous.as_deref(), Some("set"));
    assert!(!dotenv(&runtime).contains(name), "{}", dotenv(&runtime));
    assert_eq!(value(&factory, selector), None);
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A reload lets go of its lock before its hooks run, so a hook that sets a setting does not wait for itself.
#[test]
fn a_reload_hook_that_sets_a_setting_does_not_deadlock() {
    let factory = factory(RUNNING);
    let once = Arc::new(AtomicBool::new(false));
    let first = Arc::clone(&once);
    on_apply(&factory, Group::Screens, move |factory| {
        if !first.swap(true, Ordering::SeqCst) {
            super::set_value(factory, "rc1-from-hook@value", Some("set"), false).unwrap();
        }
    });
    let disk = disk(&factory);
    disk.first_named("screen-facade")
        .and_then(|facade| facade.first_named("screen"))
        .unwrap()
        .append("subscreens-item")
        .set_attribute("name", "rc1-hook");
    // a node is not sent between threads: its text is
    let disk = disk.write_string();
    let (done, finished) = std::sync::mpsc::channel();
    let reloading = Arc::clone(&factory);
    std::thread::spawn(move || {
        let disk = MNode::parse_text("disk", &disk).unwrap();
        let _ = done.send(apply_conf(&reloading, &disk).map(|report| report.applied));
    });
    let applied = finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the reload ended")
        .unwrap();
    assert!(applied > 0);
    assert!(once.load(Ordering::SeqCst));
    assert_eq!(value(&factory, "rc1-from-hook@value").as_deref(), Some("set"));
    env::clear_property("moqui_conf__rc1_from_hook__value");
}

/// A group's hooks run one run at a time, and again when the conf moved on while they ran (here a hook that sets a
/// setting of its own group), so the last run reads the latest conf.
#[test]
fn a_groups_hooks_run_again_when_the_conf_moved_on_while_they_ran() {
    let factory = factory(RUNNING);
    let selector = format!("{RATE_LIMIT}[rc1-again]@limit");
    let seen: Arc<std::sync::Mutex<Vec<Option<String>>>> = Arc::default();
    let (recorded, read) = (Arc::clone(&seen), selector.clone());
    on_apply(&factory, Group::WebProtection, move |factory| {
        let now = value(factory, &read);
        moqui_lib::sync::lock(&recorded).push(now.clone());
        if now.as_deref() == Some("1") {
            super::set_value(factory, &read, Some("2"), false).unwrap();
        }
    });
    super::set_value(&factory, &selector, Some("1"), false).unwrap();
    assert_eq!(
        *moqui_lib::sync::lock(&seen),
        [Some("1".to_string()), Some("2".to_string())]
    );
    super::set_value(&factory, &selector, None, false).unwrap();
}

/// Wait up to 5 s for `done`.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let asked = std::time::Instant::now();
    while !done() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(5),
            "{what} did not happen"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Whether a caller besides the run in progress has asked `group`'s hooks to run.
fn another_asked(factory: &ExecutionContextFactory, group: Group) -> bool {
    let runner = factory.extension(|_| super::HookRunner::default());
    let groups = moqui_lib::sync::lock(&runner.groups);
    groups.get(&group).is_some_and(|runs| runs.requested > runs.started)
}

/// Two sets at once run their group's hooks one after the other, never together, and the last run reads the conf both
/// left: the first run is held until the second set waits for it, so they surely meet.
#[test]
fn two_sets_at_once_run_their_hooks_one_at_a_time_and_the_last_reads_the_latest() {
    let factory = factory(RUNNING);
    let (limit, window) = (
        format!("{RATE_LIMIT}[rc1-together]@limit"),
        format!("{RATE_LIMIT}[rc1-together]@window-seconds"),
    );
    let inside = Arc::new(AtomicBool::new(false));
    let overlapped = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    let last: Arc<std::sync::Mutex<(Option<String>, Option<String>)>> = Arc::default();
    let (held, crossed, counted, seen, read) = (
        Arc::clone(&inside),
        Arc::clone(&overlapped),
        Arc::clone(&runs),
        Arc::clone(&last),
        (limit.clone(), window.clone()),
    );
    on_apply(&factory, Group::WebProtection, move |factory| {
        if held.swap(true, Ordering::SeqCst) {
            crossed.store(true, Ordering::SeqCst);
        }
        if counted.fetch_add(1, Ordering::SeqCst) == 0 {
            eventually("the second set's wait", || another_asked(factory, Group::WebProtection));
        }
        *moqui_lib::sync::lock(&seen) = (value(factory, &read.0), value(factory, &read.1));
        held.store(false, Ordering::SeqCst);
    });
    let threads: Vec<_> = [(limit.clone(), "5"), (window.clone(), "60")]
        .into_iter()
        .map(|(selector, set)| {
            let factory = Arc::clone(&factory);
            std::thread::spawn(move || super::set_value(&factory, &selector, Some(set), false))
        })
        .collect();
    for thread in threads {
        thread.join().unwrap().unwrap();
    }
    assert!(!overlapped.load(Ordering::SeqCst), "two runs of the hooks overlapped");
    // the first set's run, then one more for the second set, which waited for it
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(
        *moqui_lib::sync::lock(&last),
        (Some("5".to_string()), Some("60".to_string()))
    );
    super::set_value(&factory, &limit, None, false).unwrap();
    super::set_value(&factory, &window, None, false).unwrap();
}

/// Two groups' hooks running at once on two threads, each setting a setting of the other's group, do not wait for each
/// other: a thread inside a run leaves its request to the run in progress, which runs again. Both hooks are held until
/// both run, so the two runs surely meet.
#[test]
fn hooks_of_two_groups_setting_each_others_settings_do_not_deadlock() {
    let factory = factory(RUNNING);
    let protection = format!("{RATE_LIMIT}[rc1-cross]@limit");
    let screens = "screen-facade@rc1-cross".to_string();
    let entered = Arc::new(AtomicUsize::new(0));
    let failures: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let hook = |set: String, value: &'static str| {
        let (entered, failures) = (Arc::clone(&entered), Arc::clone(&failures));
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&runs);
        let hook = move |factory: &ExecutionContextFactory| {
            if counted.fetch_add(1, Ordering::SeqCst) > 0 {
                return;
            }
            entered.fetch_add(1, Ordering::SeqCst);
            eventually("both groups running", || entered.load(Ordering::SeqCst) == 2);
            if let Err(err) = super::set_value(factory, &set, Some(value), false) {
                moqui_lib::sync::lock(&failures).push(format!("{set}: {err}"));
            }
        };
        (hook, runs)
    };
    let (protection_hook, protection_runs) = hook(screens.clone(), "from-protection");
    let (screens_hook, screens_runs) = hook(protection.clone(), "from-screens");
    on_apply(&factory, Group::WebProtection, protection_hook);
    on_apply(&factory, Group::Screens, screens_hook);
    let (done, finished) = std::sync::mpsc::channel();
    for (selector, set) in [(protection.clone(), "5"), (screens.clone(), "first")] {
        let (factory, done) = (Arc::clone(&factory), done.clone());
        std::thread::spawn(move || {
            let _ = done.send(super::set_value(&factory, &selector, Some(set), false).map(|_| ()));
        });
    }
    for _ in 0..2 {
        finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("both sets ended")
            .unwrap();
    }
    assert!(moqui_lib::sync::lock(&failures).is_empty(), "{:?}", failures);
    // each hook's set landed, after the other thread's own set of that setting
    assert_eq!(value(&factory, &screens).as_deref(), Some("from-protection"));
    assert_eq!(value(&factory, &protection).as_deref(), Some("from-screens"));
    // and each group ran again for the set its run did not wait for
    assert_eq!(protection_runs.load(Ordering::SeqCst), 2);
    assert_eq!(screens_runs.load(Ordering::SeqCst), 2);
    super::set_value(&factory, &protection, None, false).unwrap();
    super::set_value(&factory, &screens, None, false).unwrap();
}

/// A hook that sets a setting of its own group at every run runs 8 times in all for one set, then stops.
#[test]
fn a_hook_that_keeps_changing_its_group_stops_after_the_cap() {
    let factory = factory(RUNNING);
    let selector = format!("{RATE_LIMIT}[rc1-cap]@limit");
    let runs = Arc::new(AtomicUsize::new(0));
    let (counted, set) = (Arc::clone(&runs), selector.clone());
    on_apply(&factory, Group::WebProtection, move |factory| {
        let run = counted.fetch_add(1, Ordering::SeqCst) + 1;
        let outcome = super::set(factory, &set, Some(&run.to_string()), false).unwrap();
        assert!(
            outcome.queued && outcome.note().contains("in progress ends"),
            "{}",
            outcome.note()
        );
    });
    let outcome = super::set(&factory, &selector, Some("0"), false).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), super::MAX_HOOK_RUNS);
    assert!(!outcome.queued && outcome.problems.is_empty(), "{outcome:?}");
    env::clear_property("moqui_conf__webapp_list__webapp__WEBROOT__web_protection__rate_limit__RC1_CAP__limit");
}

/// A run whose hook panics still ends: a caller waiting for the run is let go and its request runs (the answer is that
/// last run's problems, none here, since it built the state again); alone, the failure is the answer. A lead that ends
/// in a panic outside the hooks lets go of the group too.
#[test]
fn a_panicking_hook_lets_go_of_the_callers_waiting() {
    let factory = factory(RUNNING);
    let (first, second) = (
        format!("{RATE_LIMIT}[rc1-panic]@limit"),
        format!("{RATE_LIMIT}[rc1-panic]@window-seconds"),
    );
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&runs);
    on_apply(&factory, Group::WebProtection, move |factory| {
        match counted.fetch_add(1, Ordering::SeqCst) {
            0 => {
                eventually("the second set's wait", || another_asked(factory, Group::WebProtection));
                panic!("a hook that fails");
            }
            2 => panic!("a hook that fails alone"),
            _ => {}
        }
    });
    let threads: Vec<_> = [first.clone(), second.clone()]
        .into_iter()
        .map(|selector| {
            let factory = Arc::clone(&factory);
            std::thread::spawn(move || super::set(&factory, &selector, Some("1"), false))
        })
        .collect();
    let outcomes: Vec<super::SetOutcome> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap().unwrap())
        .collect();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.problems.is_empty() && !outcome.queued),
        "{outcomes:?}"
    );
    let alone = super::set(&factory, &first, Some("2"), false).unwrap();
    assert_eq!(alone.problems, ["web-protection: the hook failed"]);
    assert!(
        alone.note().contains("applying it failed") && !alone.note().contains("applies now"),
        "{}",
        alone.note()
    );

    // a lead dropped in a panic clears the group's lead and this thread's depth
    let runner = factory.extension(|_| super::HookRunner::default());
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        moqui_lib::sync::lock(&runner.groups)
            .entry(Group::Caches)
            .or_default()
            .running_on = Some(std::thread::current().id());
        let _leading = super::Leading::new(&runner, Group::Caches);
        panic!("outside the hooks");
    }));
    assert!(unwound.is_err());
    assert_eq!(
        moqui_lib::sync::lock(&runner.groups)
            .get(&Group::Caches)
            .and_then(|runs| runs.running_on),
        None
    );
    assert_eq!(super::IN_HOOKS.with(std::cell::Cell::get), 0);
    super::set_value(&factory, &first, None, false).unwrap();
    super::set_value(&factory, &second, None, false).unwrap();
}

/// A secret the seal's store keeps makes nothing in the running conf, which keeps no secret, though it is set beside
/// a setting that does go there.
#[test]
fn a_secret_kept_in_the_store_makes_no_element_in_the_running_conf() {
    use crate::seal::{Keeper, Mode, SealConf};
    let factory = factory(RUNNING);
    let runtime = runtime("store");
    // a production instance before its seal is made keeps a secret in this process only: enough for the test
    let keeper = Keeper::open(
        &crate::seal::file::path_in(&runtime),
        SealConf::with_purpose("production"),
    )
    .unwrap();
    keeper.start(Mode::Unattended).unwrap();
    factory.set_seal_keeper(Arc::new(keeper));
    let given = Inputs {
        secrets_in_env: false,
        ..inputs(&runtime, &no_environment, &[])
    };
    let outcomes = set_with(
        &factory,
        &[
            ("rc1-vault-holder/inner@password", Some("rc1-s3cret")),
            ("rc1-plain-two@port", Some("7")),
        ],
        true,
        &given,
    )
    .unwrap();
    assert_eq!(outcomes[0].kept, Kept::SealStore);
    assert_eq!(outcomes[1].kept, Kept::DotEnv);
    let conf = factory.conf_text();
    assert!(
        !conf.contains("rc1-vault-holder") && !conf.contains("rc1-s3cret"),
        "{conf}"
    );
    assert_eq!(value(&factory, "rc1-plain-two@port").as_deref(), Some("7"));
    set_with(&factory, &[("rc1-plain-two@port", None)], true, &given).unwrap();
    let _ = std::fs::remove_dir_all(&runtime);
}

/// A set takes the class a reload's diff gives the change: an element it makes is added whole and waits for a restart
/// when anything in it does; a cache's settings change only for a cache it makes.
#[test]
fn a_sets_class_follows_the_diff_for_the_elements_it_makes() {
    let edge = Class::Restart("the edge link starts at boot");
    assert_eq!(set_class(&["edge", "node"], "key", Some(0)), edge);
    assert_eq!(set_class(&["edge", "node"], "key", Some(1)), Class::Live);
    assert_eq!(set_class(&["edge", "node"], "key", None), Class::Live);
    let caches = Class::Apply(Group::Caches);
    assert_eq!(set_class(&["cache-list", "cache"], "expire-time-idle", Some(1)), caches);
    assert!(matches!(
        set_class(&["cache-list", "cache"], "expire-time-idle", None),
        Class::Restart(_)
    ));
    assert!(matches!(
        set_class(&["cache-list", "cache", "inner"], "x", Some(2)),
        Class::Restart(_)
    ));
    let webapp = ["webapp-list", "webapp"];
    assert_eq!(set_class(&webapp, "http-host", Some(1)), Class::Live);
    assert!(matches!(set_class(&webapp, "http-port", Some(1)), Class::Restart(_)));
    let rate_limit = ["webapp-list", "webapp", "web-protection", "rate-limit"];
    assert_eq!(
        set_class(&rate_limit, "limit", Some(2)),
        Class::Apply(Group::WebProtection)
    );
    // the outcome of a set that makes the edge element
    let factory = factory(RUNNING);
    let outcome = set_with(
        &factory,
        &[("edge/node@rc1-made", Some("x"))],
        false,
        &inputs(Path::new("."), &no_environment, &[]),
    )
    .unwrap()
    .remove(0);
    assert_eq!(outcome.class, edge);
    env::clear_property("moqui_conf__edge__node__rc1_made");
}

/// A `-D` the boot took sets the attribute again at the next start over a value kept until the restart too, and the
/// note says so; a note never says a change applies when applying it failed; a secret's values are masked in `Debug`.
#[test]
fn an_outcome_names_a_boot_property_failed_hooks_and_masks_a_secret() {
    let factory = factory(RUNNING);
    let runtime = runtime("until-restart-d");
    let (selector, name) = ("rc1-env-d@port", "moqui_conf__rc1_env_d__port");
    conf_env::keep_boot_overrides(
        &factory,
        vec![conf_env::Override {
            name: name.to_string(),
            source: Source::Property,
            selector: Some(selector.to_string()),
            secret: false,
            outcome: conf_env::Outcome::Set,
        }],
    );
    let environment = |asked: &str| (asked == name).then(|| "2".to_string());
    let outcome = set_with(
        &factory,
        &[(selector, Some("3"))],
        true,
        &inputs(&runtime, &environment, &[]),
    )
    .unwrap()
    .remove(0);
    assert_eq!(
        (outcome.kept, outcome.outranked_by),
        (Kept::UntilRestart, Some(Source::Property))
    );
    assert!(outcome.note().contains("a -D of"), "{}", outcome.note());
    env::clear_property(name);

    let failed = SetOutcome {
        problems: vec!["web-protection: the hook failed".to_string()],
        class: Class::Apply(Group::WebProtection),
        ..outcome.clone()
    };
    assert!(
        failed.note().contains("applying it failed") && !failed.note().contains("applies now"),
        "{}",
        failed.note()
    );
    let secret = SetOutcome {
        secret: true,
        previous: Some("rc1-old-secret".to_string()),
        before_kept: Some("rc1-kept-secret".to_string()),
        ..outcome
    };
    let shown = format!("{secret:?}");
    assert!(
        !shown.contains("rc1-old-secret") && !shown.contains("rc1-kept-secret"),
        "{shown}"
    );
    let _ = std::fs::remove_dir_all(&runtime);
}
