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

//! Entity definitions parsed from `MNode`. No SQL and no driver.
//!
//! The model keeps no `MNode`, so a loaded model is `Send` and `Sync`.

mod condition;
mod view;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

pub use condition::{ConditionGroup, ConditionItem, DateFilter, ECondition, EntityConditionDef};
pub use view::{
    prefixed_name, AliasAll, AliasExpr, CaseAlias, ComplexAlias, ComplexField, ComplexPart, FieldAlias, MemberEntity,
    PqAlias, SubSelect, AGGREGATE_FUNCTIONS,
};

use view::SourceField;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionaryType {
    pub type_name: String,
    pub java_type: String,
    pub default_sql_type: String,
}

/// `not_null` is the attribute only. A primary key does not set it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldDefinition {
    pub name: String,
    /// `column-name`, or Java's upper-case underscored field name (before database folding).
    pub column_name: String,
    pub field_type: String,
    /// Dictionary Java type of `field_type`.
    pub java_type: String,
    pub is_pk: bool,
    pub encrypt: bool,
    pub enable_audit_log: Option<String>,
    pub enable_localization: bool,
    pub create_only: bool,
    pub default_value: Option<String>,
    pub not_null: bool,
}

impl FieldDefinition {
    /// True when the Java type is a string, which is when Java applies `UPPER()` for ignore-case.
    pub fn is_text(&self) -> bool {
        is_text_java_type(&self.java_type)
    }
}

pub(crate) fn is_text_java_type(java_type: &str) -> bool {
    java_type == "String" || java_type == "java.lang.String"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMap {
    pub field_name: String,
    pub related: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub related: String,
    pub value: String,
}

/// `name` is `title#fullName`, or the related entity's full name when there is no title.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Relationship {
    pub name: String,
    pub relationship_type: String,
    pub related: String,
    pub related_entity_name: String,
    pub title: Option<String>,
    pub short_alias: Option<String>,
    pub fk_name: Option<String>,
    pub is_auto_reverse: bool,
    pub mutable: bool,
    /// `key-map` as written.
    pub key_map: Vec<KeyMap>,
    /// Field on this entity to field on the related entity, expanded as Java
    /// `getRelationshipExpandedKeyMapInternal` does. Filled at load.
    pub key_pairs: Vec<(String, String)>,
    pub key_values: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDefinition {
    pub name: String,
    pub unique: bool,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterDetail {
    pub relationship: String,
    pub use_master: Option<String>,
    pub details: Vec<MasterDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterDefinition {
    pub name: String,
    pub details: Vec<MasterDetail>,
}

#[derive(Clone, Default)]
pub struct EntityDefinition {
    pub entity_name: String,
    pub package_name: String,
    pub full_name: String,
    /// `table-name`, or Java's upper-case underscored entity name (before database folding).
    pub table_name: String,
    pub group: Option<String>,
    pub entity_use: String,
    pub cache: String,
    pub authorize_skip: String,
    pub create_only: bool,
    pub enable_audit_log: Option<String>,
    pub sequence_bank_size: u64,
    pub sequence_primary_stagger: u64,
    pub sequence_primary_prefix: Option<String>,
    pub sequence_primary_use_uuid: bool,
    pub sequence_secondary_padded_length: u64,
    pub optimistic_lock: bool,
    pub no_update_stamp: bool,
    pub has_dependents: bool,
    pub is_view: bool,
    pub short_alias: Option<String>,
    pub fields: Vec<FieldDefinition>,
    pub relationships: Vec<Relationship>,
    pub indexes: Vec<IndexDefinition>,
    pub masters: Vec<MasterDefinition>,
    /// `member-entity` and `member-relationship` in document order. Complete after
    /// [`EntityModel::complete_view`].
    pub member_entities: Vec<MemberEntity>,
    /// Every alias, including those `alias-all` expands to, after [`EntityModel::complete_view`].
    pub aliases: Vec<FieldAlias>,
    /// Aliases with `pq-expression`, as Java's `pqExpressionNodeMap`: not selected and not among the view's fields; the
    /// value layer evaluates each on a row it reads.
    pub pq_aliases: Vec<PqAlias>,
    /// `alias-all` as written. Already expanded into `aliases` once the view is complete.
    pub alias_alls: Vec<AliasAll>,
    pub entity_condition: Option<EntityConditionDef>,
}

impl std::fmt::Debug for EntityDefinition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EntityDefinition")
            .field("full_name", &self.full_name)
            .field("is_view", &self.is_view)
            .field("fields", &self.fields.len())
            .field("relationships", &self.relationships.len())
            .finish_non_exhaustive()
    }
}

impl EntityDefinition {
    /// An empty view-entity to fill in code, then pass to [`EntityModel::complete_view`].
    pub fn new_view(entity_name: impl Into<String>, package_name: impl Into<String>) -> Self {
        let entity_name = entity_name.into();
        let package_name = package_name.into();
        Self {
            full_name: format!("{package_name}.{entity_name}"),
            entity_name,
            package_name,
            entity_use: "transactional".to_string(),
            cache: "false".to_string(),
            authorize_skip: "false".to_string(),
            is_view: true,
            ..Self::default()
        }
    }

    pub fn field(&self, name: &str) -> Option<&FieldDefinition> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// Primary key field names. For a view, the aliases of member primary keys.
    pub fn pk_field_names(&self) -> Vec<&str> {
        if self.is_view {
            return self
                .aliases
                .iter()
                .filter(|alias| alias.is_pk)
                .map(|alias| alias.name.as_str())
                .collect();
        }
        self.fields
            .iter()
            .filter(|field| field.is_pk)
            .map(|field| field.name.as_str())
            .collect()
    }

    /// Field names, or alias names for a view.
    pub fn field_names(&self) -> Vec<&str> {
        if self.is_view {
            self.aliases.iter().map(|alias| alias.name.as_str()).collect()
        } else {
            self.fields.iter().map(|field| field.name.as_str()).collect()
        }
    }

    /// A field, or an alias for a view.
    pub fn has_field(&self, name: &str) -> bool {
        if self.is_view {
            self.alias(name).is_some()
        } else {
            self.field(name).is_some()
        }
    }

    /// Java type of a field, or of an alias for a view. Empty when the alias has no type.
    pub fn java_type_of(&self, name: &str) -> Option<&str> {
        if self.is_view {
            self.alias(name).map(|alias| alias.java_type.as_str())
        } else {
            self.field(name).map(|field| field.java_type.as_str())
        }
    }

    fn view_field(&self, name: &str) -> Option<SourceField> {
        if self.is_view {
            self.alias(name).map(|alias| SourceField {
                field_type: alias.field_type.clone(),
                is_pk: alias.is_pk,
                encrypt: alias.encrypt,
                enable_localization: alias.enable_localization,
            })
        } else {
            self.field(name).map(|field| SourceField {
                field_type: Some(field.field_type.clone()),
                is_pk: field.is_pk,
                encrypt: field.encrypt,
                enable_localization: field.enable_localization,
            })
        }
    }

    /// Full relationship name, `short-alias`, or the related entity name when there is no title.
    pub fn relationship(&self, name: &str) -> Option<&Relationship> {
        self.relationships
            .iter()
            .find(|relationship| relationship.matches(name))
    }

    pub fn member(&self, entity_alias: &str) -> Option<&MemberEntity> {
        self.member_entities
            .iter()
            .find(|member| member.entity_alias == entity_alias)
    }

    pub fn alias(&self, name: &str) -> Option<&FieldAlias> {
        self.aliases.iter().find(|alias| alias.name == name)
    }

    pub fn pq_alias(&self, name: &str) -> Option<&PqAlias> {
        self.pq_aliases.iter().find(|alias| alias.name == name)
    }
}

impl Relationship {
    pub fn matches(&self, name: &str) -> bool {
        if self.name == name || self.short_alias.as_deref() == Some(name) {
            return true;
        }
        self.title.is_none() && self.related_entity_name == name
    }
}

pub struct EntityModel {
    dictionary: Vec<DictionaryType>,
    entities: Vec<EntityDefinition>,
    by_full: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
    by_short_alias: HashMap<String, usize>,
}

impl EntityModel {
    /// `conf` is a tree that contains `<database-list>`. Documents whose root is not `entities` are skipped.
    ///
    /// As Java's `EntityFacadeImpl`, a full name defined again is a warning and the later definition is used, and an
    /// `extend-entity` that matches no entity is skipped with a warning.
    pub fn load(conf: &MNode, documents: &[MNode]) -> Result<Self, Error> {
        let dictionary = parse_dictionary(conf)?;
        let legacy_names = crate::dialect::legacy_names(conf)?;
        // Each node as the document holds it; only one an `extend-entity` changes is copied first ([`apply_extend`]),
        // so the documents stay as they were read.
        let mut nodes: Vec<MNode> = Vec::new();
        let mut view_flags: Vec<bool> = Vec::new();
        let mut by_full: HashMap<String, usize> = HashMap::new();
        let mut extends = Vec::new();
        for document in documents {
            if !document.has_name("entities") {
                continue;
            }
            for child in document.children() {
                match child.name().as_str() {
                    "entity" | "view-entity" => {
                        let node = child.clone();
                        let (entity_name, package_name) = entity_identity(&node)?;
                        let full_name = format!("{package_name}.{entity_name}");
                        if let Some(index) = by_full.get(&full_name).copied() {
                            tracing::warn!(
                                "Entity {full_name} was found again at {}, so overriding definition from previous location {}",
                                where_of(&node),
                                where_of(&nodes[index])
                            );
                            view_flags[index] = child.has_name("view-entity");
                            nodes[index] = node;
                            continue;
                        }
                        by_full.insert(full_name, nodes.len());
                        view_flags.push(child.has_name("view-entity"));
                        nodes.push(node);
                    }
                    "extend-entity" => extends.push(child.clone()),
                    _ => {}
                }
            }
        }
        let mut copied = vec![false; nodes.len()];
        for extend_node in &extends {
            apply_extend(&mut nodes, &mut copied, &by_full, extend_node)?;
        }

        let mut entities = Vec::with_capacity(nodes.len());
        let mut index_by_full = HashMap::with_capacity(nodes.len());
        let mut index_by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (node, is_view) in nodes.iter().zip(view_flags) {
            let definition = parse_entity(node, is_view, &dictionary, legacy_names)?;
            let index = entities.len();
            index_by_name
                .entry(definition.entity_name.clone())
                .or_default()
                .push(index);
            index_by_full.insert(definition.full_name.clone(), index);
            entities.push(definition);
        }
        resolve_relationships(&mut entities, &index_by_name)?;
        create_auto_reverses(&mut entities, &index_by_full)?;
        expand_key_pairs(&mut entities, &index_by_full);
        for (name, indexes) in &index_by_name {
            if indexes.len() > 1 {
                let chosen = preferred(&entities, indexes);
                let all: Vec<String> = indexes
                    .iter()
                    .map(|index| format!("{} at {}", entities[*index].full_name, where_of(&nodes[*index])))
                    .collect();
                tracing::warn!(
                    "Entity name {name} is defined more than once ({}); the name alone means {}",
                    all.join(", "),
                    entities[chosen].full_name
                );
            }
        }
        let mut by_short_alias = HashMap::new();
        for (index, entity) in entities.iter().enumerate() {
            if let Some(alias) = entity.short_alias.as_ref().filter(|alias| !alias.is_empty()) {
                by_short_alias.entry(alias.clone()).or_insert(index);
            }
        }
        let mut model = Self {
            dictionary,
            entities,
            by_full: index_by_full,
            by_name: index_by_name,
            by_short_alias,
        };
        model.complete_views()?;
        // A relationship to a view expands against the view's primary key aliases, which exist now.
        expand_key_pairs(&mut model.entities, &model.by_full);
        Ok(model)
    }

    /// Every `*.xml` in `directory` whose root is `entities`. `*.eecas.xml` has another root and is skipped.
    pub fn load_directory(conf: &MNode, directory: &Path) -> Result<Self, Error> {
        let documents = entity_documents(directory)?;
        Self::load(conf, &documents)
    }

    /// [`Self::load_directory`] over several directories in order, the framework's first and then each
    /// component's `entity` directory, as one model: a component's `extend-entity` applies to any of them.
    pub fn load_directories(conf: &MNode, directories: &[&Path]) -> Result<Self, Error> {
        Self::load_paths(conf, directories)
    }

    /// [`Self::load_directories`] where a path may also be one entity file, such as a `<load-entity location>` of the
    /// conf. A file whose root is not `entities` is skipped.
    pub fn load_paths(conf: &MNode, paths: &[&Path]) -> Result<Self, Error> {
        let mut documents = Vec::new();
        for path in paths {
            if path.is_file() {
                let node = MNode::parse_file(path)?;
                if node.has_name("entities") {
                    documents.push(node);
                }
            } else {
                documents.extend(entity_documents(path)?);
            }
        }
        Self::load(conf, &documents)
    }

    pub fn dictionary_types(&self) -> &[DictionaryType] {
        &self.dictionary
    }

    pub fn dictionary_type(&self, type_name: &str) -> Option<&DictionaryType> {
        self.dictionary.iter().find(|entry| entry.type_name == type_name)
    }

    pub fn entities(&self) -> &[EntityDefinition] {
        &self.entities
    }

    pub fn full_names(&self) -> Vec<&str> {
        self.entities.iter().map(|entity| entity.full_name.as_str()).collect()
    }

    /// Full name, an entity name that belongs to one definition, or a `short-alias`, as Java's
    /// definition cache keys them.
    pub fn entity(&self, name: &str) -> Result<&EntityDefinition, Error> {
        self.entity_index(name).map(|index| &self.entities[index])
    }

    fn entity_index(&self, name: &str) -> Result<usize, Error> {
        if let Some(index) = self.by_full.get(name) {
            return Ok(*index);
        }
        match self.by_name.get(name) {
            Some(indexes) if !indexes.is_empty() => Ok(preferred(&self.entities, indexes)),
            _ => self
                .by_short_alias
                .get(name)
                .copied()
                .ok_or_else(|| Error::new(format!("no entity named {name}"))),
        }
    }

    pub fn entity_full(&self, full_name: &str) -> Option<&EntityDefinition> {
        self.by_full.get(full_name).map(|index| &self.entities[*index])
    }

    /// A `view-entity` element made at runtime, as Java's `new EntityDefinition(efi, node)` for a `DbViewEntity` or a
    /// data document's view: parsed as one read from a file and completed against this model. It is not added to the
    /// model, so its members must be in it.
    pub fn derive_view(&self, node: &MNode) -> Result<EntityDefinition, Error> {
        let mut view = parse_entity(node, true, &self.dictionary, false)?;
        self.complete_view(&mut view)?;
        Ok(view)
    }

    /// Complete every view, members first, so a view that joins another view sees it finished.
    fn complete_views(&mut self) -> Result<(), Error> {
        let mut state = vec![Completion::Pending; self.entities.len()];
        for index in 0..self.entities.len() {
            self.complete_at(index, &mut state)?;
        }
        Ok(())
    }

    fn complete_at(&mut self, index: usize, state: &mut [Completion]) -> Result<(), Error> {
        if !self.entities[index].is_view || state[index] == Completion::Done {
            return Ok(());
        }
        if state[index] == Completion::Running {
            return Err(Error::new(format!(
                "view-entity {} is a member of itself",
                self.entities[index].full_name
            )));
        }
        state[index] = Completion::Running;
        // Complete member views first, so their aliases are there when this view's are derived.
        let members: Vec<String> = self.entities[index]
            .member_entities
            .iter()
            .filter(|member| member.relationship.is_none())
            .map(|member| member.entity_name.clone())
            .collect();
        for member in members {
            let member_index = self.entity_index(&member)?;
            self.complete_at(member_index, state)?;
        }
        let mut view = std::mem::take(&mut self.entities[index]);
        let completed = self.complete_view(&mut view);
        self.entities[index] = view;
        completed?;
        state[index] = Completion::Done;
        Ok(())
    }
}

/// The definition an entity name defined in more than one package means, as Java's definition cache has it after
/// loading every entity in full name order: the framework's (`moqui.`) when there is one, else the full name that sorts
/// last.
fn preferred(entities: &[EntityDefinition], indexes: &[usize]) -> usize {
    let framework = indexes
        .iter()
        .copied()
        .filter(|index| entities[*index].full_name.starts_with("moqui."))
        .max_by(|a, b| entities[*a].full_name.cmp(&entities[*b].full_name));
    framework.unwrap_or_else(|| {
        indexes
            .iter()
            .copied()
            .max_by(|a, b| entities[*a].full_name.cmp(&entities[*b].full_name))
            .unwrap_or(indexes[0])
    })
}

/// Where a node was read, for a message.
fn where_of(node: &MNode) -> String {
    match node.file_location() {
        Some(location) => format!("{location}:{}", node.line()),
        None => "an unknown location".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completion {
    Pending,
    Running,
    Done,
}

fn entity_documents(directory: &Path) -> Result<Vec<MNode>, Error> {
    let entries = std::fs::read_dir(directory)
        .map_err(|err| Error::new(format!("could not read {}", directory.display())).with_source(err))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| Error::new(format!("could not read {}", directory.display())).with_source(err))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("xml") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut documents = Vec::new();
    for path in paths {
        let node = MNode::parse_file(&path)?;
        if node.has_name("entities") {
            documents.push(node);
        }
    }
    Ok(documents)
}

fn parse_dictionary(conf: &MNode) -> Result<Vec<DictionaryType>, Error> {
    let mut nodes = Vec::new();
    collect_dictionary(conf, &mut nodes);
    let mut types = Vec::new();
    let mut seen = HashSet::new();
    for node in nodes {
        let type_name = require_attr(&node, "type")?;
        if !seen.insert(type_name.clone()) {
            return Err(located(&node, format!("duplicate dictionary type {type_name}")));
        }
        types.push(DictionaryType {
            type_name,
            java_type: require_attr(&node, "java-type")?,
            default_sql_type: require_attr(&node, "default-sql-type")?,
        });
    }
    Ok(types)
}

fn collect_dictionary(node: &MNode, out: &mut Vec<MNode>) {
    if node.has_name("database-list") {
        out.extend(node.children_named("dictionary-type"));
        return;
    }
    for child in node.children() {
        collect_dictionary(&child, out);
    }
}

/// `copied` marks the nodes copied from their documents already; a node is copied before its first change.
fn apply_extend(
    nodes: &mut [MNode],
    copied: &mut [bool],
    by_full: &HashMap<String, usize>,
    extend_node: &MNode,
) -> Result<(), Error> {
    let (entity_name, package_name) = entity_identity(extend_node)?;
    let full_name = format!("{package_name}.{entity_name}");
    let Some(index) = by_full.get(&full_name) else {
        // Java applies an extend-entity only while it builds the entity it names, so one that names no entity (or
        // another package) is never used.
        tracing::warn!(
            "extend-entity {full_name} at {} matches no entity, skipping it",
            where_of(extend_node)
        );
        return Ok(());
    };
    if !copied[*index] {
        nodes[*index] = nodes[*index].deep_copy();
        copied[*index] = true;
    }
    let entity_node = &nodes[*index];
    entity_node.copy_attributes_from(extend_node);
    for field in extend_node.children_named("field") {
        let name = require_attr(&field, "name")?;
        if let Some(existing) = entity_node.first_named_attrs("field", &[("name", name.as_str())]) {
            existing.copy_attributes_from(&field);
        } else {
            entity_node.append_node(field.deep_copy());
        }
    }
    replace_or_append(entity_node, extend_node, "relationship", relationship_key)?;
    replace_or_append(entity_node, extend_node, "index", |node| require_attr(node, "name"))?;
    for master in extend_node.children_named("master") {
        entity_node.append_node(master.deep_copy());
    }
    Ok(())
}

fn relationship_key(node: &MNode) -> Result<String, Error> {
    let related = related_attr(node)?;
    let title = optional_attr(node, "title").unwrap_or_default();
    Ok(format!("{related}\0{title}"))
}

fn replace_or_append(
    entity_node: &MNode,
    extend_node: &MNode,
    child_name: &str,
    key_of: impl Fn(&MNode) -> Result<String, Error>,
) -> Result<(), Error> {
    for copy in extend_node.children_named(child_name) {
        let copy_key = key_of(&copy)?;
        let position = entity_node
            .children()
            .iter()
            .position(|child| child.name() == child_name && key_of(child).ok().as_deref() == Some(copy_key.as_str()));
        if let Some(position) = position {
            entity_node.replace_child(position, copy.deep_copy())?;
        } else {
            entity_node.append_node(copy.deep_copy());
        }
    }
    Ok(())
}

fn parse_entity(
    node: &MNode,
    is_view: bool,
    dictionary: &[DictionaryType],
    legacy_names: bool,
) -> Result<EntityDefinition, Error> {
    let dictionary_names: HashSet<String> = dictionary.iter().map(|entry| entry.type_name.clone()).collect();
    let java_of = |field_type: &str| {
        dictionary
            .iter()
            .find(|entry| entry.type_name == field_type)
            .map(|entry| entry.java_type.clone())
            .unwrap_or_default()
    };
    let (entity_name, package_name) = entity_identity(node)?;
    let full_name = format!("{package_name}.{entity_name}");
    let table_name = optional_attr(node, "table-name").unwrap_or_else(|| default_sql_name(&entity_name, legacy_names));
    let create_only = is_true(node, "create-only");
    let enable_audit_log = optional_attr(node, "enable-audit-log");
    let no_update_stamp = is_true(node, "no-update-stamp");
    let mut fields = if is_view {
        Vec::new()
    } else {
        parse_fields(
            node,
            create_only,
            enable_audit_log.as_deref(),
            &dictionary_names,
            legacy_names,
        )?
    };
    if !is_view && !no_update_stamp && !fields.iter().any(|field| field.name == "lastUpdatedStamp") {
        if !dictionary_names.contains("date-time") {
            return Err(located(
                node,
                format!("entity {full_name} needs dictionary type date-time for lastUpdatedStamp"),
            ));
        }
        fields.push(FieldDefinition {
            name: "lastUpdatedStamp".to_string(),
            column_name: default_sql_name("lastUpdatedStamp", legacy_names),
            field_type: "date-time".to_string(),
            enable_audit_log: enable_audit_log.clone(),
            create_only,
            ..FieldDefinition::default()
        });
    }
    for field in &mut fields {
        field.java_type = java_of(&field.field_type);
    }
    let relationships = node
        .children_named("relationship")
        .iter()
        .map(parse_relationship)
        .collect::<Result<Vec<_>, _>>()?;
    let indexes = node
        .children_named("index")
        .iter()
        .map(parse_index)
        .collect::<Result<Vec<_>, _>>()?;
    let masters = node
        .children_named("master")
        .iter()
        .map(parse_master)
        .collect::<Result<Vec<_>, _>>()?;
    let (member_entities, aliases, pq_aliases, alias_alls, entity_condition) = if is_view {
        let mut member_entities = Vec::new();
        for child in node.children() {
            if matches!(child.name().as_str(), "member-entity" | "member-relationship") {
                member_entities.push(view::parse_member(&child)?);
            }
        }
        // As Java, an alias with `pq-expression` is kept apart from the selected aliases.
        let mut aliases = Vec::new();
        let mut pq_aliases = Vec::new();
        for child in node.children_named("alias") {
            match view::parse_pq_alias(&child)? {
                Some(pq_alias) => pq_aliases.push(pq_alias),
                None => aliases.push(view::parse_alias(&child, &dictionary_names)?),
            }
        }
        for alias in &mut aliases {
            if let Some(field_type) = &alias.field_type {
                alias.java_type = java_of(field_type);
            }
        }
        let alias_alls = node
            .children_named("alias-all")
            .iter()
            .map(view::parse_alias_all)
            .collect::<Result<Vec<_>, _>>()?;
        let entity_condition = node
            .first_named("entity-condition")
            .map(|condition| EntityConditionDef::parse(&condition))
            .transpose()?;
        (member_entities, aliases, pq_aliases, alias_alls, entity_condition)
    } else {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), None)
    };
    Ok(EntityDefinition {
        entity_name,
        package_name,
        full_name,
        table_name,
        group: optional_attr(node, "group").or_else(|| optional_attr(node, "group-name")),
        entity_use: optional_attr(node, "use").unwrap_or_else(|| "transactional".to_string()),
        cache: optional_attr(node, "cache").unwrap_or_else(|| "false".to_string()),
        authorize_skip: optional_attr(node, "authorize-skip").unwrap_or_else(|| "false".to_string()),
        create_only,
        enable_audit_log,
        sequence_bank_size: parse_u64(node, "sequence-bank-size", 50)?,
        sequence_primary_stagger: parse_u64(node, "sequence-primary-stagger", 1)?,
        sequence_primary_prefix: optional_attr(node, "sequence-primary-prefix"),
        sequence_primary_use_uuid: is_true(node, "sequence-primary-use-uuid"),
        sequence_secondary_padded_length: parse_u64(node, "sequence-secondary-padded-length", 2)?,
        optimistic_lock: is_true(node, "optimistic-lock"),
        no_update_stamp,
        has_dependents: false,
        is_view,
        short_alias: optional_attr(node, "short-alias"),
        fields,
        relationships,
        indexes,
        masters,
        member_entities,
        aliases,
        pq_aliases,
        alias_alls,
        entity_condition,
    })
}

fn parse_fields(
    node: &MNode,
    entity_create_only: bool,
    entity_audit: Option<&str>,
    dictionary: &HashSet<String>,
    legacy_names: bool,
) -> Result<Vec<FieldDefinition>, Error> {
    let mut fields = Vec::new();
    for child in node.children_named("field") {
        let name = require_attr(&child, "name")?;
        let field_type = require_attr(&child, "type")?;
        if !dictionary.contains(&field_type) {
            return Err(located(
                &child,
                format!("field {name} type {field_type} is not a dictionary type"),
            ));
        }
        fields.push(FieldDefinition {
            column_name: optional_attr(&child, "column-name").unwrap_or_else(|| default_sql_name(&name, legacy_names)),
            name,
            field_type,
            java_type: String::new(),
            is_pk: is_true(&child, "is-pk"),
            encrypt: is_true(&child, "encrypt"),
            enable_audit_log: optional_attr(&child, "enable-audit-log").or_else(|| entity_audit.map(str::to_string)),
            enable_localization: is_true(&child, "enable-localization"),
            create_only: match optional_attr(&child, "create-only") {
                Some(value) => value == "true",
                None => entity_create_only,
            },
            default_value: optional_attr(&child, "default"),
            not_null: is_true(&child, "not-null"),
        });
    }
    Ok(fields)
}

/// Java `EntityJavaUtil.camelCaseToUnderscored`: split before *each* upper-case
/// character after the first, including consecutive capitals. Java's char uppercasing
/// does not expand characters (for example, ß stays ß). Non-ASCII mappings are best-effort;
/// Rust's full Unicode case mappings are not always Java's simple character mappings.
pub fn camel_case_to_underscored(name: &str) -> String {
    let mut out = Vec::new();
    // Java iterates UTF-16 chars, not Unicode code points: supplementary characters
    // are copied as surrogate pairs, without case conversion or extra separators.
    for (index, unit) in name.encode_utf16().enumerate() {
        let Some(ch) = char::from_u32(u32::from(unit)) else {
            out.push(unit);
            continue;
        };
        if index > 0 && ch.is_uppercase() {
            out.push(u16::from(b'_'));
        }
        let mut upper = ch.to_uppercase();
        let first = upper.next().unwrap_or(ch);
        out.push(if upper.next().is_none() && first.len_utf16() == 1 {
            first as u16
        } else {
            unit
        });
    }
    String::from_utf16_lossy(&out)
}

fn default_sql_name(name: &str, legacy_names: bool) -> String {
    if legacy_names {
        name.to_string()
    } else {
        camel_case_to_underscored(name)
    }
}

fn parse_relationship(node: &MNode) -> Result<Relationship, Error> {
    let relationship_type = require_attr(node, "type")?;
    let mutable = match optional_attr(node, "mutable") {
        Some(value) => value == "true",
        None => !relationship_type.starts_with("one"),
    };
    Ok(Relationship {
        name: String::new(),
        relationship_type,
        related: related_attr(node)?,
        related_entity_name: String::new(),
        title: optional_attr(node, "title"),
        short_alias: optional_attr(node, "short-alias"),
        fk_name: optional_attr(node, "fk-name"),
        is_auto_reverse: is_true(node, "is-auto-reverse"),
        mutable,
        key_map: parse_key_maps(node)?,
        key_pairs: Vec::new(),
        key_values: parse_key_values(node)?,
    })
}

fn parse_key_maps(node: &MNode) -> Result<Vec<KeyMap>, Error> {
    let mut maps = Vec::new();
    for child in node.children_named("key-map") {
        maps.push(KeyMap {
            field_name: require_attr(&child, "field-name")?,
            related: optional_attr(&child, "related").or_else(|| optional_attr(&child, "related-field-name")),
        });
    }
    Ok(maps)
}

fn parse_key_values(node: &MNode) -> Result<Vec<KeyValue>, Error> {
    let mut values = Vec::new();
    for child in node.children_named("key-value") {
        values.push(KeyValue {
            related: require_attr(&child, "related")?,
            value: require_attr(&child, "value")?,
        });
    }
    Ok(values)
}

fn parse_index(node: &MNode) -> Result<IndexDefinition, Error> {
    let name = require_attr(node, "name")?;
    let mut fields = Vec::new();
    for child in node.children_named("index-field") {
        fields.push(require_attr(&child, "name")?);
    }
    if fields.is_empty() {
        return Err(located(node, format!("index {name} has no index-field")));
    }
    Ok(IndexDefinition {
        name,
        unique: is_true(node, "unique"),
        fields,
    })
}

fn parse_master(node: &MNode) -> Result<MasterDefinition, Error> {
    let mut details = Vec::new();
    for child in node.children_named("detail") {
        details.push(parse_detail(&child)?);
    }
    Ok(MasterDefinition {
        name: optional_attr(node, "name").unwrap_or_else(|| "default".to_string()),
        details,
    })
}

fn parse_detail(node: &MNode) -> Result<MasterDetail, Error> {
    let mut details = Vec::new();
    for child in node.children_named("detail") {
        details.push(parse_detail(&child)?);
    }
    Ok(MasterDetail {
        relationship: require_attr(node, "relationship")?,
        use_master: optional_attr(node, "use-master"),
        details,
    })
}

fn resolve_relationships(
    entities: &mut [EntityDefinition],
    by_name: &HashMap<String, Vec<usize>>,
) -> Result<(), Error> {
    let mut names: HashMap<String, String> = HashMap::new();
    for entity in entities.iter() {
        names.insert(entity.full_name.clone(), entity.full_name.clone());
    }
    for (short_name, indexes) in by_name {
        if indexes.len() == 1 {
            names.insert(short_name.clone(), entities[indexes[0]].full_name.clone());
        }
    }
    for entity in entities.iter() {
        if let Some(alias) = entity.short_alias.as_ref().filter(|alias| !alias.is_empty()) {
            names.entry(alias.clone()).or_insert_with(|| entity.full_name.clone());
        }
    }
    let short_of: HashMap<String, String> = entities
        .iter()
        .map(|entity| (entity.full_name.clone(), entity.entity_name.clone()))
        .collect();
    for entity in entities.iter_mut() {
        let source = entity.full_name.clone();
        for relationship in &mut entity.relationships {
            let raw = relationship.related.clone();
            let Some(full_name) = names.get(&raw).cloned() else {
                return Err(Error::new(format!(
                    "entity {source} relationship related {raw} was not found"
                )));
            };
            let Some(related_entity_name) = short_of.get(&full_name).cloned() else {
                return Err(Error::new(format!(
                    "entity {source} relationship related {raw} was not found"
                )));
            };
            relationship.name = relationship_name(relationship.title.as_deref(), &full_name);
            relationship.related = full_name;
            relationship.related_entity_name = related_entity_name;
        }
    }
    Ok(())
}

fn relationship_name(title: Option<&str>, related_full_name: &str) -> String {
    match title {
        Some(title) if !title.is_empty() => format!("{title}#{related_full_name}"),
        _ => related_full_name.to_string(),
    }
}

struct PendingReverse {
    relationship_type: String,
    related: String,
    title: Option<String>,
    key_map: Vec<KeyMap>,
    source_name: String,
    source_full: String,
    source_is_view: bool,
    source_pks: Vec<String>,
}

fn create_auto_reverses(entities: &mut [EntityDefinition], by_full: &HashMap<String, usize>) -> Result<(), Error> {
    let mut order: Vec<String> = by_full.keys().cloned().collect();
    order.sort();
    for source_full in order {
        let source_index = by_full[&source_full];
        let pending: Vec<PendingReverse> = {
            let source = &entities[source_index];
            source
                .relationships
                .iter()
                .filter(|relationship| !relationship.is_auto_reverse && relationship.related != source.full_name)
                .map(|relationship| PendingReverse {
                    relationship_type: relationship.relationship_type.clone(),
                    related: relationship.related.clone(),
                    title: relationship.title.clone(),
                    key_map: relationship.key_map.clone(),
                    source_name: source.entity_name.clone(),
                    source_full: source.full_name.clone(),
                    source_is_view: source.is_view,
                    source_pks: pk_strings(source),
                })
                .collect()
        };
        for reverse in pending {
            let Some(&target_index) = by_full.get(&reverse.related) else {
                return Err(Error::new(format!(
                    "entity {} relationship to {} was not loaded",
                    reverse.source_full, reverse.related
                )));
            };
            let target_pks = pk_strings(&entities[target_index]);
            let expanded = expand_key_map(&reverse.relationship_type, &reverse.key_map, &entities[target_index]);
            if points_back(
                &entities[target_index],
                &reverse.source_full,
                &reverse.source_name,
                reverse.title.as_deref(),
            ) {
                continue;
            }
            let relationship_type = if target_pks == reverse.source_pks {
                "one-nofk".to_string()
            } else {
                "many".to_string()
            };
            let name = relationship_name(reverse.title.as_deref(), &reverse.source_full);
            let key_map = expanded
                .into_iter()
                .map(|(field_name, related)| KeyMap {
                    field_name: related,
                    related: Some(field_name),
                })
                .collect();
            let target = &mut entities[target_index];
            if !reverse.source_is_view && reverse.relationship_type != "many" {
                target.has_dependents = true;
            }
            target.relationships.push(Relationship {
                name,
                relationship_type,
                related: reverse.source_full,
                related_entity_name: reverse.source_name,
                title: reverse.title,
                is_auto_reverse: true,
                mutable: true,
                key_map,
                ..Relationship::default()
            });
        }
    }
    Ok(())
}

/// Fill `key_pairs` on every relationship, auto-reverse ones included.
fn expand_key_pairs(entities: &mut [EntityDefinition], by_full: &HashMap<String, usize>) {
    for index in 0..entities.len() {
        let pairs: Vec<Vec<(String, String)>> = entities[index]
            .relationships
            .iter()
            .map(|relationship| match by_full.get(&relationship.related) {
                Some(&target) => expand_key_map(
                    &relationship.relationship_type,
                    &relationship.key_map,
                    &entities[target],
                ),
                None => Vec::new(),
            })
            .collect();
        for (relationship, pairs) in entities[index].relationships.iter_mut().zip(pairs) {
            relationship.key_pairs = pairs;
        }
    }
}

fn pk_strings(entity: &EntityDefinition) -> Vec<String> {
    entity
        .fields
        .iter()
        .filter(|field| field.is_pk)
        .map(|field| field.name.clone())
        .collect()
}

/// Java `EntityDefinition.getRelationshipExpandedKeyMapInternal`.
///
/// With no key-map on a `one` relationship, every related primary key maps to the same name. One
/// key-map without `related` maps to the related single primary key on a `one` relationship,
/// otherwise to the same name. With several, a `related` name that is not a related field falls
/// back to the related single primary key on a `one` relationship.
pub fn expand_key_map(relationship_type: &str, key_map: &[KeyMap], target: &EntityDefinition) -> Vec<(String, String)> {
    let pks: Vec<String> = target.pk_field_names().into_iter().map(str::to_string).collect();
    let one = relationship_type.starts_with("one");
    if key_map.is_empty() && one {
        return pks.into_iter().map(|pk| (pk.clone(), pk)).collect();
    }
    if key_map.len() == 1 {
        let map = &key_map[0];
        let related = match map.related.as_deref().filter(|value| !value.is_empty()) {
            Some(related) => related.to_string(),
            None if one && pks.len() == 1 => pks[0].clone(),
            None => map.field_name.clone(),
        };
        return vec![(map.field_name.clone(), related)];
    }
    key_map
        .iter()
        .map(|map| {
            let mut related = map
                .related
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| map.field_name.clone());
            if one && pks.len() == 1 && !target.has_field(&related) {
                related = pks[0].clone();
            }
            (map.field_name.clone(), related)
        })
        .collect()
}

fn points_back(target: &EntityDefinition, source_full: &str, source_name: &str, title: Option<&str>) -> bool {
    target.relationships.iter().any(|relationship| {
        let related = relationship.related == source_full || relationship.related == source_name;
        related && relationship.title.as_deref() == title
    })
}

fn entity_identity(node: &MNode) -> Result<(String, String), Error> {
    let entity_name = require_attr(node, "entity-name")?;
    let package_name = optional_attr(node, "package")
        .or_else(|| optional_attr(node, "package-name"))
        .ok_or_else(|| located(node, format!("entity {entity_name} is missing package")))?;
    Ok((entity_name, package_name))
}

fn related_attr(node: &MNode) -> Result<String, Error> {
    optional_attr(node, "related")
        .or_else(|| optional_attr(node, "related-entity-name"))
        .ok_or_else(|| located(node, "relationship is missing related"))
}

fn is_true(node: &MNode, name: &str) -> bool {
    node.attribute_is(name, "true")
}

fn optional_attr(node: &MNode, name: &str) -> Option<String> {
    node.attribute(name).filter(|value| !value.is_empty())
}

fn require_attr(node: &MNode, name: &str) -> Result<String, Error> {
    optional_attr(node, name).ok_or_else(|| located(node, format!("missing attribute {name}")))
}

fn parse_u64(node: &MNode, name: &str, default: u64) -> Result<u64, Error> {
    let Some(text) = optional_attr(node, name) else {
        return Ok(default);
    };
    text.parse::<u64>()
        .map_err(|_| located(node, format!("attribute {name} is not an integer: {text}")))
}

fn located(node: &MNode, message: impl Into<String>) -> Error {
    Error::at(message, node.file_location(), node.line(), node.column())
}

#[cfg(test)]
mod tests;
