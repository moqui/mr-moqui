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

//! The checks of a Moqui XML file (plans/design-dev-aide.md 5.6, F8): the schema digest (an unknown element or
//! attribute, a missing required attribute, a value outside its enumeration), the reference rules (an unknown entity,
//! field, service, service parameter or location), and the expression lint (a JavaScript expression or script that
//! does not parse). Each problem names its rule and the section of the coding-agent skill that has it, so the Problems
//! panel, `moqui check` and an agent read the same words.
//!
//! The text may be an editor's unsaved buffer: everything but a service file's expression lint reads the text given.
//!
//! Schema problems are warnings: the XSDs trail some attributes and values the framework reads (a `condition` on a
//! `conditional-response`, `template="fancy"`), so they inform without failing `moqui check`. A reference that does
//! not resolve, an expression that does not parse and XML that does not parse are errors.

use std::path::Path;
use std::sync::Arc;

use moqui_lib::node::MNode;

use crate::refs::{self, RefKind};
use crate::schema::{self, Digest, ElementSpec};
use crate::{Problem, Severity};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};

/// The skill whose sections hold the rules the checks quote.
pub const SKILL_FILE: &str = "framework/skill/moqui-coding-agent.md";

/// Each rule, the skill section that explains it, and the sentence of that section the problems quote.
pub const RULES: &[(&str, &str, &str)] = &[
    (
        "xml.parse",
        "Checks",
        "A file that does not parse as XML keeps the definitions loaded before.",
    ),
    (
        "schema.unknown-element",
        "Checks",
        "An element the schema does not allow here is a typo or misplaced.",
    ),
    (
        "schema.unknown-attribute",
        "Checks",
        "An attribute the schema does not define for the element is ignored.",
    ),
    ("schema.missing-attribute", "Checks", "A required attribute is missing."),
    ("schema.value", "Checks", "The value is not one the attribute allows."),
    (
        "reference.entity",
        "References",
        "No entity or view entity has this name.",
    ),
    (
        "reference.service",
        "References",
        "No service has this name, and it is not an entity-auto name of an entity.",
    ),
    (
        "reference.field",
        "References",
        "The entity of the find around it has no field of this name.",
    ),
    (
        "reference.parameter",
        "References",
        "The service called has no in-parameter of this name.",
    ),
    ("reference.location", "References", "Nothing is at this location."),
    (
        "expression",
        "Expressions",
        "Expressions and scripts are JavaScript, and this one does not parse.",
    ),
    (
        "service.definition",
        "Checks",
        "The service does not register as written.",
    ),
    (
        "extension.base",
        "Extending",
        "A screen-extend file names no base screen at its path.",
    ),
    (
        "extension.anchor",
        "Extending",
        "The base screen has no element of this name to extend.",
    ),
    ("extension.entity", "Extending", "No entity has this name to extend."),
    (
        "extension.rule",
        "Extending",
        "No service or entity has this name for the rule to run on.",
    ),
];

fn problem(path: &str, line: u32, column: u32, severity: Severity, rule: &str, detail: &str) -> Problem {
    let (section, sentence) = RULES
        .iter()
        .find(|(name, _, _)| *name == rule)
        .map_or(("Checks", ""), |(_, section, sentence)| (*section, *sentence));
    let message = if sentence.is_empty() {
        detail.to_string()
    } else {
        format!("{detail}: {sentence}")
    };
    Problem {
        path: path.to_string(),
        line,
        column,
        severity,
        rule: rule.to_string(),
        message,
        skill: Some(format!("{SKILL_FILE}#{section}")),
    }
}

/// The problems of `text`, the content of the file at `path` (saved or not), on `factory`'s model, in the entered
/// context or a new one.
#[must_use]
pub fn check_text(factory: &Arc<ExecutionContextFactory>, path: &Path, text: &str) -> Vec<Problem> {
    let entered_here = ExecutionContext::with_current(|_| ())
        .is_none()
        .then(|| factory.new_context());
    let _entered = entered_here.as_ref().map(ExecutionContext::enter);
    let _authz = entered_here.as_ref().map(ExecutionContext::disable_authz);
    let display = path.display().to_string();
    let root = match MNode::parse_text(&display, text) {
        Ok(root) => root,
        Err(err) => {
            let (line, column) = parse_error_position(&err.to_string());
            return vec![problem(
                &display,
                line,
                column,
                Severity::Error,
                "xml.parse",
                &err.to_string(),
            )];
        }
    };
    let mut problems = Vec::new();
    let lines = LineIndex::new(text);
    if let Some(digest) = schema::digest(&xsd_dir(factory), &root.name()) {
        check_schema(&digest, &root, None, &display, &lines, &mut problems);
    }
    check_references(factory, &root, &display, &lines, &mut problems);
    check_expressions(path, text, &root, &display, &mut problems);
    problems.sort_by_key(|problem| (problem.line, problem.column));
    problems.dedup();
    problems
}

/// The framework's XSDs: the factory's framework directory, else the one this build knows (a test factory's framework
/// directory may hold none).
#[must_use]
pub fn xsd_dir(factory: &ExecutionContextFactory) -> std::path::PathBuf {
    let dir = factory.framework_dir().join("xsd");
    if dir.join("xml-screen-3.xsd").is_file() {
        dir
    } else {
        moqui_core::context::framework_dir().join("xsd")
    }
}

/// The problems of the file at `path` as it is on disk.
#[must_use]
pub fn check_file(factory: &Arc<ExecutionContextFactory>, path: &Path) -> Vec<Problem> {
    match std::fs::read_to_string(path) {
        Ok(text) => check_text(factory, path, &text),
        Err(err) => vec![Problem::file_error(
            &path.display().to_string(),
            "file.read",
            err.to_string(),
        )],
    }
}

fn check_schema(
    digest: &Digest,
    node: &MNode,
    parent: Option<&ElementSpec>,
    path: &str,
    lines: &LineIndex<'_>,
    problems: &mut Vec<Problem>,
) {
    let name = node.name();
    let spec = digest.element(&name);
    let Some(spec) = spec else {
        if parent.is_some_and(|parent| !parent.open_children) {
            problems.push(problem(
                path,
                node.line(),
                node.column(),
                Severity::Warning,
                "schema.unknown-element",
                &format!(
                    "<{name}> is not allowed in <{}>",
                    parent.map_or("", |parent| parent.name.as_str())
                ),
            ));
        }
        return;
    };
    if let Some(parent) = parent {
        if !parent.open_children && !parent.children.contains(&name) {
            problems.push(problem(
                path,
                node.line(),
                node.column(),
                Severity::Warning,
                "schema.unknown-element",
                &format!("<{name}> is not allowed in <{}>", parent.name),
            ));
        }
    }
    node.for_each_attribute(|attribute, value| {
        if attribute.starts_with("xmlns") || attribute.starts_with("xsi:") {
            return;
        }
        let (line, column) = lines.attribute(node.line(), node.column(), attribute);
        match spec.attribute(attribute) {
            None if !spec.open_attributes => problems.push(problem(
                path,
                line,
                column,
                Severity::Warning,
                "schema.unknown-attribute",
                &format!("<{name}> has no attribute {attribute}"),
            )),
            Some(declared)
                if !declared.values.is_empty()
                    && !declared.open_values
                    && refs::is_literal(value)
                    && !declared.values.iter().any(|allowed| allowed == value) =>
            {
                problems.push(problem(
                    path,
                    line,
                    column,
                    Severity::Warning,
                    "schema.value",
                    &format!(
                        "{attribute}=\"{value}\" on <{name}>; allowed: {}",
                        declared.values.join(", ")
                    ),
                ));
            }
            _ => {}
        }
    });
    for declared in spec.attributes.iter().filter(|declared| declared.required) {
        if !node.has_attribute(&declared.name) {
            problems.push(problem(
                path,
                node.line(),
                node.column(),
                Severity::Warning,
                "schema.missing-attribute",
                &format!("<{name}> needs {}", declared.name),
            ));
        }
    }
    for child in node.child_iter() {
        check_schema(digest, &child, Some(spec), path, lines, problems);
    }
}

fn check_references(
    factory: &Arc<ExecutionContextFactory>,
    root: &MNode,
    path: &str,
    lines: &LineIndex<'_>,
    problems: &mut Vec<Problem>,
) {
    let facade = factory.entity();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        stack.extend(node.child_iter());
        let element = node.name();
        node.for_each_attribute(|attribute, value| {
            let Some(rule) = refs::rule_for(&element, attribute) else {
                return;
            };
            if !refs::is_literal(value) {
                return;
            }
            let found = match rule.kind {
                RefKind::Entity => facade.as_ref().map_or(Ok(()), |facade| {
                    if moqui_core::entity::info::is_entity_defined(facade, value) {
                        Ok(())
                    } else {
                        Err(("reference.entity", format!("entity {value}")))
                    }
                }),
                RefKind::Service => {
                    if moqui_core::service::is_service_defined(value, facade.as_ref()) {
                        Ok(())
                    } else {
                        Err(("reference.service", format!("service {value}")))
                    }
                }
                RefKind::FindField => match (refs::find_entity_of(&node), facade.as_ref()) {
                    (Some(entity), Some(facade)) if refs::is_literal(&entity) => {
                        // `order-by` and `select-field` take a comma list, each name with an optional `-`, `+` or `^`.
                        let missing: Vec<&str> = refs::field_names(value)
                            .filter(|field| {
                                facade.with_model(|model| model.entity(&entity).is_ok_and(|def| !def.has_field(field)))
                            })
                            .collect();
                        if missing.is_empty() {
                            Ok(())
                        } else {
                            Err(("reference.field", format!("field {} of {entity}", missing.join(", "))))
                        }
                    }
                    _ => Ok(()),
                },
                RefKind::ServiceParameter => match refs::service_of(&node).filter(|name| refs::is_literal(name)) {
                    Some(service) => match moqui_core::service::define::find(&service) {
                        Some(def)
                            if def.implements.is_empty()
                                && !def.in_params.is_empty()
                                && !def.in_params.iter().any(|param| param.name == value) =>
                        {
                            Err(("reference.parameter", format!("in-parameter {value} of {service}")))
                        }
                        _ => Ok(()),
                    },
                    None => Ok(()),
                },
                RefKind::Location => {
                    if !value.contains("://") || value.starts_with("http") {
                        Ok(())
                    } else {
                        let location = value.split('#').next().unwrap_or(value);
                        match factory.resource().get(location) {
                            Ok(resource) if resource.exists() => Ok(()),
                            _ => Err(("reference.location", format!("location {value}"))),
                        }
                    }
                }
            };
            if let Err((rule, detail)) = found {
                let (line, column) = lines.attribute(node.line(), node.column(), attribute);
                problems.push(problem(path, line, column, Severity::Error, rule, &detail));
            }
        });
    }
}

fn check_expressions(path: &Path, text: &str, root: &MNode, display: &str, problems: &mut Vec<Problem>) {
    // Screens, forms and widget templates: the screen lint reads the text.
    if let Ok(Some(report)) = crate::screen_lint::lint_screen_text(path, text) {
        for site in &report.sites {
            if let Some(error) = &site.error {
                problems.push(problem(
                    display,
                    site.line,
                    0,
                    Severity::Error,
                    "expression",
                    &format!("{}@{}: {error}", site.element, site.attribute),
                ));
            }
        }
        for (line, error) in report.generated_errors.iter().chain(&report.template_errors) {
            problems.push(problem(display, *line, 0, Severity::Error, "expression", error));
        }
        return;
    }
    // A service file: the service check, which reads the file on disk, when the text is what is on disk.
    if root.has_name("services") && std::fs::read_to_string(path).is_ok_and(|disk| disk == text) {
        if let Ok(failures) = moqui_core::service::check(&[path.to_path_buf()]) {
            for failure in failures {
                let rule =
                    if failure.contains("parse") || failure.contains("JavaScript") || failure.contains("expression") {
                        "expression"
                    } else {
                        "service.definition"
                    };
                problems.push(problem(display, 0, 0, Severity::Error, rule, &failure));
            }
        }
    }
}

/// The extensions of every component (plans/design-dev-aide.md 5.6), from the artifact index read into memory: each
/// `screen-extend` file has a base screen at its path and each form, section, container or transition it names there,
/// each `extend-entity` an entity, and each ECA rule the service or entity it runs on.
#[must_use]
pub fn check_extensions(factory: &Arc<ExecutionContextFactory>) -> Vec<Problem> {
    let index = crate::index::MemoryIndex::build(factory);
    extension_problems(&index.extensions(&[]))
}

/// The problems of the extensions whose targets are gone.
#[must_use]
pub fn extension_problems(extensions: &[crate::index::Extension]) -> Vec<Problem> {
    extensions
        .iter()
        .filter(|extension| !extension.resolved)
        .map(|extension| {
            let (rule, detail) = match (extension.ref_type.as_str(), extension.kind.as_str()) {
                ("anchor", _) => (
                    "extension.anchor",
                    format!("\"{}\" extends {}", extension.target, extension.scope),
                ),
                (_, "screen-extend") => ("extension.base", format!("no screen at {}", extension.target)),
                (_, "extend-entity") => ("extension.entity", format!("extend-entity {}", extension.target)),
                (_, kind) => ("extension.rule", format!("{kind} on {}", extension.target)),
            };
            problem(&extension.path, extension.line, 0, Severity::Error, rule, &detail)
        })
        .collect()
}

/// Line and column of a parse error message that names them (`line 3, column 7`, or `:3:7`), else 0.
fn parse_error_position(message: &str) -> (u32, u32) {
    let number_after = |key: &str| -> Option<u32> {
        let at = message.find(key)? + key.len();
        message[at..]
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    };
    (number_after("line").unwrap_or(0), number_after("column").unwrap_or(0))
}

/// The lines of the text, to place a problem on the attribute it is about.
struct LineIndex<'a> {
    lines: Vec<&'a str>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            lines: text.split('\n').collect(),
        }
    }

    /// The 1-based line and column of `attribute=` in the start tag that begins at `line` and `column`, else the start
    /// tag's own position.
    fn attribute(&self, line: u32, column: u32, attribute: &str) -> (u32, u32) {
        let start_line = line.max(1) as usize - 1;
        let pattern = format!("{attribute}=");
        for (offset, text) in self.lines.iter().enumerate().skip(start_line).take(40) {
            let from = if offset == start_line {
                text.char_indices()
                    .nth(column.saturating_sub(1) as usize)
                    .map_or(0, |(at, _)| at)
            } else {
                0
            };
            let mut search = from;
            while let Some(found) = text[search..].find(&pattern) {
                let at = search + found;
                let before = text[..at].chars().last();
                if before.is_none_or(|ch| ch.is_whitespace()) {
                    let column = text[..at].chars().count() + 1;
                    return (
                        u32::try_from(offset + 1).unwrap_or(line),
                        u32::try_from(column).unwrap_or(column as u32),
                    );
                }
                search = at + pattern.len();
            }
            if text[from..].contains('>') && offset > start_line {
                break;
            }
        }
        (line, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_attribute_is_found_in_its_start_tag() {
        let text = "<a>\n  <entity-find entity-name=\"X\"\n      list=\"y\"/>\n</a>";
        let lines = LineIndex::new(text);
        assert_eq!(lines.attribute(2, 3, "entity-name"), (2, 16));
        assert_eq!(lines.attribute(2, 3, "list"), (3, 7));
        assert_eq!(lines.attribute(2, 3, "none"), (2, 3));
    }

    #[test]
    fn a_service_file_with_each_kind_of_problem() {
        let ctx = ExecutionContextFactory::test_context_with_components(
            "",
            moqui_core::service::ServicePools::idle(),
            moqui_core::context::framework_dir(),
            Vec::new(),
        );
        let facade = moqui_core::testing::open();
        ctx.factory().set_entity(facade);
        let _entered = ctx.enter();
        let text = r#"<services>
    <service verb="run" noun="Thing" bogus="1">
        <actions>
            <entity-find entity-name="moqui.basic.Enumeration" list="enums" cache="sometimes">
                <econdition field-name="noSuchField" value="X"/></entity-find>
            <entity-find-one entity-name="no.such.Entity" value-field="x"/>
            <service-call name="no.such.Service.run#Thing"/>
            <script location="component://no-such-component/script/x.js"/>
        </actions>
        <no-such-element/>
    </service>
</services>"#;
        let problems = check_text(ctx.factory(), Path::new("/tmp/service/test/Things.xml"), text);
        let rules: Vec<(&str, u32)> = problems
            .iter()
            .map(|problem| (problem.rule.as_str(), problem.line))
            .collect();
        for expected in [
            ("schema.unknown-attribute", 2),
            ("schema.value", 4),
            ("reference.field", 5),
            ("reference.entity", 6),
            ("reference.service", 7),
            ("reference.location", 8),
            ("schema.unknown-element", 10),
        ] {
            assert!(rules.contains(&expected), "{expected:?} not in {rules:?}");
        }
        let field = problems
            .iter()
            .find(|problem| problem.rule == "reference.field")
            .unwrap();
        assert_eq!(field.column, 29, "{field:?}");
        assert_eq!(
            field.skill.as_deref(),
            Some("framework/skill/moqui-coding-agent.md#References")
        );
        assert!(field.message.contains("noSuchField"), "{}", field.message);
        let broken = check_text(ctx.factory(), Path::new("/tmp/x.xml"), "<services>\n  <service>");
        assert_eq!(broken[0].rule, "xml.parse");
    }

    #[test]
    fn every_rule_quotes_a_sentence_of_its_skill_section() {
        let skill = moqui_core::context::framework_dir().join("skill/moqui-coding-agent.md");
        let text = std::fs::read_to_string(&skill).unwrap();
        for (rule, section, sentence) in RULES {
            let heading = format!("## {section}");
            let start = text
                .find(&heading)
                .unwrap_or_else(|| panic!("{rule}: no section {heading}"));
            let end = text[start + heading.len()..]
                .find("\n## ")
                .map_or(text.len(), |at| start + heading.len() + at);
            assert!(
                text[start..end].contains(sentence),
                "{rule}: section {section} does not say {sentence:?}"
            );
        }
    }
}
