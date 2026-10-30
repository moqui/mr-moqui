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

use crate::reload::reload_paths;
use crate::ArtifactKind;
use moqui_core::context::{ComponentInfo, ExecutionContext, ExecutionContextFactory};

struct Bed {
    ctx: ExecutionContext,
    dir: PathBuf,
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Bed {
    fn factory(&self) -> &Arc<ExecutionContextFactory> {
        self.ctx.factory()
    }

    fn component(&self) -> PathBuf {
        self.dir.join("devtest")
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.component().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

/// A framework directory and one component, `devtest`, in a scratch directory.
fn bed(name: &str) -> Bed {
    let dir = std::env::temp_dir().join(format!("moqui-devtools-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("framework")).unwrap();
    std::fs::create_dir_all(dir.join("devtest")).unwrap();
    let component = ComponentInfo {
        name: "devtest".to_string(),
        version: String::new(),
        location: dir.join("devtest"),
        depends_on: Vec::new(),
        purposes: Vec::new(),
    };
    let ctx = ExecutionContextFactory::test_context_with_components(
        "",
        moqui_core::service::ServicePools::idle(),
        dir.join("framework"),
        vec![component],
    );
    Bed { ctx, dir }
}

fn services(body: &str) -> String {
    format!("<services>{body}</services>")
}

fn service(verb: &str, value: &str) -> String {
    format!(r#"<service verb="{verb}" noun="Thing"><actions><set field="from" value="{value}"/></actions></service>"#)
}

fn defined(name: &str) -> Option<String> {
    moqui_core::service::define::find(name).map(|def| def.actions.clone())
}

#[test]
fn a_changed_service_file_reloads_its_services_added_and_removed() {
    let bed = bed("services");
    let path = bed.write(
        "service/devtest/Things.xml",
        &services(&format!("{}{}", service("run", "first"), service("keep", "first"))),
    );
    let _entered = bed.ctx.enter();
    assert!(defined("devtest.Things.run#Thing").unwrap().contains("first"));
    bed.write(
        "service/devtest/Things.xml",
        &services(&format!("{}{}", service("keep", "second"), service("add", "second"))),
    );
    let report = reload_paths(bed.factory(), &[path]);
    assert_eq!(report.files.len(), 1, "{report:?}");
    let file = &report.files[0];
    assert_eq!(file.kind, ArtifactKind::Service);
    assert_eq!(file.component.as_deref(), Some("devtest"));
    assert_eq!(file.added, ["devtest.Things.add#Thing"], "{file:?}");
    assert_eq!(file.removed, ["devtest.Things.run#Thing"], "{file:?}");
    assert!(file.problems.is_empty(), "{file:?}");
    assert!(
        file.summary.starts_with("service file reloaded: 2 services"),
        "{}",
        file.summary
    );
    assert!(defined("devtest.Things.run#Thing").is_none());
    assert!(defined("devtest.Things.keep#Thing").unwrap().contains("second"));
    assert!(defined("devtest.Things.add#Thing").is_some());
    assert_eq!(crate::recent_reports(bed.factory()).len(), 1);
}

#[test]
fn a_service_file_that_does_not_parse_keeps_the_services_loaded_before() {
    let bed = bed("broken");
    let path = bed.write("service/devtest/Broken.xml", &services(&service("run", "good")));
    let _entered = bed.ctx.enter();
    assert!(defined("devtest.Broken.run#Thing").unwrap().contains("good"));
    bed.write(
        "service/devtest/Broken.xml",
        "<services><service verb=\"run\" noun=\"Thing\">",
    );
    let report = reload_paths(bed.factory(), &[path]);
    let file = &report.files[0];
    assert_eq!(file.problems.len(), 1, "{file:?}");
    assert_eq!(file.problems[0].rule, "reload.service");
    assert!(file.summary.contains("does not parse"), "{}", file.summary);
    assert!(defined("devtest.Broken.run#Thing").unwrap().contains("good"));
}

#[test]
fn a_changed_seca_file_rebuilds_the_rule_set() {
    let bed = bed("seca");
    let rule = |id: &str| {
        format!(
            r#"<seca id="{id}" service="devtest.Things.run#Thing" when="post-service"><actions><log message="x"/></actions></seca>"#
        )
    };
    bed.write("service/devtest/Things.xml", &services(&service("run", "first")));
    let path = bed.write("service/Things.secas.xml", &format!("<secas>{}</secas>", rule("one")));
    let _entered = bed.ctx.enter();
    let (before, _, _) = moqui_core::service::reload_eca_rules(bed.factory());
    bed.write(
        "service/Things.secas.xml",
        &format!("<secas>{}{}</secas>", rule("one"), rule("two")),
    );
    let report = reload_paths(bed.factory(), &[path]);
    let file = &report.files[0];
    assert_eq!(file.kind, ArtifactKind::Seca);
    assert_eq!(before, 1);
    assert!(file.summary.contains("2 SECA"), "{}", file.summary);
}

#[test]
fn a_seca_rule_on_a_service_added_since_runs_after_the_service_file_reloads() {
    let bed = bed("seca-new-service");
    let rule = r#"<seca id="onNew" service="devtest.Things.add#Thing" when="post-service"><actions><log message="x"/></actions></seca>"#;
    let path = bed.write("service/devtest/Things.xml", &services(&service("run", "first")));
    bed.write("service/Things.secas.xml", &format!("<secas>{rule}</secas>"));
    let _entered = bed.ctx.enter();
    moqui_core::service::reload_eca_rules(bed.factory());
    // the rule names a service not defined yet: left out
    assert!(moqui_core::service::info::seca_rule_texts("devtest.Things.add#Thing").is_empty());
    bed.write(
        "service/devtest/Things.xml",
        &services(&format!("{}{}", service("run", "first"), service("add", "first"))),
    );
    let report = reload_paths(bed.factory(), &[path]);
    assert_eq!(report.files[0].added, ["devtest.Things.add#Thing"], "{report:?}");
    assert_eq!(
        moqui_core::service::info::seca_rule_texts("devtest.Things.add#Thing").len(),
        1
    );
}

#[test]
fn a_screen_extend_that_does_not_parse_leaves_its_screen_rendering() {
    let bed = bed("extend-broken");
    bed.write(
        "screen/Base.xml",
        r#"<screen><widgets><container id="Main"><label text="base"/></container></widgets></screen>"#,
    );
    bed.write(
        "screen-extend/devtest/screen/Base.xml",
        "<screen-extend><widgets-extend name=\"Main\">",
    );
    let _entered = bed.ctx.enter();
    let facade = moqui_core::screen::ScreenFacade::of(bed.factory());
    let def = facade.screen_def(bed.factory(), "component://devtest/screen/Base.xml");
    assert!(def.is_ok(), "{:?}", def.err());
}

#[test]
fn files_are_classified_by_their_place_and_kinds_that_do_not_reload_say_why() {
    let bed = bed("kinds");
    let paths: Vec<PathBuf> = [
        "entity/DevEntities.xml",
        "entity/Dev.eecas.xml",
        "service/devtest/Dev.rest.xml",
        "screen/Dev/Page.xml",
        "screen/Dev/Page.qvue",
        "screen-extend/example/screen/Gallery/Page.xml",
        "data/DevSeedData.xml",
        "component.xml",
        "src/lib.rs",
        "skill/dev.md",
        "README.md",
    ]
    .iter()
    .map(|relative| bed.write(relative, "<x/>"))
    .collect();
    let _entered = bed.ctx.enter();
    let report = reload_paths(bed.factory(), &paths);
    let kind_of = |name: &str| {
        report
            .files
            .iter()
            .find(|file| Path::new(&file.path).ends_with(name))
            .map(|file| file.kind)
    };
    assert_eq!(kind_of("entity/DevEntities.xml"), Some(ArtifactKind::Entity));
    assert_eq!(kind_of("entity/Dev.eecas.xml"), Some(ArtifactKind::Eeca));
    assert_eq!(kind_of("service/devtest/Dev.rest.xml"), Some(ArtifactKind::Rest));
    assert_eq!(kind_of("screen/Dev/Page.xml"), Some(ArtifactKind::Screen));
    assert_eq!(kind_of("screen/Dev/Page.qvue"), Some(ArtifactKind::ScreenText));
    assert_eq!(
        kind_of("screen-extend/example/screen/Gallery/Page.xml"),
        Some(ArtifactKind::ScreenExtend)
    );
    assert_eq!(kind_of("data/DevSeedData.xml"), Some(ArtifactKind::Data));
    assert_eq!(kind_of("component.xml"), Some(ArtifactKind::Conf));
    assert_eq!(kind_of("src/lib.rs"), Some(ArtifactKind::Rust));
    assert_eq!(kind_of("skill/dev.md"), Some(ArtifactKind::Skill));
    // A file of no known kind is left out.
    assert_eq!(kind_of("README.md"), None);
    // Kinds are reloaded in dependency order: the conf and Rust first, then entities, services, rules, screens.
    let orders: Vec<ArtifactKind> = report.files.iter().map(|file| file.kind).collect();
    let entity = orders.iter().position(|kind| *kind == ArtifactKind::Entity).unwrap();
    let screen = orders.iter().position(|kind| *kind == ArtifactKind::Screen).unwrap();
    assert!(entity < screen, "{orders:?}");
    assert!(report.restart && report.rebuild);
    let entity_file = report
        .files
        .iter()
        .find(|file| file.kind == ArtifactKind::Entity)
        .unwrap();
    assert!(entity_file.restart && entity_file.summary.contains("restart"));
    // A path outside every component is not reported.
    let outside = reload_paths(bed.factory(), &[bed.dir.join("elsewhere.xml")]);
    assert!(outside.files.is_empty());
}

#[cfg(feature = "watch")]
#[test]
fn the_watcher_reloads_a_service_file_changed_on_disk() {
    let bed = bed("watch");
    bed.write("service/devtest/Watched.xml", &services(&service("run", "first")));
    {
        let _entered = bed.ctx.enter();
        assert!(defined("devtest.Watched.run#Thing").unwrap().contains("first"));
    }
    assert!(crate::watch::watch(
        bed.factory(),
        std::time::Duration::from_millis(100)
    ));
    // An editor's write to a temporary file and rename over the original.
    let temp = bed.write("service/devtest/.Watched.xml.swp", &services(&service("run", "second")));
    std::fs::rename(&temp, bed.component().join("service/devtest/Watched.xml")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let reports = crate::recent_reports(bed.factory());
        if let Some(report) = reports.first() {
            assert_eq!(report.files.len(), 1, "{report:?}");
            assert_eq!(report.files[0].kind, ArtifactKind::Service);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no reload within 10 s");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _entered = bed.ctx.enter();
    assert!(defined("devtest.Watched.run#Thing").unwrap().contains("second"));
}

/// Each rule a check quotes is in its skill section word for word (plans/design-dev-aide.md 13.1), so a person and a
/// model read the same rule.
#[test]
fn each_check_rule_is_in_its_skill_section() {
    let skill = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/base-component/framework/skill/moqui-coding-agent.md"
    ))
    .unwrap();
    for (rule, section, sentence) in crate::check::RULES {
        let start = skill
            .find(&format!("\n## {section}\n"))
            .unwrap_or_else(|| panic!("{rule}: no section {section}"));
        let body = &skill[start + 1..];
        let body = &body[..body[3..].find("\n## ").map_or(body.len(), |end| end + 3)];
        assert!(
            body.contains(sentence),
            "{rule}: \"{sentence}\" is not in section {section}"
        );
    }
}

/// The Rust services register at the `moqui_devkit::` locations the service file names, and at no other.
#[test]
fn the_services_register_at_the_locations_the_service_file_names() {
    let mut map = std::collections::BTreeMap::new();
    crate::service::register(&mut map).unwrap();
    let registered: Vec<&str> = map.keys().copied().collect();
    let file = moqui_core::context::framework_dir().join("service/org/moqui/impl/DevToolsServices.xml");
    let text = std::fs::read_to_string(file).unwrap();
    let mut named: Vec<String> = text
        .split("location=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
        .filter(|location| location.starts_with("moqui_devkit::"))
        .collect();
    named.sort_unstable();
    named.dedup();
    assert_eq!(registered, named);
}
