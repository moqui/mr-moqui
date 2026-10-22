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

//! `moqui lint --screens`: every expression site in screen and form XML must parse as JavaScript,
//! and the Groovy idioms that survive are classified per file. This is the pre-scan for the screen
//! trees. FreeMarker bodies of `<text template="true">` are listed and not parsed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use moqui_lib::node::MNode;

use crate::context::Error;

/// Elements that hold XML actions. The generated script of each must parse.
const ACTION_CONTAINERS: &[&str] = &["actions", "always-actions", "pre-actions", "row-actions"];

/// Action elements that appear outside a container, such as a transition's `service-call`.
const LOOSE_ACTIONS: &[&str] = &[
    "service-call",
    "entity-find",
    "entity-find-one",
    "entity-find-count",
    "script",
];

/// Screens that stay Groovy for now, by path suffix and reason. They are listed in the report and not scanned. None
/// are left: the speed test and the SQL runners became JavaScript in phase 11.
pub const KNOWN_EXCLUSIONS: &[(&str, &str)] = &[];

/// Root elements of screen and form XML files.
const ROOTS: &[&str] = &["screen", "forms", "widget-template", "widget-templates"];

const JS_GLOBALS: &[&str] = &[
    "Math", "JSON", "Object", "Array", "Number", "String", "Date", "Boolean", "Symbol", "Map", "Set", "Intl",
    "Reflect", "Promise", "BigInt", "RegExp", "Error",
];

/// Statics the script runtime defines as globals besides the JavaScript ones, so a call on them is not Java.
const HOST_STATICS: &[&str] = &["ResourceReference"];

/// Java collection methods that have no JavaScript meaning.
const COLLECTION_METHODS: &[&str] = &[
    ".size()",
    ".isEmpty()",
    ".contains(",
    ".containsKey(",
    ".keySet()",
    ".entrySet()",
    ".put(",
    ".toList()",
    ".getAt(",
    ".equals(",
];

/// Methods that are Java collection calls in Groovy and also the host decimal, `Set`, or `ec.context` API in a port.
/// They cannot be told apart without types, so they are counted and not flagged.
const ADD_METHODS: &[&str] = &[".add(", ".addAll(", ".putAll("];

/// A Groovy idiom and whether the text still runs as JavaScript when it is present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Idiom {
    Elvis,
    EmptyCollectionTruthiness,
    Range,
    Closure,
    GString,
    GStringDollarName,
    RegexOperator,
    DecimalLiteral,
    /// `1.5`: a `BigDecimal` in Groovy and a number in JavaScript, so it is valid and may differ in arithmetic.
    DecimalPointLiteral,
    BigDecimal,
    /// `.setScale(n, 'MODE')` is the host decimal API.
    SetScale,
    NewType,
    /// `new Date`, `new Set` and the other JavaScript classes.
    NewJsType,
    MapLiteral,
    ListStar,
    AsCast,
    InOperator,
    Def,
    StaticClass,
    QuotedProperty,
    CollectionMethod,
    /// `.add(`, `.addAll(` and `.putAll(`.
    AddCall,
    Catch,
    /// `catch (e)`, which is JavaScript. `catch (Exception e)` is [`Idiom::Catch`].
    CatchBare,
    SafeNavigation,
    ContextPlus,
    /// A call on a global the host defines, such as `ResourceReference.isTextFilename(..)`.
    HostStatic,
    /// A host decimal under `<`, `>`, `==`, `!=`, `!x`, `x ||`, `x ? :` or an arithmetic operator. The value is always
    /// truthy and its `valueOf` throws, so the test or the arithmetic does not do what Groovy did.
    DecimalOperator,
    /// `push`, `add`, `first()`, `each(` and the like on an `entity-find` result, which is a read-only host list.
    FindListMutation,
    /// `x in list` on an array, which JavaScript reads as an index test.
    InOnList,
    /// A `const` or `let` of a name another `<script>` of the same service already declares at the function level of
    /// the generated script, which JavaScript refuses to run.
    DuplicateDeclaration,
    /// `.replaceAll('[^a-z]+', '')` and the like: Java reads the string as a regular expression, JavaScript as text.
    StringAsRegex,
    /// `==` or `!=` between an entity value copied with `cloneValue()` and another value: Groovy compared the fields,
    /// JavaScript compares identity, so the test is never true. `a.equals(b)` compares the fields.
    EntityValueIdentity,
}

/// How an idiom counts in a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Runs as JavaScript and means the same. Counted in the totals and not flagged.
    Valid,
    /// Looks like JavaScript and probably misbehaves at run time.
    Hazard,
    /// Groovy or Java that has to be rewritten.
    Groovy,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Valid => "valid JavaScript",
            Severity::Hazard => "hazard",
            Severity::Groovy => "groovy",
        }
    }
}

impl Idiom {
    pub fn name(self) -> &'static str {
        match self {
            Idiom::Elvis => "elvis",
            Idiom::EmptyCollectionTruthiness => "empty-collection-truthiness",
            Idiom::Range => "range",
            Idiom::Closure => "closure",
            Idiom::GString => "gstring",
            Idiom::GStringDollarName => "gstring-dollar-name",
            Idiom::RegexOperator => "regex-operator",
            Idiom::DecimalLiteral => "decimal-literal",
            Idiom::DecimalPointLiteral => "decimal-point-literal",
            Idiom::BigDecimal => "bigdecimal",
            Idiom::SetScale => "set-scale",
            Idiom::NewType => "new-type",
            Idiom::NewJsType => "new-js-type",
            Idiom::MapLiteral => "map-literal",
            Idiom::ListStar => "list-star",
            Idiom::AsCast => "as-cast",
            Idiom::InOperator => "in-operator",
            Idiom::Def => "def",
            Idiom::StaticClass => "static-class",
            Idiom::QuotedProperty => "quoted-property",
            Idiom::CollectionMethod => "collection-method",
            Idiom::AddCall => "add-call",
            Idiom::Catch => "catch",
            Idiom::CatchBare => "catch-bare",
            Idiom::SafeNavigation => "safe-navigation",
            Idiom::ContextPlus => "context-plus",
            Idiom::HostStatic => "host-static",
            Idiom::DecimalOperator => "decimal-operator",
            Idiom::FindListMutation => "find-list-mutation",
            Idiom::InOnList => "in-on-list",
            Idiom::DuplicateDeclaration => "duplicate-declaration",
            Idiom::StringAsRegex => "string-as-regex",
            Idiom::EntityValueIdentity => "entity-value-identity",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Idiom::SafeNavigation
            | Idiom::DecimalPointLiteral
            | Idiom::SetScale
            | Idiom::NewJsType
            | Idiom::AddCall
            | Idiom::CatchBare
            | Idiom::HostStatic => Severity::Valid,
            Idiom::DecimalOperator
            | Idiom::FindListMutation
            | Idiom::InOnList
            | Idiom::EmptyCollectionTruthiness
            | Idiom::DuplicateDeclaration
            | Idiom::EntityValueIdentity => Severity::Hazard,
            _ => Severity::Groovy,
        }
    }

    /// Groovy or a hazard: what a default report flags.
    pub fn is_groovy(self) -> bool {
        self.severity() != Severity::Valid
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteKind {
    /// An attribute that is a whole expression.
    Expression,
    /// A `${...}` inside plain attribute text, which is a JsTemplateLiteral.
    Template,
    /// A `<script>` or `<expression>` body.
    Script,
}

#[derive(Clone, Debug)]
pub struct Site {
    pub line: u32,
    pub element: String,
    pub attribute: String,
    pub kind: SiteKind,
    pub text: String,
    pub error: Option<String>,
    pub idioms: Vec<(Idiom, String)>,
}

impl Site {
    /// A parse failure, a Groovy idiom or a hazard. With `all`, also an idiom that is valid JavaScript.
    fn flagged(&self, all: bool) -> bool {
        self.error.is_some() || self.idioms.iter().any(|(idiom, _)| all || idiom.is_groovy())
    }
}

#[derive(Clone, Debug)]
pub struct FtlRef {
    pub line: u32,
    pub element: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct FileReport {
    pub path: PathBuf,
    pub sites: Vec<Site>,
    pub generated_errors: Vec<(u32, String)>,
    /// `.ftl` locations. They are listed and not compiled.
    pub ftl: Vec<FtlRef>,
    /// Inline template bodies that compile in the fancy dialect.
    pub dialect_bodies: usize,
    /// Inline template bodies that do not compile, such as leftover FreeMarker.
    pub template_errors: Vec<(u32, String)>,
    /// Browser-only parts of a screen the tui mode renders: a `qvt` or `qvue` text with no `tui` text beside it, or a
    /// `qvue` screen without `tui` in its modes (D10). Information: a browser-only screen is allowed.
    pub tui_missing: Vec<(u32, String)>,
    /// `tui` text bodies with a `${` that is not inside `JSON.stringify(`, which can write a value that breaks the JSON.
    pub tui_unsafe: Vec<u32>,
    /// Elements of a screen's widgets that no macro names, often a typo: a render logs each and writes a placeholder.
    pub unknown_elements: Vec<(u32, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct ScreenLintReport {
    pub files: Vec<FileReport>,
    /// Path and reason of each file that matched [`KNOWN_EXCLUSIONS`].
    pub excluded: Vec<(PathBuf, &'static str)>,
    /// `.groovy` files under the roots of a Groovy lint. They are listed and not parsed.
    pub groovy_files: Vec<PathBuf>,
    /// XML files a Groovy lint could not parse, with the error.
    pub unparsed: Vec<(PathBuf, String)>,
    /// The first line of the rendered report. Empty is the screen lint's.
    pub title: &'static str,
    /// Flag and list the sites that hold only idioms that are valid JavaScript (`lint --groovy --all`).
    pub show_all: bool,
}

impl ScreenLintReport {
    pub fn site_count(&self) -> usize {
        self.files.iter().map(|file| file.sites.len()).sum()
    }

    /// One line per parse failure: a site that is not JavaScript, or an action block whose
    /// generated script does not parse.
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for file in &self.files {
            for site in &file.sites {
                if let Some(error) = &site.error {
                    out.push(format!(
                        "{}:{} {}@{}: {error}",
                        file.path.display(),
                        site.line,
                        site.element,
                        site.attribute
                    ));
                }
            }
            for (line, name) in &file.unknown_elements {
                out.push(format!(
                    "{}:{line} {name}: no screen macro names this element, so it renders as a placeholder",
                    file.path.display()
                ));
            }
            for (line, error) in file.generated_errors.iter().chain(&file.template_errors) {
                out.push(format!("{}:{line} {error}", file.path.display()));
            }
        }
        out
    }

    /// Sites per idiom, and the number of files holding each.
    pub fn idiom_totals(&self) -> BTreeMap<Idiom, (usize, usize)> {
        let mut totals: BTreeMap<Idiom, (usize, BTreeSet<usize>)> = BTreeMap::new();
        for (index, file) in self.files.iter().enumerate() {
            for site in &file.sites {
                let kinds: BTreeSet<Idiom> = site.idioms.iter().map(|(idiom, _)| *idiom).collect();
                for idiom in kinds {
                    let entry = totals.entry(idiom).or_default();
                    entry.0 += 1;
                    entry.1.insert(index);
                }
            }
        }
        totals
            .into_iter()
            .map(|(idiom, (n, files))| (idiom, (n, files.len())))
            .collect()
    }

    /// Sites that still hold a Groovy idiom or a hazard, or fail to parse.
    pub fn flagged_count(&self) -> usize {
        self.files
            .iter()
            .map(|file| file.sites.iter().filter(|site| site.flagged(self.show_all)).count())
            .sum()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let failures = self.failures();
        let ftl: usize = self.files.iter().map(|file| file.ftl.len()).sum();
        let dialect: usize = self.files.iter().map(|file| file.dialect_bodies).sum();
        let _ = writeln!(
            out,
            "{}",
            if self.title.is_empty() {
                "Screen expression lint"
            } else {
                self.title
            }
        );
        let _ = writeln!(
            out,
            "files scanned: {} (excluded: {})",
            self.files.len(),
            self.excluded.len()
        );
        let _ = writeln!(out, "expression sites: {}", self.site_count());
        let site_failures: usize = self
            .files
            .iter()
            .map(|file| file.sites.iter().filter(|site| site.error.is_some()).count())
            .sum();
        let _ = writeln!(
            out,
            "parse failures: {} (site: {site_failures}, generated action script or template body: {})",
            failures.len(),
            failures.len() - site_failures
        );
        let _ = writeln!(
            out,
            "sites with a Groovy idiom, hazard or failure{}: {}",
            if self.show_all {
                " (or valid JavaScript idiom)"
            } else {
                ""
            },
            self.flagged_count()
        );
        let _ = writeln!(out, "fancy template bodies compiled: {dialect}");
        let _ = writeln!(out, "FreeMarker template locations (not compiled): {ftl}");
        let _ = writeln!(out, "\nIdiom totals (sites, files)");
        for severity in [Severity::Groovy, Severity::Hazard, Severity::Valid] {
            let totals: Vec<_> = self
                .idiom_totals()
                .into_iter()
                .filter(|(idiom, _)| idiom.severity() == severity)
                .collect();
            if totals.is_empty() {
                continue;
            }
            let note = if severity == Severity::Valid && !self.show_all {
                ", not flagged"
            } else {
                ""
            };
            let _ = writeln!(out, " {}{note}", severity.name());
            for (idiom, (sites, files)) in totals {
                let _ = writeln!(out, "  {:<30} {sites:>5} {files:>5}", idiom.name());
            }
        }
        let _ = writeln!(out, "\nParse failures");
        for failure in &failures {
            let _ = writeln!(out, "  {failure}");
        }
        if !self.groovy_files.is_empty() || !self.unparsed.is_empty() {
            let _ = writeln!(out, "\nGroovy files (not parsed): {}", self.groovy_files.len());
            for path in &self.groovy_files {
                let _ = writeln!(out, "  {}", path.display());
            }
            let _ = writeln!(out, "\nXML files that do not parse: {}", self.unparsed.len());
            for (path, error) in &self.unparsed {
                let _ = writeln!(out, "  {}: {error}", path.display());
            }
        }
        // The tui worklist is the screen lint's; a Groovy pre-scan reports only what is left to convert.
        if self.title.is_empty() {
            let missing: usize = self.files.iter().map(|file| file.tui_missing.len()).sum();
            let unsafe_texts: usize = self.files.iter().map(|file| file.tui_unsafe.len()).sum();
            let screens = self.files.iter().filter(|file| !file.tui_missing.is_empty()).count();
            let _ = writeln!(
                out,
                "\nTUI worklist: {missing} browser-only part(s) in {screens} screen(s) with no tui text (information)"
            );
            for file in &self.files {
                for (line, detail) in &file.tui_missing {
                    let _ = writeln!(out, "  {}:{line} {detail}", file.path.display());
                }
            }
            let _ = writeln!(
                out,
                "\nTUI texts with a ${{}} outside JSON.stringify(), which can break the JSON: {unsafe_texts}"
            );
            for file in &self.files {
                for line in &file.tui_unsafe {
                    let _ = writeln!(out, "  {}:{line}", file.path.display());
                }
            }
        }
        let _ = writeln!(out, "\nKnown exclusions (not scanned)");
        for (path, reason) in &self.excluded {
            let _ = writeln!(out, "  {}: {reason}", path.display());
        }
        let _ = writeln!(out, "\nFreeMarker");
        for file in &self.files {
            for item in &file.ftl {
                let _ = writeln!(
                    out,
                    "  {}:{} {} {}",
                    file.path.display(),
                    item.line,
                    item.element,
                    item.detail
                );
            }
        }
        let _ = writeln!(out, "\nPer file (flagged sites only; before text)");
        for file in &self.files {
            let flagged: Vec<&Site> = file.sites.iter().filter(|site| site.flagged(self.show_all)).collect();
            if flagged.is_empty() {
                continue;
            }
            let _ = writeln!(
                out,
                "\n{} ({} sites, {} flagged)",
                file.path.display(),
                file.sites.len(),
                flagged.len()
            );
            for site in flagged {
                let idioms: Vec<&str> = site.idioms.iter().map(|(idiom, _)| idiom.name()).collect();
                let _ = writeln!(
                    out,
                    "  {}: {}@{} [{}] {}",
                    site.line,
                    site.element,
                    site.attribute,
                    idioms.join(","),
                    site.error.as_deref().unwrap_or("")
                );
                let _ = writeln!(out, "      before: {}", one_line(&site.text));
            }
        }
        out
    }
}

/// Scan the roots. A root that is a file is scanned as is. A directory is walked for XML files.
/// Under `runtime` style roots only paths with a `screen` directory component count, so service
/// and data XML is skipped.
///
/// # Errors
///
/// Returns an error when a directory cannot be read or an XML file does not parse.
pub fn lint_screens(roots: &[PathBuf]) -> Result<ScreenLintReport, Error> {
    let mut paths = Vec::new();
    for root in roots {
        collect(root, false, &mut paths)?;
    }
    paths.sort();
    paths.dedup();
    let mut report = ScreenLintReport::default();
    let (names, opaque) = crate::screen::generate::widget_vocabulary()?;
    for path in paths {
        if let Some((_, reason)) = KNOWN_EXCLUSIONS.iter().find(|(suffix, _)| path.ends_with(suffix)) {
            report.excluded.push((path, reason));
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
        let root = MNode::parse_text(&path.display().to_string(), &text)?;
        if !ROOTS.contains(&root.name().as_str()) {
            continue;
        }
        let mut file = FileReport {
            path,
            ..FileReport::default()
        };
        scan_node(&root, false, &mut file);
        scan_widgets(&root, &names, &opaque, &mut file);
        super::lint_hazards::refine(&root, &mut file);
        if root.name() == "screen" {
            scan_tui(&root, &mut file);
        }
        report.files.push(file);
    }
    Ok(report)
}

/// The render modes a screen allows, as `allows_render_mode` reads them: none set, or `all`, is every mode.
fn screen_allows(screen: &MNode, mode: &str) -> bool {
    let modes = screen.attribute("render-modes").unwrap_or_default();
    modes.trim().is_empty()
        || modes
            .split(',')
            .any(|name| matches!(name.trim(), "all") || name.trim() == mode)
}

fn text_types(text: &MNode) -> Vec<String> {
    text.attribute("type")
        .unwrap_or_default()
        .split(',')
        .map(|kind| kind.trim().to_string())
        .filter(|kind| !kind.is_empty())
        .collect()
}

/// The two tui findings of plans/archive/Screen-TUI-Impl.md 2.9.
fn scan_tui(screen: &MNode, file: &mut FileReport) {
    let modes = screen.attribute("render-modes").unwrap_or_default();
    let named: Vec<&str> = modes.split(',').map(str::trim).collect();
    if named.contains(&"qvue") && !named.contains(&"tui") && !named.contains(&"all") {
        file.tui_missing
            .push((screen.line(), format!("render-modes \"{modes}\" has qvue and no tui")));
    }
    let renders_tui = screen_allows(screen, "tui");
    let mut stack = vec![screen.clone()];
    while let Some(node) = stack.pop() {
        if node.name() == "render-mode" {
            let texts = node.children_named("text");
            let has_tui = texts
                .iter()
                .any(|text| text_types(text).iter().any(|kind| kind == "tui"));
            let browser = texts
                .iter()
                .find(|text| text_types(text).iter().any(|kind| kind == "qvt" || kind == "qvue"));
            if let (true, false, Some(text)) = (renders_tui, has_tui, browser) {
                file.tui_missing.push((
                    text.line(),
                    format!(
                        "render-mode text type \"{}\" has no tui text",
                        text_types(text).join(",")
                    ),
                ));
            }
        }
        if node.name() == "text" && text_types(&node).iter().any(|kind| kind == "tui") {
            let body = node.text().unwrap_or_default();
            if node.attribute("template").as_deref() != Some("false") && has_bare_expansion(&body) {
                file.tui_unsafe.push(node.line());
            }
        }
        stack.extend(node.children().into_iter().rev());
    }
}

/// A `${...}` whose expression is not a `JSON.stringify(...)` call.
fn has_bare_expansion(body: &str) -> bool {
    let mut rest = body;
    while let Some(at) = rest.find("${") {
        rest = &rest[at + 2..];
        if !rest.trim_start().starts_with("JSON.stringify(") {
            return true;
        }
    }
    false
}

/// Root elements a Groovy lint scans: screens and forms, services, ECA rules, and data files.
const GROOVY_ROOTS: &[&str] = &[
    "screen",
    "forms",
    "widget-template",
    "widget-templates",
    "services",
    "secas",
    "eecas",
    "emecas",
    "gitecas",
    "entity-facade-xml",
    "seed-data",
];

/// Scans a parsed file with the same walk the lints use, for the tests of the hazard rules.
#[cfg(test)]
pub(crate) fn scan_for_tests(root: &MNode, file: &mut FileReport) {
    scan_node(root, false, file);
}

/// `moqui lint --groovy`: the pre-scan of a component tree that has not been migrated, such as mantle. Every XML
/// file under the roots whose root element is a screen, form, service, ECA or data file is scanned as the screen lint
/// scans a screen, and the `.groovy` files are listed. Nothing fails: the report is what is left to convert.
///
/// # Errors
///
/// Returns an error when a directory cannot be read.
pub fn lint_groovy(roots: &[PathBuf], show_all: bool) -> Result<ScreenLintReport, Error> {
    let mut paths = Vec::new();
    let mut groovy = Vec::new();
    for root in roots {
        collect_all(root, &mut paths, &mut groovy)?;
    }
    paths.sort();
    groovy.sort();
    let mut report = ScreenLintReport {
        title: "Groovy idiom lint",
        groovy_files: groovy,
        show_all,
        ..ScreenLintReport::default()
    };
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
        let root = match MNode::parse_text(&path.display().to_string(), &text) {
            Ok(root) => root,
            Err(err) => {
                report.unparsed.push((path, err.to_string()));
                continue;
            }
        };
        if !GROOVY_ROOTS.contains(&root.name().as_str()) {
            continue;
        }
        let mut file = FileReport {
            path,
            ..FileReport::default()
        };
        scan_node(&root, false, &mut file);
        super::lint_hazards::refine(&root, &mut file);
        report.files.push(file);
    }
    Ok(report)
}

/// Every `.xml` and `.groovy` file under `path`, skipping hidden directories and build output.
pub(crate) fn collect_all(path: &Path, xml: &mut Vec<PathBuf>, groovy: &mut Vec<PathBuf>) -> Result<(), Error> {
    if path.is_file() {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("xml") => xml.push(path.to_path_buf()),
            Some("groovy") => groovy.push(path.to_path_buf()),
            _ => {}
        }
        return Ok(());
    }
    let entries = std::fs::read_dir(path).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| Error::new(err.to_string()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "build" || name == "target" || name == "node_modules" {
            continue;
        }
        collect_all(&entry.path(), xml, groovy)?;
    }
    Ok(())
}

fn collect(path: &Path, in_screen: bool, out: &mut Vec<PathBuf>) -> Result<(), Error> {
    if path.is_file() {
        if path.extension().is_some_and(|ext| ext == "xml") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    let entries = std::fs::read_dir(path).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| Error::new(err.to_string()))?;
        let child = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if child.is_dir() {
            collect(&child, in_screen || name == "screen", out)?;
        } else if in_screen || path_has_screen(path) {
            collect(&child, true, out)?;
        }
    }
    Ok(())
}

fn path_has_screen(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == "screen")
}

fn scan_node(node: &MNode, in_actions: bool, file: &mut FileReport) {
    let name = node.name();
    let action_root = ACTION_CONTAINERS.contains(&name.as_str()) || LOOSE_ACTIONS.contains(&name.as_str());
    if !in_actions {
        if action_root {
            check_generated(node, file);
        } else if name == "condition" {
            check_condition(node, file);
        }
    }
    scan_attributes(node, &name, file);
    scan_body(node, &name, file);
    for child in node.children() {
        scan_node(&child, in_actions || action_root || name == "condition", file);
    }
}

/// The elements of each `widgets`, `fail-widgets` and `widget-template` that the html macros do not name, walked as the
/// generator walks them: a subtree a macro reads for itself (`opaque`) is not looked into, and its own `widgets` are
/// found on their own.
fn scan_widgets(node: &MNode, names: &HashSet<String>, opaque: &HashSet<String>, file: &mut FileReport) {
    fn walk(node: &MNode, names: &HashSet<String>, opaque: &HashSet<String>, file: &mut FileReport) {
        for child in node.children() {
            let name = child.name();
            // form assembly replaces it with the template's widgets before anything is generated
            if name == "widget-template-include" {
                continue;
            }
            if !names.contains(&name) {
                file.unknown_elements.push((child.line(), name));
            } else if !opaque.contains(&name) {
                walk(&child, names, opaque, file);
            }
        }
    }
    if matches!(node.name().as_str(), "widgets" | "fail-widgets" | "widget-template") {
        walk(node, names, opaque, file);
    }
    for child in node.children() {
        scan_widgets(&child, names, opaque, file);
    }
}

fn check_generated(node: &MNode, file: &mut FileReport) {
    if let Err(err) = generated_ok(node) {
        let mut worst = (node.clone(), err.to_string());
        narrow(node, &mut worst);
        file.generated_errors
            .push((worst.0.line(), format!("{} {}", worst.0.name(), worst.1)));
    }
}

/// The deepest descendant that fails by itself, so the report names an element and not a whole block.
fn narrow(node: &MNode, worst: &mut (MNode, String)) {
    for child in node.children() {
        if let Err(err) = generated_ok(&child) {
            *worst = (child.clone(), err.to_string());
            narrow(&child, worst);
            return;
        }
    }
}

fn generated_ok(node: &MNode) -> Result<(), Error> {
    let name = node.name();
    let copy = node.deep_copy();
    let xml = if ACTION_CONTAINERS.contains(&name.as_str()) {
        copy.set_name("actions");
        copy.write_string()
    } else {
        // The form widgets run an `entity-find` with no `list`. The generator wants a name.
        if name == "entity-find" && copy.attribute("list").is_none() {
            copy.set_attribute("list", "_formList");
        }
        format!("<actions>{}</actions>", copy.write_string())
    };
    super::check_actions(&xml)
}

fn check_condition(node: &MNode, file: &mut FileReport) {
    let xml = format!("<actions><if>{}</if></actions>", node.deep_copy().write_string());
    if let Err(err) = super::check_actions(&xml) {
        file.generated_errors.push((node.line(), format!("condition {err}")));
    }
}

pub(crate) fn is_expression_attr(element: &str, attr: &str) -> bool {
    match attr {
        "condition" | "from" | "in-map" | "parameter-map" | "text-map" => true,
        "list" => matches!(
            element,
            "section-iterate"
                | "iterate"
                | "form-list"
                | "tree-sub-node"
                | "list-options"
                | "order-map-list"
                | "filter-map-list"
        ),
        "map" => element == "form-single",
        _ => false,
    }
}

fn scan_attributes(node: &MNode, element: &str, file: &mut FileReport) {
    let line = node.line();
    for (attr, value) in node.attributes() {
        if value.trim().is_empty() || attr.contains(':') {
            continue;
        }
        if is_expression_attr(element, &attr) {
            file.sites
                .push(site(line, element, &attr, SiteKind::Expression, &value, true));
            continue;
        }
        let text = value.as_str();
        for embedded in embedded_expressions(text) {
            file.sites
                .push(site(line, element, &attr, SiteKind::Template, &embedded, false));
        }
        if let Some(at) = dollar_name(text) {
            let mut found = site(line, element, &attr, SiteKind::Template, text, false);
            // The text around a `$name` is not an expression, so it is not parsed: the idiom is the finding.
            found.error = None;
            found
                .idioms
                .push((Idiom::GStringDollarName, text[at..].chars().take(24).collect()));
            file.sites.push(found);
        }
    }
}

fn scan_body(node: &MNode, element: &str, file: &mut FileReport) {
    let line = node.line();
    if element == "text" {
        scan_text(node, line, file);
        return;
    }
    let Some(body) = node.text().filter(|text| !text.trim().is_empty()) else {
        return;
    };
    match element {
        "script" | "expression" => file
            .sites
            .push(site(line, element, "text", SiteKind::Script, &body, true)),
        "message" | "log" => {
            for embedded in embedded_expressions(&body) {
                file.sites
                    .push(site(line, element, "text", SiteKind::Template, &embedded, false));
            }
        }
        _ => {}
    }
}

/// Text types of render modes that were dropped. Their bodies are never rendered.
const DROPPED_TEXT_TYPES: &[&str] = &["vuet", "vue", "js"];

/// A `<text>` body with `template` not `false` is a fancy template and is compiled here. A `.ftl`
/// location is listed.
fn scan_text(node: &MNode, line: u32, file: &mut FileReport) {
    let kinds = node.attribute("type").unwrap_or_default();
    let dropped = !kinds.is_empty() && kinds.split(',').all(|kind| DROPPED_TEXT_TYPES.contains(&kind.trim()));
    if let Some(location) = node.attribute("location").filter(|location| location.contains(".ftl")) {
        file.ftl.push(FtlRef {
            line,
            element: "text".to_string(),
            detail: format!("location {location}"),
        });
    }
    let Some(body) = node.text().filter(|text| !text.trim().is_empty()) else {
        return;
    };
    if node.attribute("template").as_deref() == Some("false") || dropped {
        return;
    }
    match super::template::check_template(&body) {
        Ok(()) => file.dialect_bodies += 1,
        Err(err) => file
            .template_errors
            .push((line, format!("text body: {}", err.message()))),
    }
}

fn site(line: u32, element: &str, attr: &str, kind: SiteKind, text: &str, whole: bool) -> Site {
    let error = super::parse_js(text).err().map(|err| err.to_string());
    let mut idioms = classify(text);
    if whole && kind == SiteKind::Expression {
        if let Some(sample) = truthiness_candidate(attr, text) {
            idioms.push((Idiom::EmptyCollectionTruthiness, sample));
        }
    }
    Site {
        line,
        element: element.to_string(),
        attribute: attr.to_string(),
        kind,
        text: text.to_string(),
        error,
        idioms,
    }
}

/// The text of each `${...}` in `text`, with nested braces and strings respected.
pub(crate) fn embedded_expressions(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'$' && bytes[index + 1] == b'{' && (index == 0 || bytes[index - 1] != b'\\') {
            let start = index + 2;
            let mut depth = 1;
            let mut quote = 0u8;
            let mut end = start;
            while end < bytes.len() {
                let byte = bytes[end];
                if quote != 0 {
                    if byte == b'\\' {
                        end += 1;
                    } else if byte == quote {
                        quote = 0;
                    }
                } else if byte == b'\'' || byte == b'"' {
                    quote = byte;
                } else if byte == b'{' {
                    depth += 1;
                } else if byte == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                end += 1;
            }
            if end > bytes.len() {
                end = bytes.len();
            }
            out.push(text[start..end].to_string());
            index = end;
        }
        index += 1;
    }
    out
}

/// Position of a `$name` interpolation, which Groovy expands and a JS template literal does not.
fn dollar_name(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    (0..bytes.len().saturating_sub(1)).find(|&index| {
        bytes[index] == b'$'
            && (bytes[index + 1].is_ascii_alphabetic() || bytes[index + 1] == b'_')
            && (index == 0 || bytes[index - 1] != b'\\')
    })
}

/// A bare condition on a name that reads like a collection. Groovy tests it for empty. JavaScript does not.
fn truthiness_candidate(attr: &str, text: &str) -> Option<String> {
    if attr != "condition" {
        return None;
    }
    const SUFFIXES: &[&str] = &[
        "List", "Map", "Set", "Rows", "Values", "Options", "Ids", "Items", "Entries",
    ];
    let masked = mask_strings(text);
    if masked.contains(".length") || masked.contains("Object.keys") {
        return None;
    }
    for operand in masked.split(['&', '|']) {
        let operand = operand.trim().trim_start_matches('!').trim();
        let operand = operand.trim_start_matches('(').trim_end_matches(')').trim();
        if operand.is_empty()
            || !operand
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            continue;
        }
        let last = operand.rsplit('.').next().unwrap_or(operand);
        if SUFFIXES.iter().any(|suffix| last.ends_with(suffix)) || last.ends_with("list") || last == "list" {
            return Some(operand.to_string());
        }
    }
    None
}

/// Replace the characters inside string literals, template literals and comments with spaces, keeping the quotes and
/// newlines. The code inside a template literal's `${..}` is kept. A space stands for each byte, so offsets in the
/// result are offsets in `text`.
pub(crate) fn mask_strings(text: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Code,
        Quote(char),
        Template,
        Line,
        Block,
    }
    fn blank(out: &mut String, ch: char) {
        for _ in 0..ch.len_utf8() {
            out.push(' ');
        }
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut mode = Mode::Code;
    let mut escaped = false;
    // Brace depth inside each open `${`; the last entry is the innermost one.
    let mut braces: Vec<usize> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        match mode {
            Mode::Quote(open) => {
                if escaped {
                    escaped = false;
                    blank(&mut out, ch);
                } else if ch == '\\' {
                    escaped = true;
                    blank(&mut out, ch);
                } else if ch == open {
                    mode = Mode::Code;
                    out.push(ch);
                } else {
                    blank(&mut out, ch);
                }
            }
            Mode::Template => {
                if escaped {
                    escaped = false;
                    blank(&mut out, ch);
                } else if ch == '\\' {
                    escaped = true;
                    blank(&mut out, ch);
                } else if ch == '`' {
                    mode = Mode::Code;
                    out.push(ch);
                } else if ch == '$' && next == Some('{') {
                    out.push_str("${");
                    braces.push(0);
                    mode = Mode::Code;
                    index += 1;
                } else {
                    blank(&mut out, ch);
                }
            }
            Mode::Line => {
                if ch == '\n' {
                    mode = Mode::Code;
                    out.push(ch);
                } else {
                    blank(&mut out, ch);
                }
            }
            Mode::Block => {
                if ch == '*' && next == Some('/') {
                    mode = Mode::Code;
                    out.push_str("  ");
                    index += 1;
                } else if ch == '\n' {
                    out.push(ch);
                } else {
                    blank(&mut out, ch);
                }
            }
            Mode::Code => match ch {
                '\'' | '"' => {
                    mode = Mode::Quote(ch);
                    out.push(ch);
                }
                '`' => {
                    mode = Mode::Template;
                    out.push(ch);
                }
                '/' if next == Some('/') => {
                    mode = Mode::Line;
                    out.push_str("  ");
                    index += 1;
                }
                '/' if next == Some('*') => {
                    mode = Mode::Block;
                    out.push_str("  ");
                    index += 1;
                }
                '{' => {
                    if let Some(depth) = braces.last_mut() {
                        *depth += 1;
                    }
                    out.push(ch);
                }
                '}' => {
                    match braces.last_mut() {
                        Some(0) => {
                            braces.pop();
                            mode = Mode::Template;
                        }
                        Some(depth) => *depth -= 1,
                        None => {}
                    }
                    out.push(ch);
                }
                _ => out.push(ch),
            },
        }
        index += 1;
    }
    out
}

fn has_gstring(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        if quote == b'"' || quote == b'\'' || quote == b'`' {
            let mut end = index + 1;
            while end < bytes.len() && bytes[end] != quote {
                if bytes[end] == b'\\' {
                    end += 1;
                }
                end += 1;
            }
            let end = end.min(bytes.len());
            if quote == b'"' {
                let inner = &text[index + 1..end];
                if inner.contains("${") || dollar_name(inner).is_some() {
                    return true;
                }
            }
            index = end;
        }
        index += 1;
    }
    false
}

fn word_at(bytes: &[u8], at: usize, len: usize) -> bool {
    let before = at == 0 || !is_ident(bytes[at - 1]);
    let after = at + len >= bytes.len() || !is_ident(bytes[at + len]);
    before && after
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn find_words(masked: &str, word: &str) -> Vec<usize> {
    let bytes = masked.as_bytes();
    masked
        .match_indices(word)
        .map(|(at, _)| at)
        .filter(|&at| word_at(bytes, at, word.len()))
        .collect()
}

pub(crate) fn snippet(text: &str, at: usize) -> String {
    let start = text.floor_char_boundary(at.saturating_sub(8));
    let end = text.ceil_char_boundary((at + 24).min(text.len()));
    text[start..end].to_string()
}

/// The Groovy idioms in one expression or script, each with a short sample.
pub fn classify(text: &str) -> Vec<(Idiom, String)> {
    let masked = mask_strings(text);
    let m = masked.as_str();
    let bytes = m.as_bytes();
    let mut out: Vec<(Idiom, String)> = Vec::new();

    if let Some(at) = m.find("?:") {
        out.push(sample(text, Idiom::Elvis, at));
    }
    if let Some(at) = m.find("?.") {
        out.push(sample(text, Idiom::SafeNavigation, at));
    }
    if let Some(at) = range_at(bytes) {
        out.push(sample(text, Idiom::Range, at));
    }
    if super::parse::has_closure_call(m) || super::parse::has_groovy_arrow(m) {
        out.push(sample(
            text,
            Idiom::Closure,
            m.find("->").or_else(|| m.find('{')).unwrap_or(0),
        ));
    }
    if has_gstring(text) {
        out.push(sample(text, Idiom::GString, text.find('"').unwrap_or(0)));
    }
    if let Some(at) = m.find("=~").or_else(|| m.find("~/")) {
        out.push(sample(text, Idiom::RegexOperator, at));
    }
    if let Some(at) = decimal_literal(bytes, true) {
        out.push(sample(text, Idiom::DecimalLiteral, at));
    }
    if let Some(at) = decimal_literal(bytes, false) {
        out.push(sample(text, Idiom::DecimalPointLiteral, at));
    }
    for word in ["BigDecimal", "toBigDecimal", "BigInteger"] {
        if let Some(&at) = find_words(m, word).first() {
            out.push(sample(text, Idiom::BigDecimal, at));
            break;
        }
    }
    if let Some(&at) = find_words(m, "setScale").first() {
        out.push(sample(text, Idiom::SetScale, at));
    }
    for at in find_words(m, "new") {
        let rest = m[at + 3..].trim_start();
        if rest.chars().next().is_some_and(char::is_uppercase) {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            let idiom = if JS_GLOBALS.contains(&name.as_str()) {
                Idiom::NewJsType
            } else {
                Idiom::NewType
            };
            out.push((idiom, name));
        }
    }
    if let Some(at) = context_plus(m) {
        out.push(sample(text, Idiom::ContextPlus, at));
    }
    if let Some(at) = map_literal(bytes) {
        out.push(sample(text, Idiom::MapLiteral, at));
    }
    if let Some(at) = m.find("*.") {
        out.push(sample(text, Idiom::ListStar, at));
    }
    for at in find_words(m, "as") {
        if m[at + 2..].trim_start().chars().next().is_some_and(char::is_alphabetic) && at > 0 {
            out.push(sample(text, Idiom::AsCast, at));
            break;
        }
    }
    for at in find_words(m, "in") {
        if m[at + 2..].trim_start().starts_with('[') {
            out.push(sample(text, Idiom::InOperator, at));
            break;
        }
    }
    if let Some(&at) = find_words(m, "def").first() {
        out.push(sample(text, Idiom::Def, at));
    }
    if let Some(&at) = find_words(m, "catch").first() {
        let idiom = if typed_catch(&m[at + "catch".len()..]) {
            Idiom::Catch
        } else {
            Idiom::CatchBare
        };
        out.push(sample(text, idiom, at));
    }
    for (class, at) in static_calls(m) {
        if JS_GLOBALS.contains(&class.as_str()) {
            continue;
        }
        let idiom = if HOST_STATICS.contains(&class.as_str()) {
            Idiom::HostStatic
        } else {
            Idiom::StaticClass
        };
        out.push((idiom, format!("{class} {}", snippet(text, at))));
    }
    if let Some(at) = qualified_class(m) {
        out.push(sample(text, Idiom::StaticClass, at));
    }
    if m.contains(".'") || m.contains(".\"") {
        out.push(sample(
            text,
            Idiom::QuotedProperty,
            m.find(".'").or_else(|| m.find(".\"")).unwrap_or(0),
        ));
    }
    for method in COLLECTION_METHODS {
        if let Some(at) = m.find(method) {
            out.push(sample(text, Idiom::CollectionMethod, at));
            break;
        }
    }
    for method in ADD_METHODS {
        if let Some(at) = m.find(method) {
            out.push(sample(text, Idiom::AddCall, at));
            break;
        }
    }
    out
}

/// `catch (Exception e)` or `catch (A | B e)`: the text after `catch` opens with a typed parameter.
fn typed_catch(rest: &str) -> bool {
    let Some(inner) = rest.trim_start().strip_prefix('(') else {
        return false;
    };
    let inner = inner.split(')').next().unwrap_or("");
    inner.split_whitespace().count() > 1 || inner.contains('|')
}

fn sample(text: &str, idiom: Idiom, at: usize) -> (Idiom, String) {
    (idiom, snippet(text, at))
}

fn range_at(bytes: &[u8]) -> Option<usize> {
    (0..bytes.len().saturating_sub(1)).find(|&index| {
        bytes[index] == b'.'
            && bytes[index + 1] == b'.'
            && (index == 0 || bytes[index - 1] != b'.')
            && (index + 2 >= bytes.len() || bytes[index + 2] != b'.')
    })
}

/// A `1.5` (`suffix` false) or a `10g` / `10G` (`suffix` true) number.
fn decimal_literal(bytes: &[u8], suffix: bool) -> Option<usize> {
    for index in 0..bytes.len() {
        if !bytes[index].is_ascii_digit() || (index > 0 && (is_ident(bytes[index - 1]) || bytes[index - 1] == b'.')) {
            continue;
        }
        let mut end = index;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if !suffix && end + 1 < bytes.len() && bytes[end] == b'.' && bytes[end + 1].is_ascii_digit() {
            return Some(index);
        }
        if suffix
            && end < bytes.len()
            && matches!(bytes[end], b'g' | b'G')
            && (end + 1 >= bytes.len() || !is_ident(bytes[end + 1]))
        {
            return Some(index);
        }
    }
    None
}

fn map_literal(bytes: &[u8]) -> Option<usize> {
    for index in 0..bytes.len() {
        if bytes[index] != b'[' {
            continue;
        }
        let mut depth = 0;
        let mut question = false;
        for (offset, &byte) in bytes[index + 1..].iter().enumerate() {
            match byte {
                b'[' | b'(' | b'{' => depth += 1,
                b')' | b'}' => depth -= 1,
                b']' if depth == 0 => break,
                b']' => depth -= 1,
                b',' if depth == 0 => break,
                b'?' if depth == 0 => question = true,
                b':' if depth == 0 => {
                    let next = bytes.get(index + 1 + offset + 1).copied();
                    if !question && next != Some(b':') {
                        return Some(index);
                    }
                    break;
                }
                _ => {}
            }
            if depth < 0 {
                break;
            }
        }
    }
    None
}

/// Groovy's `context + [name: value]`, a copy of the context with entries added.
fn context_plus(masked: &str) -> Option<usize> {
    find_words(masked, "context").into_iter().find(|&at| {
        let rest = masked[at + "context".len()..].trim_start();
        rest.strip_prefix('+')
            .is_some_and(|rest| rest.trim_start().starts_with('['))
    })
}

/// A Java class written with its package, such as `org.moqui.resource.ResourceReference.isTextFilename(..)`: the script
/// has no `org` or `java` name, so it fails at run time. Only the class name is a host global where there is one.
fn qualified_class(masked: &str) -> Option<usize> {
    const PACKAGES: &[&str] = &[
        "org.moqui.",
        "org.apache.",
        "java.lang.",
        "java.util.",
        "java.math.",
        "java.sql.",
        "java.io.",
        "java.text.",
        "java.time.",
        "groovy.",
    ];
    let bytes = masked.as_bytes();
    for package in PACKAGES {
        let mut from = 0;
        while let Some(found) = masked[from..].find(package) {
            let at = from + found;
            from = at + package.len();
            if at > 0 && (is_ident(bytes[at - 1]) || bytes[at - 1] == b'.') {
                continue;
            }
            // a capitalized class name follows the lower case package path
            let rest = &masked[at..];
            let path_end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            let has_class = rest[..path_end]
                .split('.')
                .any(|part| part.chars().next().is_some_and(|c| c.is_ascii_uppercase()));
            if has_class {
                return Some(at);
            }
        }
    }
    None
}

fn static_calls(masked: &str) -> Vec<(String, usize)> {
    let bytes = masked.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_uppercase()
            && (index == 0 || (!is_ident(bytes[index - 1]) && bytes[index - 1] != b'.'))
        {
            let mut end = index;
            while end < bytes.len() && is_ident(bytes[end]) {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'.' {
                let call_start = end + 1;
                let mut call_end = call_start;
                while call_end < bytes.len() && is_ident(bytes[call_end]) {
                    call_end += 1;
                }
                if call_end > call_start && bytes[call_start].is_ascii_lowercase() {
                    out.push((masked[index..end].to_string(), index));
                }
            }
            index = end.max(index + 1);
        } else {
            index += 1;
        }
    }
    out
}

fn one_line(text: &str) -> String {
    let joined: Vec<&str> = text.split_whitespace().collect();
    let line = joined.join(" ");
    if line.chars().count() > 300 {
        let mut clipped: String = line.chars().take(300).collect();
        clipped.push_str("...");
        clipped
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<&'static str> {
        classify(text).into_iter().map(|(idiom, _)| idiom.name()).collect()
    }

    #[test]
    fn context_plus_is_its_own_idiom() {
        assert!(names("context + [a: 1]").contains(&"context-plus"));
        assert!(names("context+[a:b]").contains(&"context-plus"));
        assert!(!names("mycontext + [a: 1]").contains(&"context-plus"));
        assert!(!names("Object.assign({}, context, {a: 1})").contains(&"context-plus"));
    }

    #[test]
    fn elvis_and_safe_navigation_are_told_apart() {
        assert_eq!(names("a ?: 'x'"), vec!["elvis"]);
        assert_eq!(names("a?.b"), vec!["safe-navigation"]);
        assert!(names("a ? b : c").is_empty());
    }

    #[test]
    fn comments_and_template_text_do_not_trigger_idioms_but_template_code_does() {
        assert!(names("// Java's list.size() as BigDecimal 1.5 ..\nx").is_empty());
        assert!(names("/* x as String; a..b */ y").is_empty());
        assert!(names("`as many as ${n}`").is_empty());
        assert!(names("`n: ${list.size()}`").contains(&"collection-method"));
        assert!(names("f('it\\'s') // don't\nlist.size()").contains(&"collection-method"));
        assert_eq!(mask_strings("a /* é */ b").len(), "a /* é */ b".len());
    }

    #[test]
    fn valid_javascript_idioms_are_not_flagged_by_default() {
        let site = |text: &str| Site {
            line: 1,
            element: "script".into(),
            attribute: "text".into(),
            kind: SiteKind::Script,
            text: text.into(),
            error: None,
            idioms: classify(text),
        };
        assert!(!site("const s = new Set(); s.add(1); a?.b; try { f() } catch (e) {}").flagged(false));
        assert!(site("const s = new Set(); s.add(1)").flagged(true));
        assert!(site("list.size()").flagged(false));
        assert_eq!(Idiom::DecimalOperator.severity(), Severity::Hazard);
    }

    #[test]
    fn strings_do_not_trigger_idioms() {
        assert!(names("'a ?: b => [x:1] new Foo()'").is_empty());
    }

    #[test]
    fn ranges_closures_and_maps() {
        assert_eq!(names("list[1..size-1]"), vec!["range"]);
        assert!(names("[...a, ...b]").is_empty());
        assert!(names("items.each { it }").contains(&"closure"));
        assert!(names("items.map(x => x.id)").is_empty());
        assert_eq!(names("[a:1, b:2]"), vec!["map-literal"]);
        assert!(names("rows[ok ? 1 : 2]").is_empty());
        assert!(names("rows[0]").is_empty());
    }

    #[test]
    fn gstrings_regex_decimals_and_types() {
        assert!(names("\"Hi ${name}\"").contains(&"gstring"));
        assert!(names("'Hi ${name}'").is_empty());
        assert_eq!(names("a =~ /x/"), vec!["regex-operator"]);
        assert!(names("price * 10g").contains(&"decimal-literal"));
        assert!(names("price * 1.5").contains(&"decimal-point-literal"));
        assert!(names("x.setScale(2)").contains(&"set-scale"));
        assert!(names("new BigDecimal(a)").contains(&"bigdecimal"));
        let new_type = classify("new TreeSet(a)");
        assert_eq!(new_type, vec![(Idiom::NewType, "TreeSet".to_string())]);
        assert_eq!(names("new Date(a)"), vec!["new-js-type"]);
        assert!(names("StringUtilities.camelCaseToPretty(x)").contains(&"static-class"));
        assert_eq!(names("ResourceReference.isTextFilename(x)"), vec!["host-static"]);
        assert!(names("Math.max(1, 2)").is_empty());
        // with the package it is a Java name the script does not have; a service name in a string is not
        assert!(names("org.moqui.resource.ResourceReference.isTextFilename(x)").contains(&"static-class"));
        assert!(names("new java.util.Date()").contains(&"static-class"));
        assert!(
            !names("ec.service.sync().name('org.moqui.impl.UserServices.update#UserAccount')")
                .contains(&"static-class")
        );
    }

    #[test]
    fn in_operator_collection_methods_and_casts() {
        assert!(names("x in ['A', 'B']").contains(&"in-operator"));
        assert!(names("'k' in obj").is_empty());
        assert!(names("list.size() > 0").contains(&"collection-method"));
        assert_eq!(names("total.add(x)"), vec!["add-call"]);
        assert_eq!(names("try { a() } catch (e) { b() }"), vec!["catch-bare"]);
        assert_eq!(names("try { a() } catch (Exception e) { b() }"), vec!["catch"]);
        assert!(names("x as String").contains(&"as-cast"));
        assert!(names("def x = 1").contains(&"def"));
    }

    #[test]
    fn embedded_expressions_respect_braces_and_strings() {
        assert_eq!(embedded_expressions("a ${b} c ${d.e('}')}"), vec!["b", "d.e('}')"]);
        assert!(embedded_expressions("\\${literal}").is_empty());
        assert_eq!(dollar_name("cost $price"), Some(5));
        assert_eq!(dollar_name("cost $5 ${x}"), None);
    }

    #[test]
    fn truthiness_candidates_are_bare_collection_names() {
        assert!(truthiness_candidate("condition", "itemList").is_some());
        assert!(truthiness_candidate("condition", "!itemList && x == 1").is_some());
        assert!(truthiness_candidate("condition", "itemList.length > 0").is_none());
        assert!(truthiness_candidate("from", "itemList").is_none());
    }

    fn workspace(path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
    }

    fn write_screen(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moqui-screen-lint-{}-{name}", std::process::id()));
        std::fs::create_dir_all(dir.join("screen")).unwrap();
        let path = dir.join("screen").join("Test.xml");
        std::fs::write(&path, format!("<screen>{body}</screen>")).unwrap();
        dir
    }

    #[test]
    fn screen_trees_have_no_parse_failures() {
        let roots = [workspace("runtime/base-component")];
        let report = lint_screens(&roots).unwrap();
        assert!(report.files.len() > 100, "{} files", report.files.len());
        assert!(report.site_count() > 1000, "{} sites", report.site_count());
        assert_eq!(report.failures(), Vec::<String>::new());
        assert_eq!(report.excluded.len(), KNOWN_EXCLUSIONS.len());
        // The widget templates are scanned, and every template body in the trees compiles in the dialect.
        assert!(
            report.files.iter().any(
                |file| file.path.ends_with("webroot/template/screen/BasicWidgetTemplates.xml") && file.sites.len() > 30
            ),
            "BasicWidgetTemplates.xml was not scanned"
        );
        let bodies: usize = report.files.iter().map(|file| file.dialect_bodies).sum();
        assert!(bodies >= 30, "{bodies} template bodies");
    }

    /// The page shell, the login pages and the menu templates are files in the dialect. Each one compiles.
    #[test]
    fn every_fancy_file_compiles() {
        fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, out);
                } else if path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".fancy"))
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        collect(&workspace("runtime"), &mut files);
        assert!(files.len() >= 7, "{files:?}");
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            crate::script::template::check_template(&source)
                .unwrap_or_else(|err| panic!("{}: {}", file.display(), err.message()));
        }
    }

    /// A render logs an element no macro names and writes a placeholder; the lint fails on it, as it is most often a
    /// typo. A subtree a macro reads for itself and a widget-template-include are not looked into.
    #[test]
    fn an_element_no_screen_macro_names_is_a_failure() {
        let dir = write_screen(
            "unknown-element",
            r#"<widgets><container><lable text="typo"/></container>
            <form-single name="F"><field name="a"><default-field><display/></default-field></field>
            <field-layout><field-ref name="a"/></field-layout></form-single>
            <section name="S"><widgets><no-such-widget/></widgets></section></widgets>"#,
        );
        let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
        let failures = report.failures();
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(
            failures[0].contains("lable: no screen macro names this element"),
            "{failures:?}"
        );
        assert!(failures[1].contains("no-such-widget"), "{failures:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn freemarker_left_in_a_text_body_is_a_failure() {
        let dir = write_screen(
            "freemarker",
            r#"<widgets><render-mode>
            <text type="html"><![CDATA[<#if x?has_content>${x!""}</#if>]]></text>
            <text type="html"><![CDATA[<#assign y = 1><@macro/>]]></text>
            <text type="vuet"><![CDATA[<#visit node>]]></text>
            <text type="html" template="false"><![CDATA[<#visit node>]]></text>
            <text type="html"><![CDATA[<#if x>${x}<#else>none</#if>]]></text>
            </render-mode></widgets>"#,
        );
        let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
        let failures = report.failures();
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(
            failures[0].contains("text body") && failures[0].contains("not JavaScript"),
            "{failures:?}"
        );
        assert!(failures[1].contains("macro calls"), "{failures:?}");
        // The dropped vuet mode and the template="false" text are not compiled; the last one is.
        assert_eq!(report.files[0].dialect_bodies, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn groovy_in_a_screen_is_a_failure() {
        let dir = write_screen(
            "groovy",
            r#"<widgets><label text="${a ?: 'x'}"/><link url="." parameter-map="[a:b]"/>
            <if condition="items"><label text="ok"/></if></widgets>
            <actions><script>def x = 1</script></actions>"#,
        );
        let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
        let failures = report.failures();
        assert!(failures.iter().any(|line| line.contains("label@text")), "{failures:?}");
        assert!(
            failures.iter().any(|line| line.contains("link@parameter-map")),
            "{failures:?}"
        );
        assert!(failures.iter().any(|line| line.contains("script@text")), "{failures:?}");
        let totals = report.idiom_totals();
        assert!(totals.contains_key(&Idiom::Elvis));
        assert!(totals.contains_key(&Idiom::MapLiteral));
        assert!(totals.contains_key(&Idiom::Def));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn browser_only_parts_and_bare_expansions_in_tui_texts_are_listed() {
        let dir = write_screen(
            "tui",
            r#"<widgets>
            <render-mode><text type="qvt"><![CDATA[<m-chart/>]]></text></render-mode>
            <render-mode><text type="qvt">x</text><text type="tui"><![CDATA[{"t":"text","text":${JSON.stringify(a)}}]]></text></render-mode>
            <render-mode><text type="tui"><![CDATA[{"t":"text","text":"${a}"}]]></text></render-mode>
            </widgets>"#,
        );
        let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
        let file = &report.files[0];
        assert_eq!(file.tui_missing.len(), 1, "{:?}", file.tui_missing);
        assert!(file.tui_missing[0].1.contains("qvt"), "{:?}", file.tui_missing);
        assert_eq!(file.tui_unsafe.len(), 1, "{:?}", file.tui_unsafe);
        assert!(report
            .render()
            .contains("TUI worklist: 1 browser-only part(s) in 1 screen(s)"));
        // information, not a failure
        assert_eq!(report.failures(), Vec::<String>::new());
        std::fs::remove_dir_all(dir).unwrap();
        let dir = write_screen("qvue", "<widgets/>");
        std::fs::write(
            dir.join("screen").join("Test.xml"),
            r#"<screen render-modes="vue,qvue"><widgets/></screen>"#,
        )
        .unwrap();
        let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(report.files[0].tui_missing.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn javascript_in_a_screen_passes_and_template_bodies_compile() {
        let dir = write_screen(
            "javascript",
            r#"<widgets><label text="${a || 'x'}" condition="a &amp;&amp; b.length"/>
            <link url="." parameter-map="{a: b}"/>
            <render-mode><text type="html"><![CDATA[<#if x>${y}</#if>]]></text></render-mode></widgets>
            <actions><set field="n" from="parseInt(m, 10)"/><script>const x = [1, 2].map(v => v * 2)</script></actions>"#,
        );
        let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(report.failures(), Vec::<String>::new());
        assert_eq!(report.files[0].dialect_bodies, 1);
        assert!(report.files[0].ftl.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
