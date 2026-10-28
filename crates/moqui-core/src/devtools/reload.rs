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

//! Reload what changed files define (plans/design-dev-aide.md 6.1 and 6.2, F3, F4, F6).
//!
//! [`reload_paths`] classifies each path by its component, its directory and its name, groups the paths by kind and
//! reloads the kinds in dependency order (entities, services, ECA rules, REST, screens and the files they read,
//! skills), each kind once and each on its own: a failure is a problem of its file and the rest go on. A service, ECA
//! or REST file that does not parse keeps the definitions loaded before, with the parse error as its problem. Entity
//! files take a restart for now (F5 is staged), and conf and Rust changes a restart or a rebuild, which the report
//! says. The dev-aide's save and the file watcher call it; the last reports are kept for the reload log and agents.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moqui_lib::node::MNode;
use serde::Serialize;

use super::Problem;
use crate::context::{ExecutionContext, ExecutionContextFactory};

/// How many reports a factory keeps (Home's reload log, the agents' `check` with no file).
const KEPT_REPORTS: usize = 200;

/// The kind of artifact a changed file holds, by its place in its component and its name.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Entity,
    Service,
    Seca,
    Eeca,
    Rest,
    Screen,
    ScreenExtend,
    /// A file a screen reads: a `.qvue`, `.qjs` or `.tui.json`, a `render-mode text` file, a widget template.
    ScreenText,
    Script,
    Skill,
    Data,
    /// `component.xml`, `MoquiConf.xml` or the runtime conf: a restart.
    Conf,
    /// Rust source or a `Cargo.toml`: a rebuild.
    Rust,
    /// webroot JavaScript and CSS: the open pages reload.
    Client,
    Other,
}

impl ArtifactKind {
    /// The order kinds reload in, so a screen never loads before the field or service it uses.
    fn order(self) -> u8 {
        match self {
            Self::Conf | Self::Rust => 0,
            Self::Entity => 1,
            Self::Service => 2,
            Self::Seca | Self::Eeca => 3,
            Self::Rest => 4,
            Self::Screen | Self::ScreenExtend | Self::ScreenText | Self::Script => 5,
            Self::Skill => 6,
            Self::Data | Self::Client | Self::Other => 7,
        }
    }
}

/// What a reload did for one file.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReload {
    pub path: String,
    /// The component the file is in, when it is in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub kind: ArtifactKind,
    /// One line: what was reloaded, or what is needed.
    pub summary: String,
    /// Definitions the file added (services).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Definitions the file no longer has.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<Problem>,
    /// The DDL a change ran (an entity file's table check).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ddl: Vec<String>,
    /// The change takes a restart to apply.
    pub restart: bool,
    /// The change takes a rebuild (`./moq build`) and a restart.
    pub rebuild: bool,
}

/// What one call of [`reload_paths`] did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadReport {
    /// When it ran, in epoch milliseconds.
    pub at: i64,
    pub millis: u64,
    pub files: Vec<FileReload>,
    /// Some file needs a restart.
    pub restart: bool,
    /// Some file needs a rebuild.
    pub rebuild: bool,
    /// webroot JavaScript or CSS changed: the open pages should reload.
    pub client_reload: bool,
}

impl ReloadReport {
    /// Every problem of every file.
    pub fn problems(&self) -> impl Iterator<Item = &Problem> {
        self.files.iter().flat_map(|file| file.problems.iter())
    }

    /// One line for the log: the files and their summaries.
    #[must_use]
    pub fn summary(&self) -> String {
        let files: Vec<String> = self
            .files
            .iter()
            .map(|file| format!("{} ({})", short_name(&file.path), file.summary))
            .collect();
        format!("{} in {} ms", files.join("; "), self.millis)
    }
}

type Listener = Arc<dyn Fn(&Arc<ExecutionContextFactory>, &ReloadReport) + Send + Sync>;

#[derive(Default)]
struct Reloads {
    reports: Mutex<VecDeque<Arc<ReloadReport>>>,
    listeners: Mutex<Vec<Listener>>,
    /// One reload at a time: a save and the watcher seeing that save do not rebuild the registry at once.
    running: Mutex<()>,
    /// Each path reloaded with its modified time then, so the watcher does not reload again what a save reloaded.
    handled: Mutex<std::collections::HashMap<PathBuf, Option<std::time::SystemTime>>>,
}

/// Whether `path` was reloaded as it is on disk now (by a save), so the watcher's event for that write is not reloaded
/// again.
#[must_use]
pub fn already_reloaded(factory: &ExecutionContextFactory, path: &Path) -> bool {
    let path = canonical(path);
    lock(&reloads_of(factory).handled)
        .get(&path)
        .is_some_and(|handled| *handled == modified(&path))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn reloads_of(factory: &ExecutionContextFactory) -> Arc<Reloads> {
    factory.extension::<Reloads>(|_| Reloads::default())
}

/// Call `listener` with each report from now on (the dev-aide's notifications, later the artifact index).
pub fn on_reload(
    factory: &ExecutionContextFactory,
    listener: impl Fn(&Arc<ExecutionContextFactory>, &ReloadReport) + Send + Sync + 'static,
) {
    lock(&reloads_of(factory).listeners).push(Arc::new(listener));
}

/// The last reports, newest first.
#[must_use]
pub fn recent_reports(factory: &ExecutionContextFactory) -> Vec<Arc<ReloadReport>> {
    lock(&reloads_of(factory).reports).iter().rev().cloned().collect()
}

/// Reload what `paths` define on `factory`, in the entered context or a new one, and answer what was done. Paths in
/// no component (other than the runtime conf) and paths of no known kind are left out of the report.
pub fn reload_paths(factory: &Arc<ExecutionContextFactory>, paths: &[PathBuf]) -> ReloadReport {
    let reloads = reloads_of(factory);
    let report = {
        let _running = lock(&reloads.running);
        {
            let mut handled = lock(&reloads.handled);
            if handled.len() > 10_000 {
                handled.clear();
            }
            for path in paths {
                let path = canonical(path);
                let stamp = modified(&path);
                handled.insert(path, stamp);
            }
        }
        let entered_here = ExecutionContext::with_current(|_| ())
            .is_none()
            .then(|| factory.new_context());
        let _entered = entered_here.as_ref().map(ExecutionContext::enter);
        let _authz = entered_here.as_ref().map(ExecutionContext::disable_authz);
        run(factory, paths)
    };
    {
        let mut reports = lock(&reloads.reports);
        if reports.len() == KEPT_REPORTS {
            reports.pop_front();
        }
        reports.push_back(Arc::new(report.clone()));
    }
    // called on a copy, so a listener may add one
    let listeners: Vec<Listener> = lock(&reloads.listeners).clone();
    for listener in &listeners {
        listener(factory, &report);
    }
    report
}

fn run(factory: &Arc<ExecutionContextFactory>, paths: &[PathBuf]) -> ReloadReport {
    let started = std::time::Instant::now();
    let mut files: Vec<FileReload> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        let path = canonical(path);
        if !seen.insert(path.clone()) {
            continue;
        }
        if let Some(file) = classify(factory, &path) {
            files.push(file);
        }
    }
    files.sort_by_key(|file| file.kind.order());
    let relinked = reload_entities(factory, &mut files);
    let services_changed = reload_services(factory, &mut files);
    // a SECA rule naming a service not defined when the rules were read was left out: read them again when services
    // came or went, an entity's own (entity-auto) among them
    reload_secas(factory, &mut files, relinked || services_changed);
    reload_eecas(factory, &mut files);
    reload_rest(&mut files);
    reload_screens(factory, &mut files);
    reload_skills(factory, &mut files);
    let mut report = ReloadReport {
        at: now_millis(),
        millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        restart: files.iter().any(|file| file.restart),
        rebuild: files.iter().any(|file| file.rebuild),
        client_reload: files.iter().any(|file| file.kind == ArtifactKind::Client),
        files,
    };
    report.files.retain(|file| file.kind != ArtifactKind::Other);
    report
}

/// The file's kind and component, with what a kind that needs no reload says; `None` for a path outside every
/// component and the runtime conf.
fn classify(factory: &ExecutionContextFactory, path: &Path) -> Option<FileReload> {
    let display = path.display().to_string();
    let conf_dir = canonical(&factory.runtime_dir().join("conf"));
    let found = factory.components().iter().find_map(|component| {
        let root = canonical(&component.location);
        path.strip_prefix(&root)
            .ok()
            .map(|relative| (component.name.clone(), relative.to_path_buf()))
    });
    let (component, kind) = match found {
        Some((name, relative)) => {
            let kind = kind_of(&name, &relative);
            (Some(name), kind)
        }
        None if path.starts_with(&conf_dir) && has_extension(path, "xml") => (None, ArtifactKind::Conf),
        None => return None,
    };
    let mut file = FileReload {
        path: display,
        component,
        kind,
        summary: String::new(),
        added: Vec::new(),
        removed: Vec::new(),
        problems: Vec::new(),
        ddl: Vec::new(),
        restart: false,
        rebuild: false,
    };
    match kind {
        ArtifactKind::Conf => {
            file.summary = "restart to apply".into();
            file.restart = true;
        }
        ArtifactKind::Rust => {
            file.summary = "rebuild to apply (./moq build), then restart".into();
            file.rebuild = true;
            file.restart = true;
        }
        ArtifactKind::Data => file.summary = "data file: load it to apply".into(),
        ArtifactKind::Client => file.summary = "open pages reload".into(),
        _ => {}
    }
    Some(file)
}

/// The kind of the file at `relative` in component `component`.
pub(crate) fn kind_of(component: &str, relative: &Path) -> ArtifactKind {
    let text = relative.to_string_lossy().replace('\\', "/");
    let name = text.rsplit('/').next().unwrap_or(&text);
    let top = text.split('/').next().unwrap_or("");
    if text == "component.xml" || text == "MoquiConf.xml" {
        return ArtifactKind::Conf;
    }
    if name == "Cargo.toml" || name.ends_with(".rs") {
        return ArtifactKind::Rust;
    }
    if component == "webroot" && (name.ends_with(".js") || name.ends_with(".css")) {
        return ArtifactKind::Client;
    }
    match top {
        "entity" if name.ends_with(".eecas.xml") => ArtifactKind::Eeca,
        "entity" if name.ends_with(".xml") => ArtifactKind::Entity,
        "service"
            if name.ends_with(".secas.xml") || name.ends_with(".emecas.xml") || name.ends_with(".gitecas.xml") =>
        {
            ArtifactKind::Seca
        }
        "service" if name.ends_with(".rest.xml") => ArtifactKind::Rest,
        "service" if name.ends_with(".xml") => ArtifactKind::Service,
        "screen-extend" if name.ends_with(".xml") => ArtifactKind::ScreenExtend,
        "screen" if name.ends_with(".xml") => ArtifactKind::Screen,
        "screen" | "template" => ArtifactKind::ScreenText,
        "skill" if name.ends_with(".md") || name == "aliases.json" => ArtifactKind::Skill,
        "data" => ArtifactKind::Data,
        _ if name.ends_with(".js") || name.ends_with(".groovy") => ArtifactKind::Script,
        _ => ArtifactKind::Other,
    }
}

/// The entity files: the model relinked once for all of them and swapped in, and their entities' tables checked
/// (F5). A file that does not parse, or a model that does not link, keeps the old model, with the problem and "restart
/// to apply" as the fallback.
/// Answers whether the model was relinked.
fn reload_entities(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload]) -> bool {
    let changed = indexes(files, &[ArtifactKind::Entity]);
    if changed.is_empty() {
        return false;
    }
    let mut failed = false;
    for &at in &changed {
        if let Err(problem) = parses(&files[at].path, "reload.entity") {
            files[at].problems.push(problem);
            failed = true;
        }
    }
    let relinked = match factory.entity() {
        Some(_) if failed => Err("an entity file does not parse, so the model was not relinked".to_string()),
        Some(facade) => {
            // a site's first entity file makes its entity directory, which the model is read from from now on
            facade.set_component_entity_dirs(crate::context::component_entity_dirs(factory));
            let paths: Vec<PathBuf> = changed.iter().map(|&at| PathBuf::from(&files[at].path)).collect();
            facade.relink(&paths).map_err(|err| err.to_string())
        }
        None => Err("no entity facade".to_string()),
    };
    match relinked {
        Ok(report) => {
            // forms built over the entities (auto-fields-entity) read the new fields on next use
            crate::screen::ScreenFacade::of(factory).clear();
            for &at in &changed {
                let file = &mut files[at];
                let named = entities_of(&file.path, &report.changed);
                file.summary = format!(
                    "entity model relinked in {} ms{}{}",
                    report.millis,
                    if named.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", named.join(", "))
                    },
                    if report.ddl.is_empty() {
                        String::new()
                    } else {
                        format!("; {} DDL statement{} ran", report.ddl.len(), plural(report.ddl.len()))
                    }
                );
                file.added = report.added.clone();
                file.removed = report.removed.clone();
                file.ddl = report.ddl.clone();
                for problem in &report.problems {
                    file.problems
                        .push(Problem::file_error(&file.path, "reload.entity", problem.clone()));
                }
            }
            true
        }
        Err(err) => {
            for &at in &changed {
                let file = &mut files[at];
                file.summary = "restart to apply the entity change".into();
                file.restart = true;
                if !failed {
                    file.problems
                        .push(Problem::file_error(&file.path, "reload.entity", err.clone()));
                }
            }
            false
        }
    }
}

/// The short names of `changed` (full names) that a file defines or extends, as its text names them.
fn entities_of(path: &str, changed: &[String]) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    changed
        .iter()
        .filter_map(|full| {
            let short = full.rsplit('.').next().unwrap_or(full);
            let quoted = format!("\"{short}\"");
            text.contains(&quoted).then(|| short.to_string())
        })
        .collect()
}

/// Answers whether services were added or removed.
fn reload_services(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload]) -> bool {
    let changed: Vec<usize> = indexes(files, &[ArtifactKind::Service]);
    if changed.is_empty() {
        return false;
    }
    if !parse_all(files, &changed, "reload.service") {
        return false;
    }
    let paths: Vec<PathBuf> = changed.iter().map(|&at| PathBuf::from(&files[at].path)).collect();
    match crate::service::reload_definitions(&paths) {
        Ok(reload) => {
            // forms built from a service's parameters (auto-fields-service) read the new ones on next use
            crate::screen::ScreenFacade::of(factory).clear();
            for &at in &changed {
                let file = &mut files[at];
                let prefix = service_prefix(&file.path);
                let mine = |name: &&String| name.rsplit_once('.').is_some_and(|(of, _)| of == prefix);
                file.added = reload.added.iter().filter(mine).cloned().collect();
                file.removed = reload.removed.iter().filter(mine).cloned().collect();
                let defined = reload.defined;
                file.summary = format!(
                    "service file reloaded: {defined} service{}{}{}",
                    plural(defined),
                    counted(", added", file.added.len()),
                    counted(", removed", file.removed.len()),
                );
                for failure in &reload.failures {
                    if failure.contains(&file.path) || failure.contains(&format!("service {prefix}.")) {
                        file.problems
                            .push(Problem::file_error(&file.path, "reload.service", failure.clone()));
                    }
                }
            }
            !reload.added.is_empty() || !reload.removed.is_empty()
        }
        Err(err) => {
            fail_all(files, &changed, "reload.service", &err.to_string());
            false
        }
    }
}

fn reload_secas(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload], services_changed: bool) {
    let changed = indexes(files, &[ArtifactKind::Seca]);
    if changed.is_empty() {
        if services_changed {
            crate::service::reload_eca_rules(factory);
        }
        return;
    }
    if !parse_all(files, &changed, "reload.eca") {
        return;
    }
    let (seca, email, git) = crate::service::reload_eca_rules(factory);
    for at in changed {
        files[at].summary = format!("ECA rules reloaded: {seca} SECA, {email} EmECA, {git} gitECA");
    }
}

fn reload_eecas(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload]) {
    let changed = indexes(files, &[ArtifactKind::Eeca]);
    if changed.is_empty() || !parse_all(files, &changed, "reload.eca") {
        return;
    }
    let Some(facade) = factory.entity() else {
        fail_all(
            files,
            &changed,
            "reload.eca",
            "no entity facade to reload the EECA rules on",
        );
        return;
    };
    let count = crate::entity::load_feed::reload_eca_rules(&facade);
    for at in changed {
        files[at].summary = format!("EECA rules reloaded: {count}");
    }
}

fn reload_rest(files: &mut [FileReload]) {
    let changed = indexes(files, &[ArtifactKind::Rest]);
    if changed.is_empty() || !parse_all(files, &changed, "reload.rest") {
        return;
    }
    match crate::service::reload_rest_roots() {
        Ok(count) => {
            for at in changed {
                files[at].summary = format!("REST API files reloaded: {count} root resource{}", plural(count));
            }
        }
        Err(err) => fail_all(files, &changed, "reload.rest", &err.to_string()),
    }
}

fn reload_screens(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload]) {
    let kinds = [
        ArtifactKind::Screen,
        ArtifactKind::ScreenExtend,
        ArtifactKind::ScreenText,
        ArtifactKind::Script,
    ];
    let changed = indexes(files, &kinds);
    if changed.is_empty() {
        return;
    }
    let paths: Vec<PathBuf> = changed.iter().map(|&at| PathBuf::from(&files[at].path)).collect();
    let screens = crate::screen::ScreenFacade::of(factory).forget(factory, &paths);
    let scripts = crate::script::forget_scripts(&paths);
    if changed.iter().any(|&at| files[at].kind == ArtifactKind::ScreenText) {
        crate::script::template::TemplateRegistry::of(factory).forget_texts();
    }
    for at in changed {
        let file = &mut files[at];
        if file.path.ends_with(".xml") {
            if let Err(problem) = parses(&file.path, "reload.screen") {
                file.problems.push(problem);
            }
        }
        file.summary = match file.kind {
            ArtifactKind::Script if scripts.is_empty() => "script: read again on next use".into(),
            ArtifactKind::Script => "script reloaded".into(),
            _ if screens.is_empty() => "screen: read again on next use".into(),
            _ => format!(
                "screen reloaded: {} cached screen{} dropped",
                screens.len(),
                plural(screens.len())
            ),
        };
    }
}

fn reload_skills(factory: &Arc<ExecutionContextFactory>, files: &mut [FileReload]) {
    let changed = indexes(files, &[ArtifactKind::Skill]);
    if changed.is_empty() {
        return;
    }
    #[cfg(feature = "llm")]
    {
        let Some(facade) = factory.entity() else {
            fail_all(
                files,
                &changed,
                "reload.skill",
                "no entity facade to sync the skills into",
            );
            return;
        };
        match crate::llm::skills::sync::sync(factory, &facade) {
            Ok(sync) => {
                for &at in &changed {
                    let file = &mut files[at];
                    file.summary = format!(
                        "skills synced: {} added, {} changed, {} retired",
                        sync.added, sync.changed, sync.retired
                    );
                    for problem in sync.problems.iter().filter(|problem| problem.contains(&file.path)) {
                        file.problems
                            .push(Problem::file_error(&file.path, "reload.skill", problem.clone()));
                    }
                }
            }
            Err(err) => fail_all(files, &changed, "reload.skill", &err.to_string()),
        }
    }
    #[cfg(not(feature = "llm"))]
    {
        let _ = factory;
        for at in changed {
            files[at].summary = "skills are not built into this host".into();
        }
    }
}

fn indexes(files: &[FileReload], kinds: &[ArtifactKind]) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, file)| kinds.contains(&file.kind))
        .map(|(at, _)| at)
        .collect()
}

/// Parse each changed file first. When one does not parse, none of them is reloaded, so the definitions loaded before
/// stay; the file that does not parse has the parse error as its problem, the others say why they waited.
fn parse_all(files: &mut [FileReload], changed: &[usize], rule: &str) -> bool {
    let mut broken = Vec::new();
    for &at in changed {
        if let Err(problem) = parses(&files[at].path, rule) {
            broken.push(short_name(&files[at].path).to_string());
            files[at].summary = "not reloaded: the file does not parse; the definitions loaded before stay".into();
            files[at].problems.push(problem);
        }
    }
    if broken.is_empty() {
        return true;
    }
    for &at in changed {
        if files[at].problems.is_empty() {
            files[at].summary = format!("not reloaded: {} does not parse", broken.join(", "));
        }
    }
    false
}

fn fail_all(files: &mut [FileReload], changed: &[usize], rule: &str, message: &str) {
    for &at in changed {
        let file = &mut files[at];
        file.summary = "not reloaded".into();
        file.problems.push(Problem::file_error(&file.path, rule, message));
    }
}

/// The file's XML parses; a deleted file is no problem (its definitions go with the reload).
fn parses(path: &str, rule: &str) -> Result<(), Problem> {
    if !Path::new(path).exists() {
        return Ok(());
    }
    MNode::parse_file(Path::new(path))
        .map(|_| ())
        .map_err(|err| Problem::file_error(path, rule, err.to_string()))
}

/// `org.moqui.impl.UserServices` for `.../service/org/moqui/impl/UserServices.xml`, as service names begin.
fn service_prefix(path: &str) -> String {
    crate::service::define::service_prefix(Path::new(path))
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|found| found == extension)
}

fn short_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn counted(label: &str, count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!("{label} {count}")
    }
}

fn canonical(path: &Path) -> PathBuf {
    if let Ok(found) = path.canonicalize() {
        return found;
    }
    // A deleted file: its directory, canonical, and its name.
    match (
        path.parent().and_then(|parent| parent.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
