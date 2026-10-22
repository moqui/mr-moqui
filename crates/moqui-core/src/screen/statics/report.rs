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

//! The compatibility report of `plans/archive/QvtServerStatic.md`: for one screen, whether it can be a static qvt template and what in it
//! would not work in the browser. One report has the location, the mode, and each violation with its screen, element and
//! operation. It is made by `moqui lint --screens --qvt-static` and by tests. A clean report changes no default.
//!
//! Three things fill it. The static macros hand every expression they write to [`record_ast`], which walks its `oxc_parser` AST, and
//! note the server-only work they cannot write ([`record_note`]). A scan of the definitions finds `always-actions` and `pre-actions`
//! with side effects (the form request runs them again), a `section-iterate` over the list of a form-list, and a label that reads
//! the page keys of a list. The full check renders the screen in `html` with the `ec` of widget expressions wrapped
//! ([`WidgetCheck`]), and each use of an area the browser does not have is recorded as the render goes on.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::{descendants_named, StaticTemplate, PAGE_SUFFIXES};
use crate::context::{Error, ExecutionContextFactory};
use crate::screen::definition::{Condition, ScreenDef, SectionKind};
use crate::screen::{make_render, with_current_render, ScreenUrlInfo};

/// Something in a screen that a static template does not do, or does differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// `ec`, `sri`, `decimal`, `utilities`, `context`, `server-global`, `side-effect`, `form-list-key`, `form-list-iterate`,
    /// `stale-cache`, `wrapped-ec`, ...
    pub kind: String,
    /// The location of the screen file.
    pub screen: String,
    pub element: String,
    pub detail: String,
}

/// The report of one screen.
#[derive(Clone, Debug, Default)]
pub struct StaticReport {
    pub location: String,
    pub path: String,
    /// The mode checked: `qvt`.
    pub mode: String,
    /// `server-static` names qvt.
    pub marked: bool,
    /// The screen can be sent as a static template (as if marked).
    pub eligible: bool,
    pub reasons: Vec<String>,
    pub violations: Vec<Violation>,
    /// The size of the compiled script, 0 when no template was made.
    pub template_bytes: usize,
    /// The size of the Vue template the script was compiled from.
    pub template_html_bytes: usize,
    /// Whether the render with the wrapped `ec` ran.
    pub full_check: bool,
}

impl StaticReport {
    /// No violation, the screen can be a template, and the full check (the html render with the wrapped `ec`) ran.
    /// Eligible with no violations and `full_check` false is the static half, not clean.
    pub fn is_clean(&self) -> bool {
        self.eligible && self.violations.is_empty() && self.full_check
    }

    /// The kinds of the violations, sorted and without repeats.
    pub fn kinds(&self) -> Vec<String> {
        let mut kinds: Vec<String> = self.violations.iter().map(|violation| violation.kind.clone()).collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }

    /// The report as text: the location, the mode, and a line for each reason and violation.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "screen {}", self.location);
        let _ = writeln!(out, "  mode {}", self.mode);
        let verdict = if self.is_clean() {
            "clean"
        } else if self.eligible && self.violations.is_empty() {
            "static half only"
        } else if self.eligible {
            "violations"
        } else {
            "stays a server merge"
        };
        let _ = writeln!(
            out,
            "  marked {} eligible {} full-check {} verdict {verdict}",
            self.marked, self.eligible, self.full_check
        );
        if self.template_bytes > 0 {
            let _ = writeln!(
                out,
                "  template {} bytes, compiled {} bytes",
                self.template_html_bytes, self.template_bytes
            );
        }
        for reason in &self.reasons {
            let _ = writeln!(out, "  reason: {reason}");
        }
        for violation in &self.violations {
            let _ = writeln!(
                out,
                "  violation [{}] {} {} : {}",
                violation.kind,
                short(&violation.screen),
                violation.element,
                violation.detail
            );
        }
        out
    }
}

fn short(location: &str) -> &str {
    location.rsplit('/').next().unwrap_or(location)
}

struct Active {
    report: StaticReport,
    force: bool,
    expression: String,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

/// A report being made on this thread. Dropping it stops recording.
pub struct ReportGuard {
    done: bool,
}

impl ReportGuard {
    /// Start recording. `force` makes screens that are not marked answer as if they were.
    pub fn start(force: bool) -> Self {
        ACTIVE.with(|slot| {
            *slot.borrow_mut() = Some(Active {
                report: StaticReport {
                    mode: "qvt".to_string(),
                    ..StaticReport::default()
                },
                force,
                expression: String::new(),
            });
        });
        Self { done: false }
    }

    /// Stop recording and return what was recorded.
    pub fn finish(mut self) -> StaticReport {
        self.done = true;
        ACTIVE
            .with(|slot| slot.borrow_mut().take())
            .map(|active| active.report)
            .unwrap_or_default()
    }
}

impl Drop for ReportGuard {
    fn drop(&mut self) {
        if !self.done {
            ACTIVE.with(|slot| slot.borrow_mut().take());
        }
    }
}

pub fn report_active() -> bool {
    ACTIVE.with(|slot| slot.borrow().is_some())
}

/// Whether the report wants the static macros to run for a screen that is not marked.
pub fn report_forces_static() -> bool {
    ACTIVE.with(|slot| slot.borrow().as_ref().is_some_and(|active| active.force))
}

fn push(kind: &str, screen: &str, element: &str, detail: &str) {
    ACTIVE.with(|slot| {
        if let Some(active) = slot.borrow_mut().as_mut() {
            let violation = Violation {
                kind: kind.to_string(),
                screen: screen.to_string(),
                element: element.to_string(),
                detail: detail.to_string(),
            };
            if !active.report.violations.contains(&violation) {
                active.report.violations.push(violation);
            }
        }
    });
}

fn current_screen() -> String {
    with_current_render(|state| crate::screen::sri::active_def(state).location.clone()).unwrap_or_default()
}

/// An expression a static macro wrote into the template: check what a browser cannot do.
pub fn record_ast(expression: &str, element: &str, attribute: &str) {
    if !report_active() {
        return;
    }
    let screen = current_screen();
    for found in crate::screen::static_ast::check_expression(expression) {
        push(
            found.kind,
            &screen,
            &format!("{element}@{attribute}"),
            &format!("{} in `{}`", found.operation, clip(expression)),
        );
    }
}

/// Server-only work a static macro could not write into the template.
pub fn record_note(kind: &str, element: &str, detail: &str) {
    if report_active() {
        push(kind, &current_screen(), element, detail);
    }
}

/// An area of `ec` the browser does not have, used by a widget expression in the full check.
pub fn record_wrapped(area: &str) {
    if !report_active() {
        return;
    }
    let (screen, expression) = (
        current_screen(),
        ACTIVE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|active| active.expression.clone())
                .unwrap_or_default()
        }),
    );
    let element = with_current_render(|state| {
        let def = crate::screen::sri::active_def(state);
        locate_expression(&def, &expression)
    })
    .flatten()
    .unwrap_or_default();
    push(
        "wrapped-ec",
        &screen,
        &element,
        &format!("ec.{area} in `{}`", clip(&expression)),
    );
}

fn clip(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 120 {
        let head: String = one_line.chars().take(117).collect();
        format!("{head}...")
    } else {
        one_line
    }
}

/// The element and attribute of the screen XML that holds `text`, to say where a widget expression came from.
fn locate_expression(def: &ScreenDef, text: &str) -> Option<String> {
    let needle = text
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim()
        .to_string();
    if needle.is_empty() {
        return None;
    }
    let root = def.screen_node().ok()?;
    let mut found = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        for (key, value) in node.attributes() {
            if value.contains(&needle) {
                found = Some(format!("{}@{key}", node.name()));
            }
        }
        if node.text().is_some_and(|body| body.contains(&needle)) {
            found = Some(node.name());
        }
        stack.extend(node.children());
        if found.is_some() {
            break;
        }
    }
    found
}

/// Set the expression a widget evaluation is for, so [`record_wrapped`] can say where it is.
pub fn set_expression(text: &str) {
    ACTIVE.with(|slot| {
        if let Some(active) = slot.borrow_mut().as_mut() {
            active.expression = text.to_string();
        }
    });
}

/// While this lives, widget expressions run with the wrapped `ec` and each use of an area outside the browser's host is recorded.
pub struct WidgetCheck {
    previous: bool,
}

impl WidgetCheck {
    pub fn start() -> Self {
        Self {
            previous: crate::script::set_widget_wrap(true),
        }
    }
}

impl Drop for WidgetCheck {
    fn drop(&mut self) {
        crate::script::set_widget_wrap(self.previous);
    }
}

pub fn report_ineligible(def: &ScreenDef, eligibility: &super::Eligibility) {
    ACTIVE.with(|slot| {
        if let Some(active) = slot.borrow_mut().as_mut() {
            active.report.location.clone_from(&def.location);
            active.report.marked = eligibility.marked;
            active.report.eligible = false;
            active.report.reasons.clone_from(&eligibility.reasons);
        }
    });
}

pub fn report_failure(def: &ScreenDef, err: &Error) {
    ACTIVE.with(|slot| {
        if let Some(active) = slot.borrow_mut().as_mut() {
            active.report.location.clone_from(&def.location);
            active.report.eligible = false;
            active
                .report
                .reasons
                .push(format!("the template could not be made: {err}"));
        }
    });
}

pub fn report_template(def: &ScreenDef, template: &StaticTemplate) {
    ACTIVE.with(|slot| {
        if let Some(active) = slot.borrow_mut().as_mut() {
            active.report.location.clone_from(&def.location);
            active.report.marked = def.server_static.iter().any(|name| name == "all" || name == "qvt");
            active.report.eligible = true;
            active.report.template_bytes = template.script.len();
            active.report.template_html_bytes = template.html.len();
        }
    });
}

// ---- the scan of the definitions ----

const SIDE_EFFECT_ELEMENTS: [&str; 5] = [
    "service-call",
    "entity-make-value",
    "entity-delete-by-condition",
    "entity-data",
    "message",
];
const SIDE_EFFECT_CALLS: [&str; 7] = [
    ".create(",
    ".update(",
    ".delete(",
    ".store(",
    ".createOrUpdate(",
    "ec.message.add",
    "ec.service.",
];

/// The verbs of a service that only reads: a call to one in `always-actions` or `pre-actions` is not a side effect.
const READ_ONLY_VERBS: [&str; 3] = ["get", "find", "setup"];

/// A service call that only reads: its verb is get, find or setup, or its definition says `read-only="true"`.
fn read_only_call(name: &str) -> bool {
    let tail = name.rsplit('.').next().unwrap_or(name);
    let verb = tail.split('#').next().unwrap_or(tail);
    READ_ONLY_VERBS.contains(&verb) || crate::service::define::find(name).is_some_and(|def| def.read_only)
}

/// `always-actions` and `pre-actions` that write, send or add a message. The form request runs them again. A service
/// call of a read-only service ([`read_only_call`]) is not one.
fn scan_actions(def: &ScreenDef, which: &str, xml: &str) {
    let Ok(root) = MNode::parse_text(&def.location, xml) else {
        return;
    };
    let mut elements = Vec::new();
    descendants_named(&root, &SIDE_EFFECT_ELEMENTS, &mut elements);
    for element in elements {
        let name = element
            .attribute("name")
            .or_else(|| element.attribute("entity-name"))
            .unwrap_or_default();
        if element.name() == "service-call" && read_only_call(&name) {
            continue;
        }
        push(
            "side-effect",
            &def.location,
            &format!("{which}/{}", element.name()),
            &format!("{} {name} is repeated by the form request", element.name()),
        );
    }
    let mut scripts = Vec::new();
    descendants_named(&root, &["script"], &mut scripts);
    for script in scripts {
        let body = script.text().unwrap_or_default();
        if let Some(call) = SIDE_EFFECT_CALLS.iter().find(|call| body.contains(*call)) {
            push(
                "side-effect",
                &def.location,
                &format!("{which}/script"),
                &format!("a script that calls {call} is repeated by the form request"),
            );
        }
    }
}

/// The names the page keys of a list go by.
fn page_keys(list: &str) -> Vec<String> {
    PAGE_SUFFIXES.iter().map(|suffix| format!("{list}{suffix}")).collect()
}

/// What a definition holds that a static template changes: see the module comment.
fn scan_definition(
    state_facade: &crate::screen::ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &Arc<ScreenDef>,
    pre_actions: bool,
) {
    if let Some(actions) = &def.always_actions {
        scan_actions(def, "always-actions", &actions.xml);
    }
    if pre_actions {
        if let Some(actions) = &def.pre_actions {
            scan_actions(def, "pre-actions", &actions.xml);
        }
    }
    // Lists the forms show.
    let mut lists: Vec<String> = Vec::new();
    for (name, definition) in &def.forms {
        if !definition.list {
            continue;
        }
        match crate::screen::form::assemble(state_facade, factory, def, name) {
            Ok(instance) => lists.push(instance.list_name.clone()),
            Err(err) => push(
                "unassembled-form",
                &def.location,
                name,
                &format!("the form could not be assembled: {err}"),
            ),
        }
    }
    for (name, section) in &def.sections {
        if let SectionKind::Iterate { list, .. } = &section.kind {
            if lists.contains(list) {
                push(
                    "form-list-key",
                    &def.location,
                    &format!("section-iterate@{name}"),
                    &format!("iterates {list}, which is the list of a form-list and is left out of the screen package"),
                );
            }
        }
    }
    // A form-list inside a section-iterate: its form request (`/actions/<form>`) runs the screen's actions once, with no
    // iteration entry, so a list such as `entry.rows` is not there and each iteration would get the same rows.
    for (section_name, section) in &def.sections {
        let (SectionKind::Iterate { .. }, Some(widgets)) = (&section.kind, &section.widgets) else {
            continue;
        };
        let Ok(node) = MNode::parse_text(&def.location, widgets.text()) else {
            continue;
        };
        let mut iterated = Vec::new();
        descendants_named(&node, &["form-list"], &mut iterated);
        for form in iterated {
            let name = form.attribute("name").unwrap_or_default();
            let list = form.attribute("list").unwrap_or_default();
            push(
                "form-list-iterate",
                &def.location,
                &format!("form-list@{name}"),
                &format!("a form-list in section-iterate {section_name} (list {list}) is one form request, outside the iteration"),
            );
        }
    }
    // A label outside the lists that reads the page keys.
    let Ok(source) = def.screen_node() else {
        return;
    };
    // A condition outside the forms that reads a form-list's list (`spaceList.length`): the list is in the form's package
    // only, so the browser reads it as undefined.
    let mut conditions: Vec<(String, String)> = Vec::new();
    collect_conditions(&source, &mut conditions);
    for (section_name, section) in &def.sections {
        if let Some(Condition::Expression(expression)) = &section.condition {
            conditions.push((format!("section@{section_name}"), expression.clone()));
        }
        for fragment in [&section.widgets, &section.fail_widgets].into_iter().flatten() {
            if let Ok(node) = MNode::parse_text(&def.location, fragment.text()) {
                collect_conditions(&node, &mut conditions);
            }
        }
    }
    for (element, expression) in conditions {
        if let Some(list) = lists.iter().find(|list| names_identifier(&expression, list)) {
            push(
                "form-list-key",
                &def.location,
                &format!("{element}@condition"),
                &format!("reads {list} outside its form-list; the list is in the form package only"),
            );
        }
    }
    let keys: Vec<String> = lists.iter().flat_map(|list| page_keys(list)).collect();
    if keys.is_empty() {
        return;
    }
    let mut outside = Vec::new();
    collect_outside_lists(&source, &mut outside);
    for label in outside {
        let text = format!(
            "{} {}",
            label.attribute("text").unwrap_or_default(),
            label.attribute("condition").unwrap_or_default()
        );
        if let Some(key) = keys.iter().find(|key| text.contains(key.as_str())) {
            push(
                "form-list-key",
                &def.location,
                "label",
                &format!("reads {key} outside its form-list; the key is in the form package only"),
            );
        }
    }
}

/// The `condition` attributes of the elements outside the forms, with the element's name.
fn collect_conditions(node: &MNode, out: &mut Vec<(String, String)>) {
    for child in node.children() {
        if matches!(child.name().as_str(), "form-list" | "form-single") {
            continue;
        }
        if let Some(condition) = child.attribute("condition").filter(|text| !text.is_empty()) {
            out.push((child.name().to_string(), condition));
        }
        collect_conditions(&child, out);
    }
}

/// Whether `expression` names `name` as a whole identifier (`spaceList.length`, not `mySpaceList`).
fn names_identifier(expression: &str, name: &str) -> bool {
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '$';
    expression.match_indices(name).any(|(at, _)| {
        let before = expression[..at].chars().next_back();
        let after = expression[at + name.len()..].chars().next();
        !before.is_some_and(|ch| ident(ch) || ch == '.') && !after.is_some_and(ident)
    })
}

fn collect_outside_lists(node: &MNode, out: &mut Vec<MNode>) {
    for child in node.children() {
        match child.name().as_str() {
            "form-list" | "form-single" => {}
            "label" => out.push(child.clone()),
            _ => collect_outside_lists(&child, out),
        }
    }
}

fn is_automatic(def: &ScreenDef) -> bool {
    def.server_static_auto
}

/// A screen marked static for a mode other than qvt sends the cache header, which is right only for output that does not vary.
fn scan_marks(def: &ScreenDef) {
    if is_automatic(def) {
        return;
    }
    for mode in &def.server_static {
        if mode != "qvt" && mode != "all" {
            push(
                "stale-cache",
                &def.location,
                "screen@server-static",
                &format!("server-static names {mode}, so its output is cached for a day and shared; a screen with data is stale or wrong for the next user"),
            );
        }
    }
}

/// The report of the screen at `path` below `root` for static qvt.
///
/// The static macros run for the screen whether or not it is marked, so the report says what marking it would do. With
/// `full` the screen is also rendered in `html` with its real actions, and its widget expressions run with the wrapped `ec`: the
/// render goes on after a violation. The current execution context supplies the user and the data.
///
/// # Errors
///
/// Returns an error when the path does not resolve.
pub fn static_report(
    factory: &Arc<ExecutionContextFactory>,
    root: &str,
    path: &str,
    parameters: &BTreeMap<String, Value>,
    full: bool,
) -> Result<StaticReport, Error> {
    let guard = ReportGuard::start(true);
    let info = ScreenUrlInfo::parse(factory, root, path, 1)?;
    let facade = crate::screen::ScreenFacade::of(factory);
    ACTIVE.with(|slot| {
        if let Some(active) = slot.borrow_mut().as_mut() {
            active.report.location.clone_from(&info.target_screen.location);
            active.report.path = path.to_string();
        }
    });
    let first_pre = if facade.is_always_standalone("qvt") && info.path_defs.len() > 2 {
        2
    } else {
        info.render_path_difference
    };
    for (index, def) in info.path_defs.iter().enumerate() {
        scan_definition(&facade, factory, def, index >= first_pre);
    }
    scan_marks(&info.target_screen);
    let eligibility = super::eligibility_of(&facade, factory, &info.target_screen, super::MERGED_MODULE, true);
    if !eligibility.eligible {
        report_ineligible(&info.target_screen, &eligibility);
        return Ok(guard.finish());
    }
    let rendered = make_render(factory)
        .root_screen(root)
        .screen_path(format!("{}.qvt", path.split('?').next().unwrap_or(path)))
        .render_mode("qvt")
        .parameters(parameters.clone())
        .render();
    if let Err(err) = rendered {
        push(
            "static-pass",
            &info.target_screen.location,
            "",
            &format!("the static render failed: {err}"),
        );
    }
    if full {
        ACTIVE.with(|slot| {
            if let Some(active) = slot.borrow_mut().as_mut() {
                active.report.full_check = true;
            }
        });
        let _wrap = WidgetCheck::start();
        let rendered = make_render(factory)
            .root_screen(root)
            .screen_path(path.split('?').next().unwrap_or(path))
            .render_mode("html")
            .parameters(parameters.clone())
            .render();
        match rendered {
            Ok(output) if output.body_text().contains("Error rendering") => push(
                "full-check",
                &info.target_screen.location,
                "",
                "a section of the render raised an error, so the widgets after it were not checked",
            ),
            Ok(_) => {}
            Err(err) => push(
                "full-check",
                &info.target_screen.location,
                "",
                &format!("the render failed, so its widgets were not checked: {err}"),
            ),
        }
    }
    Ok(guard.finish())
}

/// The location of a screen file and its report, or the error that stopped it.
pub type FileReport = (String, Result<StaticReport, Error>);

/// The reports of screen files that may not be mounted anywhere: each is mounted under a root made for the run, so the path walk, the
/// forms and the macros work as they do for a request. The full check needs the data of a real path, so this is the static half.
///
/// # Errors
///
/// Returns an error when the root cannot be written or loaded.
pub fn static_reports_for_files(
    factory: &Arc<ExecutionContextFactory>,
    locations: &[String],
) -> Result<Vec<FileReport>, Error> {
    let mut xml =
        String::from("<screen require-authentication=\"false\"><widgets><subscreens-active/></widgets><subscreens>");
    for (index, location) in locations.iter().enumerate() {
        let _ = write!(
            xml,
            "<subscreens-item name=\"S{index}\" location=\"{}\"/>",
            moqui_lib::string::encode_for_xml_attribute(location, false)
        );
    }
    xml.push_str("</subscreens></screen>");
    let dir = TempDir::create("moqui-qvt-report")?;
    let root = dir.0.join("root.xml");
    write_new(&root, &xml)?;
    let root_location = root.to_string_lossy().into_owned();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut out = Vec::new();
    for (index, location) in locations.iter().enumerate() {
        let report = static_report(factory, &root_location, &format!("/S{index}"), &BTreeMap::new(), false);
        out.push((location.clone(), report));
    }
    Ok(out)
}

/// A directory under the system temp dir with a random name, made by this process alone (owner-only
/// on unix), and removed with what is in it on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn create(prefix: &str) -> Result<Self, Error> {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|err| Error::new(format!("could not read random bytes: {err}")))?;
        let path = std::env::temp_dir().join(format!("{prefix}-{:016x}", u64::from_le_bytes(bytes)));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(&path)
            .map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_new(path: &Path, text: &str) -> Result<(), Error> {
    use std::io::Write as _;
    std::fs::File::create_new(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|err| Error::new(format!("{}: {err}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::{write_new, TempDir};

    #[test]
    fn the_report_root_dir_is_random_and_removed() {
        let first = TempDir::create("moqui-qvt-report").unwrap();
        let second = TempDir::create("moqui-qvt-report").unwrap();
        assert_ne!(first.0, second.0);
        let file = first.0.join("root.xml");
        write_new(&file, "<screen/>").unwrap();
        assert!(
            write_new(&file, "<screen/>").is_err(),
            "an existing file is not written over"
        );
        let path = first.0.clone();
        drop(first);
        assert!(!path.exists());
    }
}
