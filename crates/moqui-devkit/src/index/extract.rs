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

//! One file's artifacts and references (plans/design-dev-aide.md 7.1), read from its text alone: what it defines,
//! with each definition's line, and each reference the rules of [`crate::refs`] find, with the artifact it is
//! made in. Pure, so the server's index, the model-only index of `moqui refs` and the tests share it.

use std::sync::LazyLock;

use moqui_lib::node::MNode;
use regex::Regex;

use crate::refs::{self, RefKind};
use crate::ArtifactKind;

/// One artifact a file defines.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRow {
    /// `entity:<full name>`, `field:<entity>.<field>`, `service:<name>`, `screen:<location>`,
    /// `transition:<screen location>#<name>:<method>` (default `any`), `form:<screen location>#<name>`,
    /// `anchor:<screen location>#<name>`,
    /// `screen-extend:<location>`, `extend-entity:<location>#<entity>`, `seca:<location>#<id or position>` (and
    /// `eeca`, `emeca`, `giteca`), `rest:<path>`, `rest:<path>:<method>` (lowercase HTTP method),
    /// `data:<location>`, `script:<location>`, `skill:<location>`, and
    /// `parameter:<owner id>@<in|out|path>.<name>` ([`parameter_id`]): a service's in or out parameter
    /// (`parameter:service:a.B.verb#Noun@in.aId`, a nested one by its dotted path, `@in.order.items`), a screen's
    /// (`parameter:screen:component://c/screen/A.xml@in.aId`), a transition's (`parameter:transition:<screen>#save:post@in.aId`,
    /// a `path-parameter` `@path.aId`), a REST method's route parameter (`parameter:rest:/a/{aId}:post@path.aId`).
    pub id: String,
    pub kind: String,
    pub name: String,
    pub short_name: String,
    pub component: String,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// What a reference names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToKind {
    Entity,
    Service,
    /// A field of the entity in `scope`.
    Field,
    /// A resource location: a screen, script or any file.
    Location,
    /// A screen by location, or by its path under a `screen` directory.
    Screen,
    /// A transition of the screen in `scope`.
    Transition,
    /// A named form, container, section or transition of the base screen in `scope`.
    Anchor,
    /// An in-parameter of the service in `scope` (a `field-map`'s `field-name`).
    Parameter,
}

impl ToKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Service => "service",
            Self::Field => "field",
            Self::Location => "location",
            Self::Screen => "screen",
            Self::Transition => "transition",
            Self::Anchor => "anchor",
            Self::Parameter => "parameter",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "entity" => Self::Entity,
            "service" => Self::Service,
            "field" => Self::Field,
            "location" => Self::Location,
            "screen" => Self::Screen,
            "transition" => Self::Transition,
            "anchor" => Self::Anchor,
            "parameter" => Self::Parameter,
            _ => return None,
        })
    }
}

/// One reference made in a file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefRow {
    pub from: String,
    pub to_kind: ToKind,
    pub to_name: String,
    /// What the name is relative to: a field's entity, a parameter's service, or an anchor's/transition's screen.
    pub to_scope: String,
    /// How it refers: the element that makes it (`entity-find`, `service-call`, `relationship`, `member-entity`,
    /// `subscreens-item`), `extend` for an extension's target, `anchor` for a named anchor of a base screen,
    /// `implements`, `link`, `script` for a string in a script, `data` for a data file's records.
    pub ref_type: String,
    pub line: u32,
    pub column: u32,
    /// The artifact it resolves to, once resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_id: Option<String>,
    pub resolved: bool,
}

/// What a file defines and refers to.
#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    pub artifacts: Vec<ArtifactRow>,
    pub refs: Vec<RefRow>,
}

/// The file being read: where it is and what kind of file it is.
#[derive(Clone, Debug)]
pub struct FileInfo<'a> {
    pub path: &'a str,
    /// `component://<component>/<path in it>`.
    pub location: &'a str,
    pub component: &'a str,
    pub kind: ArtifactKind,
}

/// The index kind of a file, or `None` when the index does not read files of its kind.
#[must_use]
pub fn file_kind(kind: ArtifactKind) -> Option<&'static str> {
    Some(match kind {
        ArtifactKind::Entity => "entity",
        ArtifactKind::Eeca => "eeca",
        ArtifactKind::Service => "service",
        ArtifactKind::Seca => "seca",
        ArtifactKind::Rest => "rest",
        ArtifactKind::Screen => "screen",
        ArtifactKind::ScreenExtend => "screen-extend",
        ArtifactKind::Data => "data",
        ArtifactKind::Script => "script",
        ArtifactKind::Skill => "skill",
        _ => return None,
    })
}

/// Read `text`, the content of the file `info` describes. A file that does not parse is an error naming why.
///
/// # Errors
///
/// Returns the parse error's text when an XML file does not parse.
pub fn extract(info: &FileInfo<'_>, text: &str) -> Result<FileIndex, String> {
    let mut out = Reader {
        info,
        index: FileIndex::default(),
    };
    match info.kind {
        ArtifactKind::Script => out.script_file(text),
        ArtifactKind::Skill => out.skill_file(text),
        ArtifactKind::Data if !info.path.ends_with(".xml") => {}
        _ => {
            let root = MNode::parse_text(info.path, text).map_err(|err| err.to_string())?;
            out.xml_file(&root);
        }
    }
    Ok(out.index)
}

struct Reader<'a> {
    info: &'a FileInfo<'a>,
    index: FileIndex,
}

impl Reader<'_> {
    fn artifact(
        &mut self,
        id: String,
        kind: &str,
        name: &str,
        node: Option<&MNode>,
        summary: String,
        parent: Option<String>,
    ) {
        let short_name = short_of(kind, name);
        self.index.artifacts.push(ArtifactRow {
            id,
            kind: kind.to_string(),
            name: name.to_string(),
            short_name,
            component: self.info.component.to_string(),
            path: self.info.path.to_string(),
            line: node.map_or(1, MNode::line),
            column: node.map_or(1, MNode::column),
            summary,
            parent,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn reference(
        &mut self,
        from: &str,
        to_kind: ToKind,
        to_name: &str,
        to_scope: &str,
        ref_type: &str,
        line: u32,
        column: u32,
    ) {
        self.index.refs.push(RefRow {
            from: from.to_string(),
            to_kind,
            to_name: to_name.to_string(),
            to_scope: to_scope.to_string(),
            ref_type: ref_type.to_string(),
            line,
            column,
            to_id: None,
            resolved: false,
        });
    }

    fn file_id(&self) -> String {
        format!("file:{}", self.info.location)
    }

    fn xml_file(&mut self, root: &MNode) {
        let location = self.info.location.to_string();
        let file_from = match self.info.kind {
            ArtifactKind::Screen => {
                let title = root
                    .attribute("default-menu-title")
                    .or_else(|| root.attribute("menu-title"))
                    .unwrap_or_default();
                let id = format!("screen:{location}");
                self.artifact(id.clone(), "screen", &location, Some(root), title, None);
                id
            }
            ArtifactKind::ScreenExtend => {
                let base = extended_location(&location);
                let id = format!("screen-extend:{location}");
                self.artifact(
                    id.clone(),
                    "screen-extend",
                    &location,
                    Some(root),
                    format!("extends {base}"),
                    None,
                );
                self.reference(&id, ToKind::Screen, &base, "", "extend", root.line(), root.column());
                for node in root.child_iter() {
                    let element = node.name();
                    if !matches!(
                        element.as_str(),
                        "widgets-extend" | "form-single" | "form-list" | "section" | "section-iterate" | "transition"
                    ) {
                        continue;
                    }
                    if let Some(name) = node.attribute("name").filter(|name| refs::is_literal(name)) {
                        self.reference(&id, ToKind::Anchor, &name, &base, "anchor", node.line(), node.column());
                    }
                }
                id
            }
            ArtifactKind::Data => {
                let id = format!("data:{location}");
                let kind = root.attribute("type").unwrap_or_default();
                self.artifact(id.clone(), "data", &location, Some(root), kind, None);
                let mut seen = std::collections::BTreeSet::new();
                for node in root.child_iter() {
                    let name = node.name();
                    if name.contains('.') && seen.insert(name.clone()) {
                        self.reference(&id, ToKind::Entity, &name, "", "data", node.line(), node.column());
                    }
                }
                return;
            }
            _ => self.file_id(),
        };
        let prefix = service_prefix(&location);
        let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        let mut stack = vec![(root.clone(), file_from, String::new())];
        while let Some((node, from, parent)) = stack.pop() {
            let element = node.name();
            let from = self
                .definition(&node, &element, &parent, &from, &prefix, &mut counts)
                .unwrap_or(from);
            self.attribute_refs(&node, &element, &from);
            if element == "script" {
                if let Some(text) = node.text() {
                    self.script_refs(&from, &text, node.line());
                }
            }
            let children: Vec<MNode> = node.child_iter().collect();
            for child in children.into_iter().rev() {
                stack.push((child, from.clone(), element.clone()));
            }
        }
    }

    /// The artifact `node` defines, when it defines one, recorded; answers its id, which the references inside it
    /// are made from.
    fn definition(
        &mut self,
        node: &MNode,
        element: &str,
        parent: &str,
        from: &str,
        prefix: &str,
        counts: &mut std::collections::BTreeMap<String, u32>,
    ) -> Option<String> {
        let location = self.info.location;
        match (self.info.kind, element) {
            (ArtifactKind::Entity | ArtifactKind::Eeca, "entity" | "view-entity") => {
                let full = full_entity_name(node)?;
                let id = format!("entity:{full}");
                let summary = description(node).unwrap_or_else(|| {
                    node.attribute("group")
                        .map(|group| format!("group {group}"))
                        .unwrap_or_default()
                });
                self.artifact(id.clone(), element, &full, Some(node), summary, None);
                Some(id)
            }
            (ArtifactKind::Entity, "extend-entity") => {
                let full = full_entity_name(node)?;
                let id = format!("extend-entity:{location}#{full}");
                self.artifact(
                    id.clone(),
                    "extend-entity",
                    &full,
                    Some(node),
                    format!("extends {full}"),
                    None,
                );
                Some(id)
            }
            (ArtifactKind::Entity, "field" | "alias") => {
                let entity = from
                    .strip_prefix("entity:")
                    .or_else(|| from.split_once('#').map(|(_, entity)| entity))?
                    .to_string();
                let name = node.attribute("name")?;
                let summary = node
                    .attribute("type")
                    .or_else(|| node.attribute("field").map(|field| format!("alias of {field}")))
                    .unwrap_or_default();
                let parent_id = format!("entity:{entity}");
                self.artifact(
                    format!("field:{entity}.{name}"),
                    "field",
                    &format!("{entity}.{name}"),
                    Some(node),
                    summary,
                    Some(parent_id),
                );
                None
            }
            (ArtifactKind::Service, "service") if parent == "services" => {
                let verb = node.attribute("verb").unwrap_or_default();
                let noun = node
                    .attribute("noun")
                    .map(|noun| format!("#{noun}"))
                    .unwrap_or_default();
                let name = if prefix.is_empty() {
                    format!("{verb}{noun}")
                } else {
                    format!("{prefix}.{verb}{noun}")
                };
                let id = format!("service:{name}");
                let summary =
                    description(node).unwrap_or_else(|| node.attribute("type").unwrap_or_else(|| "inline".to_string()));
                self.artifact(id.clone(), "service", &name, Some(node), summary, None);
                Some(id)
            }
            (ArtifactKind::Service, "implements") => {
                if let Some(service) = node.attribute("service").filter(|name| refs::is_literal(name)) {
                    self.reference(
                        from,
                        ToKind::Service,
                        &service,
                        "",
                        "implements",
                        node.line(),
                        node.column(),
                    );
                }
                None
            }
            (ArtifactKind::Seca | ArtifactKind::Eeca, "seca" | "eeca" | "emeca" | "giteca") if parent != element => {
                let position = counts.entry(element.to_string()).or_insert(0);
                *position += 1;
                let key = node.attribute("id").unwrap_or_else(|| position.to_string());
                let id = format!("{element}:{location}#{key}");
                let on = match element {
                    "seca" => node
                        .attribute("service")
                        .map(|service| format!("{} {service}", node.attribute("when").unwrap_or_default())),
                    "eeca" => node.attribute("entity"),
                    "giteca" => node.attribute("event"),
                    _ => None,
                }
                .unwrap_or_default();
                self.artifact(id.clone(), element, &format!("{element} {key}"), Some(node), on, None);
                Some(id)
            }
            (ArtifactKind::Rest, "resource" | "id") if parent != "method" => {
                let name = rest_path(node);
                let id = format!("rest:{name}");
                self.artifact(
                    id.clone(),
                    "rest",
                    &name,
                    Some(node),
                    description(node).unwrap_or_default(),
                    None,
                );
                Some(id)
            }
            (ArtifactKind::Rest, "method") => {
                let method = node.attribute("type")?.to_ascii_lowercase();
                let id = format!("{from}:{method}");
                self.artifact(
                    id.clone(),
                    "rest",
                    &format!("{}:{method}", rest_path(node)),
                    Some(node),
                    format!("HTTP {method}"),
                    Some(from.to_string()),
                );
                // Each method binds the path variables of its route; inputs declared by its service stay there.
                let path = rest_path(node);
                let mut names = std::collections::BTreeSet::new();
                for segment in path.split('/') {
                    if let Some(name) = segment.strip_prefix('{').and_then(|value| value.strip_suffix('}')) {
                        names.insert(name.to_string());
                    }
                }
                let mut declaration = Some(node.clone());
                while let Some(at) = declaration {
                    for child in at.child_iter().filter(|child| {
                        child.has_name("path-parameter") || (at.has_name("method") && child.has_name("id"))
                    }) {
                        if let Some(name) = child.attribute("name") {
                            names.insert(name);
                        }
                    }
                    declaration = at.parent();
                }
                for name in names {
                    self.artifact(
                        parameter_id(&id, "path", &name),
                        "parameter",
                        &parameter_name(&id, "path", &name),
                        Some(node),
                        "path String, required".to_string(),
                        Some(id.clone()),
                    );
                }
                Some(id)
            }
            (ArtifactKind::Rest, "service") if parent == "method" => {
                if let Some(name) = node.attribute("name").filter(|name| refs::is_literal(name)) {
                    self.reference(from, ToKind::Service, &name, "", "rest", node.line(), node.column());
                }
                None
            }
            (ArtifactKind::Rest, "entity") if parent == "method" || parent == "resource" => {
                if let Some(name) = node.attribute("name").filter(|name| refs::is_literal(name)) {
                    self.reference(from, ToKind::Entity, &name, "", "rest", node.line(), node.column());
                }
                None
            }
            (ArtifactKind::Service, "parameter") => {
                // in-parameters or out-parameters of a service, maybe inside another parameter (a Map's or List's)
                let mut names = vec![node.attribute("name")?];
                let mut up = node.parent();
                while let Some(ancestor) = up.as_ref().filter(|ancestor| ancestor.has_name("parameter")) {
                    names.push(ancestor.attribute("name")?);
                    up = ancestor.parent();
                }
                let direction = match up?.name().as_str() {
                    "in-parameters" => "in",
                    "out-parameters" => "out",
                    _ => return None,
                };
                let owner = from;
                names.reverse();
                let name = names.join(".");
                let parent_id = match name.rsplit_once('.') {
                    Some((up, _)) => parameter_id(owner, direction, up),
                    None => owner.to_string(),
                };
                let mut summary = vec![format!(
                    "{direction} {}",
                    node.attribute("type").unwrap_or_else(|| "String".to_string())
                )];
                summary.push(format!(
                    "required={}",
                    node.attribute("required").unwrap_or_else(|| "false".to_string())
                ));
                if let Some(default) = node.attribute("default-value").or_else(|| node.attribute("default")) {
                    summary.push(format!("default {default}"));
                }
                self.artifact(
                    parameter_id(owner, direction, &name),
                    "parameter",
                    &parameter_name(owner, direction, &name),
                    Some(node),
                    summary.join(", "),
                    Some(parent_id),
                );
                None
            }
            (ArtifactKind::Screen, "parameter" | "path-parameter")
                if matches!(parent, "screen" | "transition" | "transition-include") =>
            {
                let name = node.attribute("name")?;
                let owner = if parent == "screen" {
                    format!("screen:{location}")
                } else {
                    from.to_string()
                };
                let direction = if element == "path-parameter" { "path" } else { "in" };
                let mut summary = vec![format!(
                    "{direction} {}",
                    node.attribute("type").unwrap_or_else(|| "String".to_string())
                )];
                summary.push(format!(
                    "required={}",
                    element == "path-parameter" || node.attribute_is("required", "true")
                ));
                if let Some(default) = node.attribute("value").or_else(|| node.attribute("from")) {
                    summary.push(format!("default {default}"));
                }
                self.artifact(
                    parameter_id(&owner, direction, &name),
                    "parameter",
                    &parameter_name(&owner, direction, &name),
                    Some(node),
                    summary.join(", "),
                    Some(owner),
                );
                None
            }
            (ArtifactKind::Screen, "transition" | "transition-include") => {
                let name = node.attribute("name")?;
                let method = node
                    .attribute("method")
                    .unwrap_or_else(|| "any".to_string())
                    .to_ascii_lowercase();
                let id = format!("transition:{location}#{name}:{method}");
                let summary = node
                    .first_named("service-call")
                    .and_then(|call| call.attribute("name"))
                    .map(|service| format!("calls {service}"))
                    .unwrap_or_default();
                self.artifact(
                    id.clone(),
                    "transition",
                    &name,
                    Some(node),
                    summary,
                    Some(format!("screen:{location}")),
                );
                Some(id)
            }
            (ArtifactKind::Screen, "form-single" | "form-list") => {
                let name = node.attribute("name")?;
                let id = format!("form:{location}#{name}");
                if let Some(transition) = node.attribute("transition").filter(|name| is_plain_name(name)) {
                    self.transition_refs(node, &id, &transition, "form");
                }
                self.artifact(
                    id.clone(),
                    "form",
                    &name,
                    Some(node),
                    element.to_string(),
                    Some(format!("screen:{location}")),
                );
                Some(id)
            }
            (
                ArtifactKind::Screen,
                "link" | "default-response" | "success-response" | "error-response" | "conditional-response",
            ) => {
                if let Some(url) = node.attribute("url").filter(|url| is_plain_name(url)) {
                    self.transition_refs(node, from, &url, "link");
                }
                None
            }
            (ArtifactKind::Screen, _) if in_widgets(node) => {
                if let Some(name) = node.attribute("name").or_else(|| node.attribute("id")) {
                    self.artifact(
                        format!("anchor:{location}#{name}"),
                        "anchor",
                        &name,
                        Some(node),
                        element.to_string(),
                        Some(format!("screen:{location}")),
                    );
                }
                None
            }
            _ => None,
        }
    }

    /// A method-less reference may reach every overload; preserve each edge rather than choosing one.
    fn transition_refs(&mut self, node: &MNode, from: &str, name: &str, ref_type: &str) {
        let mut root = node.clone();
        while let Some(parent) = root.parent() {
            root = parent;
        }
        let method = node.attribute("method").map(|value| value.to_ascii_lowercase());
        let mut methods: Vec<String> = root
            .child_iter()
            .filter(|child| {
                matches!(child.name().as_str(), "transition" | "transition-include") && child.attribute_is("name", name)
            })
            .map(|child| {
                child
                    .attribute("method")
                    .unwrap_or_else(|| "any".to_string())
                    .to_ascii_lowercase()
            })
            .collect();
        methods.sort();
        methods.dedup();
        if let Some(method) = method {
            let selected = if methods.contains(&method) {
                method
            } else {
                "any".to_string()
            };
            methods.retain(|value| *value == selected);
        }
        if methods.is_empty() {
            methods.push("any".to_string());
        }
        for method in methods {
            self.reference(
                from,
                ToKind::Transition,
                &format!("{name}:{method}"),
                self.info.location,
                ref_type,
                node.line(),
                node.column(),
            );
        }
    }

    /// The references the rules of [`refs::RULES`] find in `node`'s attributes.
    fn attribute_refs(&mut self, node: &MNode, element: &str, from: &str) {
        let mut found = Vec::new();
        node.for_each_attribute(|attribute, value| {
            let Some(rule) = refs::rule_for(element, attribute) else {
                return;
            };
            if !refs::is_literal(value) {
                return;
            }
            let ref_type = match element {
                "extend-entity" | "seca" | "eeca" => "extend",
                "subscreens-item" => "subscreens-item",
                other => other,
            };
            match rule.kind {
                // an extend-entity names its entity with its package
                RefKind::Entity if element == "extend-entity" => found.push((
                    ToKind::Entity,
                    full_entity_name(node).unwrap_or_else(|| value.to_string()),
                    String::new(),
                    ref_type,
                )),
                RefKind::Entity => found.push((ToKind::Entity, value.to_string(), String::new(), ref_type)),
                RefKind::Service => found.push((ToKind::Service, value.to_string(), String::new(), ref_type)),
                RefKind::FindField => {
                    if let Some(entity) = refs::find_entity_of(node).filter(|name| refs::is_literal(name)) {
                        for field in refs::field_names(value) {
                            found.push((ToKind::Field, field.to_string(), entity.clone(), ref_type));
                        }
                    }
                }
                // an in-parameter of the service called
                RefKind::ServiceParameter => {
                    if let Some(service) = refs::service_of(node).filter(|name| refs::is_literal(name)) {
                        found.push((ToKind::Parameter, value.to_string(), service, ref_type));
                    }
                }
                RefKind::Location if value.contains("://") => {
                    let kind = if element == "subscreens-item" {
                        ToKind::Screen
                    } else {
                        ToKind::Location
                    };
                    found.push((kind, value.to_string(), String::new(), ref_type));
                }
                RefKind::Location => {}
            }
        });
        for (kind, name, scope, ref_type) in found {
            self.reference(from, kind, &name, &scope, ref_type, node.line(), node.column());
        }
    }

    /// The service and entity names in the string literals of a script's calls.
    fn script_refs(&mut self, from: &str, text: &str, first_line: u32) {
        for ScriptName { kind, name, offset } in script_names(text) {
            let line = first_line + u32::try_from(text[..offset].matches('\n').count()).unwrap_or(0);
            self.reference(from, kind, &name, "", "script", line, 0);
        }
    }

    fn script_file(&mut self, text: &str) {
        let location = self.info.location.to_string();
        let id = format!("script:{location}");
        self.artifact(id.clone(), "script", &location, None, String::new(), None);
        self.script_refs(&id, text, 1);
    }

    fn skill_file(&mut self, text: &str) {
        if !self.info.path.ends_with(".md") {
            return;
        }
        let location = self.info.location.to_string();
        let mut name = String::new();
        let mut summary = String::new();
        if let Some(front) = text
            .strip_prefix("---")
            .and_then(|rest| rest.split_once("\n---"))
            .map(|(front, _)| front)
        {
            for line in front.lines() {
                if let Some(value) = line.strip_prefix("name:") {
                    name = value.trim().trim_matches('"').to_string();
                } else if let Some(value) = line.strip_prefix("description:") {
                    summary = value.trim().trim_matches('"').to_string();
                }
            }
        }
        if name.is_empty() {
            name = location
                .rsplit('/')
                .next()
                .unwrap_or(&location)
                .trim_end_matches(".md")
                .to_string();
        }
        self.artifact(format!("skill:{location}"), "skill", &name, None, summary, None);
    }
}

static SERVICE_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"service\s*\.\s*(?:sync|async|special|job)\s*\(\s*\)\s*\.\s*name\s*\(\s*["'`]([A-Za-z0-9_.#]+)["'`]"#)
        .expect("service call pattern")
});
static ENTITY_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"entity\s*\.\s*(?:find|makeValue|makeFind|fastFindOne)\s*\(\s*["'`]([A-Za-z0-9_.]+)["'`]"#)
        .expect("entity call pattern")
});

/// A service or entity name in a string literal of a script.
#[derive(Debug, PartialEq)]
pub struct ScriptName {
    pub kind: ToKind,
    pub name: String,
    /// The byte offset of the name in the script.
    pub offset: usize,
}

/// The names in the string literals of a script's service and entity calls (`ec.service.sync().name("…")`,
/// `ec.entity.find("…")`, `makeValue("…")`), in the order they come.
#[must_use]
pub fn script_names(text: &str) -> Vec<ScriptName> {
    let calls = [(ToKind::Service, &*SERVICE_CALL), (ToKind::Entity, &*ENTITY_CALL)];
    let mut found: Vec<ScriptName> = calls
        .into_iter()
        .flat_map(|(kind, call)| {
            call.captures_iter(text)
                .filter_map(|captures| captures.get(1))
                .map(move |name| ScriptName {
                    kind,
                    name: name.as_str().to_string(),
                    offset: name.start(),
                })
        })
        .collect();
    found.sort_by_key(|found| found.offset);
    found
}

/// Whether a screen element is a widget a `screen-extend` may anchor to by its `name` or `id`: inside `widgets` (or
/// `fail-widgets`), and not a form's own field or column.
fn in_widgets(node: &MNode) -> bool {
    let mut up = node.parent();
    while let Some(ancestor) = up {
        let name = ancestor.name();
        match name.as_str() {
            "form-single" | "form-list" | "actions" | "condition" => return false,
            "widgets" | "fail-widgets" => return true,
            _ => {}
        }
        up = ancestor.parent();
    }
    false
}

/// A URL that is a transition's name of the same screen: no path, no scheme, no expression.
fn is_plain_name(url: &str) -> bool {
    !url.is_empty()
        && refs::is_literal(url)
        && url
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        && url != "."
}

/// The first sentence of a `description` child, at most 160 characters.
fn description(node: &MNode) -> Option<String> {
    let text = node.first_named("description")?.text()?;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let first = text.split(". ").next().unwrap_or(&text).trim_end_matches('.');
    (!first.is_empty()).then(|| first.chars().take(160).collect())
}

/// `package.entity-name` of an `entity`, `view-entity` or `extend-entity` element.
fn full_entity_name(node: &MNode) -> Option<String> {
    let name = node.attribute("entity-name").filter(|name| !name.is_empty())?;
    Some(match node.attribute("package").filter(|package| !package.is_empty()) {
        Some(package) if !name.contains('.') => format!("{package}.{name}"),
        _ => name,
    })
}

/// The dotted prefix of the services in a service file at `location`: its path under `service/` without `.xml`.
fn service_prefix(location: &str) -> String {
    location
        .split_once("/service/")
        .map(|(_, rest)| rest.strip_suffix(".xml").unwrap_or(rest).replace('/', "."))
        .unwrap_or_default()
}

/// The REST route through resources and variable `id` segments, including enclosing path parameters.
fn rest_path(node: &MNode) -> String {
    let mut parts = Vec::new();
    let mut current = Some(node.clone());
    while let Some(node) = current {
        match node.name().as_str() {
            "resource" => parts.push(node.attribute("name").unwrap_or_default()),
            "id" => parts.push(format!("{{{}}}", node.attribute("name").unwrap_or_default())),
            _ => {}
        }
        current = node.parent();
    }
    parts.reverse();
    format!("/{}", parts.join("/"))
}

/// The id of the parameter `name` (a nested one by its dotted path) of the artifact `owner`, `direction` `in`, `out`
/// or `path`: `parameter:<owner>@<direction>.<name>`. [`parse_parameter_id`] reads it back.
#[must_use]
pub fn parameter_id(owner: &str, direction: &str, name: &str) -> String {
    format!("parameter:{owner}@{direction}.{name}")
}

/// The name of a parameter artifact: its owner's name, the direction and its own (`a.B.save#Order@in.quantity`), so a
/// find of a bare name such as `orderId` matches it by its short name (the last part) only, after an entity's field.
#[must_use]
pub fn parameter_name(owner: &str, direction: &str, name: &str) -> String {
    let owner = owner.split_once(':').map_or(owner, |(_, name)| name);
    format!("{owner}@{direction}.{name}")
}

/// The owner id, direction and name of a [`parameter_id`]: the text after the last `@` (a parameter's name has none)
/// is the direction, a dot, and the name. Owners may include a method suffix (`:post`, `:any`).
#[must_use]
pub fn parse_parameter_id(id: &str) -> Option<(&str, &str, &str)> {
    let (owner, rest) = id.strip_prefix("parameter:")?.rsplit_once('@')?;
    let (direction, name) = rest.split_once('.')?;
    (matches!(direction, "in" | "out" | "path") && !owner.is_empty() && !name.is_empty())
        .then_some((owner, direction, name))
}

/// The base screen a `screen-extend` file at `location` extends: `component://<its path under screen-extend/>`.
#[must_use]
pub fn extended_location(location: &str) -> String {
    location
        .split_once("/screen-extend/")
        .map(|(_, rest)| format!("component://{rest}"))
        .unwrap_or_default()
}

/// The short name of an artifact: an entity's or a service's last segment (`OrderHeader`, `place#Order`), a file's
/// name, a field's own name.
#[must_use]
pub fn short_of(kind: &str, name: &str) -> String {
    match kind {
        "entity" | "view-entity" | "extend-entity" | "field" | "parameter" => {
            name.rsplit('.').next().unwrap_or(name).to_string()
        }
        "service" => match name.split_once('#') {
            Some((head, noun)) => format!("{}#{noun}", head.rsplit('.').next().unwrap_or(head)),
            None => name.rsplit('.').next().unwrap_or(name).to_string(),
        },
        "screen" | "screen-extend" | "data" | "script" => name
            .rsplit('/')
            .next()
            .unwrap_or(name)
            .split('.')
            .next()
            .unwrap_or(name)
            .to_string(),
        _ => name.to_string(),
    }
}

/// The search key of a name (the index keeps its short name's): its segments, split at dots, `#`, slashes and lower to upper case changes, lower case
/// and space separated (`OrderHeader` is `order header`).
#[must_use]
pub fn search_key(name: &str) -> String {
    let mut key = String::with_capacity(name.len() + 8);
    let mut previous_lower = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            if !key.is_empty() && !key.ends_with(' ') && ch.is_uppercase() && previous_lower {
                key.push(' ');
            }
            key.extend(ch.to_lowercase());
            previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
        } else {
            if !key.is_empty() && !key.ends_with(' ') {
                key.push(' ');
            }
            previous_lower = false;
        }
    }
    key.trim_end().to_string()
}

/// The segments of a search text, lower case: `OrdHdr` is `ord`, `hdr`; `place#Order` is `place`, `order`.
#[must_use]
pub fn query_segments(text: &str) -> Vec<String> {
    search_key(text).split_whitespace().map(str::to_string).collect()
}

/// How well a name's search key matches a query's segments: each segment, in order, matches a later segment of the
/// key as a prefix (better) or as an abbreviation starting with its first letter (`hdr` in `header`). `None` when it
/// does not match. Lower is better.
#[must_use]
pub fn match_score(key: &str, query: &[String]) -> Option<u32> {
    let segments: Vec<&str> = key.split_whitespace().collect();
    let mut at = 0;
    let mut score = 0u32;
    for wanted in query {
        let mut found = false;
        while at < segments.len() {
            let segment = segments[at];
            at += 1;
            if segment == wanted {
                found = true;
                break;
            }
            if segment.starts_with(wanted.as_str()) {
                score += 1;
                found = true;
                break;
            }
            if is_abbreviation(wanted, segment) {
                score += 3;
                found = true;
                break;
            }
            score += 2;
        }
        if !found {
            return None;
        }
    }
    Some(score + u32::try_from(segments.len().saturating_sub(at)).unwrap_or(0))
}

fn is_abbreviation(wanted: &str, segment: &str) -> bool {
    let mut chars = segment.chars();
    let mut wanted_chars = wanted.chars();
    match (wanted_chars.next(), chars.next()) {
        (Some(first), Some(segment_first)) if first == segment_first => {}
        _ => return false,
    }
    for ch in wanted_chars {
        if !chars.any(|candidate| candidate == ch) {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
