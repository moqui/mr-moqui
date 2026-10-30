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

//! The reference rules (plans/design-dev-aide.md 5.3, F8): which attribute names which kind of artifact, and how it
//! resolves in its context. One table drives the reference checks, `moqui refs`, and later the editor's value
//! completion, go to definition and the artifact index.

use moqui_lib::node::MNode;

/// What an attribute value names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// An entity or view entity by its full or short name.
    Entity,
    /// A service by name, or an entity-auto name (`create#moqui.basic.Enumeration`).
    Service,
    /// A field of the entity named by the nearest `entity-find`, `entity-find-one` or `entity-find-count` around it.
    FindField,
    /// An in-parameter of the service of the `service-call` around it.
    ServiceParameter,
    /// A resource location (`component://...`) that must exist.
    Location,
}

/// One rule: the attribute `attribute` of an element `element` names a `kind`.
#[derive(Clone, Copy, Debug)]
pub struct RefRule {
    pub element: &'static str,
    pub attribute: &'static str,
    pub kind: RefKind,
}

const fn rule(element: &'static str, attribute: &'static str, kind: RefKind) -> RefRule {
    RefRule {
        element,
        attribute,
        kind,
    }
}

/// The rules. `*` for the element matches any element.
pub const RULES: &[RefRule] = &[
    rule("entity-find", "entity-name", RefKind::Entity),
    rule("entity-find-one", "entity-name", RefKind::Entity),
    rule("entity-find-count", "entity-name", RefKind::Entity),
    rule("entity-make-value", "entity-name", RefKind::Entity),
    rule("entity-delete-by-condition", "entity-name", RefKind::Entity),
    rule("extend-entity", "entity-name", RefKind::Entity),
    rule("member-entity", "entity-name", RefKind::Entity),
    rule("relationship", "related", RefKind::Entity),
    rule("auto-fields-entity", "entity-name", RefKind::Entity),
    rule("eeca", "entity", RefKind::Entity),
    rule("service-call", "name", RefKind::Service),
    rule("seca", "service", RefKind::Service),
    rule("auto-fields-service", "service-name", RefKind::Service),
    rule("econdition", "field-name", RefKind::FindField),
    rule("select-field", "field-name", RefKind::FindField),
    rule("order-by", "field-name", RefKind::FindField),
    rule("field-map", "field-name", RefKind::ServiceParameter),
    rule("*", "location", RefKind::Location),
];

/// The rule for `attribute` on `element`, when there is one.
#[must_use]
pub fn rule_for(element: &str, attribute: &str) -> Option<&'static RefRule> {
    RULES
        .iter()
        .find(|rule| rule.attribute == attribute && (rule.element == element || rule.element == "*"))
}

/// Whether a value is a literal name that can be checked: not empty and with no `${}` expansion in it.
#[must_use]
pub fn is_literal(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains("${") && !value.contains('$')
}

/// The field names of a [`RefKind::FindField`] value: one name, or a comma list (`order-by`, `select-field`) whose
/// names may start with `-`, `+` or `^` and end with ` NULLS FIRST` and the like.
pub fn field_names(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').filter_map(|name| {
        let name = name.trim().trim_start_matches(['-', '+', '^']);
        let name = name.split_whitespace().next().unwrap_or("");
        (!name.is_empty()).then_some(name)
    })
}

/// The entity a [`RefKind::FindField`] attribute's field belongs to: the `entity-name` of the nearest find around it.
#[must_use]
pub fn find_entity_of(node: &MNode) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        let name = parent.name();
        if matches!(name.as_str(), "entity-find" | "entity-find-one" | "entity-find-count") {
            return parent.attribute("entity-name");
        }
        // A view entity's conditions name its aliases, not an entity's fields.
        if matches!(
            name.as_str(),
            "view-entity" | "entity" | "extend-entity" | "data-document"
        ) {
            return None;
        }
        current = parent.parent();
    }
    None
}

/// The service a [`RefKind::ServiceParameter`] attribute's parameter belongs to: the `name` of the `service-call` it is
/// directly in.
#[must_use]
pub fn service_of(node: &MNode) -> Option<String> {
    node.parent()
        .filter(|parent| parent.has_name("service-call"))
        .and_then(|parent| parent.attribute("name"))
}

/// One reference found in a file: an attribute that a [`RULES`] entry says names an artifact.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub path: String,
    pub line: u32,
    pub element: String,
    pub attribute: String,
    #[serde(skip)]
    pub kind: RefKind,
    pub value: String,
    /// The artifact the reference is in: `service:<name>`, `entity:<name>`, `seca:<service>`, `eeca:<entity>`, or
    /// `file:<path>` for a screen or any other file.
    pub from: String,
}

/// The references in the XML file at `path` (entity, service, ECA, REST and screen files alike). A file that does not
/// parse has none.
#[must_use]
pub fn scan_file(path: &std::path::Path) -> Vec<Reference> {
    let Ok(root) = MNode::parse_file(path) else {
        return Vec::new();
    };
    let display = path.display().to_string();
    let file_artifact = format!("file:{display}");
    let prefix = moqui_core::service::define::service_prefix(path);
    let mut found = Vec::new();
    let mut stack = vec![(root, file_artifact, String::new())];
    while let Some((node, from, parent)) = stack.pop() {
        let element = node.name();
        let from = match element.as_str() {
            "service" if parent == "services" => format!(
                "service:{prefix}.{}{}",
                node.attribute("verb").unwrap_or_default(),
                node.attribute("noun")
                    .map(|noun| format!("#{noun}"))
                    .unwrap_or_default()
            ),
            "entity" | "view-entity" | "extend-entity" => {
                format!("entity:{}", full_entity_name(&node).unwrap_or_default())
            }
            "seca" => format!("seca:{}", node.attribute("service").unwrap_or_default()),
            "eeca" => format!("eeca:{}", node.attribute("entity").unwrap_or_default()),
            _ => from,
        };
        node.for_each_attribute(|attribute, value| {
            if let Some(rule) = rule_for(&element, attribute) {
                // a location is a resource when it has a scheme; a rust service's location is a function path
                let resource = rule.kind != RefKind::Location || value.contains("://");
                if is_literal(value) && resource {
                    found.push(Reference {
                        path: display.clone(),
                        line: node.line(),
                        element: element.clone(),
                        attribute: attribute.to_string(),
                        kind: rule.kind,
                        value: value.to_string(),
                        from: from.clone(),
                    });
                }
            }
        });
        for child in node.child_iter() {
            stack.push((child, from.clone(), element.clone()));
        }
    }
    found.sort_by_key(|reference| reference.line);
    found
}

/// `package.entity-name` of an `entity`, `view-entity` or `extend-entity` element.
fn full_entity_name(node: &MNode) -> Option<String> {
    let name = node.attribute("entity-name")?;
    Some(match node.attribute("package").filter(|package| !package.is_empty()) {
        Some(package) if !name.contains('.') => format!("{package}.{name}"),
        _ => name,
    })
}

/// The XML files of the components' `entity`, `service`, `screen` and `screen-extend` directories, where references
/// are made.
#[must_use]
pub fn component_xml_files(factory: &moqui_core::context::ExecutionContextFactory) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for component in factory.components() {
        for dir in ["entity", "service", "screen", "screen-extend"] {
            collect(&component.location.join(dir), &mut files);
        }
    }
    files
}

fn collect(dir: &std::path::Path, into: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<std::path::PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "xml") {
            into.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_with_the_artifact_they_are_in() {
        let dir = std::env::temp_dir().join(format!("moqui-refs-scan-{}", std::process::id()));
        let path = dir.join("comp/service/test/Things.xml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"<services>
    <service verb="run" noun="Thing">
        <actions>
            <entity-find entity-name="moqui.basic.Enumeration" list="enums">
                <econdition field-name="enumTypeId" value="X"/></entity-find>
            <service-call name="test.Things.other#Thing" in-map="${dynamic}"/>
            <service-call name="${dynamic}"/>
        </actions>
    </service>
</services>"#,
        )
        .unwrap();
        let found = scan_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
        let summary: Vec<(u32, &str, &str, &str)> = found
            .iter()
            .map(|reference| {
                (
                    reference.line,
                    reference.attribute.as_str(),
                    reference.value.as_str(),
                    reference.from.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    4,
                    "entity-name",
                    "moqui.basic.Enumeration",
                    "service:test.Things.run#Thing"
                ),
                (5, "field-name", "enumTypeId", "service:test.Things.run#Thing"),
                (6, "name", "test.Things.other#Thing", "service:test.Things.run#Thing"),
            ]
        );
    }
}
