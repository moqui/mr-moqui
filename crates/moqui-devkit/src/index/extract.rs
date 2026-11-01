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
    /// `form-field:<screen location>#<form>@<field>`, `subscreen:<screen location>#<item>`,
    /// `ext:<screen location>#<ext-action|ext-panel>:<name>`, `anchor:<screen location>#<name>` (a `screen-extend`'s
    /// own transitions, forms, form fields, subscreens items and ext items by its location), `screen-extend:<location>`,
    /// `extend-entity:<location>#<entity>`, `relationship:<entity>@<[title#]related entity>`, `index:<entity>@<name>`,
    /// `view-member:<view entity>@<entity alias>`, `seca:<location>#<key>` (and `eeca`, `emeca`, `giteca`; the key is
    /// [`eca_key`]), `rest:<path>`, `rest:<path>:<method>` (lowercase HTTP method), `data:<location>`,
    /// `record:<entity>:<pk values>` ([`record_id`]), `widget-template:<location>#<name>`, `script:<location>`,
    /// `skill:<location>`, and
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
    /// A named form, container or section of the base screen in `scope`.
    Anchor,
    /// A transition of a `screen-extend`: the base screen's transition of that name, which it replaces, else none (a
    /// transition it adds).
    TransitionAnchor,
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
            Self::TransitionAnchor => "transition-anchor",
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
            "transition-anchor" => Self::TransitionAnchor,
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

/// What reading a data record needs of the entity model: an entity's full name and primary key, and where a nested
/// record's relationship leads. The server's and a project's index answer from their entity model; without one
/// ([`NoModel`]) records are not indexed, and a relationship keeps its related entity's name as written.
pub trait RecordModel {
    /// The full name and the primary key field names of the entity `name` (full, short or short alias).
    fn keys(&self, name: &str) -> Option<(String, Vec<String>)>;
    /// The full name of the entity the relationship `name` of `entity` leads to, with its key map (a field of `entity`,
    /// the field of the related entity it fills).
    fn related(&self, entity: &str, name: &str) -> Option<(String, Vec<(String, String)>)>;
}

/// No entity model: records are not indexed.
pub struct NoModel;

impl RecordModel for NoModel {
    fn keys(&self, _name: &str) -> Option<(String, Vec<String>)> {
        None
    }

    fn related(&self, _entity: &str, _name: &str) -> Option<(String, Vec<(String, String)>)> {
        None
    }
}

/// The longest id the index keeps (`DevArtifact.artifactId` is `text-medium`); a record with a longer one is left out.
const MAX_ID: usize = 255;

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
        // a template directory's widget templates (the index reads its XML files only)
        ArtifactKind::ScreenText => "template",
        _ => return None,
    })
}

/// Read `text`, the content of the file `info` describes, with no entity model ([`NoModel`]). A file that does not
/// parse is an error naming why.
///
/// # Errors
///
/// Returns the parse error's text when an XML file does not parse.
pub fn extract(info: &FileInfo<'_>, text: &str) -> Result<FileIndex, String> {
    extract_with(info, text, &NoModel)
}

/// [`extract`] with the entity model `model`, which names a data record's entity and primary key.
///
/// # Errors
///
/// Returns the parse error's text when an XML file does not parse.
pub fn extract_with(info: &FileInfo<'_>, text: &str, model: &dyn RecordModel) -> Result<FileIndex, String> {
    let mut out = Reader {
        info,
        model,
        index: FileIndex::default(),
    };
    match info.kind {
        ArtifactKind::Script => out.script_file(text),
        ArtifactKind::Skill => out.skill_file(text),
        ArtifactKind::Data if !info.path.ends_with(".xml") => {}
        // a template directory's other files are no widget templates
        ArtifactKind::ScreenText if !info.path.ends_with(".xml") || !text.contains("<widget-templates") => {}
        _ => {
            let root = MNode::parse_text(info.path, text).map_err(|err| err.to_string())?;
            out.xml_file(&root);
        }
    }
    Ok(out.index)
}

struct Reader<'a> {
    info: &'a FileInfo<'a>,
    model: &'a dyn RecordModel,
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
                        // a transition merges by name: it replaces the base screen's transition of its name, else adds one
                        let kind = if element == "transition" {
                            ToKind::TransitionAnchor
                        } else {
                            ToKind::Anchor
                        };
                        self.reference(&id, kind, &name, &base, "anchor", node.line(), node.column());
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
                self.records(root, &id);
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
            (ArtifactKind::Entity, "relationship") => {
                let entity = entity_of(from)?.to_string();
                let related = node
                    .attribute("related")
                    .or_else(|| node.attribute("related-entity-name"))
                    .filter(|name| !name.is_empty())?;
                // Moqui's relationship name: `title#` and the related entity's full name
                let full = self
                    .model
                    .keys(&related)
                    .map_or_else(|| related.clone(), |(full, _)| full);
                let key = match node.attribute("title").filter(|title| !title.is_empty()) {
                    Some(title) => format!("{title}#{full}"),
                    None => full,
                };
                let mut summary = format!("{} {related}", node.attribute("type").unwrap_or_default());
                if let Some(alias) = node.attribute("short-alias").filter(|alias| !alias.is_empty()) {
                    summary.push_str(&format!(", short-alias {alias}"));
                }
                let id = format!("relationship:{entity}@{key}");
                self.artifact(
                    id.clone(),
                    "relationship",
                    &format!("{entity}@{key}"),
                    Some(node),
                    summary,
                    Some(format!("entity:{entity}")),
                );
                Some(id)
            }
            (ArtifactKind::Entity, "index") => {
                let entity = entity_of(from)?.to_string();
                let name = node.attribute("name").filter(|name| !name.is_empty())?;
                let fields: Vec<String> = node
                    .child_iter()
                    .filter(|child| child.has_name("index-field"))
                    .filter_map(|child| child.attribute("name"))
                    .collect();
                let unique = if node.attribute_is("unique", "true") {
                    "unique: "
                } else {
                    ""
                };
                let id = format!("index:{entity}@{name}");
                self.artifact(
                    id.clone(),
                    "index",
                    &format!("{entity}@{name}"),
                    Some(node),
                    format!("{unique}{}", fields.join(", ")),
                    Some(format!("entity:{entity}")),
                );
                Some(id)
            }
            (ArtifactKind::Entity, "index-field") if parent == "index" => {
                let entity = from.strip_prefix("index:")?.rsplit_once('@')?.0.to_string();
                let name = node.attribute("name").filter(|name| refs::is_literal(name))?;
                self.reference(
                    from,
                    ToKind::Field,
                    &name,
                    &entity,
                    "index-field",
                    node.line(),
                    node.column(),
                );
                None
            }
            (ArtifactKind::Entity, "member-entity" | "member-relationship") if parent == "view-entity" => {
                let view = from.strip_prefix("entity:")?.to_string();
                let alias = node.attribute("entity-alias").filter(|alias| !alias.is_empty())?;
                let joined = node
                    .attribute("join-from-alias")
                    .filter(|alias| !alias.is_empty())
                    .map(|alias| format!(" from {alias}"))
                    .unwrap_or_default();
                let summary = if element == "member-entity" {
                    format!("{}{joined}", node.attribute("entity-name").unwrap_or_default())
                } else {
                    format!(
                        "relationship {}{joined}",
                        node.attribute("relationship").unwrap_or_default()
                    )
                };
                let id = format!("view-member:{view}@{alias}");
                self.artifact(
                    id.clone(),
                    "view-member",
                    &format!("{view}@{alias}"),
                    Some(node),
                    summary,
                    Some(format!("entity:{view}")),
                );
                Some(id)
            }
            (ArtifactKind::Entity, "seed-data") => {
                // seed records declared with an entity are its children
                let parent_id = entity_of(from).map(|entity| format!("entity:{entity}"));
                if let Some(parent_id) = parent_id {
                    self.records(node, &parent_id);
                }
                None
            }
            (ArtifactKind::Service, "service-include") if parent == "services" => {
                // a service by this file's name whose definition is the same verb and noun in another file
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
                let location = node.attribute("location").unwrap_or_default();
                let included_prefix = service_prefix(&location);
                let included = if included_prefix.is_empty() {
                    format!("{verb}{noun}")
                } else {
                    format!("{included_prefix}.{verb}{noun}")
                };
                let id = format!("service:{name}");
                self.artifact(
                    id.clone(),
                    "service",
                    &name,
                    Some(node),
                    format!("includes {included}"),
                    None,
                );
                if refs::is_literal(&location) && !included_prefix.is_empty() {
                    self.reference(
                        &id,
                        ToKind::Service,
                        &included,
                        "",
                        "service-include",
                        node.line(),
                        node.column(),
                    );
                }
                Some(id)
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
                let key = eca_key(node, element, counts);
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
            (ArtifactKind::Screen | ArtifactKind::ScreenExtend, "parameter" | "path-parameter")
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
            (ArtifactKind::Screen | ArtifactKind::ScreenExtend, "transition" | "transition-include") => {
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
                    Some(self.screen_owner()),
                );
                Some(id)
            }
            (ArtifactKind::Screen | ArtifactKind::ScreenExtend, "form-single" | "form-list") => {
                let name = node.attribute("name")?;
                let id = format!("form:{location}#{name}");
                // a screen-extend's form may post to a transition of the base screen: not read as one of its own
                if self.info.kind == ArtifactKind::Screen {
                    if let Some(transition) = node.attribute("transition").filter(|name| is_plain_name(name)) {
                        self.transition_refs(node, &id, &transition, "form");
                    }
                }
                self.artifact(
                    id.clone(),
                    "form",
                    &name,
                    Some(node),
                    element.to_string(),
                    Some(self.screen_owner()),
                );
                Some(id)
            }
            (ArtifactKind::Screen | ArtifactKind::ScreenExtend, "field")
                if matches!(parent, "form-single" | "form-list") =>
            {
                // the references in a field stay the form's, so a screen's list of references keeps them
                let form = from.strip_prefix("form:")?;
                let name = node.attribute("name").filter(|name| !name.is_empty())?;
                let form_name = form.rsplit_once('#').map_or(form, |(_, name)| name);
                self.artifact(
                    format!("form-field:{form}@{name}"),
                    "form-field",
                    &format!("{form_name}@{name}"),
                    Some(node),
                    form_field_summary(node),
                    Some(from.to_string()),
                );
                None
            }
            (ArtifactKind::Screen | ArtifactKind::ScreenExtend, "subscreens-item") => {
                let name = node.attribute("name").filter(|name| !name.is_empty())?;
                let id = format!("subscreen:{location}#{name}");
                let summary = node.attribute("location").unwrap_or_default();
                self.artifact(
                    id.clone(),
                    "subscreen",
                    &name,
                    Some(node),
                    summary,
                    Some(self.screen_owner()),
                );
                Some(id)
            }
            (ArtifactKind::Screen | ArtifactKind::ScreenExtend, "ext-action" | "ext-panel") => {
                let name = node.attribute("name").filter(|name| !name.is_empty())?;
                let id = format!("ext:{location}#{element}:{name}");
                let mut summary = format!("{element}, slot {}", node.attribute("slot").unwrap_or_default());
                if let Some(label) = node.attribute("label").filter(|label| !label.is_empty()) {
                    summary.push_str(&format!(", {label}"));
                }
                if self.info.kind == ArtifactKind::Screen {
                    if let Some(transition) = node.attribute("transition").filter(|name| is_plain_name(name)) {
                        self.transition_refs(node, &id, &transition, element);
                    }
                }
                self.artifact(id.clone(), "ext", &name, Some(node), summary, Some(self.screen_owner()));
                Some(id)
            }
            (ArtifactKind::ScreenText, "widget-template") if parent == "widget-templates" => {
                let name = node.attribute("name").filter(|name| !name.is_empty())?;
                let id = format!("widget-template:{location}#{name}");
                let summary = node.child_iter().next().map(|child| child.name()).unwrap_or_default();
                self.artifact(id.clone(), "widget-template", &name, Some(node), summary, None);
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

    /// The artifact a screen file's transitions, forms and items belong to: its screen, or its `screen-extend`.
    fn screen_owner(&self) -> String {
        if self.info.kind == ArtifactKind::ScreenExtend {
            format!("screen-extend:{}", self.info.location)
        } else {
            format!("screen:{}", self.info.location)
        }
    }

    /// The records of a data file or a `seed-data` element, under `parent`. An element the model names no entity for
    /// is skipped, and the others go on.
    fn records(&mut self, container: &MNode, parent: &str) {
        for node in container.child_iter() {
            if let Some((entity, pk)) = self.model.keys(&node.name()) {
                self.record(&node, &entity, &pk, &std::collections::BTreeMap::new(), parent);
            }
        }
    }

    /// One record, `record:<entity>:<pk values>` ([`record_id`]), and the records nested in it: a child element that is
    /// a relationship of `entity` (or the name of its related entity) is a record of the related entity whose key-map
    /// fields come from this one, as the data loader reads it. A record with a primary key value missing (one the
    /// loader sequences) or an id too long is not indexed; its nested records still are.
    fn record(
        &mut self,
        node: &MNode,
        entity: &str,
        pk: &[String],
        inherited: &std::collections::BTreeMap<String, String>,
        parent: &str,
    ) {
        let value_of = |field: &str| {
            node.attribute(field)
                .or_else(|| node.first_named(field).and_then(|child| child.text()))
                .map(|value| value.trim().to_string())
                .or_else(|| inherited.get(field).cloned())
                .filter(|value| !value.is_empty())
        };
        let values: Option<Vec<String>> = pk.iter().map(|field| value_of(field)).collect();
        if let Some(values) = values.filter(|values| !values.is_empty()) {
            let id = record_id(entity, &values);
            if id.len() <= MAX_ID {
                let short = entity.rsplit('.').next().unwrap_or(entity);
                let summary = match node.attribute("description").filter(|text| !text.is_empty()) {
                    Some(description) => format!("{short}: {}", description.chars().take(160).collect::<String>()),
                    None => short.to_string(),
                };
                let name = id.strip_prefix("record:").unwrap_or(&id).to_string();
                self.artifact(id, "record", &name, Some(node), summary, Some(parent.to_string()));
            }
        }
        for child in node.child_iter() {
            let name = child.name();
            if pk.contains(&name) {
                continue;
            }
            let Some((related, pairs)) = self.model.related(entity, &name) else {
                continue;
            };
            let Some((related, related_pk)) = self.model.keys(&related) else {
                continue;
            };
            let carried: std::collections::BTreeMap<String, String> = pairs
                .iter()
                .filter_map(|(field, related_field)| value_of(field).map(|value| (related_field.clone(), value)))
                .collect();
            self.record(&child, &related, &related_pk, &carried, parent);
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

/// The entity whose fields, relationships and indexes the element at `from` declares: an `entity:` or `view-entity`'s, or
/// an `extend-entity:<location>#<entity>`'s.
fn entity_of(from: &str) -> Option<&str> {
    from.strip_prefix("entity:")
        .or_else(|| from.split_once('#').map(|(_, entity)| entity))
        .filter(|entity| !entity.is_empty())
}

/// The key of an ECA rule in its file: its `id`, else its `rule-name` (an EmECA's or gitECA's), else what it runs on
/// and when, with an ordinal from the second rule on the same target and event (`mantle.order.OrderServices.place#Order:post-service`,
/// `mantle.order.OrderHeader:create+update:2`). A rule added or removed elsewhere in the file keeps the others' keys.
fn eca_key(node: &MNode, element: &str, counts: &mut std::collections::BTreeMap<String, u32>) -> String {
    if let Some(key) = node
        .attribute("id")
        .or_else(|| node.attribute("rule-name"))
        .filter(|key| !key.is_empty())
    {
        return key;
    }
    let target = match element {
        "seca" => format!(
            "{}:{}",
            node.attribute("service").unwrap_or_default(),
            node.attribute("when").unwrap_or_default()
        ),
        "eeca" => {
            let events: Vec<&str> = [
                ("on-create", "create"),
                ("on-update", "update"),
                ("on-delete", "delete"),
                ("on-find-one", "find-one"),
                ("on-find-list", "find-list"),
                ("on-find-iterator", "find-iterator"),
                ("on-find-count", "find-count"),
            ]
            .into_iter()
            .filter(|(attribute, _)| node.attribute_is(attribute, "true"))
            .map(|(_, event)| event)
            .collect();
            format!("{}:{}", node.attribute("entity").unwrap_or_default(), events.join("+"))
        }
        "giteca" => node.attribute("event").unwrap_or_default(),
        _ => String::new(),
    };
    let ordinal = counts.entry(format!("{element} {target}")).or_insert(0);
    *ordinal += 1;
    if *ordinal == 1 {
        target
    } else {
        format!("{target}:{ordinal}")
    }
}

/// A form field's widget and title: the first widget of its `default-field` (else its first `conditional-field`, else
/// its `header-field`), and the title that field gives.
fn form_field_summary(node: &MNode) -> String {
    let Some(field) = node
        .first_named("default-field")
        .or_else(|| node.first_named("conditional-field"))
        .or_else(|| node.first_named("header-field"))
    else {
        return String::new();
    };
    let widget = field.child_iter().next().map(|child| child.name()).unwrap_or_default();
    match field.attribute("title").filter(|title| !title.is_empty()) {
        Some(title) if widget.is_empty() => format!("title {title}"),
        Some(title) => format!("{widget}, title {title}"),
        None => widget,
    }
}

/// The id of a data record: `record:<entity full name>:<primary key values>`, the values in the order of the entity's
/// primary key fields, comma separated, with `%` and `,` in a value written `%25` and `%2C`.
#[must_use]
pub fn record_id(entity: &str, values: &[String]) -> String {
    let values: Vec<String> = values
        .iter()
        .map(|value| value.replace('%', "%25").replace(',', "%2C"))
        .collect();
    format!("record:{entity}:{}", values.join(","))
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
        // `title#` and the related entity's last segment: `Owner#Party`
        "relationship" => {
            let key = name.rsplit_once('@').map_or(name, |(_, key)| key);
            match key.split_once('#') {
                Some((title, related)) => format!("{title}#{}", related.rsplit('.').next().unwrap_or(related)),
                None => key.rsplit('.').next().unwrap_or(key).to_string(),
            }
        }
        "index" | "view-member" | "form-field" => name.rsplit_once('@').map_or(name, |(_, own)| own).to_string(),
        // the primary key values
        "record" => name.split_once(':').map_or(name, |(_, values)| values).to_string(),
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
